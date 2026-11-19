"""Reader registration and host ownership without an OpenCode-specific branch."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from observatory.core import Store
from observatory.local_agents import LocalAgents
from observatory.native_readers import NativeReaderSpec
from observatory.native_sources import NativeSourceSpec


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
    def test_injected_reader_contributes_status_and_can_be_disabled(self):
        with tempfile.TemporaryDirectory() as root:
            store = Store(Path(root) / "agenturm.db")
            try:
                reader = FixtureReader()
                spec = NativeReaderSpec("fixture", lambda _store, _capture: reader,
                                        mcp_client="Fixture")
                monitor = LocalAgents(store, source_specs=(), reader_specs=(spec,))
                self.assertIsNone(monitor.opencode)
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
                self.assertEqual([row["agent_id"] for row in result["sessions"]],
                                 ["fixture-agent"])
                self.assertEqual(result["opencode_discovered_sessions"], 0)
                disabled = LocalAgents(store, source_specs=(), reader_specs=())
                self.assertEqual(disabled.reader_plugins, {})
                self.assertEqual(disabled.status()["sessions"], [])
            finally:
                store.close()

    def test_registration_rejects_conflicts_and_incomplete_readers(self):
        spec = NativeReaderSpec("fixture", lambda _store, _capture: FixtureReader())
        with self.assertRaisesRegex(ValueError, "duplicate native reader ID"):
            LocalAgents(None, source_specs=(), reader_specs=(spec, spec))
        with self.assertRaisesRegex(ValueError, "must be unique"):
            LocalAgents(None, source_specs=(NativeSourceSpec("fixture", "Fixture",
                       lambda: Path("/absent")),), reader_specs=(spec,))
        with self.assertRaisesRegex(TypeError, "lacks required methods"):
            LocalAgents(None, source_specs=(), reader_specs=(
                NativeReaderSpec("invalid", lambda _store, _capture: object()),))
        with self.assertRaisesRegex(ValueError, "invalid native reader ID"):
            NativeReaderSpec("", lambda _store, _capture: FixtureReader())


if __name__ == "__main__":
    unittest.main()
