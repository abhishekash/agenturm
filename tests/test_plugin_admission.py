"""Durable plugin inbox: atomicity, lease fencing and child acknowledgment."""
import tempfile
import unittest
import sqlite3
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
from observatory.plugin_wire import PollBatch, ProtocolError
from tests import test_plugin_process


GENERATION = "a" * 64


def record(event="one", state="completed", revision=0):
    return {"event_id": event, "revision": revision, "kind": "lifecycle",
            "time_ns": "9007199254740993", "payload": {"state": state}}


class AdmissionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.store = Store(str(Path(self.temp.name) / "agenturm.db"))
        self.lease = activate_source(self.store, plugin_id="fixture.plugin",
                                     source_id="fixture-source", generation=GENERATION)

    def batch(self, cursor="next", records=None):
        return PollBatch("fixture-source", cursor, tuple(records if records is not None else [record()]))

    def snapshot(self):
        with self.store._connect() as db:
            cursor = db.execute("SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 "
                                "WHERE source_id='fixture-source'").fetchone()
            evidence = db.execute("SELECT kind,time_ns,payload_json FROM plugin_evidence_v1").fetchall()
            dirty = db.execute("SELECT source_id,dirty_seq FROM plugin_evidence_dirty_v1").fetchall()
            return tuple(cursor), [tuple(row) for row in evidence], [tuple(row) for row in dirty]

    def test_evidence_cursor_and_dirty_marker_are_atomic_and_replay_is_exact(self):
        batch = self.batch()
        self.assertEqual(commit_batch(self.store, self.lease, "", batch), "next")
        expected = self.snapshot()
        self.assertEqual(expected[0], ("next", 1))
        self.assertEqual(expected[1][0][1], 9007199254740993)
        self.assertEqual(expected[2], [("fixture-source", 1)])
        self.assertEqual(commit_batch(self.store, self.lease, "", batch), "next")
        self.assertEqual(self.snapshot(), expected)
        with self.assertRaisesRegex(ProtocolError, "did not advance"):
            commit_batch(self.store, self.lease, "next", self.batch(records=[record("other")]))
        self.assertEqual(self.snapshot(), expected)

    def test_later_conflict_rolls_back_earlier_record_and_cursor(self):
        commit_batch(self.store, self.lease, "", self.batch())
        before = self.snapshot()
        with self.assertRaisesRegex(ProtocolError, "conflicting"):
            commit_batch(self.store, self.lease, "next", self.batch("later", [
                record("new"), record("one", "failed")]))
        self.assertEqual(self.snapshot(), before)

    def test_identity_is_durable_and_correction_revisions_are_monotonic(self):
        original = {**record(), "identity": {"session_id": "native-session",
                                                "worker_id": "worker-1", "turn_id": "turn-1"}}
        commit_batch(self.store, self.lease, "", self.batch(records=[original]))
        corrected = {**original, "revision": 1, "payload": {"state": "failed"}}
        commit_batch(self.store, self.lease, "next", self.batch("corrected", [corrected]))
        with self.store._connect() as db:
            rows = db.execute("SELECT revision,native_session_id,native_worker_id,native_turn_id "
                              "FROM plugin_evidence_v1 ORDER BY revision").fetchall()
        self.assertEqual([tuple(row) for row in rows], [
            (0, "native-session", "worker-1", "turn-1"),
            (1, "native-session", "worker-1", "turn-1")])
        before = self.snapshot()
        with self.assertRaisesRegex(ProtocolError, "stale plugin event revision"):
            commit_batch(self.store, self.lease, "corrected", self.batch("stale", [original]))
        self.assertEqual(self.snapshot(), before)
        changed_identity = {**corrected, "revision": 2,
                            "identity": {"session_id": "other-session"}}
        with self.assertRaisesRegex(ProtocolError, "identity or kind changed"):
            commit_batch(self.store, self.lease, "corrected", self.batch("moved", [changed_identity]))
        self.assertEqual(self.snapshot(), before)

    def test_identity_shape_is_validated_before_any_write(self):
        bad = [None, {}, {"worker_id": "worker"}, {"session_id": ""},
               {"session_id": "session", "unexpected": "value"},
               {"session_id": "session", "worker_id": 1}]
        for identity in bad:
            with self.subTest(identity=identity), self.assertRaisesRegex(ProtocolError, "identity|record"):
                commit_batch(self.store, self.lease, "", self.batch(records=[
                    record("good"), {**record("bad"), "identity": identity}]))
        self.assertEqual(self.snapshot(), (("", 0), [], []))

    def test_capacity_refuses_entire_batch_without_advancing_cursor(self):
        with patch("observatory.plugin_admission.MAX_SOURCE_RECORDS", 1):
            with self.assertRaisesRegex(ProtocolError, "capacity"):
                commit_batch(self.store, self.lease, "", self.batch(records=[
                    record("one"), record("two")]))
            self.assertEqual(self.snapshot(), (("", 0), [], []))
            commit_batch(self.store, self.lease, "", self.batch(records=[record("one")]))
            before = self.snapshot()
            with self.assertRaisesRegex(ProtocolError, "capacity"):
                commit_batch(self.store, self.lease, "next", self.batch("later", [record("two")]))
            self.assertEqual(self.snapshot(), before)

    def test_old_evidence_schema_migrates_identity_columns(self):
        path = Path(self.temp.name) / "old.db"
        with sqlite3.connect(path) as db:
            db.execute("CREATE TABLE plugin_source_cursor_v1 (source_id TEXT PRIMARY KEY,"
                       "receiver_id TEXT NOT NULL,plugin_id TEXT NOT NULL,generation TEXT NOT NULL,"
                       "lease_epoch INTEGER NOT NULL,cursor TEXT NOT NULL DEFAULT '',"
                       "prior_cursor TEXT NOT NULL DEFAULT '',last_batch_digest TEXT NOT NULL DEFAULT '',"
                       "dirty_seq INTEGER NOT NULL DEFAULT 0,updated_ns INTEGER NOT NULL)")
            db.execute("CREATE TABLE plugin_evidence_v1 (source_id TEXT NOT NULL,"
                       "event_id_hash TEXT NOT NULL,revision INTEGER NOT NULL,kind TEXT NOT NULL,"
                       "time_ns INTEGER NOT NULL,payload_json TEXT NOT NULL,record_digest TEXT NOT NULL,"
                       "PRIMARY KEY(source_id,event_id_hash,revision))")
        migrated = Store(str(path))
        with migrated._connect() as db:
            columns = {row[1] for row in db.execute("PRAGMA table_info(plugin_evidence_v1)")}
            cursor_columns = {row[1] for row in db.execute("PRAGMA table_info(plugin_source_cursor_v1)")}
        self.assertTrue({"native_session_id", "native_worker_id", "native_turn_id",
                         "policy_epoch"} <= columns)
        self.assertTrue({"policy_epoch", "capture_enabled"} <= cursor_columns)

    def test_capture_revocation_erases_evidence_and_fences_worker(self):
        commit_batch(self.store, self.lease, "", self.batch())
        self.assertEqual(set_source_capture(self.store, source_id="fixture-source", enabled=False), 2)
        with self.store._connect() as db:
            row = db.execute("SELECT cursor,dirty_seq,policy_epoch,capture_enabled "
                             "FROM plugin_source_cursor_v1 WHERE source_id='fixture-source'").fetchone()
            evidence = db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0]
        self.assertEqual(tuple(row), ("next", 2, 2, 0))
        self.assertEqual(evidence, 0)
        with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
            commit_batch(self.store, self.lease, "next", self.batch("later", [record("two")]))
        with self.assertRaisesRegex(ProtocolError, "capture revoked"):
            activate_source(self.store, plugin_id="fixture.plugin",
                            source_id="fixture-source", generation=GENERATION)
        self.assertEqual(set_source_capture(self.store, source_id="fixture-source", enabled=True), 3)
        resumed = activate_source(self.store, plugin_id="fixture.plugin",
                                  source_id="fixture-source", generation=GENERATION)
        self.assertEqual((resumed.cursor, resumed.policy_epoch), ("next", 3))
        commit_batch(self.store, resumed, "next", self.batch("later", [record("two")]))
        with self.store._connect() as db:
            rows = db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0]
            epoch = db.execute("SELECT policy_epoch FROM plugin_evidence_v1").fetchone()[0]
        self.assertEqual(rows, 1)
        self.assertEqual(epoch, 3)

    def test_capture_policy_noop_does_not_fence_or_dirty(self):
        self.assertEqual(set_source_capture(self.store, source_id="fixture-source", enabled=True), 1)
        self.assertEqual(self.snapshot(), (("", 0), [], []))
        commit_batch(self.store, self.lease, "", self.batch())

    def test_capture_policy_requires_existing_source(self):
        with self.assertRaisesRegex(ProtocolError, "unknown plugin source"):
            set_source_capture(self.store, source_id="missing", enabled=False)
        with self.assertRaises(TypeError):
            set_source_capture(self.store, source_id="fixture-source", enabled=0)

    def test_invalid_metadata_in_second_record_never_writes_first(self):
        with self.assertRaisesRegex(ProtocolError, "metadata"):
            commit_batch(self.store, self.lease, "", self.batch(records=[
                record("good"), {**record("bad"), "payload": {"state": "completed", "text": "prompt"}}]))
        self.assertEqual(self.snapshot(), (("", 0), [], []))

    def test_activation_fences_old_worker_and_preserves_resume_cursor(self):
        commit_batch(self.store, self.lease, "", self.batch())
        new = activate_source(self.store, plugin_id="fixture.plugin",
                              source_id="fixture-source", generation=GENERATION)
        self.assertEqual(new.cursor, "next")
        self.assertNotEqual(new.epoch, self.lease.epoch)
        with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
            commit_batch(self.store, self.lease, "next", self.batch("later", [record("two")]))
        self.assertEqual(commit_batch(self.store, new, "next", self.batch("later", [record("two")])), "later")

    def test_restart_after_commit_before_ack_accepts_exact_replay(self):
        batch = self.batch()
        commit_batch(self.store, self.lease, "", batch)
        before = self.snapshot()
        restarted = Store(str(Path(self.temp.name) / "agenturm.db"))
        new = activate_source(restarted, plugin_id="fixture.plugin",
                              source_id="fixture-source", generation=GENERATION)
        self.assertEqual(new.cursor, "next")
        self.assertEqual(commit_batch(restarted, new, "", batch), "next")
        self.assertEqual(self.snapshot(), before)
        with self.assertRaisesRegex(ProtocolError, "stale plugin cursor"):
            commit_batch(restarted, new, "", self.batch("other", [record("different")]))

    def test_rejects_raw_content_and_malformed_metadata(self):
        invalid = [
            {**record(), "payload": {"state": ["completed"]}},
            {**record(), "payload": {"state": "completed", "reason_code": "secret text"}},
            {**record(), "kind": "usage", "payload": {"input_tokens": True}},
            {**record(), "kind": "usage", "payload": {"input_tokens": -1}},
            {**record(), "kind": "diagnostic", "payload": {"code": "free text", "severity": "error"}},
            {**record(), "kind": "message", "payload": {"text": "prompt"}},
        ]
        for item in invalid:
            with self.subTest(item=item), self.assertRaises(ProtocolError):
                commit_batch(self.store, self.lease, "", self.batch(records=[item]))
        self.assertEqual(self.snapshot(), (("", 0), [], []))

    def test_other_plugin_and_artifact_cannot_take_source(self):
        for plugin_id, generation in (("other.plugin", GENERATION),
                                      ("fixture.plugin", "b" * 64)):
            with self.subTest(plugin_id=plugin_id, generation=generation):
                with self.assertRaisesRegex(ProtocolError, "ownership or artifact"):
                    activate_source(self.store, plugin_id=plugin_id,
                                    source_id="fixture-source", generation=generation)

    def test_child_ack_follows_database_commit(self):
        fixture = test_plugin_process.ProcessHostTests("runTest")
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        child_store = Store(str(Path(self.temp.name) / "child.db"))
        manifest = fixture.manifest()
        matching = activate_source(child_store, plugin_id="fixture.plugin",
                                   source_id="fixture-source", generation=manifest.artifact_sha256)
        with fixture.host() as host:
            host.poll_commit("", lambda batch: commit_batch(child_store, matching, "", batch))
            with child_store._connect() as db:
                self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1 "
                                            "WHERE source_id='fixture-source'").fetchone()[0], "next")
        self.assertEqual(fixture.marker.read_text().splitlines(), ["next"])
        fixture.marker.unlink()
        with fixture.host() as host:
            with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
                host.poll_commit("next", lambda batch: commit_batch(child_store, self.lease, "next", batch))
        self.assertFalse(fixture.marker.exists())


if __name__ == "__main__":
    unittest.main()
