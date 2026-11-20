"""Installed Codex directory reader through durable per-file Store admission."""
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory.plugin_admission import member_cursors
from observatory.plugin_config import read_plugin_config
from observatory.plugin_projection import project_source
from observatory.plugin_runtime import PluginSession
from observatory.plugin_supervisor import PluginSupervisor
from observatory.source_coverage import process_source_availability
from observatory.plugin_wire import ProtocolError, decode_message, encode_message


ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "examples" / "rust_codex_harness_plugin" / "install.py"


def rollout(path, session):
    rows = [
        {"type": "session_meta", "payload": {"id": session}},
        {"type": "event_msg", "timestamp": "2026-11-14T10:00:01Z",
         "payload": {"type": "task_started", "turn_id": "turn-1"}},
        {"type": "event_msg", "timestamp": "2026-11-14T10:00:02Z",
         "payload": {"type": "item_completed", "turn_id": "turn-1",
                     "item": {"type": "UserMessage", "id": "user-one",
                              "content": "private prompt"}}},
        {"type": "event_msg", "timestamp": "2026-11-14T10:00:03Z",
         "payload": {"type": "task_complete", "turn_id": "turn-1"}},
    ]
    path.write_text("".join(json.dumps(row, separators=(",", ":")) + "\n" for row in rows))


