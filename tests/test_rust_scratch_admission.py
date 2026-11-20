"""Feature-off Rust scratch transaction against disposable Python Store fixtures."""
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import (_validated, activate_source, commit_batch,
                                          set_source_capture)
from observatory.plugin_wire import PollBatch


SOURCE = "scratch-source"
PLUGIN = "fixture.plugin"
GENERATION = "a" * 64


def fact(event="first", state="started"):
    return {"event_id": event, "revision": 0, "kind": "lifecycle",
            "time_ns": "9007199254740993", "payload": {"state": state},
            "identity": {"session_id": "native-one"}}


class RustScratchAdmissionTests(unittest.TestCase):
    def setUp(self):
        binary = os.environ.get("AGENTURM_RUST_SCRATCH_ADMISSION_BIN")
        if not binary:
            self.skipTest("installed Rust scratch admission binary required")
        self.binary = str(Path(binary).resolve(strict=True))
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.rust_store = Store(self.root / "rust.db")
        self.addCleanup(self.rust_store.close)
        self.python_store = Store(self.root / "python.db")
        self.addCleanup(self.python_store.close)
        self.rust_lease = activate_source(self.rust_store, plugin_id=PLUGIN,
                                          source_id=SOURCE, generation=GENERATION)
        self.python_lease = activate_source(self.python_store, plugin_id=PLUGIN,
                                            source_id=SOURCE, generation=GENERATION)
        with self.rust_store._connect() as db:
            db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                       ("storage_admission_scratch_v1", "enabled"))

    def request(self, batch, expected="", *, lease=None, path=None):
        digest, rows, checked = _validated(batch, SOURCE)
        owner = lease or self.rust_lease
        return {"schema_version": 1, "sqlite_path": str(path or self.rust_store.path),
                "lease": {"receiver_id": owner.receiver_id, "plugin_id": owner.plugin_id,
                          "source_id": owner.source_id, "generation": owner.generation,
                          "epoch": owner.epoch, "policy_epoch": owner.policy_epoch},
                "expected_cursor": expected, "cursor": checked.cursor,
                "batch_digest": digest,
                "records": [{"event_id_hash": row[1], "revision": row[2],
                             "kind": row[3], "time_ns": row[4], "payload_json": row[5],
                             "record_digest": row[6], "native_session_id": row[7],
                             "native_worker_id": row[8], "native_turn_id": row[9],
                             "native_operation_id": row[10]} for row in rows]}

    def apply(self, request, success=True):
        result = subprocess.run([self.binary], input=json.dumps(request), text=True,
                                capture_output=True, timeout=10)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("scratch admission rejected", result.stderr)
        return result.stderr

    @staticmethod
    def snapshot(store):
        with store._connect() as db:
            cursor = tuple(db.execute("SELECT cursor,prior_cursor,last_batch_digest,dirty_seq "
                                      "FROM plugin_source_cursor_v1 WHERE source_id=?",
                                      (SOURCE,)).fetchone())
            evidence = [tuple(row) for row in db.execute(
                "SELECT event_id_hash,revision,kind,time_ns,payload_json,record_digest,"
                "native_session_id,native_worker_id,native_turn_id,native_operation_id,"
                "member_id,policy_epoch FROM plugin_evidence_v1 WHERE source_id=? "
                "ORDER BY event_id_hash", (SOURCE,))]
            dirty = [tuple(row) for row in db.execute(
                "SELECT source_id,dirty_seq FROM plugin_evidence_dirty_v1 WHERE source_id=?",
                (SOURCE,))]
            receipt = tuple(db.execute("SELECT cursor,refs_digest FROM plugin_content_batch_v1 "
                                       "WHERE source_id=?", (SOURCE,)).fetchone() or ())
            status = tuple(db.execute("SELECT cursor,reason,refs_count,policy_epoch "
                                      "FROM plugin_content_status_v1 WHERE source_id=?",
                                      (SOURCE,)).fetchone() or ())
        return cursor, evidence, dirty, receipt, status

    def test_simple_batch_and_exact_replay_match_python_admission(self):
        batch = PollBatch(SOURCE, "one", (fact(), fact("second", "completed")))
        request = self.request(batch)
        self.assertEqual(self.apply(request)["disposition"], "committed")
        self.assertEqual(commit_batch(self.python_store, self.python_lease, "", batch), "one")
        expected = self.snapshot(self.python_store)
        self.assertEqual(self.snapshot(self.rust_store), expected)
        self.assertEqual(self.apply(request)["disposition"], "replay")
        self.assertEqual(commit_batch(self.python_store, self.python_lease, "", batch), "one")
        self.assertEqual(self.snapshot(self.rust_store), expected)
        self.assertEqual(self.snapshot(self.python_store), expected)
        idle = self.request(PollBatch(SOURCE, "one", ()), expected="one")
        self.assertEqual(self.apply(idle)["disposition"], "idle")
        self.assertEqual(self.snapshot(self.rust_store), expected)

    def test_late_conflict_and_stale_cursor_roll_back_whole_batch(self):
        first = PollBatch(SOURCE, "one", (fact(),))
        self.apply(self.request(first))
        before = self.snapshot(self.rust_store)
        late = PollBatch(SOURCE, "two", (fact("new"), fact("first", "completed")))
        self.assertIn("conflicting", self.apply(self.request(late, "one"), success=False))
        self.assertEqual(self.snapshot(self.rust_store), before)
        self.assertIn("stale source cursor", self.apply(self.request(
            PollBatch(SOURCE, "three", (fact("later"),)), "stale"), success=False))
        self.assertEqual(self.snapshot(self.rust_store), before)
        conflict = self.request(first)
        conflict["batch_digest"] = "b" * 64
        self.assertIn("stale source cursor", self.apply(conflict, success=False))
        self.assertEqual(self.snapshot(self.rust_store), before)

    def test_stale_lease_revocation_and_unmarked_database_reject(self):
        batch = PollBatch(SOURCE, "one", (fact(),))
        request = self.request(batch)
        bad = json.loads(json.dumps(request))
        bad["lease"]["epoch"] += 1
        self.assertIn("stale", self.apply(bad, success=False))
        no_mark = self.request(batch, path=self.python_store.path)
        self.assertIn("not an explicit scratch", self.apply(no_mark, success=False))
        self.assertEqual(self.snapshot(self.python_store)[1], [])
        set_source_capture(self.rust_store, source_id=SOURCE, enabled=False)
        self.assertIn("stale", self.apply(request, success=False))
        self.assertEqual(self.snapshot(self.rust_store)[1], [])

    def test_unsupported_correction_rejected_before_write(self):
        request = self.request(PollBatch(SOURCE, "one", (fact(),)))
        request["records"][0]["revision"] = 1
        self.assertIn("unsupported", self.apply(request, success=False))
        self.assertEqual(self.snapshot(self.rust_store)[0][0], "")


if __name__ == "__main__":
    unittest.main()
