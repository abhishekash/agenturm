"""Installed Rust process reader over a Claude Code main-session JSONL file."""
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


PACKAGE = Path(__file__).resolve().parents[1] / "examples" / "rust_claude_harness_plugin"


def row(kind, uuid, second, content, **message):
    return {"type": kind, "uuid": uuid, "sessionId": "claude-session",
            "timestamp": f"2026-10-24T12:00:{second:02d}Z", "cwd": "/private/work",
            "message": {"content": content, **message}}


class RustClaudeHarnessTests(unittest.TestCase):
    def setUp(self):
        executable = os.environ.get("AGENTURM_RUST_CLAUDE_BIN")
        if not executable:
            self.skipTest("Rust Claude binary is built in the Rust CI job")
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.source = self.root / "claude-session.jsonl"
        installed = subprocess.run(
            [sys.executable, str(PACKAGE / "install.py"), "--binary", executable,
             "--destination", str(self.root / "installed"), "--source", str(self.source)],
            capture_output=True, text=True, check=True)
        self.config = Path(installed.stdout.strip())
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

    def test_main_transcript_matches_native_terminal_and_usage_without_content(self):
        source_rows = [
            row("user", "u-1", 0, "private task"),
            row("assistant", "a-1", 1,
                [{"type": "text", "text": "private thought"},
                 {"type": "tool_use", "id": "call-1", "name": "Bash",
                  "input": {"command": "private command"}}],
                id="msg-1", usage={"input_tokens": 3, "output_tokens": 4,
                                   "cache_read_input_tokens": 2,
                                   "cache_creation_input_tokens": 1},
                stop_reason="tool_use"),
            row("user", "r-1", 2,
                [{"type": "tool_result", "tool_use_id": "call-1",
                  "is_error": True, "content": "private output"}]),
            row("assistant", "a-2", 3, [{"type": "text", "text": "private answer"}],
                id="msg-2", usage={"input_tokens": 2, "output_tokens": 5},
                stop_reason="end_turn"),
        ]
        self.append(*source_rows)
        native = Store(self.root / "native.db")
        self.addCleanup(native.close)
        native_reader = Session(native, self.source, "Claude Code", capture_details=False)
        native_reader.poll()
        self.assertEqual(native_reader.run_state, "completed")
        with native._connect() as db:
            usages = [json.loads(item[0]) for item in db.execute(
                "SELECT attributes FROM spans WHERE service='claude-code' "
                "AND name LIKE 'Claude Code ·%'")]
        self.assertEqual(len(usages), 2)
        self.assertEqual(sum(item["gen_ai.usage.input_tokens"] for item in usages), 8)
        self.assertEqual(sum(item["gen_ai.usage.output_tokens"] for item in usages), 9)
        with self.worker() as process:
            admitted = process.poll_once()
            self.assertEqual(len(admitted.records), 11)
            self.assertEqual(process.poll_once().records, ())
            projected = self.projected()
            self.assertEqual(len(projected), 1)
            self.assertEqual(projected[0]["native_session_id"], "claude-session")
            self.assertEqual(projected[0]["run_state"], native_reader.run_state)
            self.assertEqual(projected[0]["metadata_usage"],
                             {"input_tokens": 8, "output_tokens": 9,
                              "cache_read_tokens": 2, "cache_write_tokens": 1})
            activity = self.store.activity_page()["activity"]
            self.assertEqual(len([fact for fact in activity if fact["domain"] == "tool"]), 2)
            self.assertEqual(len({fact["operation_id"] for fact in activity
                                  if fact["domain"] == "tool"}), 1)
            self.assertEqual(len({fact["turn_id"] for fact in activity}), 1)
            self.assertEqual(len(self.store.activity_page(
                turn_id=activity[0]["turn_id"])["activity"]), len(activity))
            self.assertEqual(len([fact for fact in activity if fact["domain"] == "check"]), 0)
            serialized = json.dumps(activity)
            with self.store._connect() as db:
                stored = "".join(item[0] for item in db.execute(
                    "SELECT payload_json FROM plugin_evidence_v1"))
            for secret in ("private task", "private thought", "private command",
                           "private output", "private answer", "/private/work"):
                self.assertNotIn(secret, serialized)
                self.assertNotIn(secret, stored)
        with self.worker() as resumed:
            self.assertEqual(resumed.cursor, admitted.cursor)
            self.assertEqual(resumed.poll_once().records, ())
            set_source_capture(self.store, source_id=self.plugin.source_id, enabled=False)
            self.assertEqual(self.store.activity_page()["activity"], [])
            with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
                resumed.poll_once()

    def test_streamed_usage_correction_restart_and_same_inode_rewrite(self):
        self.append(row("user", "u-1", 0, "private task"),
                    row("assistant", "a-1", 1, [], id="msg-1",
                        usage={"input_tokens": 3, "output_tokens": 1},
                        stop_reason="tool_use"))
        inode = self.source.stat().st_ino
        with self.worker() as first:
            first_batch = first.poll_once()
            self.assertEqual(self.projected()[0]["metadata_usage"],
                             {"input_tokens": 3, "output_tokens": 1})
            turn = self.store.activity_page()["activity"][0]["turn_id"]
        self.append(row("assistant", "a-2", 2, [], id="msg-1",
                        usage={"input_tokens": 5, "output_tokens": 2},
                        stop_reason="tool_use"))
        with self.worker() as resumed:
            self.assertEqual(resumed.cursor, first_batch.cursor)
            correction = resumed.poll_once()
            self.assertTrue(any(record["kind"] == "usage"
                                and record["revision"] > 0 for record in correction.records))
            self.assertEqual(self.projected()[0]["metadata_usage"],
                             {"input_tokens": 5, "output_tokens": 2})
            self.assertEqual({fact["turn_id"] for fact in self.store.activity_page()["activity"]},
                             {turn})
        self.append(row("assistant", "a-3", 3, [], id="msg-1",
                        usage={"input_tokens": 4, "output_tokens": 1},
                        stop_reason="tool_use"))
        with self.worker() as resumed_again:
            lower = resumed_again.poll_once()
            self.assertTrue(any(record["kind"] == "usage" and
                                record["payload"]["input_tokens"] == 5 for record in lower.records))
            self.assertEqual(self.projected()[0]["metadata_usage"],
                             {"input_tokens": 5, "output_tokens": 2})
            self.source.write_text(json.dumps(row("user", "u-1", 3, "x")) + "\n")
            self.assertEqual(self.source.stat().st_ino, inode)
            changed = resumed_again.poll_once()
            self.assertEqual(changed.records[0]["payload"]["code"], "possible_source_gap")
            self.assertNotEqual(first_batch.records[0]["event_id"],
                                changed.records[1]["event_id"])
            self.projected()

    def test_missing_result_flag_refuses_before_cursor_ack(self):
        self.append(row("user", "u-1", 0,
                        [{"type": "tool_result", "tool_use_id": "call-1",
                          "content": "private output"}]))
        with self.worker() as process:
            with self.assertRaises(ProtocolError):
                process.poll_once()
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0], "")

    def test_unsupported_row_is_coarse_diagnostic(self):
        self.append({"type": "system", "uuid": "system-1", "sessionId": "claude-session",
                     "timestamp": "2026-10-24T12:00:00Z", "subtype": "private-model"})
        with self.worker() as process:
            batch = process.poll_once()
            self.assertEqual(len(batch.records), 1)
            self.assertEqual(process.last_diagnostic["code"], "unsupported_claude_row")
            self.assertEqual(self.projected(), [])
        with self.store._connect() as db:
            stored = "".join(item[0] for item in db.execute(
                "SELECT payload_json FROM plugin_evidence_v1"))
        self.assertNotIn("private-model", stored)

    def test_installer_refuses_symlink_destination(self):
        destination = self.root / "another-install"
        destination.mkdir()
        outside = self.root / "outside-executable"
        (destination / "rust-claude-harness").symlink_to(outside)
        result = subprocess.run([sys.executable, str(PACKAGE / "install.py"),
            "--binary", os.environ["AGENTURM_RUST_CLAUDE_BIN"],
            "--destination", str(destination), "--source", str(self.source)],
            capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(outside.exists())
        link = self.root / "directory-link"
        link.symlink_to(self.root / "installed", target_is_directory=True)
        result = subprocess.run([sys.executable, str(PACKAGE / "install.py"),
            "--binary", os.environ["AGENTURM_RUST_CLAUDE_BIN"],
            "--destination", str(link / "nested"), "--source", str(self.source)],
            capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "installed" / "nested").exists())


if __name__ == "__main__":
    unittest.main()