class CodexDirectoryTests(unittest.TestCase):
    def setUp(self):
        binary = os.environ.get("AGENTURM_RUST_CODEX_BIN")
        if not binary:
            self.skipTest("installed Rust Codex binary required")
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.sources = self.root / "sessions"
        (self.sources / "2026" / "11").mkdir(parents=True)
        self.first = self.sources / "2026" / "11" / "one.jsonl"
        self.second = self.sources / "2026" / "11" / "two.jsonl"
        rollout(self.first, "session-one")
        rollout(self.second, "session-two")
        result = subprocess.run(
            [sys.executable, str(INSTALLER), "--binary", binary,
             "--destination", str(self.root / "installed"), "--source", str(self.sources),
             "--directory"], check=True, capture_output=True, text=True)
        self.plugin = read_plugin_config(result.stdout.strip())[0]
        self.assertEqual(self.plugin.required_features, ("source-discovery/1",))
        self.store = Store(self.root / "store.db")
        self.addCleanup(self.store.close)

    def worker(self):
        return PluginSession(self.store, self.plugin.manifest,
                             source_id=self.plugin.source_id,
                             allowed_grants=self.plugin.allowed_grants,
                             timeout=self.plugin.timeout,
                             source_root=self.plugin.source_root,
                             required_features=self.plugin.runtime_features)

    def sessions(self):
        project_source(self.store, self.plugin.source_id)
        return {row["native_session_id"]: row for row in
                self.store.plugin_sessions()["sessions"]}

    def test_root_unavailable_then_restored_keeps_last_outcomes_and_recovers_coverage(self):
        source = replace(self.plugin, poll_interval=0.05)
        manager = PluginSupervisor(self.store, (source,), max_failures=20,
                                   backoff_seconds=0.2).start()
        self.addCleanup(manager.close)

        def until(predicate, seconds=8):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                status = manager.snapshot()[0]
                if predicate(status):
                    return status
                time.sleep(0.02)
            self.fail(f"source did not reach expected condition: {manager.snapshot()}")

        healthy = until(lambda status: status["state"] == "running"
                        and status["last_success_ns"] > 0)
        self.assertEqual({row["run_state"] for row in self.sessions().values()},
                         {"completed"})
        moved = self.root / "sessions-unavailable"
        self.sources.rename(moved)
        try:
            failed = until(lambda status: status["state"] == "degraded"
                           and status["last_failure_ns"] > healthy["last_success_ns"])
            self.assertEqual(process_source_availability([failed],
                             self.store.plugin_sessions()["sessions"])[0]["availability"],
                             "unavailable")
            self.assertEqual({row["run_state"] for row in self.sessions().values()},
                             {"completed"})
        finally:
            moved.rename(self.sources)
        recovered = until(lambda status: status["state"] == "running"
                          and status["last_success_ns"] > status["last_failure_ns"])
        self.assertEqual(process_source_availability([recovered],
                         self.store.plugin_sessions()["sessions"])[0]["availability"],
                         "ready")

    def test_files_resume_disappear_and_new_session_after_gap(self):
        with self.worker() as worker:
            changes = [worker.poll_once() for _ in range(2)]
            self.assertTrue(all(batch.member_id and batch.member_cursor for batch in changes))
            self.assertEqual(len(worker.members), 2)
            self.assertEqual(worker.poll_once().records, ())
            self.assertEqual(set(self.sessions()), {"session-one", "session-two"})
            self.assertEqual({row["run_state"] for row in self.sessions().values()},
                             {"completed"})
            self.assertEqual(len(self.store.activity_page()["activity"]), 2)
            previous = dict(worker.members)
        with self.worker() as worker:
            self.assertEqual(worker.members, previous)
            self.assertEqual(worker.poll_once().records, ())
            self.first.unlink()
            missing = worker.poll_once()
            self.assertEqual(missing.records[0]["payload"]["code"], "possible_source_gap")
            self.assertEqual(missing.member_cursor, "")
            self.assertEqual(len(member_cursors(self.store, worker.lease)), 1)
            invalidated = self.sessions()
            self.assertEqual({row["reason_code"] for row in invalidated.values()},
                             {"source_gap"})
            self.assertEqual(self.store.activity_page()["activity"], [])
            third = self.sources / "2026" / "11" / "three.jsonl"
            rollout(third, "session-three")
            worker.poll_once()
            current = self.sessions()
            self.assertEqual(current["session-three"]["run_state"], "completed")
            self.assertEqual(current["session-one"]["run_state"], "unknown")
            self.assertEqual(current["session-two"]["run_state"], "unknown")
            self.assertEqual(len(self.store.activity_page()["activity"]), 1)
        with self.store._connect() as db:
            retained = "".join(row[0] for row in db.execute(
                "SELECT payload_json FROM plugin_evidence_v1"))
        self.assertNotIn(str(self.sources), retained)

    def test_same_length_rewrite_of_one_file_reports_gap(self):
        with self.worker() as worker:
            worker.poll_once()
            worker.poll_once()
            self.assertEqual(set(self.sessions()), {"session-one", "session-two"})
            before = self.second.stat().st_size
            rollout(self.second, "session-new")
            self.assertEqual(self.second.stat().st_size, before)
            changed = None
            for _ in range(3):
                batch = worker.poll_once()
                if any(row["kind"] == "diagnostic" for row in batch.records):
                    changed = batch
                    break
            self.assertIsNotNone(changed)
            self.assertEqual(changed.records[0]["payload"]["code"], "possible_source_gap")
            sessions = self.sessions()
            self.assertEqual(sessions["session-new"]["run_state"], "completed")
            self.assertEqual(sessions["session-two"]["run_state"], "unknown")
            self.assertEqual(len(self.store.activity_page()["activity"]), 1)

    def test_five_incomplete_files_advance_scan_position_without_evidence(self):
        self.first.unlink()
        self.second.unlink()
        for index in range(5):
            (self.sources / f"incomplete-{index}.jsonl").write_bytes(b"")
        with self.worker() as worker:
            first = worker.poll_once()
            second = worker.poll_once()
            self.assertEqual(first.records, ())
            self.assertEqual(second.records, ())
            self.assertEqual((first.member_id, second.member_id), ("", ""))
            self.assertTrue(first.cursor.startswith("d1:1:"))
            self.assertTrue(second.cursor.startswith("d1:2:"))
            self.assertEqual(worker.members, {})
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 0)

    def test_checkpoint_load_failure_closes_directly_started_worker(self):
        worker = self.worker()
        with patch("observatory.plugin_runtime.member_cursors",
                   side_effect=ProtocolError("damaged member checkpoints")):
            with self.assertRaisesRegex(ProtocolError, "damaged member checkpoints"):
                worker.start()
        self.assertIsNone(worker.process.process)
        self.assertFalse(worker.process._slot_held)
        self.assertIsNone(worker.lease)

    def test_near_structural_limit_pages_without_restarting_same_cursor(self):
        rows = [{"type": "session_meta", "payload": {"id": "many-messages"}},
                {"type": "event_msg", "timestamp": "2026-11-14T10:00:01Z",
                 "payload": {"type": "task_started", "turn_id": "turn-1"}}]
        rows.extend({"type": "event_msg", "timestamp": "2026-11-14T10:00:02Z",
                     "payload": {"type": "item_completed", "turn_id": "turn-1",
                                 "item": {"type": "UserMessage", "id": f"user-{n:03}",
                                          "content": "private prompt"}}}
                    for n in range(300))
        rows.append({"type": "event_msg", "timestamp": "2026-11-14T10:00:03Z",
                     "payload": {"type": "task_complete", "turn_id": "turn-1"}})
        self.first.write_text("".join(json.dumps(row, separators=(",", ":")) + "\n"
                                      for row in rows))
        self.second.unlink()
        batches = []
        with self.worker() as worker:
            for _ in range(20):
                batch = worker.poll_once()
                if not batch.member_id:
                    break
                batches.append(batch)
            else:
                self.fail("bounded discovery never reached an idle poll")
            self.assertEqual(len(worker.members), 1)
            self.assertEqual(worker.poll_once().records, ())
        all_records = [row for batch in batches for row in batch.records]
        messages = [row for row in all_records if row["kind"] == "message"]
        self.assertEqual(len(messages), 300)
        self.assertGreater(len(batches), 1)
        with self.assertRaisesRegex(ProtocolError, "structural token limit"):
            decode_message(encode_message({"type": "batch", "records": all_records}))


if __name__ == "__main__":
    unittest.main()
