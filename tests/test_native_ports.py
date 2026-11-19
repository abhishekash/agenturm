"""Native readers use a source-scoped host query for persisted usage."""
import tempfile
import unittest
from pathlib import Path

from observatory.core import Store, normalize_native
from observatory.local_agents import LocalAgents
from observatory.native_ports import NativeEvidenceFacade
from observatory.native_readers import NativeReaderSpec
from observatory.native_sources import NativeSourceSpec
from observatory.adapters.pi import PiAdapter


class NativePortTests(unittest.TestCase):
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
