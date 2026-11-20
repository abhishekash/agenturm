"""Versioned discovery checkpoints commit with evidence and source cursor."""
import tempfile
import unittest
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import (activate_source, commit_batch, member_cursors,
                                          set_source_capture)
from observatory.plugin_projection import project_source
from observatory.plugin_wire import PollBatch, ProtocolError, MAX_DISCOVERY_MEMBERS


SOURCE = "discovery-source"
MEMBER = "a" * 64


def lifecycle(event="one"):
    return {"event_id": event, "revision": 0, "kind": "lifecycle",
            "time_ns": "100", "identity": {"session_id": "native-one"},
            "payload": {"state": "completed"}}


def gap(event="gap"):
    return {"event_id": event, "revision": 0, "kind": "diagnostic",
            "time_ns": "200", "payload": {"code": "possible_source_gap",
                                            "severity": "warning"}}


class DiscoveryCheckpointTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.store = Store(Path(temp.name) / "store.db")
        self.addCleanup(self.store.close)
        self.lease = activate_source(self.store, plugin_id="fixture.plugin",
                                     source_id=SOURCE, generation="a" * 64)

    def test_member_cursor_and_evidence_commit_replay_and_resume_atomically(self):
        first = PollBatch(SOURCE, "d1:1:" + MEMBER, (lifecycle(),), MEMBER, "local-1")
        self.assertEqual(commit_batch(self.store, self.lease, "", first), first.cursor)
        self.assertEqual(member_cursors(self.store, self.lease), {MEMBER: "local-1"})
        self.assertEqual(commit_batch(self.store, self.lease, "", first), first.cursor)
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 1)
        before = member_cursors(self.store, self.lease)
        with self.assertRaisesRegex(ProtocolError, "conflicting"):
            commit_batch(self.store, self.lease, first.cursor, PollBatch(
                SOURCE, "d1:2:" + MEMBER, ({**lifecycle(), "payload": {"state": "failed"}},),
                MEMBER, "local-2"))
        self.assertEqual(member_cursors(self.store, self.lease), before)
        resumed = activate_source(self.store, plugin_id="fixture.plugin",
                                  source_id=SOURCE, generation="a" * 64)
        self.assertEqual(resumed.cursor, first.cursor)
        self.assertEqual(member_cursors(self.store, resumed), before)
        with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
            member_cursors(self.store, self.lease)

    def test_missing_member_requires_gap_and_clears_exact_checkpoint(self):
        first = PollBatch(SOURCE, "d1:1:" + MEMBER, (lifecycle(),), MEMBER, "local-1")
        commit_batch(self.store, self.lease, "", first)
        with self.assertRaisesRegex(ProtocolError, "source-gap"):
            commit_batch(self.store, self.lease, first.cursor,
                         PollBatch(SOURCE, "d1:2:" + MEMBER, (), MEMBER, ""))
        self.assertEqual(member_cursors(self.store, self.lease), {MEMBER: "local-1"})
        removed = PollBatch(SOURCE, "d1:2:" + MEMBER, (gap(),), MEMBER, "")
        commit_batch(self.store, self.lease, first.cursor, removed)
        self.assertEqual(member_cursors(self.store, self.lease), {})
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT native_session_id FROM "
                                        "plugin_source_gap_invalidated_v1").fetchone()[0],
                             "native-one")
        self.assertEqual(commit_batch(self.store, self.lease, first.cursor, removed), removed.cursor)

    def test_capacity_and_revocation_fence(self):
        with self.store._connect() as db:
            db.executemany("INSERT INTO plugin_source_member_cursor_v1(source_id,member_id,cursor) "
                           "VALUES(?,?,?)", ((SOURCE, f"{n:064x}", "local")
                                              for n in range(MAX_DISCOVERY_MEMBERS)))
        self.assertEqual(len(member_cursors(self.store, self.lease)), MAX_DISCOVERY_MEMBERS)
        with self.assertRaisesRegex(ProtocolError, "member limit"):
            commit_batch(self.store, self.lease, "", PollBatch(
                SOURCE, "d1:1:" + MEMBER, (), MEMBER, "local"))
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0], "")
        set_source_capture(self.store, source_id=SOURCE, enabled=False)
        with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
            member_cursors(self.store, self.lease)

    def test_idle_scan_position_is_durable_without_false_evidence(self):
        scan = PollBatch(SOURCE, "d1:1:" + MEMBER, ())
        self.assertEqual(commit_batch(self.store, self.lease, "", scan), scan.cursor)
        self.assertEqual(commit_batch(self.store, self.lease, "", scan), scan.cursor)
        with self.store._connect() as db:
            self.assertEqual(tuple(db.execute(
                "SELECT cursor,dirty_seq FROM plugin_source_cursor_v1").fetchone()),
                (scan.cursor, 0))
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 0)
        resumed = activate_source(self.store, plugin_id="fixture.plugin",
                                  source_id=SOURCE, generation="a" * 64)
        self.assertEqual(resumed.cursor, scan.cursor)

    def test_prior_gap_cannot_remove_member_or_hide_newer_session(self):
        first = PollBatch(SOURCE, "d1:1:" + MEMBER, (lifecycle(),), MEMBER, "local-1")
        commit_batch(self.store, self.lease, "", first)
        earlier = PollBatch(SOURCE, "d1:2:" + MEMBER, (gap(),), MEMBER, "local-2")
        commit_batch(self.store, self.lease, first.cursor, earlier)
        self.assertEqual(commit_batch(self.store, self.lease, first.cursor, earlier),
                         earlier.cursor)  # Exact lost-ack replay remains valid.
        newer = PollBatch(SOURCE, "d1:3:" + MEMBER, (
            {**lifecycle("newer"), "identity": {"session_id": "native-two"}},),
            MEMBER, "local-3")
        commit_batch(self.store, self.lease, earlier.cursor, newer)
        before = member_cursors(self.store, self.lease)
        for member_cursor in ("", "local-4"):
            with self.subTest(member_cursor=member_cursor):
                with self.assertRaisesRegex(ProtocolError, "source-gap evidence must be new"):
                    commit_batch(self.store, self.lease, newer.cursor, PollBatch(
                        SOURCE, "d1:4:" + MEMBER, (gap(),), MEMBER, member_cursor))
                self.assertEqual(member_cursors(self.store, self.lease), before)
        project_source(self.store, SOURCE)
        rows = {row["native_session_id"]: row for row in
                self.store.plugin_sessions()["sessions"]}
        self.assertEqual(rows["native-two"]["run_state"], "completed")
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0],
                             newer.cursor)

    def test_corrupt_or_excess_checkpoint_rows_are_bounded(self):
        with self.store._connect() as db:
            db.execute("INSERT INTO plugin_source_member_cursor_v1 VALUES(?,?,?)",
                       (SOURCE, MEMBER, "x" * 513))
        with self.assertRaisesRegex(ProtocolError, "checkpoint limit"):
            member_cursors(self.store, self.lease)
        with self.store._connect() as db:
            db.execute("DELETE FROM plugin_source_member_cursor_v1")
            db.executemany("INSERT INTO plugin_source_member_cursor_v1 VALUES(?,?,?)",
                           ((SOURCE, f"{n:064x}", "x")
                            for n in range(MAX_DISCOVERY_MEMBERS + 1)))
        with self.assertRaisesRegex(ProtocolError, "checkpoint limit"):
            member_cursors(self.store, self.lease)


if __name__ == "__main__":
    unittest.main()
