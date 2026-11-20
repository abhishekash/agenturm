"""Native readers use a source-scoped host query for persisted usage."""
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

from observatory.core import Store, normalize_native
from observatory.local_agents import LocalAgents
from observatory.native_ports import NativeEvidenceFacade
from observatory.native_readers import NativeReaderSpec
from observatory.native_sources import NativeSourceSpec
from observatory.native_sources import builtin_sources
from observatory.adapters.pi import PiAdapter


class NativePortTests(unittest.TestCase):
    def test_peer_link_hook_runs_once_for_registered_adapter_group(self):
        linked = []
        class Adapter:
            def link_sessions(self, peers):
                linked.append([peer.name for peer in peers])
        collector = object.__new__(LocalAgents)
        collector.sessions = {name: SimpleNamespace(name=name, adapter=Adapter())
                              for name in ('first', 'second')}
        collector.link_adapter_sessions()
        self.assertEqual(linked, [['first', 'second']])

    def test_failed_optional_hooks_leave_other_adapters_visible(self):
        errors = []
        linked = []
        class Broken:
            def link_sessions(self, _peers):
                raise ValueError('broken peer hook')
            def status_metadata(self, _session):
                raise ValueError('broken status hook')
        class Healthy:
            def link_sessions(self, peers):
                linked.extend(peer.name for peer in peers)
            def status_metadata(self, _session):
                return {'fixture_health': 'ok'}
        collector = object.__new__(LocalAgents)
        collector.record_error = lambda exc, operation: errors.append((type(exc).__name__, operation))
        broken = SimpleNamespace(name='broken', adapter=Broken())
        healthy = SimpleNamespace(name='healthy', adapter=Healthy(),
                                  adapter_status_metadata=lambda: {'fixture_health': 'ok'})
        broken.adapter_status_metadata = lambda: broken.adapter.status_metadata(broken)
        collector.sessions = {'broken': broken, 'healthy': healthy}
        collector.link_adapter_sessions()
        self.assertEqual(linked, ['healthy'])
        self.assertEqual(collector.adapter_status_metadata(broken), {})
        self.assertEqual(collector.adapter_status_metadata(healthy), {'fixture_health': 'ok'})
        self.assertEqual([kind for kind, _ in errors], ['ValueError', 'ValueError'])

        healthy.adapter_status_metadata = lambda: {'source_id': 'spoofed'}
        self.assertEqual(collector.adapter_status_metadata(healthy), {})
        self.assertEqual(errors[-1][0], 'ValueError')

    def test_status_extensions_are_isolated_and_cannot_override_health(self):
        errors = []
        class Broken:
            def status_extension(self, _capture, _on_error):
                raise ValueError('broken')
        class Healthy:
            def status_extension(self, capture, _on_error):
                return {'fixture_status': 'captured' if capture else 'metadata'}
        collector = object.__new__(LocalAgents)
        collector.capture_details = False
        collector.record_error = lambda exc, operation: errors.append((type(exc).__name__, operation))
        peers = [SimpleNamespace(adapter=Broken()), SimpleNamespace(adapter=Healthy())]
        self.assertEqual(collector.adapter_status_extensions(peers),
                         {'fixture_status': 'metadata'})
        self.assertEqual(errors[0][0], 'ValueError')
        peers[0].adapter.status_extension = lambda _capture, _on_error: {'sessions': []}
        self.assertEqual(collector.adapter_status_extensions(peers),
                         {'fixture_status': 'metadata'})
        self.assertEqual(errors[-1][0], 'ValueError')
        peers[0].adapter.status_extension = lambda _capture, _on_error: {'bad': object()}
        self.assertEqual(collector.adapter_status_extensions(peers),
                         {'fixture_status': 'metadata'})
        self.assertEqual(errors[-1][0], 'TypeError')

    def test_empty_status_shape_comes_from_registered_source(self):
        errors = []
        collector = object.__new__(LocalAgents)
        collector.capture_details = False
        collector.record_error = lambda exc, operation: errors.append((type(exc).__name__, operation))
        collector.source_specs = builtin_sources()
        self.assertEqual(collector.adapter_status_extensions([]), {'kiro_folders': []})
        self.assertEqual(errors, [])

        fixture = NativeSourceSpec('fixture', 'Fixture', lambda: Path('/unused'),
            adapter_factory=PiAdapter,
            empty_status_extension=lambda: {'fixture_status': 'empty'})
        collector.source_specs = (fixture,)
        self.assertEqual(collector.adapter_status_extensions([]), {'fixture_status': 'empty'})
        active = SimpleNamespace(adapter=SimpleNamespace(
            status_extension=lambda _capture, _on_error: {'fixture_status': 'active'}))
        self.assertEqual(collector.adapter_status_extensions([active]), {'fixture_status': 'active'})

        broken = NativeSourceSpec('broken', 'Broken', lambda: Path('/unused'),
            adapter_factory=PiAdapter,
            empty_status_extension=lambda: {'sessions': ['spoofed']})
        collector.source_specs = (fixture, broken)
        self.assertEqual(collector.adapter_status_extensions([]), {'fixture_status': 'empty'})
        self.assertEqual(errors[-1][0], 'ValueError')

    def test_collector_injects_narrow_host_facade_into_both_reader_types(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            source = root / 'session.jsonl'
            source.touch()
            store = Store(root / 'port.sqlite')
            received = []
            class Reader:
                def poll(self): return 0
                def status_sessions(self): return []
                def diagnostics(self): return {}
            def make_reader(evidence, _capture):
                received.append(evidence)
                return Reader()
            try:
                reader_spec = NativeReaderSpec('fixture-reader', make_reader)
                source_spec = NativeSourceSpec('fixture-source', 'Fixture', lambda: root,
                    accepts_file=lambda path: Path(path) == source, adapter_factory=PiAdapter)
                collector = LocalAgents(store, source_specs=(source_spec,),
                                        reader_specs=(reader_spec,))
                collector.discover()
                self.assertIs(received[0], collector.evidence)
                self.assertIs(next(iter(collector.sessions.values())).store, collector.evidence)
                self.assertIsInstance(collector.evidence, NativeEvidenceFacade)
                self.assertFalse(hasattr(collector.evidence, '_connect'))
                self.assertFalse(hasattr(collector.evidence, 'upsert_agent_nodes'))
            finally:
                store.close()

    def test_model_usage_is_exact_source_scoped_and_owned(self):
        with tempfile.TemporaryDirectory() as root:
            store = Store(Path(root) / 'port.sqlite')
            try:
                span = normalize_native({
                    'trace_id': 'a' * 32, 'span_id': 'b' * 16,
                    'name': 'model', 'start_unix_nano': 1,
                    'end_unix_nano': 2,
                    'attributes': {'observatory.native_usage': {'input_tokens': 9}},
                    'resource_attributes': {'observatory.source_id': 'owned-source'},
                }, 'claude-code')
                store.ingest([span])
                self.assertEqual(store.native_model_usage('owned-source', 'a' * 32, 'b' * 16),
                                 {'input_tokens': 9})
                self.assertEqual(store.native_model_usage('other-source', 'a' * 32, 'b' * 16), {})
                self.assertEqual(store.native_model_usage('owned-source', 'c' * 32, 'b' * 16), {})
            finally:
                store.close()

    def test_injected_adapter_owns_namespace_and_optional_status_metadata(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            source = root / 'session.jsonl'
            source.touch()
            store = Store(root / 'port.sqlite')
            class FixtureAdapter(PiAdapter):
                agent_namespace = 'external-fixture'
                def status_metadata(self, session):
                    return {'fixture_source': session.session_id}
            try:
                spec = NativeSourceSpec('fixture', 'Fixture', lambda: root,
                    accepts_file=lambda path: Path(path) == source,
                    adapter_factory=FixtureAdapter)
                collector = LocalAgents(store, source_specs=(spec,), reader_specs=())
                collector.discover()
                session = next(iter(collector.sessions.values()))
                self.assertEqual(session.agent_namespace, 'external-fixture')
                self.assertEqual(session.adapter_status_metadata(),
                                 {'fixture_source': 'session'})
            finally:
                store.close()
