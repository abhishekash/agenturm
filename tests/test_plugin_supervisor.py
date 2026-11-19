"""Restart, quarantine, revocation and bounded shutdown of explicit plugins."""
import tempfile
import time
import json
import threading
import unittest
from pathlib import Path
from urllib.request import urlopen

from observatory.core import Store
from observatory.plugin_admission import set_source_capture
from observatory.plugin_supervisor import PluginSource, PluginSupervisor
from tests import test_plugin_process


class SupervisorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.store = Store(str(Path(self.temp.name) / "agenturm.db"))
        self.addCleanup(self.store.close)
        self.fixture = test_plugin_process.ProcessHostTests("runTest")
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)

    def supervisor(self, mode, *, max_failures=3):
        source = PluginSource(self.fixture.manifest(mode), "fixture-source",
                              allowed_grants=("configured-source-read",),
                              poll_interval=0.05, timeout=0.5)
        manager = PluginSupervisor(self.store, (source,), max_failures=max_failures,
                                   backoff_seconds=0.05)
        self.addCleanup(manager.close)
        return manager

    def wait_state(self, manager, wanted, seconds=5):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            status = manager.snapshot()[0]
            if status["state"] == wanted:
                return status
            time.sleep(0.02)
        self.fail(f"plugin did not reach {wanted}: {manager.snapshot()}")

    def test_crash_restarts_from_durable_cursor_then_runs(self):
        manager = self.supervisor("crash_once").start()
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            status = manager.snapshot()[0]
            if status["state"] == "running" and status["restart_count"] >= 1 and status["last_cursor"] == "next":
                break
            time.sleep(0.02)
        else:
            self.fail(f"plugin did not recover: {manager.snapshot()}")
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 1)
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0], "next")
        self.assertEqual(status["consecutive_failures"], 0)

    def test_repeated_crash_quarantines_with_reason(self):
        manager = self.supervisor("crash", max_failures=2).start()
        status = self.wait_state(manager, "quarantined")
        self.assertEqual(status["restart_count"], 1)
        self.assertEqual(status["consecutive_failures"], 2)
        self.assertEqual(status["last_error_code"], "protocol_error")
        self.assertIn("data pipe", status["reason"])
        from observatory.server import BoundedHTTPServer, handler_for
        handler = handler_for(self.store, quiet=True)
        self.addCleanup(handler.response_cache.close)
        server = BoundedHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        with urlopen(f"http://127.0.0.1:{server.server_address[1]}/api/health", timeout=3) as response:
            worker = json.load(response)["local_agents"]["plugin_workers"][0]
        self.assertEqual(worker["state"], "quarantined")
        self.assertEqual(worker["last_failure_code"], "protocol_error")
        self.assertTrue(worker["last_failure_reason"])

    def test_policy_revocation_stops_restarts(self):
        manager = self.supervisor("idle").start()
        self.wait_state(manager, "running")
        set_source_capture(self.store, source_id="fixture-source", enabled=False)
        status = self.wait_state(manager, "revoked")
        self.assertEqual(status["last_error_code"], "capture_revoked")

    def test_close_terminates_worker_and_threads(self):
        manager = self.supervisor("timeout").start()
        self.wait_state(manager, "running")
        manager.close()
        self.assertEqual(manager.snapshot()[0]["state"], "stopped")
        self.assertFalse(any(thread.is_alive() for thread in manager.threads))

    def test_rejects_duplicate_sources_before_launch(self):
        source = PluginSource(self.fixture.manifest("idle"), "fixture-source",
                              allowed_grants=("configured-source-read",))
        with self.assertRaisesRegex(ValueError, "duplicate"):
            PluginSupervisor(self.store, (source, source))


if __name__ == "__main__":
    unittest.main()
