"""Process source failures must not make retained outcomes look current."""
from contextlib import redirect_stdout
from io import StringIO
import json
from pathlib import Path
from types import SimpleNamespace
import tempfile
import threading
import unittest
from urllib.request import urlopen
from rich.cells import cell_len

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch
from observatory.plugin_projection import project_source
from observatory.plugin_wire import PollBatch
from observatory.server import BoundedHTTPServer, handler_for
from observatory.source_coverage import (coverage_detail, coverage_warning,
                                        process_source_availability)


class ProcessSourceAvailabilityTests(unittest.TestCase):
    def test_freshness_requires_success_after_latest_failure(self):
        worker = {"source_id": "source-a", "state": "running",
                  "last_success_ns": 100, "last_failure_ns": 0}
        sessions = [{"plugin_source_id": "source-a"}]
        self.assertEqual(process_source_availability([worker], sessions)[0]["availability"],
                         "ready")
        self.assertEqual(process_source_availability([], sessions)[0]["reason_code"],
                         "unmonitored")
        self.assertEqual(process_source_availability(None, sessions)[0]["reason_code"],
                         "unmonitored")
        worker.update(state="degraded", last_failure_ns=200,
                      last_failure_code="worker_error")
        self.assertEqual(process_source_availability([worker], sessions)[0]["availability"],
                         "unavailable")
        worker["state"] = "running"
        self.assertEqual(process_source_availability([worker], sessions)[0]["availability"],
                         "unavailable")
        worker["last_success_ns"] = 300
        self.assertEqual(process_source_availability([worker], sessions)[0]["availability"],
                         "ready")

    def test_exact_long_owner_is_preserved_in_health_and_bounded_for_display(self):
        source = "s" * 256
        statuses = process_source_availability([], [{"plugin_source_id": source},
                                                  {"plugin_source_id": "界" * 100}])
        self.assertEqual([entry["source_id"] for entry in statuses], [source])
        local = {"source_coverage": [{"label": "Process adapters", "unit": "sessions",
                                      "active": 1, "omitted": 0, "state": "partial"}],
                 "process_source_availability": statuses}
        warning = coverage_warning(local)
        detail = coverage_detail(local)
        self.assertNotIn(source, warning + detail)
        self.assertIn("s" * 31 + "…", warning)
        self.assertLessEqual(cell_len(warning.split(" unavailable")[0].removeprefix("Coverage limited: Process source ")), 32)

    def test_health_and_plain_coverage_preserve_recorded_outcome(self):
        from observatory.terminal import print_snapshot
        from tests.test_terminal_search import ManualFeed

        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        store = Store(str(Path(temp.name) / "store.db"))
        self.addCleanup(store.close)
        lease = activate_source(store, plugin_id="fixture.plugin",
                                source_id="source-a", generation="a" * 64)
        commit_batch(store, lease, "", PollBatch("source-a", "one", ({
            "event_id": "end", "revision": 0, "kind": "lifecycle",
            "time_ns": "100", "payload": {"state": "completed"},
            "identity": {"session_id": "session-a"}},)))
        project_source(store, "source-a")
        worker = {"source_id": "source-a", "plugin_id": "fixture.plugin",
                  "state": "running", "last_success_ns": 100,
                  "last_failure_ns": 0, "last_failure_code": ""}
        store.plugin_supervisor = SimpleNamespace(snapshot=lambda: [dict(worker)])
        handler = handler_for(store, quiet=True)
        self.addCleanup(handler.response_cache.close)
        server = BoundedHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        def health():
            with urlopen(f"http://127.0.0.1:{server.server_port}/api/health", timeout=3) as response:
                return json.load(response)["local_agents"]

        ready = health()
        self.assertEqual(ready["sessions"][0]["run_state"], "completed")
        self.assertNotIn("source_availability", ready["sessions"][0])
        manager = store.plugin_supervisor
        store.plugin_supervisor = None
        unmonitored = health()
        self.assertEqual(unmonitored["sessions"][0]["source_availability"], "unavailable")
        self.assertEqual(unmonitored["process_source_availability"][0]["reason_code"],
                         "unmonitored")
        store.plugin_supervisor = manager
        worker.update(state="degraded", last_failure_ns=200,
                      last_failure_code="worker_error")
        unavailable = health()
        session = unavailable["sessions"][0]
        self.assertEqual((session["run_state"], session["source_availability"]),
                         ("completed", "unavailable"))
        self.assertEqual(session["plugin_source_id"], "source-a")
        self.assertIn("Process source source-a unavailable", coverage_warning(unavailable))
        self.assertIn("Process source source-a: unavailable", coverage_detail(unavailable))
        self.assertEqual(unavailable["source_coverage"][-1]["state"], "partial")
        self.assertEqual(store.plugin_sessions()["sessions"][0]["run_state"], "completed")

        feed = ManualFeed()
        feed.snapshot.agents = []
        feed.snapshot.local_status.update(unavailable)
        output = StringIO()
        with redirect_stdout(output):
            print_snapshot(feed, refresh=False)
        self.assertIn("Process source source-a unavailable", output.getvalue())

        worker["state"] = "running"
        self.assertEqual(health()["sessions"][0]["source_availability"], "unavailable")
        worker["last_success_ns"] = 300
        recovered = health()
        self.assertNotIn("source_availability", recovered["sessions"][0])
        self.assertNotIn("Process source source-a unavailable", coverage_warning(recovered))


if __name__ == "__main__":
    unittest.main()
