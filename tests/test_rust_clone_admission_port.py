"""Behavioral contract for the Rust marked-clone metadata subset.

This deliberately limited fixture is not a PluginAdmissionPort conformance
suite. The Rust fixture boundary does not expose diagnostic/member reads,
reference admission, discovery gaps, relationships, or runtime dispatch.
"""
from __future__ import annotations

import json
import os
import sqlite3
import sys
import tempfile
import unittest
from contextlib import closing
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import activate_source
from observatory.plugin_wire import PollBatch
from tests.support.rust_metadata_adapter import (
    RustFixtureError,
    RustMarkedCloneMetadataAdapter,
    UnsupportedMetadataSubset,
)


PLUGIN = "contract.fixture"
SOURCE = "rust-metadata-source"
GENERATION = "a" * 64


def lifecycle(event_id: str, state: str = "completed", session: str = "session-1") -> dict:
    return {"event_id": event_id, "revision": 0, "kind": "lifecycle",
            "time_ns": "9007199254740993", "payload": {"state": state},
            "identity": {"session_id": session}}


def message(event_id: str, session: str = "session-2") -> dict:
    return {"event_id": event_id, "revision": 0, "kind": "message",
            "time_ns": "9007199254740994", "payload": {"role": "assistant"},
            "identity": {"session_id": session}}


