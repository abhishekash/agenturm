"""Reader registration and host ownership without an OpenCode-specific branch."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

from observatory.core import Store
from observatory.local_agents import LocalAgents
from observatory.native_readers import NativeReaderSpec, builtin_readers
from observatory.native_sources import NativeSourceSpec
from observatory.adapters.pi import PiAdapter


class FixtureReader:
    def __init__(self):
        self.polls = 0
        self.servers = ()

    def poll(self):
        self.polls += 1

    def set_mcp_servers(self, servers):
        self.servers = tuple(servers)

    def status_sessions(self):
        return [{"agent_id": "fixture-agent", "source_id": "fixture-source",
                 "runtime": "Fixture", "run_state": "unknown"}]

    def diagnostics(self):
        return {"polls": self.polls}


class RegistryTests(unittest.TestCase):
    def test_opencode_reader_is_declared_in_bundled_module(self):
        specs = builtin_readers()
        self.assertEqual([spec.id for spec in specs], ["opencode"])
        self.assertEqual(specs[0].mcp_client, "OpenCode")

    def test_fresh_builtin_specs_are_one_status_declaration(self):
        # The collector discovers bundled specs again during initialization.
        # Explicitly passing a separately created tuple must not duplicate
        # the same trusted compatibility callback.
        monitor = LocalAgents(None, source_specs=(), reader_specs=builtin_readers())
        self.assertEqual(len(monitor.reader_status_specs), 1)
        self.assertIn("opencode_read_errors", monitor.status())

    def test_new_bundled_reader_needs_no_central_dispatch_edit(self):
        module = SimpleNamespace(__name__="observatory.adapters.fixture_reader",
                                 reader_spec=lambda: NativeReaderSpec(
                                     "fixture", lambda _store, _capture: FixtureReader()))
        with patch("observatory.native_sources.bundled_adapter_modules", return_value=(module,)):
            specs = builtin_readers()
        self.assertEqual([spec.id for spec in specs], ["fixture"])
        self.assertIsInstance(specs[0].build(None, False), FixtureReader)

    def test_duplicate_bundled_reader_ids_fail(self):
        module = SimpleNamespace(__name__="observatory.adapters.fixture_reader",
                                 reader_spec=lambda: NativeReaderSpec(
                                     "fixture", lambda _store, _capture: FixtureReader()))
        with patch("observatory.native_sources.bundled_adapter_modules", return_value=(module, module)):
            with self.assertRaisesRegex(ValueError, "duplicate bundled reader ID"):
                builtin_readers()

    def test_injected_reader_contributes_status_and_can_be_disabled(self):
        with tempfile.TemporaryDirectory() as root:
            store = Store(Path(root) / "agenturm.db")
            try:
                reader = FixtureReader()
                spec = NativeReaderSpec("fixture", lambda _store, _capture: reader,
                                        mcp_client="Fixture", label="Fixture harness",
                                        coverage=lambda _reader: {"omitted": 2, "limit": 10},
                                        status_extension=lambda _reader, diagnostics: {
                                            "fixture_polls": diagnostics["polls"]},
                                        status_keys=("fixture_polls",))
                monitor = LocalAgents(store, source_specs=(), reader_specs=(spec,))
                self.assertNotIn("opencode", monitor.reader_plugins)
                monitor.discover = lambda: None
                original_poll = reader.poll
                def poll_once():
                    original_poll()
                    monitor.stop.set()
                reader.poll = poll_once
                with patch("observatory.local_agents.discover_mcp_configurations",
                           return_value=[{"client": "Fixture", "server": "fixture-mcp"}]):
                    monitor.loop()
                result = monitor.status()
                self.assertEqual(reader.servers, ("fixture-mcp",))
                self.assertEqual(result["reader_plugins"]["fixture"], {"polls": 1})
                self.assertEqual(result["fixture_polls"], 1)
                self.assertEqual(result["source_coverage"][1],
                                 {"label": "Fixture harness", "unit": "sessions",
                                  "active": 1, "omitted": 2, "limit": 10,
                                  "state": "partial"})
                self.assertEqual([row["agent_id"] for row in result["sessions"]],
                                 ["fixture-agent"])
                self.assertEqual(result["opencode_discovered_sessions"], 0)
                self.assertEqual(result["opencode_omitted_sessions"], 0)
                self.assertEqual(result["opencode_diagnostics"], {})
                self.assertEqual(result["opencode_read_errors"], 0)
                disabled = LocalAgents(store, source_specs=(), reader_specs=())
                self.assertEqual(disabled.reader_plugins, {})
                self.assertEqual(disabled.status()["sessions"], [])
                unknown = LocalAgents(store, source_specs=(), reader_specs=(
                    NativeReaderSpec("unknown", lambda _store, _capture: FixtureReader()),))
                unknown_coverage = unknown.status()["source_coverage"][1]
                self.assertEqual(unknown_coverage["active"], 1)
                self.assertIsNone(unknown_coverage["omitted"])
                self.assertEqual(unknown_coverage["state"], "partial")
            finally:
                store.close()

    def test_registration_rejects_conflicts_and_incomplete_readers(self):
        spec = NativeReaderSpec("fixture", lambda _store, _capture: FixtureReader())
        with self.assertRaisesRegex(ValueError, "duplicate native reader ID"):
            LocalAgents(None, source_specs=(), reader_specs=(spec, spec))
        with self.assertRaisesRegex(ValueError, "must be unique"):
            LocalAgents(None, source_specs=(NativeSourceSpec("fixture", "Fixture",
                       lambda: Path("/absent"), adapter_factory=PiAdapter),), reader_specs=(spec,))
        with self.assertRaisesRegex(TypeError, "lacks required methods"):
            LocalAgents(None, source_specs=(), reader_specs=(
                NativeReaderSpec("invalid", lambda _store, _capture: object()),))
        with self.assertRaisesRegex(ValueError, "invalid native reader ID"):
            NativeReaderSpec("", lambda _store, _capture: FixtureReader())
        with self.assertRaisesRegex(ValueError, "invalid native reader specification"):
            NativeReaderSpec("fixture", lambda _store, _capture: FixtureReader(),
                             coverage="not callable")
        with self.assertRaisesRegex(ValueError, "invalid native reader specification"):
            NativeReaderSpec("fixture", lambda _store, _capture: FixtureReader(),
                             status_extension=lambda _reader, _diagnostics: {},
                             status_keys=())

    def test_reader_status_fields_cannot_replace_host_fields(self):
        status = lambda _reader, _diagnostics: {"read_errors": 0}
        with self.assertRaisesRegex(ValueError, "conflicts with host fields"):
            LocalAgents(None, source_specs=(), reader_specs=(NativeReaderSpec(
                "fixture", lambda _store, _capture: FixtureReader(),
                status_extension=status, status_keys=("read_errors",)),))

    def test_reader_status_callback_cannot_add_undeclared_fields(self):
        with tempfile.TemporaryDirectory() as root:
            store = Store(Path(root) / "agenturm.db")
            try:
                spec = NativeReaderSpec(
                    "fixture", lambda _store, _capture: FixtureReader(),
                    status_extension=lambda _reader, _diagnostics: {
                        "fixture_polls": 0, "sessions": "overridden"},
                    status_keys=("fixture_polls",))
                collector = LocalAgents(store, source_specs=(), reader_specs=(spec,))
                status = collector.status()
                self.assertEqual(status["sessions"], [{"agent_id": "fixture-agent",
                    "source_id": "fixture-source", "runtime": "Fixture",
                    "run_state": "unknown"}])
                self.assertNotIn("fixture_polls", status)
                self.assertEqual(status["last_read_error"]["operation"],
                                 "Inspecting native reader fixture status extension")
            finally:
                store.close()

    def test_bundled_status_contract_follows_replaced_reader(self):
        with tempfile.TemporaryDirectory() as root:
            store = Store(Path(root) / "agenturm.db")
            try:
                reader = FixtureReader()
                reader.discovered_sessions = 7
                reader.omitted_sessions = 2
                reader.errors = 3
                reader.diagnostics = lambda: {"read_errors": 3}
                collector = LocalAgents(store, source_specs=(), reader_specs=(
                    NativeReaderSpec("opencode", lambda _store, _capture: reader,
                                     status_extension=lambda _reader, _diagnostics: {
                                         "custom_status": "ready"},
                                     status_keys=("custom_status",)),))
                status = collector.status()
                self.assertEqual(status["opencode_discovered_sessions"], 7)
                self.assertEqual(status["opencode_omitted_sessions"], 2)
                self.assertEqual(status["opencode_read_errors"], 3)
                self.assertEqual(status["opencode_diagnostics"], {"read_errors": 3})
                self.assertEqual(status["custom_status"], "ready")
            finally:
                store.close()

    def test_replacement_reader_cannot_redeclare_compatibility_key(self):
        spec = NativeReaderSpec("opencode", lambda _store, _capture: FixtureReader(),
                                status_extension=lambda _reader, _diagnostics: {
                                    "opencode_read_errors": 900},
                                status_keys=("opencode_read_errors",))
        with self.assertRaisesRegex(ValueError, "conflicts with host fields"):
            LocalAgents(None, source_specs=(), reader_specs=(spec,))

    def test_broken_reader_coverage_is_isolated(self):
        with tempfile.TemporaryDirectory() as root:
            store = Store(Path(root) / "agenturm.db")
            try:
                spec = NativeReaderSpec("fixture", lambda _store, _capture: FixtureReader(),
                                        coverage=lambda _reader: {"omitted": -1})
                monitor = LocalAgents(store, source_specs=(), reader_specs=(spec,))
                result = monitor.status()
                self.assertEqual(result["source_coverage"][1]["state"], "unavailable")
                self.assertEqual(result["source_coverage"][1]["active"], 1)
                self.assertEqual(result["source_coverage"][1]["omitted"], None)
                self.assertEqual(result["read_errors"], 1)
            finally:
                store.close()


if __name__ == "__main__":
    unittest.main()
