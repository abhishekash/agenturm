"""A separately installed Rust process reader over actual Pi session JSONL."""
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from observatory.core import Store
from observatory.local_agents import Session
from observatory.plugin_admission import set_source_capture
from observatory.plugin_config import read_plugin_config
from observatory.plugin_projection import project_source
from observatory.plugin_runtime import PluginSession
from observatory.plugin_wire import ProtocolError
from observatory.terminal_ui import copy_event_text


PACKAGE = Path(__file__).resolve().parents[1] / "examples" / "rust_pi_harness_plugin"


def row(id, second, message):
    return {"type": "message", "id": id,
            "timestamp": f"2026-10-24T12:00:{second:02d}Z", "message": message}


class RustPiHarnessTests(unittest.TestCase):
    def setUp(self):
        executable = os.environ.get("AGENTURM_RUST_PI_BIN")
        if not executable:
            self.skipTest("Rust Pi binary is built in the Rust CI job")
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.source = self.root / "pi-session.jsonl"
        output = subprocess.run(
            [sys.executable, str(PACKAGE / "install.py"), "--binary", executable,
             "--destination", str(self.root / "installed"), "--source", str(self.source)],
            capture_output=True, text=True, check=True)
        self.config = Path(output.stdout.strip())
        self.plugin = read_plugin_config(self.config)[0]
        self.assertEqual(self.plugin.replaces, ())
        self.store = Store(self.root / "process.db")
        self.addCleanup(self.store.close)

    def append(self, *rows):
        with self.source.open("a") as stream:
            for item in rows:
                stream.write(json.dumps(item, separators=(",", ":")) + "\n")

    def worker(self):
        return PluginSession(self.store, self.plugin.manifest,
                             source_id=self.plugin.source_id,
                             allowed_grants=self.plugin.allowed_grants,
                             timeout=self.plugin.timeout,
                             source_root=self.plugin.source_root)

    def projected(self):
        project_source(self.store, self.plugin.source_id)
        return self.store.plugin_sessions()["sessions"]

    def test_actual_pi_file_matches_native_observations_without_content(self):
        source_rows = [
            {"type": "session", "id": "pi-session", "cwd": "/private/work"},
            row("user-1", 0, {"role": "user", "content": [{"type": "text", "text": "private task"}]}),
            row("assistant-1", 1, {"role": "assistant", "stopReason": "toolUse",
                                 "usage": {"input": 8, "output": 3, "cacheRead": 2},
                                 "content": [{"type": "toolCall", "id": "call-1", "name": "read",
                                              "arguments": {"path": "/private/work"}},
                                             {"type": "toolCall", "id": "call-2", "name": "bash",
                                              "arguments": {"command": "private command"}}]}),
            row("tool-1", 2, {"role": "toolResult", "toolCallId": "call-1",
                              "toolName": "read", "isError": True,
                              "content": [{"type": "text", "text": "private output"}]}),
            row("tool-2", 3, {"role": "toolResult", "toolCallId": "call-2",
                              "toolName": "bash", "isError": False,
                              "content": [{"type": "text", "text": "private output"}]}),
            row("assistant-2", 4, {"role": "assistant", "stopReason": "error", "content": []}),
        ]
        self.append(*source_rows)
        native = Store(self.root / "native.db")
        self.addCleanup(native.close)
        native_reader = Session(native, self.source, "Pi", capture_details=False)
        native_reader.poll()
        self.assertEqual(native_reader.run_state, "interrupted")
        native_activity = native.activity_page()["activity"]
        self.assertEqual(len([item for item in native_activity if item["domain"] == "tool"]), 4)
        with native._connect() as db:
            native_usage = [json.loads(item[0]) for item in db.execute(
                "SELECT attributes FROM spans WHERE service='pi' AND name LIKE 'Pi ·%'")]
        self.assertEqual(len(native_usage), 1)
        self.assertEqual(native_usage[0]["gen_ai.usage.input_tokens"], 8)
        self.assertEqual(native_usage[0]["gen_ai.usage.output_tokens"], 3)
        self.assertEqual(native_usage[0]["gen_ai.usage.cache_read.input_tokens"], 2)
        with self.worker() as process:
            admitted = process.poll_once()
            self.assertEqual(len(admitted.records), 13)
            self.assertEqual(process.poll_once().records, ())
            projected = self.projected()
            self.assertEqual(len(projected), 1)
            self.assertEqual(projected[0]["native_session_id"], "pi-session")
            self.assertEqual(projected[0]["run_state"], native_reader.run_state)
            self.assertEqual(projected[0]["metadata_usage"],
                             {"input_tokens": 8, "output_tokens": 3,
                              "cache_read_tokens": 2})
            activity = self.store.activity_page()["activity"]
            tools = [item for item in activity if item["domain"] == "tool"]
            self.assertEqual(len(tools), 4)
            self.assertEqual({item["result"] for item in tools},
                             {"started", "failed", "succeeded"})
            self.assertEqual(len({item["operation_id"] for item in tools}), 2)
            self.assertEqual(len({item["turn_id"] for item in activity}), 1)
            turn = activity[0]["turn_id"]
            self.assertEqual(len(self.store.activity_page(turn_id=turn)["activity"]), len(activity))
            self.assertEqual(len(self.store.activity_page(review=True)["activity"]), 4)
            self.assertIn("Coverage: partial", copy_event_text(tools[0]))
            serialized = json.dumps(activity)
            for secret in ("private task", "private command", "private output", "/private/work"):
                self.assertNotIn(secret, serialized)
            with self.store._connect() as db:
                admitted_json = "".join(item[0] for item in db.execute(
                    "SELECT payload_json FROM plugin_evidence_v1"))
            self.assertNotIn("private", admitted_json)
        with self.worker() as resumed:
            self.assertEqual(resumed.cursor, admitted.cursor)
            self.assertEqual(resumed.poll_once().records, ())
            set_source_capture(self.store, source_id=self.plugin.source_id, enabled=False)
            self.assertEqual(self.store.activity_page()["activity"], [])
            with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
                resumed.poll_once()

    def test_append_after_restart_preserves_turn_and_rotation_reports_gap(self):
        self.append({"type": "session", "id": "pi-session"},
                    row("user-1", 0, {"role": "user"}))
        with self.worker() as first:
            initial = first.poll_once()
            self.projected()
            turn = self.store.activity_page()["activity"][0]["turn_id"]
        self.append(row("assistant-1", 1, {"role": "assistant", "stopReason": "stop"}))
        with self.worker() as resumed:
            self.assertEqual(resumed.cursor, initial.cursor)
            next_batch = resumed.poll_once()
            self.assertEqual(len(next_batch.records), 2)
            self.projected()
            self.assertEqual(self.store.plugin_sessions()["sessions"][0]["run_state"], "completed")
            self.assertEqual({item["turn_id"] for item in self.store.activity_page()["activity"]}, {turn})
            self.append(row("tool-late", 2, {"role": "toolResult", "toolCallId": "late-call",
                                             "toolName": "read", "isError": False}))
            self.assertEqual(len(resumed.poll_once().records), 2)
            self.projected()
            self.assertEqual(self.store.plugin_sessions()["sessions"][0]["run_state"], "running")
            replacement = self.root / "next.jsonl"
            replacement.write_text(json.dumps({"type": "session", "id": "next-session"}) + "\n"
                                   + json.dumps(row("user-2", 3, {"role": "user"})) + "\n")
            os.replace(replacement, self.source)
            changed = resumed.poll_once()
            self.assertEqual(changed.records[0]["payload"]["code"], "possible_source_gap")
            self.assertEqual(changed.records[1]["identity"]["session_id"], "next-session")
            self.projected()
            self.assertEqual({item["native_session_id"] for item in self.store.plugin_sessions()["sessions"]},
                             {"pi-session", "next-session"})
            self.assertNotEqual(self.store.activity_page()["activity"][0]["turn_id"], turn)

    def test_bad_pi_result_fails_before_cursor_ack(self):
        self.append({"type": "session", "id": "pi-session"},
                    row("tool-1", 1, {"role": "toolResult", "toolCallId": "call-1",
                                      "toolName": "read", "content": "private output"}))
        with self.worker() as worker:
            with self.assertRaises(ProtocolError):
                worker.poll_once()
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0], "")

    def test_same_inode_truncate_reuses_row_id_in_new_generation(self):
        self.append({"type": "session", "id": "pi-session"},
                    row("shared", 0, {"role": "user"}))
        inode = self.source.stat().st_ino
        with self.worker() as worker:
            first = worker.poll_once()
            self.assertEqual(len(first.records), 2)
            self.source.write_text(
                json.dumps({"type": "session", "id": "pi-session"}) + "\n"
                + json.dumps(row("shared", 1, {"role": "assistant",
                                               "stopReason": "stop"})) + "\n")
            self.assertEqual(self.source.stat().st_ino, inode)
            second = worker.poll_once()
            self.assertEqual(second.records[0]["payload"]["code"], "possible_source_gap")
            self.assertEqual(len(second.records), 3)
            self.assertNotEqual(first.records[0]["event_id"], second.records[1]["event_id"])
            self.projected()
            session = self.store.plugin_sessions()["sessions"][0]
            self.assertEqual((session["run_state"], session["reason_code"]),
                             ("unknown", "source_gap"))
            self.assertEqual(self.store.activity_page()["activity"], [])

    def test_distant_same_length_rewrite_quarantines_prior_evidence(self):
        self.append({"type": "session", "id": "pi-session"},
                    row("user-1", 0, {"role": "user"}),
                    *({"type": "model_change", "id": f"model-{index:02}",
                       "timestamp": "2026-10-24T12:00:01Z"}
                      for index in range(20)))
        inode = self.source.stat().st_ino
        with self.worker() as worker:
            first = worker.poll_once()
            self.projected()
            self.assertEqual(len(self.store.activity_page()["activity"]), 1)
            raw = self.source.read_text()
            self.assertGreater(len(raw), 512)
            self.source.write_text(raw.replace("model-03", "model-XX", 1))
            self.assertEqual(self.source.stat().st_ino, inode)
            self.assertEqual(self.source.stat().st_size, len(raw))
            second = worker.poll_once()
            self.assertNotEqual(first.cursor, second.cursor)
            self.assertEqual(second.records[0]["payload"]["code"],
                             "possible_source_gap")
            self.projected()
            session = self.store.plugin_sessions()["sessions"][0]
            self.assertEqual((session["run_state"], session["reason_code"]),
                             ("unknown", "source_gap"))
            self.assertEqual(self.store.activity_page()["activity"], [])

    def test_unsupported_row_is_visible_as_coarse_diagnostic(self):
        self.append({"type": "session", "id": "pi-session"},
                    {"type": "model_change", "id": "model-1",
                     "timestamp": "2026-10-24T12:00:01Z", "modelId": "private-model"})
        with self.worker() as worker:
            batch = worker.poll_once()
            self.assertEqual(len(batch.records), 1)
            self.assertEqual(worker.last_diagnostic["code"], "unsupported_pi_row")
            self.assertEqual(self.projected(), [])
        with self.store._connect() as db:
            stored = "".join(item[0] for item in db.execute(
                "SELECT payload_json FROM plugin_evidence_v1"))
        self.assertNotIn("private-model", stored)

    def test_installer_refuses_dangling_artifact_symlink(self):
        destination = self.root / "second-install"
        destination.mkdir()
        outside = self.root / "outside-executable"
        (destination / "rust-pi-harness").symlink_to(outside)
        result = subprocess.run(
            [sys.executable, str(PACKAGE / "install.py"),
             "--binary", os.environ["AGENTURM_RUST_PI_BIN"],
             "--destination", str(destination), "--source", str(self.source)],
            capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(outside.exists())


if __name__ == "__main__":
    unittest.main()