@unittest.skipUnless(sys.platform.startswith("linux"), "clone writer lease is Linux-only")
class RustCloneMetadataSubsetContractTests(unittest.TestCase):
    """The contract intentionally covers only Rust's marked-clone metadata subset."""

    def setUp(self):
        binary = os.environ.get("AGENTURM_RUST_METADATA_ADAPTER_BIN")
        if not binary:
            self.skipTest("Rust marked-clone metadata fixture binary required")
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.canonical_path = self.root / "canonical.db"
        canonical = Store(str(self.canonical_path))
        self.receiver_id = canonical.receiver_id
        activate_source(canonical, plugin_id=PLUGIN, source_id="seed-source",
                        generation=GENERATION)
        canonical.close()
        self.canonical_before = self._logical_db_snapshot(self.canonical_path)
        self.adapter = self.new_adapter(SOURCE)

    def tearDown(self):
        if hasattr(self, "adapter"):
            self.adapter.close()
        self.assertEqual(self._logical_db_snapshot(self.canonical_path),
                         self.canonical_before)

    @staticmethod
    def _logical_db_snapshot(path: Path):
        with closing(sqlite3.connect(path)) as db:
            tables = [row[0] for row in db.execute(
                "SELECT name FROM sqlite_master WHERE type='table' "
                "AND name NOT LIKE 'sqlite_%' ORDER BY name")]
            snapshot = []
            for table in tables:
                quoted = '"' + table.replace('"', '""') + '"'
                try:
                    rows = tuple(sorted(
                        (tuple(row) for row in db.execute(f"SELECT * FROM {quoted}")),
                        key=repr))
                except sqlite3.DatabaseError:
                    continue
                snapshot.append((table, rows))
            return tuple(snapshot)

    def new_adapter(self, source_id: str) -> RustMarkedCloneMetadataAdapter:
        clone_path = self.root / f"{source_id}.db"
        with closing(sqlite3.connect(self.canonical_path)) as canonical:
            with closing(sqlite3.connect(clone_path)) as clone:
                canonical.backup(clone)
        with closing(sqlite3.connect(clone_path)) as clone:
            clone.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                          ("storage_activation_clone_v1", "enabled"))
            clone.commit()
        return RustMarkedCloneMetadataAdapter(
            os.environ["AGENTURM_RUST_METADATA_ADAPTER_BIN"], clone_path)

    @staticmethod
    def _rows(db, table: str, source_id: str):
        exists = db.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", (table,)).fetchone()
        if exists is None:
            return None
        if table == "activity_index_v1":
            query = "SELECT * FROM activity_index_v1 WHERE kind='plugin' AND owner_key=? ORDER BY rowid"
        elif table == "investigation_dirty_v1":
            query = "SELECT * FROM investigation_dirty_v1 WHERE kind='plugin' AND owner_key=? ORDER BY rowid"
        else:
            query = f"SELECT * FROM {table} WHERE source_id=? ORDER BY rowid"
        return tuple(tuple(row) for row in db.execute(query, (source_id,)))

    def snapshot(self, source_id: str = SOURCE, *, adapter=None):
        tables = (
            "plugin_source_cursor_v1", "plugin_evidence_v1", "plugin_evidence_dirty_v1",
            "plugin_session_projection_v1", "plugin_projection_cursor_v1",
            "activity_index_v1", "investigation_dirty_v1", "plugin_content_v1",
            "plugin_content_batch_v1", "plugin_content_status_v1",
            "plugin_reference_owner_v1", "plugin_source_member_cursor_v1",
            "plugin_session_member_v1", "plugin_source_gap_invalidated_v1",
            "plugin_retention_v1", "plugin_retention_invalidated_v1",
            "plugin_command_content_v1", "plugin_command_batch_v1",
            "storage_admission_request_v2", "storage_source_activate_v2",
            "storage_source_capture_set_v2", "storage_source_revoke_v2",
            "storage_admission_batch_binding_v1",
        )
        target = self.adapter if adapter is None else adapter
        with closing(sqlite3.connect(target.clone_path)) as db:
            return tuple((table, self._rows(db, table, source_id)) for table in tables)

    def activate(self, *, request_id="1" * 32, source_id: str = SOURCE):
        return self.adapter.activate_source(
            plugin_id=PLUGIN, source_id=source_id,
            generation=GENERATION, request_id=request_id)

    def test_activation_reopen_and_supported_mixed_batch_commit_replay_conflicts(self):
        lease = self.activate()
        self.assertEqual(lease.cursor, "")
        first = PollBatch(SOURCE, "cursor-1", (
            lifecycle("event-life"), message("event-message")))
        request = self.adapter.batch_request(lease, "", first, request_id="2" * 32)
        committed = self.adapter.commit_batch(
            lease, "", first, request_id=request["request_id"])
        self.assertEqual((committed["disposition"], committed["cursor"]),
                         ("committed", "cursor-1"))
        before_replay = self.snapshot()
        replay = self.adapter.commit_batch(
            lease, "", first, request_id=request["request_id"])
        self.assertEqual(replay["disposition"], "replay")
        self.assertEqual(self.snapshot(), before_replay)

        altered = PollBatch(SOURCE, "cursor-1", (
            lifecycle("event-life", "failed"), message("event-message")))
        with self.assertRaises(RustFixtureError):
            self.adapter.commit_batch(
                lease, "", altered, request_id=request["request_id"])
        self.assertEqual(self.snapshot(), before_replay)

        conflict = PollBatch(SOURCE, "cursor-2", (
            lifecycle("event-life", "failed"),))
        with self.assertRaises(RustFixtureError):
            self.adapter.commit_batch(
                lease, "cursor-1", conflict, request_id="3" * 32)
        self.assertEqual(self.snapshot(), before_replay)

        self.adapter.close()
        self.adapter = RustMarkedCloneMetadataAdapter(
            os.environ["AGENTURM_RUST_METADATA_ADAPTER_BIN"], self.adapter.clone_path)
        resumed = self.activate(request_id="4" * 32)
        self.assertEqual(resumed.cursor, "cursor-1")

    def test_reactivation_fences_stale_lease_without_mutation(self):
        stale = self.activate()
        fresh = self.activate(request_id="2" * 32)
        before = self.snapshot()
        with self.assertRaisesRegex(RustFixtureError, "stale or unsupported source lease"):
            self.adapter.commit_batch(
                stale, "", PollBatch(SOURCE, "cursor-1", (lifecycle("stale"),)),
                request_id="3" * 32)
        self.assertEqual(self.snapshot(), before)
        self.assertNotEqual(stale.epoch, fresh.epoch)

    def test_initial_empty_idle_and_cursor_only_advances(self):
        lease = self.activate()
        idle = PollBatch(SOURCE, "", ())
        self.assertEqual(self.adapter.commit_batch(
            lease, "", idle, request_id="2" * 32)["disposition"], "idle")
        before = self.snapshot()
        advance = PollBatch(SOURCE, "cursor-empty", ())
        self.assertEqual(self.adapter.commit_batch(
            lease, "", advance, request_id="3" * 32)["disposition"], "committed")
        committed = self.snapshot()
        self.assertEqual(self.adapter.commit_batch(
            lease, "", advance, request_id="3" * 32)["disposition"], "replay")
        self.assertEqual(self.snapshot(), committed)
        with self.assertRaises(RustFixtureError):
            self.adapter.commit_batch(
                lease, "cursor-empty", idle, request_id="4" * 32)
        self.assertEqual(self.snapshot(), committed)
        self.assertNotEqual(before, committed)

    def test_capture_reenable_revoke_and_old_lease_fencing(self):
        lease = self.activate()
        first = PollBatch(SOURCE, "cursor-1", (
            lifecycle("event-life"), message("event-message")))
        self.adapter.commit_batch(lease, "", first, request_id="2" * 32)

        disabled = self.adapter.set_capture_enabled(
            lease, enabled=False, expected_enabled=True, expected_cursor="cursor-1",
            expected_dirty_seq=1, request_id="3" * 32)
        self.assertEqual(disabled["disposition"], "committed")
        self.assertEqual(disabled["lease"].policy_epoch, 2)
        self.assertEqual(self.adapter.capture_revoked(SOURCE), True)
        disabled_snapshot = self.snapshot()
        with self.assertRaises(RustFixtureError):
            self.adapter.commit_batch(
                lease, "cursor-1", PollBatch(SOURCE, "cursor-2", (lifecycle("old"),)),
                request_id="4" * 32)
        self.assertEqual(self.snapshot(), disabled_snapshot)

        enabled = self.adapter.set_capture_enabled(
            disabled["lease"], enabled=True, expected_enabled=False,
            expected_cursor="cursor-1", expected_dirty_seq=2, request_id="5" * 32)
        self.assertEqual(enabled["disposition"], "committed")
        self.assertFalse(self.adapter.capture_revoked(SOURCE))
        reactivated = self.activate(request_id="6" * 32)
        self.assertEqual((reactivated.cursor, reactivated.policy_epoch), ("cursor-1", 3))

        second = PollBatch(SOURCE, "cursor-2", (lifecycle("event-next", "failed"),))
        self.adapter.commit_batch(reactivated, "cursor-1", second, request_id="7" * 32)
        revoked = self.adapter.revoke_source(
            reactivated, cursor="cursor-2", expected_dirty_seq=4, request_id="8" * 32)
        self.assertEqual((revoked["capture_enabled"], revoked["cursor"],
                          revoked["policy_epoch"], revoked["dirty_seq"]),
                         (False, "cursor-2", 4, 5))
        self.assertTrue(self.adapter.capture_revoked(SOURCE))
        revoked_snapshot = self.snapshot()
        self.assertEqual(revoked_snapshot[1][1], ())
        with self.assertRaises(RustFixtureError):
            self.adapter.commit_batch(
                reactivated, "cursor-2", PollBatch(SOURCE, "cursor-3", (lifecycle("late"),)),
                request_id="9" * 32)
        self.assertEqual(self.snapshot(), revoked_snapshot)

    def test_subset_rejects_refs_diagnostics_relationships_members_and_root_gaps(self):
        lease = self.activate()
        before = self.snapshot()
        cases = (
            ("reference payload", PollBatch(SOURCE, "cursor-1", (lifecycle("ref"),)),
             {"reference_refs": ({"id": "secret"},)}),
            ("diagnostic", PollBatch(SOURCE, "cursor-1", ({
                "event_id": "diagnostic", "revision": 0, "kind": "diagnostic",
                "time_ns": "1", "payload": {"code": "warning", "severity": "warning"},
            },)), {}),
            ("relationship", PollBatch(SOURCE, "cursor-1", ({
                "event_id": "relationship", "revision": 0, "kind": "relationship",
                "time_ns": "1", "payload": {"parent_session_id": "parent", "kind": "delegated"},
                "identity": {"session_id": "child"},
            },)), {}),
            ("member cursor", PollBatch(SOURCE, "cursor-1", (),
                                          member_id="b" * 64, member_cursor="member-1"), {}),
            ("root gap", PollBatch(SOURCE, "cursor-1", ({
                "event_id": "gap", "revision": 0, "kind": "diagnostic",
                "time_ns": "1", "payload": {"code": "possible_source_gap",
                                                "severity": "warning"},
            },), root_gap=True), {}),
        )
        for index, (label, batch, kwargs) in enumerate(cases, start=2):
            with self.subTest(unsupported=label):
                with self.assertRaises(UnsupportedMetadataSubset):
                    self.adapter.commit_batch(lease, "", batch,
                                              request_id=f"{index:032x}", **kwargs)
                self.assertEqual(self.snapshot(), before)

    def test_unsupported_reference_member_and_gap_histories_fail_closed(self):
        histories = (
            ("provider reference owner",
             "INSERT INTO plugin_reference_owner_v1(source_id,provider_id) VALUES(?,?)",
             ("pi-content",)),
            ("member cursor",
             "INSERT INTO plugin_source_member_cursor_v1(source_id,member_id,cursor) VALUES(?,?,?)",
             ("member-1", "member-cursor")),
            ("source gap",
             "INSERT INTO plugin_source_gap_invalidated_v1(source_id,native_session_id,last_record_ns) "
             "VALUES(?,?,?)", ("*", 0)),
        )
        for index, (label, sql, tail) in enumerate(histories, start=1):
            with self.subTest(history=label):
                source_id = f"unsupported-{index}"
                adapter = self.new_adapter(source_id)
                try:
                    lease = adapter.activate_source(
                        plugin_id=PLUGIN, source_id=source_id,
                        generation=GENERATION, request_id=f"{index:032x}")
                    with closing(sqlite3.connect(adapter.clone_path)) as db:
                        db.execute(sql, (source_id, *tail))
                        db.commit()
                    before = self.snapshot(source_id, adapter=adapter)
                    with self.assertRaises(RustFixtureError):
                        adapter.set_capture_enabled(
                            lease, enabled=False, expected_enabled=True,
                            expected_cursor="", expected_dirty_seq=0,
                            request_id=f"{index + 100:032x}")
                    self.assertEqual(self.snapshot(source_id, adapter=adapter), before)
                finally:
                    adapter.close()

    def test_normal_clone_dispatch_rejects_admit_capture_revoke_without_writes(self):
        lease = self.activate()
        admit = self.adapter.batch_request(
            lease, "", PollBatch(SOURCE, "cursor-1", (lifecycle("service"),)),
            request_id="2" * 32)
        capture = {"version": 2, "request_id": "3" * 32,
                   "receiver_id": lease.receiver_id,
                   "operation": {"op": "set_source_capture", "lease": lease.wire(),
                                 "enabled": False, "expected_capture_enabled": True,
                                 "expected_cursor": "", "expected_dirty_seq": 0}}
        revoke = {"version": 2, "request_id": "4" * 32,
                  "receiver_id": lease.receiver_id,
                  "operation": {"op": "revoke_source", "lease": lease.wire(),
                                "cursor": "", "expected_dirty_seq": 0}}
        for label, request in (("AdmitBatch", admit), ("SetSourceCapture", capture),
                               ("RevokeSource", revoke)):
            with self.subTest(operation=label):
                before = self.snapshot()
                with self.assertRaises(RustFixtureError) as raised:
                    self.adapter.send_normal_dispatch(request)
                expected_message = {
                    "AdmitBatch": "mixed admission is unavailable through the storage service path",
                    "SetSourceCapture": "real-schema clone accepts only activation or admission",
                    "RevokeSource": "clone revocation is unavailable through the storage service path",
                }[label]
                self.assertIn(expected_message, str(raised.exception))
                self.assertEqual(self.snapshot(), before)


if __name__ == "__main__":
    unittest.main()
