"""Installed Rust Codex rollout reader through the real Store admission path."""
import json
import os
import shutil
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


ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "examples" / "rust_codex_harness_plugin"
NATIVE_FIXTURE = ROOT / "tests" / "fixtures" / "native_review" / "codex-0.167.1.synthetic.jsonl"


def row(kind, second, payload):
    return {"type": kind, "timestamp": f"2026-11-14T10:00:{second:02d}Z",
            "payload": payload}


class RustCodexHarnessTests(unittest.TestCase):
    def setUp(self):
        executable = os.environ.get("AGENTURM_RUST_CODEX_BIN")
        if not executable:
            self.skipTest("Rust Codex binary is built in the Rust CI job")
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.source = self.root / "codex-session.jsonl"
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

    def test_current_codex_rollout_fixture_matches_explicit_native_outcome(self):
        shutil.copyfile(NATIVE_FIXTURE, self.source)
        native = Store(self.root / "native.db")
        self.addCleanup(native.close)
        native_reader = Session(native, self.source, "Codex", capture_details=False)
        native_reader.poll()
        self.assertEqual(native_reader.run_state, "completed")
        with self.worker() as worker:
            batch = worker.poll_once()
            self.assertEqual(len(batch.records), 12)
            self.assertEqual(worker.poll_once().records, ())
            sessions = self.projected()
            self.assertEqual(len(sessions), 1)
            self.assertEqual(sessions[0]["run_state"], native_reader.run_state)
            self.assertEqual(sessions[0]["reason_code"], "codex_task_complete")
            self.assertEqual(sessions[0]["metadata_usage"],
                             {"input_tokens": 123, "output_tokens": 45,
                              "cache_read_tokens": 100, "cache_write_tokens": 0,
                              "total_tokens": 168})
            activity = self.store.activity_page()["activity"]
            self.assertEqual(len([fact for fact in activity if fact["domain"] == "message"]), 2)
            self.assertEqual(len([fact for fact in activity if fact["domain"] == "tool"]), 3)
            checks = [fact for fact in activity if fact["domain"] == "check"]
            self.assertEqual({(fact["action"], fact["result"]) for fact in checks},
                             {("lint", "passed"), ("test", "passed")})
            turns = {fact["turn_id"] for fact in activity}
            self.assertEqual(len(turns), 1)
            turn = turns.pop()
            self.assertEqual(len(turn), 32)
            self.assertEqual(len(self.store.activity_page(turn_id=turn)["activity"]), len(activity))
            self.assertIn("Coverage: partial", copy_event_text(checks[0]))
            with self.store._connect() as db:
                admitted = "".join(item[0] for item in db.execute(
                    "SELECT payload_json FROM plugin_evidence_v1"))
            for private in ("Current schema fixture", "npm run check", "Synthetic successful",
                            "/synthetic/project", "synthetic-response"):
                self.assertNotIn(private, json.dumps(activity) + admitted)
        with self.worker() as resumed:
            self.assertEqual(resumed.cursor, batch.cursor)
            self.assertEqual(resumed.poll_once().records, ())
            set_source_capture(self.store, source_id=self.plugin.source_id, enabled=False)
            self.assertEqual(self.store.activity_page()["activity"], [])
            with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
                resumed.poll_once()

    def test_pr_text_does_not_complete_and_abort_reason_is_explicit(self):
        self.append({"type": "session_meta", "payload": {"id": "thread-1"}},
                    row("event_msg", 1, {"type": "task_started", "turn_id": "turn-1"}),
                    row("event_msg", 2, {"type": "item_completed", "turn_id": "turn-1",
                                          "item": {"type": "AgentMessage", "phase": "commentary",
                                                   "content": "PR raised; more work remains"}}))
        with self.worker() as worker:
            worker.poll_once()
            self.assertEqual(self.projected()[0]["run_state"], "running")
            self.append(row("event_msg", 3, {"type": "task_aborted", "turn_id": "turn-1",
                                             "reason": "private raw reason"}))
            worker.poll_once()
            self.assertEqual(self.projected()[0]["run_state"], "interrupted")
            self.assertEqual(self.projected()[0]["reason_code"], "codex_task_aborted")
            with self.store._connect() as db:
                retained = "".join(item[0] for item in db.execute(
                    "SELECT payload_json FROM plugin_evidence_v1"))
            self.assertNotIn("PR raised", retained)
            self.assertNotIn("private raw reason", retained)
            self.append(row("event_msg", 4, {"type": "task_complete", "turn_id": "turn-1"}))
            worker.poll_once()
            self.assertEqual(self.projected()[0]["run_state"], "completed")
            self.assertEqual(self.projected()[0]["reason_code"], "codex_task_complete")

    def test_call_result_correlation_and_missing_result_is_not_success(self):
        self.append({"type": "session_meta", "payload": {"id": "thread-1"}},
                    row("event_msg", 1, {"type": "task_started", "turn_id": "turn-1"}),
                    row("response_item", 2, {"type": "function_call", "call_id": "call-1",
                                             "name": "functions.exec", "arguments": "private command"}),
                    row("response_item", 3, {"type": "function_call_output", "call_id": "call-1",
                                             "isError": False, "output": "private output"}),
                    row("response_item", 4, {"type": "function_call_output", "call_id": "call-2",
                                             "output": "looks successful"}))
        with self.worker() as worker:
            worker.poll_once()
            self.projected()
            activity = self.store.activity_page()["activity"]
            tools = [fact for fact in activity if fact["domain"] == "tool"]
            self.assertEqual(len(tools), 2)
            self.assertEqual({fact["result"] for fact in tools}, {"started", "succeeded"})
            self.assertEqual(len({fact["operation_id"] for fact in tools}), 1)
            self.assertEqual(worker.last_diagnostic["code"], "codex_tool_result_unknown")
            self.assertNotIn("private", json.dumps(activity))
            self.assertNotIn("looks successful", json.dumps(activity))

    def test_malformed_usage_fails_without_ack_and_same_inode_rewrite_has_new_generation(self):
        self.append({"type": "session_meta", "payload": {"id": "thread-1"}},
                    row("token_usage_record", 1, {"response_id": "r1",
                                                  "usage": {"input_tokens": "bad"}}))
        with self.worker() as worker:
            with self.assertRaises(ProtocolError):
                worker.poll_once()
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0], "")
        self.source.write_text(json.dumps({"type": "session_meta", "payload": {"id": "thread-1"}}) + "\n"
                               + json.dumps(row("event_msg", 2, {"type": "task_started",
                                                                 "turn_id": "turn-1"})) + "\n")
        inode = self.source.stat().st_ino
        with self.worker() as worker:
            first = worker.poll_once()
            self.projected()
            self.source.write_text(json.dumps({"type": "session_meta", "payload": {"id": "thread-1"}}) + "\n"
                                   + json.dumps(row("event_msg", 3, {"type": "task_aborted",
                                                                     "turn_id": "turn-1"})) + "\n")
            self.assertEqual(self.source.stat().st_ino, inode)
            second = worker.poll_once()
            self.assertEqual(second.records[0]["payload"]["code"], "possible_source_gap")
            self.assertNotEqual(first.records[0]["event_id"], second.records[1]["event_id"])
            session = self.projected()[0]
            self.assertEqual((session["run_state"], session["reason_code"]),
                             ("unknown", "source_gap"))

    def test_installer_refuses_existing_symlinked_artifact(self):
        destination = self.root / "other-install"
        destination.mkdir()
        outside = self.root / "outside"
        (destination / "rust-codex-harness").symlink_to(outside)
        result = subprocess.run([sys.executable, str(PACKAGE / "install.py"),
                                 "--binary", os.environ["AGENTURM_RUST_CODEX_BIN"],
                                 "--destination", str(destination), "--source", str(self.source)],
                                capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(outside.exists())
        target = self.root / "ancestor-target"
        target.mkdir()
        linked_parent = self.root / "ancestor-link"
        linked_parent.symlink_to(target, target_is_directory=True)
        other = subprocess.run([sys.executable, str(PACKAGE / "install.py"),
                                "--binary", os.environ["AGENTURM_RUST_CODEX_BIN"],
                                "--destination", str(linked_parent / "nested"),
                                "--source", str(self.source)],
                               capture_output=True, text=True)
        self.assertNotEqual(other.returncode, 0)
        self.assertEqual(list(target.iterdir()), [])

    def test_repeated_usage_response_id_never_adds_a_changed_counter_twice(self):
        self.append({"type": "session_meta", "payload": {"id": "thread-1"}},
                    row("event_msg", 1, {"type": "task_started", "turn_id": "turn-1"}),
                    row("token_usage_record", 2, {"turn_id": "turn-1", "response_id": "response-1",
                                                  "usage": {"input_tokens": 7}}))
        with self.worker() as worker:
            first = worker.poll_once()
            self.assertEqual(self.projected()[0]["metadata_usage"], {"input_tokens": 7})
            self.append(row("token_usage_record", 3, {"turn_id": "turn-1",
                                                      "response_id": "response-1",
                                                      "usage": {"input_tokens": 9}}))
            with self.assertRaises(ProtocolError):
                worker.poll_once()
            with self.store._connect() as db:
                self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0],
                                 first.cursor)
            self.assertEqual(self.projected()[0]["metadata_usage"], {"input_tokens": 7})

    def test_conflicting_explicit_terminal_events_are_unknown(self):
        self.append({"type": "session_meta", "payload": {"id": "thread-1"}},
                    row("event_msg", 1, {"type": "task_started", "turn_id": "turn-1"}),
                    row("event_msg", 2, {"type": "task_complete", "turn_id": "turn-1"}),
                    row("event_msg", 2, {"type": "task_aborted", "turn_id": "turn-1"}))
        with self.worker() as worker:
            worker.poll_once()
            session = self.projected()[0]
            self.assertEqual(session["run_state"], "unknown")
            self.assertEqual(session["reason_code"], "ambiguous_lifecycle")

    def test_late_old_turn_usage_does_not_rewind_following_unmarked_activity(self):
        self.append({"type": "session_meta", "payload": {"id": "thread-1"}},
                    row("event_msg", 1, {"type": "task_started", "turn_id": "turn-1"}),
                    row("event_msg", 2, {"type": "task_started", "turn_id": "turn-2"}),
                    row("token_usage_record", 3, {"turn_id": "turn-1", "response_id": "late-r1",
                                                  "usage": {"input_tokens": 4}}),
                    row("response_item", 4, {"type": "function_call", "call_id": "new-call-1",
                                             "name": "functions.exec", "arguments": "private"}))
        with self.worker() as first:
            first_batch = first.poll_once()
            self.assertTrue(first_batch.cursor.endswith(":turn-2"))
            self.projected()
            first_tool = [fact for fact in self.store.activity_page()["activity"]
                          if fact["domain"] == "tool"][0]
            new_turn = first_tool["turn_id"]
            self.assertEqual(self.projected()[0]["run_state"], "running")
        self.append(row("response_item", 5, {"type": "function_call", "call_id": "new-call-2",
                                             "name": "functions.exec"}),
                    row("event_msg", 6, {"type": "task_complete", "turn_id": "turn-1"}))
        with self.worker() as resumed:
            second_batch = resumed.poll_once()
            self.assertTrue(second_batch.cursor.endswith(":turn-2"))
            self.projected()
            tools = [fact for fact in self.store.activity_page()["activity"]
                     if fact["domain"] == "tool"]
            self.assertEqual(len(tools), 2)
            self.assertEqual({fact["turn_id"] for fact in tools}, {new_turn})
            self.assertEqual(self.projected()[0]["run_state"], "running")
            self.assertEqual(resumed.last_diagnostic["code"], "codex_historical_outcome")


if __name__ == "__main__":
    unittest.main()
