"""Standalone sample adapter through the real process and durable Store path."""
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
from observatory.plugin_config import read_plugin_config
from observatory.plugin_projection import project_source
from observatory.plugin_runtime import PluginSession
from observatory.plugin_wire import ProtocolError
from observatory.server import BoundedHTTPServer, serve


PACKAGE = Path(__file__).resolve().parents[1] / "examples" / "sample_harness_plugin"


class SampleHarnessPluginTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source_file = self.root / "sample.jsonl"
        installed = self.root / "installed"
        result = subprocess.run([sys.executable, str(PACKAGE / "install.py"),
                                 "--destination", str(installed), "--source", str(self.source_file)],
                                capture_output=True, text=True, check=True)
        self.config = Path(result.stdout.strip())
        self.assertEqual(self.config, installed / "plugins.toml")
        self.executable = installed / "sample-harness"
        self.manifest = installed / "sample-harness.toml"
        self.store = Store(str(self.root / "store.db"))
        self.addCleanup(self.store.close)

    def source(self):
        return read_plugin_config(self.config)[0]

    def session(self):
        source = self.source()
        return PluginSession(self.store, source.manifest, source_id=source.source_id,
                             allowed_grants=source.allowed_grants,
                             timeout=source.timeout, source_root=source.source_root)

    def append(self, *rows):
        with self.source_file.open("ab") as stream:
            for row in rows:
                stream.write((json.dumps(row, separators=(",", ":")) + "\n").encode())

    def projected(self):
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            try:
                project_source(self.store, "sample-source")
                if not self.store.plugin_sessions()["pending_sources"]:
                    return self.store.plugin_sessions()["sessions"]
            except ProtocolError as exc:
                if "source changed; retry" not in str(exc):
                    raise
            time.sleep(0.01)
        self.fail("sample source projection did not settle")

    def test_external_artifact_reads_append_replay_and_correction(self):
        self.append(
            {"id": "started", "session": "native-1", "at_ns": "100",
             "kind": "lifecycle", "state": "started"},
            {"id": "usage-1", "session": "native-1", "at_ns": "150",
             "kind": "usage", "usage": {"input_tokens": 4, "output_tokens": 0}},
            {"id": "finished", "session": "native-1", "at_ns": "200",
             "kind": "lifecycle", "state": "completed", "reason_code": "normal"})
        with self.session() as session:
            first = session.poll_once()
            self.assertEqual(len(first.records), 3)
            self.assertEqual(session.poll_once().records, ())
            row = self.projected()[0]
            self.assertEqual((row["run_state"], row["reason_code"]), ("completed", "normal"))
            self.assertEqual(row["metadata_usage"], {"input_tokens": 4, "output_tokens": 0})
        with self.session() as resumed:
            self.assertEqual(resumed.cursor, first.cursor)
            self.assertEqual(resumed.poll_once().records, ())
            self.append({"id": "usage-1", "session": "native-1", "at_ns": "150",
                         "kind": "usage", "revision": 1,
                         "usage": {"input_tokens": 5, "output_tokens": 1}})
            corrected = resumed.poll_once()
            self.assertEqual(len(corrected.records), 1)
            self.assertEqual(self.projected()[0]["metadata_usage"],
                             {"input_tokens": 5, "output_tokens": 1})
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 3)

    def test_partial_line_waits_and_rotation_resumes_without_stale_bytes(self):
        self.source_file.write_bytes(b'{"id":"pending"')
        with self.session() as session:
            first = session.poll_once()
            self.assertEqual(first.records, ())
            self.assertTrue(first.cursor)
            with self.source_file.open("ab") as stream:
                stream.write(b',"session":"a","at_ns":"10","kind":"lifecycle","state":"running"}\n')
            self.assertEqual(session.poll_once().records[0]["event_id"], "sample:pending")
            replacement = self.root / "replacement.jsonl"
            replacement.write_text(json.dumps({"id": "new-file", "session": "b",
                                               "at_ns": "20", "kind": "lifecycle",
                                               "state": "completed"}) + "\n")
            os.replace(replacement, self.source_file)
            self.assertEqual(session.poll_once().records[0]["event_id"], "sample:new-file")
            self.assertEqual({row["native_session_id"] for row in self.projected()}, {"a", "b"})

    def test_oversized_source_line_fails_without_cursor_advance(self):
        self.source_file.write_bytes(b"x" * (16 * 1024 + 1) + b"\n")
        with self.session() as session:
            with self.assertRaises(ProtocolError):
                session.poll_once()
        with self.store._connect() as db:
            row = db.execute("SELECT cursor,dirty_seq FROM plugin_source_cursor_v1").fetchone()
            self.assertEqual(tuple(row), ("", 0))

    def test_same_inode_rewrite_at_equal_size_resets_cursor(self):
        old = {"id": "old", "session": "a", "at_ns": "10",
               "kind": "lifecycle", "state": "running"}
        new = {**old, "id": "new"}
        self.append(old)
        with self.session() as session:
            self.assertEqual(session.poll_once().records[0]["event_id"], "sample:old")
            previous_size = self.source_file.stat().st_size
            with self.source_file.open("wb") as stream:
                stream.write((json.dumps(new, separators=(",", ":")) + "\n").encode())
            self.assertEqual(self.source_file.stat().st_size, previous_size)
            self.assertEqual(session.poll_once().records[0]["event_id"], "sample:new")

    def test_unapproved_content_field_fails_without_admission(self):
        self.append({"id": "message", "session": "a", "at_ns": "10",
                     "kind": "lifecycle", "state": "running", "prompt": "private text"})
        with self.session() as session:
            with self.assertRaises(ProtocolError):
                session.poll_once()
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0], "")

    def test_installed_config_reaches_receiver_health_without_kernel_registration(self):
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
                            health = json.load(response)
                        local = health["local_agents"]
                        if local.get("sessions") and local.get("plugin_workers"):
                            break
                    except OSError:
                        pass
                    time.sleep(0.05)
                else:
                    self.fail(f"sample plugin did not reach receiver health: {failures}")
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
