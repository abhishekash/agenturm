"""Versioned discovery checkpoints commit with evidence and source cursor."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory.plugin_admission import (activate_source, commit_batch, member_cursors,
                                          set_source_capture)
from observatory.plugin_projection import project_source
from observatory.plugin_wire import PollBatch, ProtocolError, MAX_DISCOVERY_MEMBERS


SOURCE = "discovery-source"
MEMBER = "a" * 64
SIBLING = "b" * 64


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
                       (SOURCE, MEMBER, "x" * 800))
        self.assertEqual(member_cursors(self.store, self.lease), {MEMBER: "x" * 800})
        with self.store._connect() as db:
            db.execute("UPDATE plugin_source_member_cursor_v1 SET cursor=? WHERE source_id=?",
                       ("x" * 801, SOURCE))
        with self.assertRaisesRegex(ProtocolError, "checkpoint limit"):
            member_cursors(self.store, self.lease)
        with self.store._connect() as db:
            db.execute("DELETE FROM plugin_source_member_cursor_v1")
            db.executemany("INSERT INTO plugin_source_member_cursor_v1 VALUES(?,?,?)",
                           ((SOURCE, f"{n:064x}", "x")
                            for n in range(MAX_DISCOVERY_MEMBERS + 1)))
        with self.assertRaisesRegex(ProtocolError, "checkpoint limit"):
            member_cursors(self.store, self.lease)

    def test_gap_invalidates_only_its_member_and_keeps_sibling_activity(self):
        first = PollBatch(SOURCE, "one", (
            lifecycle("first"),
            {"event_id": "first-usage", "revision": 0, "kind": "usage",
             "time_ns": "101", "identity": {"session_id": "native-one"},
             "payload": {"input_tokens": 9}},
            {"event_id": "first-relation", "revision": 0, "kind": "relationship",
             "time_ns": "101", "identity": {"session_id": "native-one"},
             "payload": {"parent_session_id": "native-two", "kind": "delegated"}},
            {"event_id": "first-message", "revision": 0, "kind": "message",
             "time_ns": "102", "identity": {"session_id": "native-one"},
             "payload": {"role": "assistant"}}), MEMBER, "local-a")
        second = PollBatch(SOURCE, "two", (
            {**lifecycle("second"), "identity": {"session_id": "native-two"}},
            {"event_id": "second-message", "revision": 0, "kind": "message",
             "time_ns": "103", "identity": {"session_id": "native-two"},
             "payload": {"role": "assistant"}}), SIBLING, "local-b")
        commit_batch(self.store, self.lease, "", first)
        commit_batch(self.store, self.lease, "one", second)
        with self.assertRaisesRegex(ProtocolError, "changed discovery member"):
            commit_batch(self.store, self.lease, "two", PollBatch(
                SOURCE, "forbidden", (lifecycle("collision"),), SIBLING, "local-b2"))
        project_source(self.store, SOURCE)
        before = {row["native_session_id"]: row for row in
                  self.store.plugin_sessions()["sessions"]}
        self.assertEqual(before["native-one"]["relationship_state"], "confirmed")
        self.assertEqual(len(self.store.activity_page()["activity"]), 2)
        removed = PollBatch(SOURCE, "three", (gap("first-gone"),), MEMBER, "")
        commit_batch(self.store, self.lease, "two", removed)
        project_source(self.store, SOURCE)
        rows = {row["native_session_id"]: row for row in
                self.store.plugin_sessions()["sessions"]}
        self.assertEqual((rows["native-one"]["run_state"],
                          rows["native-one"]["reason_code"],
                          rows["native-one"]["metadata_usage"],
                          rows["native-one"]["relationship_state"],
                          rows["native-one"]["parent_agent_id"]),
                         ("unknown", "source_gap", {}, "unresolved", None))
        self.assertEqual((rows["native-two"]["run_state"],
                          rows["native-two"]["reason_code"]),
                         ("completed", ""))
        self.assertEqual([item["session_id"] for item in
                          self.store.activity_page()["activity"]],
                         [rows["native-two"]["session_id"]])
        self.assertEqual(member_cursors(self.store, self.lease), {SIBLING: "local-b"})
        with self.store._connect() as db:
            self.assertEqual(dict(db.execute(
                "SELECT native_session_id,member_id FROM plugin_session_member_v1")),
                {"native-one": MEMBER, "native-two": SIBLING})
        resumed = activate_source(self.store, plugin_id="fixture.plugin",
                                  source_id=SOURCE, generation="a" * 64)
        self.assertEqual(member_cursors(self.store, resumed), {SIBLING: "local-b"})
        self.assertEqual({row["native_session_id"]: row["reason_code"] for row in
                          self.store.plugin_sessions()["sessions"]},
                         {"native-one": "source_gap", "native-two": ""})
        set_source_capture(self.store, source_id=SOURCE, enabled=False)
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_session_member_v1")
                             .fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1")
                             .fetchone()[0], 0)

    def test_legacy_evidence_forces_source_wide_gap_without_startup_backfill(self):
        legacy = PollBatch(SOURCE, "one", (lifecycle("legacy"),))
        commit_batch(self.store, self.lease, "", legacy)
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_session_member_v1")
                             .fetchone()[0], 0)
        mapped = PollBatch(SOURCE, "two", (
            {**lifecycle("mapped"), "identity": {"session_id": "mapped"}},),
            MEMBER, "local-a")
        commit_batch(self.store, self.lease, "one", mapped)
        commit_batch(self.store, self.lease, "two",
                     PollBatch(SOURCE, "three", (gap("missing"),), MEMBER, ""))
        project_source(self.store, SOURCE)
        self.assertEqual({row["reason_code"] for row in
                          self.store.plugin_sessions()["sessions"]}, {"source_gap"})

    def test_upgraded_directory_rows_remain_unowned_and_force_safe_fallback(self):
        commit_batch(self.store, self.lease, "", PollBatch(
            SOURCE, "one", (lifecycle(),), MEMBER, "local-a"))
        # Recreate the #204 evidence schema while preserving its real member
        # cursor and fact, then reopen so initialize() performs the upgrade.
        with self.store._connect() as db:
            db.executescript("""
                CREATE TABLE plugin_evidence_old_v1 (
                    source_id TEXT NOT NULL, event_id_hash TEXT NOT NULL,
                    revision INTEGER NOT NULL, kind TEXT NOT NULL,
                    time_ns INTEGER NOT NULL, payload_json TEXT NOT NULL,
                    record_digest TEXT NOT NULL, native_session_id TEXT NOT NULL DEFAULT '',
                    native_worker_id TEXT NOT NULL DEFAULT '',
                    native_turn_id TEXT NOT NULL DEFAULT '',
                    native_operation_id TEXT NOT NULL DEFAULT '',
                    policy_epoch INTEGER NOT NULL DEFAULT 1,
                    admitted_ns INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY(source_id,event_id_hash,revision));
                INSERT INTO plugin_evidence_old_v1 SELECT source_id,event_id_hash,
                    revision,kind,time_ns,payload_json,record_digest,native_session_id,
                    native_worker_id,native_turn_id,native_operation_id,policy_epoch,
                    admitted_ns FROM plugin_evidence_v1;
                DROP TABLE plugin_evidence_v1;
                ALTER TABLE plugin_evidence_old_v1 RENAME TO plugin_evidence_v1;
            """)
            db.execute("DELETE FROM plugin_session_member_v1 WHERE source_id=?", (SOURCE,))
            self.assertNotIn("member_id", {row[1] for row in
                             db.execute("PRAGMA table_info(plugin_evidence_v1)")})
        reopened = Store(self.store.path)
        self.addCleanup(reopened.close)
        with reopened._connect() as db:
            self.assertIn("member_id", {row[1] for row in
                           db.execute("PRAGMA table_info(plugin_evidence_v1)")})
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_session_member_v1")
                             .fetchone()[0], 0)  # No startup backfill.
        corrected = PollBatch(SOURCE, "two", (
            {**lifecycle(), "revision": 1, "time_ns": "101"},),
            MEMBER, "local-a2")
        self.assertEqual(commit_batch(self.store, self.lease, "one", corrected), "two")
        self.assertEqual(commit_batch(self.store, self.lease, "one", corrected), "two")
        with self.store._connect() as db:
            self.assertEqual(db.execute(
                "SELECT member_id FROM plugin_session_member_v1 WHERE source_id=? "
                "AND native_session_id='native-one'", (SOURCE,)).fetchone()[0], "")
            self.assertEqual(tuple(db.execute(
                "SELECT revision,member_id FROM plugin_evidence_v1 WHERE source_id=? "
                "AND native_session_id='native-one'", (SOURCE,)).fetchall()[0]), (1, ""))
        commit_batch(self.store, self.lease, "two", PollBatch(
            SOURCE, "three", (
                {**lifecycle("new"), "identity": {"session_id": "new"}},),
            SIBLING, "local-b"))
        commit_batch(self.store, self.lease, "three", PollBatch(
            SOURCE, "four", (gap("old-file-gone"),), MEMBER, ""))
        project_source(self.store, SOURCE)
        self.assertEqual({row["reason_code"] for row in
                          self.store.plugin_sessions()["sessions"]}, {"source_gap"})

    def test_legacy_single_file_admission_does_not_use_member_cap(self):
        with patch("observatory.plugin_retention.MAX_INVALIDATED_SESSIONS", 1):
            commit_batch(self.store, self.lease, "", PollBatch(
                SOURCE, "one", (lifecycle(),)))
            commit_batch(self.store, self.lease, "one", PollBatch(
                SOURCE, "two", (
                    {**lifecycle("another"), "identity": {"session_id": "another"}},)))
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_session_member_v1")
                             .fetchone()[0], 0)

    def test_member_identity_cannot_migrate_after_retention_and_cap_is_atomic(self):
        commit_batch(self.store, self.lease, "", PollBatch(
            SOURCE, "1", (lifecycle(),), MEMBER, "local-a"))
        with patch("observatory.plugin_admission.MAX_SOURCE_RECORDS", 4):
            for index in range(1, 5):
                commit_batch(self.store, self.lease, str(index), PollBatch(
                    SOURCE, str(index + 1), (
                        {**lifecycle(f"other-{index}"),
                         "time_ns": str(100 + index),
                         "identity": {"session_id": f"other-{index}"}},),
                    MEMBER, f"local-{index}"))
        with self.store._connect() as db:
            self.assertIsNotNone(db.execute(
                "SELECT 1 FROM plugin_retention_invalidated_v1 "
                "WHERE source_id=? AND native_session_id='native-one'",
                (SOURCE,)).fetchone())
            self.assertEqual(db.execute(
                "SELECT member_id FROM plugin_session_member_v1 "
                "WHERE source_id=? AND native_session_id='native-one'",
                (SOURCE,)).fetchone()[0], MEMBER)
        with self.assertRaisesRegex(ProtocolError, "changed discovery member"):
            commit_batch(self.store, self.lease, "5", PollBatch(
                SOURCE, "six", (lifecycle("reused"),), SIBLING, "local-b"))
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1")
                             .fetchone()[0], "5")
        with patch("observatory.plugin_retention.MAX_INVALIDATED_SESSIONS", 5):
            with self.assertRaisesRegex(ProtocolError, "member capacity"):
                commit_batch(self.store, self.lease, "5", PollBatch(
                    SOURCE, "six", (
                        {**lifecycle("sixth"), "identity": {"session_id": "sixth"}},),
                    SIBLING, "local-b"))
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_session_member_v1")
                             .fetchone()[0], 5)

    def test_retention_pruned_member_stays_unknown_without_gapping_sibling(self):
        commit_batch(self.store, self.lease, "", PollBatch(
            SOURCE, "1", (lifecycle(),), MEMBER, "local-a"))
        with patch("observatory.plugin_admission.MAX_SOURCE_RECORDS", 4):
            for index in range(1, 5):
                commit_batch(self.store, self.lease, str(index), PollBatch(
                    SOURCE, str(index + 1), (
                        {**lifecycle(f"fill-{index}"), "time_ns": str(100 + index),
                         "identity": {"session_id": f"fill-{index}"}},),
                    MEMBER, f"local-{index}"))
        commit_batch(self.store, self.lease, "5", PollBatch(
            SOURCE, "6", (
                {**lifecycle("healthy"), "time_ns": "200",
                 "identity": {"session_id": "healthy"}},
                {"event_id": "healthy-message", "revision": 0, "kind": "message",
                 "time_ns": "201", "identity": {"session_id": "healthy"},
                 "payload": {"role": "assistant"}}), SIBLING, "local-b"))
        commit_batch(self.store, self.lease, "6", PollBatch(
            SOURCE, "7", (gap("member-gone"),), MEMBER, ""))
        project_source(self.store, SOURCE)
        rows = {row["native_session_id"]: row for row in
                self.store.plugin_sessions()["sessions"]}
        self.assertEqual(rows["native-one"]["reason_code"], "retention_gap")
        self.assertEqual(rows["healthy"]["run_state"], "completed")
        self.assertNotEqual(rows["healthy"]["reason_code"], "source_gap")
        self.assertEqual([item["session_id"] for item in
                          self.store.activity_page()["activity"]],
                         [rows["healthy"]["session_id"]])

    def test_projected_owner_without_evidence_is_still_gapped(self):
        commit_batch(self.store, self.lease, "", PollBatch(
            SOURCE, "one", (lifecycle(),), MEMBER, "local-a"))
        commit_batch(self.store, self.lease, "one", PollBatch(
            SOURCE, "two", (
                {**lifecycle("sibling"), "identity": {"session_id": "sibling"}},),
            SIBLING, "local-b"))
        project_source(self.store, SOURCE)
        with self.store._connect() as db:
            # Model a stale projection after its evidence disappeared without
            # a retention marker. The durable binding remains authoritative.
            db.execute("DELETE FROM plugin_evidence_v1 WHERE source_id=? "
                       "AND native_session_id='native-one'", (SOURCE,))
        commit_batch(self.store, self.lease, "two", PollBatch(
            SOURCE, "three", (gap("missing-file"),), MEMBER, ""))
        with self.store._connect() as db:
            self.assertEqual({row[0] for row in db.execute(
                "SELECT native_session_id FROM plugin_source_gap_invalidated_v1 "
                "WHERE source_id=?", (SOURCE,))}, {"native-one"})
        project_source(self.store, SOURCE)
        self.assertEqual({row["native_session_id"]: row["reason_code"] for row in
                          self.store.plugin_sessions()["sessions"]},
                         {"native-one": "source_gap", "sibling": ""})


if __name__ == "__main__":
    unittest.main()
