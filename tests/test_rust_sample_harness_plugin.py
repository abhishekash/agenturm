"""Installed Rust sample adapter through the real process, Store and health paths."""
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.request import urlopen

from observatory.core import Store
from observatory.plugin_admission import set_source_capture
from observatory.plugin_config import read_plugin_config
from observatory.plugin_projection import project_source
from observatory.plugin_runtime import PluginSession
from observatory.plugin_wire import ProtocolError
from observatory.server import BoundedHTTPServer, handler_for, serve


PACKAGE = Path(__file__).resolve().parents[1] / "examples" / "rust_sample_harness_plugin"


class RustSampleHarnessTests(unittest.TestCase):
    def setUp(self):
        executable = os.environ.get("AGENTURM_RUST_SAMPLE_BIN")
        if not executable:
            self.skipTest("Rust sample binary is built in the Rust CI job")
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source_file = self.root / "events.jsonl"
        self.binary = Path(executable).resolve()
        result = subprocess.run(
            [sys.executable, str(PACKAGE / "install.py"), "--binary", str(self.binary),
             "--destination", str(self.root / "installed"), "--source", str(self.source_file)],
            capture_output=True, text=True, check=True)
        self.config = Path(result.stdout.strip())
        self.source = read_plugin_config(self.config)[0]
        self.store = Store(str(self.root / "store.db"))
        self.addCleanup(self.store.close)

    def append(self, *rows):
        with self.source_file.open("ab") as stream:
            for row in rows:
                stream.write((json.dumps(row, separators=(",", ":")) + "\n").encode())

    def session(self):
        return PluginSession(self.store, self.source.manifest,
                             source_id=self.source.source_id,
                             allowed_grants=self.source.allowed_grants,
                             timeout=self.source.timeout, source_root=self.source.source_root)

    def projected(self):
        project_source(self.store, self.source.source_id)
        return {row["native_session_id"]: row
                for row in self.store.plugin_sessions()["sessions"]}

    def test_installed_rust_adapter_replays_corrections_and_relationship(self):
        self.append(
            {"id": "parent-start", "session": "parent", "at_ns": "100",
             "kind": "lifecycle", "state": "running"},
            {"id": "child-start", "session": "child", "at_ns": "110",
             "kind": "lifecycle", "state": "started"},
            {"id": "parent-link", "session": "child", "at_ns": "120",
             "kind": "relationship", "parent_session_id": "parent",
             "relationship_kind": "delegated"},
            {"id": "usage", "session": "child", "at_ns": "130",
             "kind": "usage", "usage": {"input_tokens": 4, "output_tokens": 0}},
            {"id": "child-end", "session": "child", "at_ns": "140",
             "kind": "lifecycle", "state": "completed", "reason_code": "normal"})
        with self.session() as worker:
            first = worker.poll_once()
            self.assertEqual(len(first.records), 5)
            self.assertEqual(worker.poll_once().records, ())
            projected = self.projected()
            self.assertEqual(projected["child"]["run_state"], "completed")
            self.assertEqual(projected["child"]["metadata_usage"],
                             {"input_tokens": 4, "output_tokens": 0})
            self.assertEqual(projected["child"]["relationship_state"], "confirmed")
            self.assertEqual(projected["child"]["relationship_kind"], "delegated")
        with self.session() as resumed:
            self.assertEqual(resumed.cursor, first.cursor)
            self.assertEqual(resumed.poll_once().records, ())
            self.append({"id": "usage", "session": "child", "at_ns": "130",
                         "kind": "usage", "revision": 1,
                         "usage": {"input_tokens": 7, "output_tokens": 1}})
            corrected = resumed.poll_once()
            self.assertEqual(len(corrected.records), 1)
            self.assertEqual(self.projected()["child"]["metadata_usage"],
                             {"input_tokens": 7, "output_tokens": 1})
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 5)

    def test_corrections_already_in_source_are_committed_across_restart(self):
        self.append(
            {"id": "state", "session": "one", "at_ns": "100",
             "kind": "lifecycle", "state": "started"},
            {"id": "state", "session": "one", "at_ns": "100",
             "kind": "lifecycle", "revision": 1, "state": "running"},
            {"id": "state", "session": "one", "at_ns": "100",
             "kind": "lifecycle", "revision": 2, "state": "completed"},
            {"id": "tokens", "session": "one", "at_ns": "110",
             "kind": "usage", "usage": {"input_tokens": 3}})
        with self.session() as worker:
            first = worker.poll_once()
            self.assertEqual([(row["event_id"], row["revision"])
                              for row in first.records], [("rust-sample:state", 0)])
        with self.session() as resumed:
            self.assertEqual(resumed.cursor, first.cursor)
            second = resumed.poll_once()
            self.assertEqual([(row["event_id"], row["revision"])
                              for row in second.records], [("rust-sample:state", 1)])
            third = resumed.poll_once()
            self.assertEqual([(row["event_id"], row["revision"])
                              for row in third.records],
                             [("rust-sample:state", 2), ("rust-sample:tokens", 0)])
            self.assertEqual(resumed.poll_once().records, ())
            project_source(self.store, self.source.source_id)
            self.assertEqual(self.projected()["one"]["run_state"], "completed")
            self.assertEqual(self.projected()["one"]["metadata_usage"],
                             {"input_tokens": 3})
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 2)

    def test_full_usage_backlog_drains_within_wire_structure_limit(self):
        self.append(*(
            {"id": f"usage-{index}", "session": "one", "at_ns": str(index),
             "kind": "usage", "usage": {"input_tokens": 1, "output_tokens": 2,
                                        "cache_read_tokens": 3, "cache_write_tokens": 4,
                                        "total_tokens": 5}}
            for index in range(256)))
        with self.session() as worker:
            counts = []
            for _ in range(5):
                prior = worker.cursor
                received = worker.poll_once()
                if not received.records:
                    break
                self.assertNotEqual(received.cursor, prior)
                counts.append(len(received.records))
            else:
                self.fail("Rust adapter did not drain 256 rows within five polls")
            self.assertGreater(len(counts), 1)
            self.assertEqual(sum(counts), 256)
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 256)

    def test_installed_rust_adapter_projects_content_free_activity(self):
        self.append(
            {"id": "start", "session": "native-1", "at_ns": "100",
             "kind": "lifecycle", "state": "started"},
            {"id": "usage", "session": "native-1", "at_ns": "110",
             "kind": "usage", "usage": {"input_tokens": 4, "output_tokens": 1}},
            {"id": "request", "session": "native-1", "at_ns": "120",
             "kind": "message", "role": "user"},
            {"id": "call", "session": "native-1", "at_ns": "130",
             "kind": "tool", "category": "shell", "result": "succeeded"},
            {"id": "check", "session": "native-1", "at_ns": "140",
             "kind": "check", "category": "test", "result": "failed"})
        handler = handler_for(self.store, quiet=True)
        server = BoundedHTTPServer(("127.0.0.1", 0), handler)
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        self.addCleanup(handler.response_cache.close)
        self.addCleanup(server.server_close)
        self.addCleanup(server_thread.join, 3)
        self.addCleanup(server.shutdown)

        def http_activity():
            with urlopen(f"http://127.0.0.1:{server.server_address[1]}/api/activity",
                         timeout=3) as response:
                return json.load(response)["activity"]

        with self.session() as worker:
            first = worker.poll_once()
            self.assertEqual(len(first.records), 5)
            self.assertEqual(worker.poll_once().records, ())
            projected = self.projected()["native-1"]
            self.assertEqual(projected["run_state"], "running")
            self.assertEqual(projected["metadata_usage"],
                             {"input_tokens": 4, "output_tokens": 1})
            activity = self.store.activity_page()["activity"]
            self.assertEqual([(row["domain"], row["result"]) for row in activity],
                             [("check", "failed"), ("tool", "succeeded"),
                              ("message", "observed")])
            self.assertTrue(all(row["detail"] == "" for row in activity))
            self.assertEqual({row["session_id"] for row in activity},
                             {projected["session_id"]})
            self.assertEqual(len(http_activity()), 3)
            self.append(
                {"id": "check", "session": "native-1", "at_ns": "140",
                 "kind": "check", "revision": 1,
                 "category": "test", "result": "passed"},
                {"id": "usage", "session": "native-1", "at_ns": "110",
                 "kind": "usage", "revision": 1,
                 "usage": {"input_tokens": 6, "output_tokens": 1}})
            correction = worker.poll_once()
            self.assertEqual(len(correction.records), 2)
            self.assertEqual(self.store.activity_page()["activity"], [])
            self.assertEqual(http_activity(), [])
            projected = self.projected()["native-1"]
            self.assertEqual(projected["metadata_usage"],
                             {"input_tokens": 6, "output_tokens": 1})
            self.assertEqual(projected["run_state"], "running")
            self.assertEqual(self.store.activity_page(domain="check")["activity"][0]["result"],
                             "passed")
            self.assertEqual(http_activity()[0]["result"], "passed")
            set_source_capture(self.store, source_id=self.source.source_id, enabled=False)
            self.assertEqual(self.store.activity_page()["activity"], [])
            self.assertEqual(http_activity(), [])
            self.assertEqual(self.store.plugin_sessions()["sessions"], [])
            with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
                worker.poll_once()

    def test_installed_rust_adapter_correlates_tool_phases_without_content(self):
        self.append(
            {"id": "request", "session": "pi-session", "at_ns": "100",
             "kind": "message", "role": "user", "turn_id": "pi-user-1"},
            {"id": "call", "session": "pi-session", "at_ns": "110",
             "kind": "tool", "category": "file", "result": "started",
             "turn_id": "pi-user-1", "operation_id": "pi-call-1"},
            {"id": "result", "session": "pi-session", "at_ns": "120",
             "kind": "tool", "category": "file", "result": "failed",
             "turn_id": "pi-user-1", "operation_id": "pi-call-1"})
        with self.session() as worker:
            self.assertEqual(len(worker.poll_once().records), 3)
            self.projected()
            rows = self.store.activity_page()["activity"]
            self.assertEqual([row["result"] for row in rows],
                             ["failed", "started", "observed"])
            self.assertEqual(rows[0]["operation_id"], rows[1]["operation_id"])
            self.assertTrue(rows[0]["operation_id"].startswith("plugin-operation:"))
            self.assertEqual(rows[0]["turn_id"], rows[2]["turn_id"])
            self.assertNotIn("pi-call-1", json.dumps(rows))
            self.assertEqual(len(self.store.activity_page(review=True)["activity"]), 2)
            self.append({"id": "result", "session": "pi-session", "at_ns": "120",
                         "kind": "tool", "category": "file", "result": "succeeded",
                         "revision": 1, "turn_id": "pi-user-1",
                         "operation_id": "pi-call-1"})
            worker.poll_once()
            self.assertEqual(self.store.activity_page()["activity"], [])
            self.projected()
            corrected = self.store.activity_page()["activity"]
            self.assertEqual(corrected[0]["result"], "succeeded")
            self.assertEqual(corrected[0]["operation_id"], rows[0]["operation_id"])
            set_source_capture(self.store, source_id=self.source.source_id, enabled=False)
            self.assertEqual(self.store.activity_page()["activity"], [])

    def test_source_replacement_reports_gap_and_revocation_erases_evidence(self):
        self.append({"id": "first", "session": "a", "at_ns": "10",
                     "kind": "lifecycle", "state": "started"})
        with self.session() as worker:
            first = worker.poll_once()
            replacement = self.root / "replacement.jsonl"
            replacement.write_text(json.dumps({"id": "second", "session": "b",
                                               "at_ns": "20", "kind": "lifecycle",
                                               "state": "completed"}) + "\n")
            os.replace(replacement, self.source_file)
            changed = worker.poll_once()
            self.assertNotEqual(first.cursor, changed.cursor)
            self.assertEqual(changed.records[0]["payload"],
                             {"code": "possible_source_gap", "severity": "warning"})
            self.assertEqual(changed.records[1]["event_id"], "rust-sample:second")
            self.assertEqual(set(self.projected()), {"a", "b"})
            set_source_capture(self.store, source_id=self.source.source_id, enabled=False)
            self.assertEqual(self.store.plugin_sessions()["sessions"], [])
            with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
                worker.poll_once()

    def test_bad_content_line_never_advances_cursor(self):
        self.append({"id": "bad", "session": "a", "at_ns": "10",
                     "kind": "lifecycle", "state": "running", "prompt": "private"})
        with self.session() as worker:
            with self.assertRaises(ProtocolError):
                worker.poll_once()
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0], "")

    def test_installed_config_projects_through_receiver_health(self):
        self.append({"id": "observed", "session": "external-1", "at_ns": "30",
                     "kind": "lifecycle", "state": "completed"})
        import socket
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        servers = []
        failures = []

        def make_server(address, handler):
            server = BoundedHTTPServer(address, handler)
            servers.append(server)
            return server

        def run_server():
            try:
                serve(str(self.root / "receiver.db"), no_auth=True, port=port,
                      local_agents=False, plugin_config=self.config)
            except Exception as exc:
                failures.append(exc)

        with patch("observatory.server.ThreadingHTTPServer", side_effect=make_server):
            thread = threading.Thread(target=run_server, daemon=True)
            thread.start()
            try:
                deadline = time.monotonic() + 8
                while time.monotonic() < deadline:
                    try:
                        with urlopen(f"http://127.0.0.1:{port}/api/health", timeout=1) as response:
                            local = json.load(response)["local_agents"]
                        if local.get("sessions") and local.get("plugin_workers"):
                            break
                    except OSError:
                        pass
                    time.sleep(0.05)
                else:
                    self.fail(f"Rust sample did not reach receiver health: {failures}")
                self.assertEqual(local["sessions"][0]["native_session_id"], "external-1")
                self.assertEqual(local["sessions"][0]["run_state"], "completed")
                self.assertEqual(local["plugin_workers"][0]["state"], "running")
            finally:
                if servers:
                    servers[0].shutdown()
                thread.join(timeout=8)
        self.assertFalse(thread.is_alive())
        self.assertEqual(failures, [])


if __name__ == "__main__":
    unittest.main()
