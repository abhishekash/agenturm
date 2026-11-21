"""Behavioral contract for the Rust marked-clone metadata subset.

The marked-clone façade exercises selected PluginSession call shapes, but this
is not a PluginAdmissionPort conformance suite or production runtime backend.
References, root gaps, relationships, and broad Python wire compatibility stay
outside the fixture subset.
"""
from __future__ import annotations

import hashlib
import json
import os
import sqlite3
import sys
import tempfile
import time
import unittest
from contextlib import closing
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch
from observatory.plugin_projection import project_source
from observatory.plugin_runtime import PluginSession
from observatory.plugin_wire import PollBatch
from tests.support.rust_metadata_adapter import (
    RustFixtureError,
    RustMarkedCloneMetadataAdapter,
    RustMarkedCloneSessionPortSubset,
    UnsupportedMetadataSubset,
    logical_clone_snapshot,
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
            lifecycle("event-life"), message("event-message"),
            {"event_id": "event-tool", "revision": 0, "kind": "tool",
             "time_ns": "9007199254740995",
             "payload": {"category": "shell", "result": "succeeded"},
             "identity": {"session_id": "tool-session", "operation_id": "op-1"}},
            {"event_id": "event-check", "revision": 0, "kind": "check",
             "time_ns": "9007199254740996",
             "payload": {"category": "test", "result": "passed"},
             "identity": {"session_id": "check-session"}},
            {"event_id": "event-usage", "revision": 0, "kind": "usage",
             "time_ns": "9007199254740997",
             "payload": {"input_tokens": 42, "output_tokens": 7}},
        ))
        request = self.adapter.batch_request(lease, "", first, request_id="2" * 32)
        committed = self.adapter.commit_batch(
            lease, "", first, request_id=request["request_id"])
        self.assertEqual((committed["disposition"], committed["cursor"]),
                         ("committed", "cursor-1"))
        with closing(sqlite3.connect(self.adapter.clone_path)) as db:
            persisted = {kind: (payload, session_id, operation_id) for kind, payload,
                         session_id, operation_id in db.execute(
                             "SELECT kind,payload_json,native_session_id,native_operation_id "
                             "FROM plugin_evidence_v1 WHERE source_id=?", (SOURCE,))}
        self.assertEqual(persisted, {
            "lifecycle": ('{"state":"completed"}', "session-1", ""),
            "message": ('{"role":"assistant"}', "session-2", ""),
            "tool": ('{"category":"shell","result":"succeeded"}', "tool-session", "op-1"),
            "check": ('{"category":"test","result":"passed"}', "check-session", ""),
            "usage": ('{"input_tokens":42,"output_tokens":7}', "", ""),
        })
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
        before_restart_replay = logical_clone_snapshot(self.adapter.clone_path)
        replay = self.adapter.commit_batch(
            lease, "", first, request_id=request["request_id"])
        self.assertEqual(replay["disposition"], "replay")
        self.assertEqual(logical_clone_snapshot(self.adapter.clone_path),
                         before_restart_replay)
        resumed = self.activate(request_id="4" * 32)
        self.assertEqual(resumed.cursor, "cursor-1")

    def test_reactivation_fences_stale_lease_without_mutation(self):
        stale = self.activate()
        fresh = self.activate(request_id="2" * 32)
        before = self.snapshot()
        with self.assertRaisesRegex(RustFixtureError, "stale or unsupported source lease") as raised:
            self.adapter.commit_batch(
                stale, "", PollBatch(SOURCE, "cursor-1", (lifecycle("stale"),)),
                request_id="3" * 32)
        self.assertEqual(self.adapter.classify_lease_failure(
            raised.exception, SOURCE), "source_fenced")
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

    def test_revoked_lease_failure_is_classified_without_mutation(self):
        lease = self.activate()
        self.adapter.set_capture_enabled(
            lease, enabled=False, expected_enabled=True, expected_cursor="",
            expected_dirty_seq=0, request_id="2" * 32)
        before = self.snapshot()
        with self.assertRaises(RustFixtureError) as raised:
            self.adapter.commit_batch(
                lease, "", PollBatch(SOURCE, "cursor-1", (lifecycle("revoked"),)),
                request_id="3" * 32)
        self.assertEqual(self.adapter.classify_lease_failure(
            raised.exception, SOURCE), "capture_revoked")
        self.assertEqual(self.snapshot(), before)

    def test_subset_rejects_refs_relationships_and_root_gaps(self):
        lease = self.activate()
        before = self.snapshot()
        cases = (
            ("reference payload", PollBatch(SOURCE, "cursor-1", (lifecycle("ref"),)),
             {"reference_refs": ({"id": "secret"},)}),
            ("relationship", PollBatch(SOURCE, "cursor-1", ({
                "event_id": "relationship", "revision": 0, "kind": "relationship",
                "time_ns": "1", "payload": {"parent_session_id": "parent", "kind": "delegated"},
                "identity": {"session_id": "child"},
            },)), {}),
            ("root gap", PollBatch(SOURCE, "cursor-1", ({
                "event_id": "gap", "revision": 0, "kind": "diagnostic",
                "time_ns": "1", "payload": {"code": "possible_source_gap",
                                                "severity": "warning"},
            },), root_gap=True), {}),
        )
        from unittest.mock import patch
        with patch.object(self.adapter, "_run", wraps=self.adapter._run) as rust_call:
            for index, (label, batch, kwargs) in enumerate(cases, start=2):
                with self.subTest(unsupported=label):
                    with self.assertRaises(UnsupportedMetadataSubset):
                        self.adapter.commit_batch(lease, "", batch,
                                                  request_id=f"{index:032x}", **kwargs)
                    self.assertEqual(self.snapshot(), before)
            rust_call.assert_not_called()

    def test_correction_revision_is_classified_outside_subset(self):
        lease = self.activate()
        correction = lifecycle("revision-one", "failed")
        correction["revision"] = 1
        before = self.snapshot()
        with self.assertRaisesRegex(UnsupportedMetadataSubset, "correction revisions"):
            self.adapter.commit_batch(
                lease, "", PollBatch(SOURCE, "cursor-1", (correction,)),
                request_id="2" * 32)
        self.assertEqual(self.snapshot(), before)

    def test_raw_prompt_and_output_fields_fail_before_rust_dispatch(self):
        from unittest.mock import patch

        from observatory.plugin_wire import ProtocolError

        lease = self.activate()
        batches = (
            PollBatch(SOURCE, "cursor-1", ({
                "event_id": "raw-message", "revision": 0, "kind": "message",
                "time_ns": "1", "payload": {"role": "assistant", "prompt": "secret",
                                               "output": "secret"},
                "identity": {"session_id": "private-session"},
            },)),
            PollBatch(SOURCE, "cursor-1", ({
                "event_id": "raw-tool", "revision": 0, "kind": "tool",
                "time_ns": "1", "payload": {"category": "shell", "result": "succeeded",
                                               "prompt": "secret", "output": "secret"},
                "identity": {"session_id": "private-session"},
            },)),
        )
        before = self.snapshot()
        with patch.object(self.adapter, "_run", wraps=self.adapter._run) as rust_call:
            for index, batch in enumerate(batches, start=2):
                with self.subTest(kind=batch.records[0]["kind"]):
                    with self.assertRaises(ProtocolError):
                        self.adapter.commit_batch(
                            lease, "", batch, request_id=f"{index:032x}")
            rust_call.assert_not_called()
        self.assertEqual(self.snapshot(), before)

    def test_plugin_session_runs_supported_clone_subset_and_resumes_after_member_gap(self):
        """Exercise the real PluginSession orchestration with a synthetic worker.

        Rust remains a marked-clone fixture. The fake process only supplies
        content-free PollBatch values and records ACK after the port returns.
        """
        from unittest.mock import patch

        from tests.test_plugin_process import ProcessHostTests

        source_id = "session-clone-source"
        clone_adapter = self.new_adapter(source_id)
        clone_path = clone_adapter.clone_path
        clone_store = Store(str(clone_path))
        self.addCleanup(clone_store.close)

        def canonical_files():
            paths = (self.canonical_path, Path(f"{self.canonical_path}-wal"),
                     Path(f"{self.canonical_path}-shm"))
            return tuple((path.stat().st_ino, path.stat().st_size,
                          path.stat().st_mtime_ns,
                          hashlib.sha256(path.read_bytes()).hexdigest())
                         if path.exists() else None for path in paths)

        canonical_before = canonical_files()

        oracle_path = self.root / "session-oracle.db"
        with closing(sqlite3.connect(self.canonical_path)) as canonical, \
                closing(sqlite3.connect(oracle_path)) as oracle_db:
            canonical.backup(oracle_db)
        oracle = Store(str(oracle_path))
        self.addCleanup(oracle.close)

        manifest_fixture = ProcessHostTests("runTest")
        manifest_fixture.setUp()
        self.addCleanup(manifest_fixture.doCleanups)
        manifest = manifest_fixture.manifest()

        member_a, member_b = "a" * 64, "b" * 64
        batches = [
            PollBatch(source_id, "session-global-1", ({
                "event_id": "session-life-a", "revision": 0, "kind": "lifecycle",
                "time_ns": "201", "payload": {"state": "started"},
                "identity": {"session_id": "session-a"},
            },), member_a, "file-cursor-a"),
            PollBatch(source_id, "session-global-2", ({
                "event_id": "session-life-b", "revision": 0, "kind": "lifecycle",
                "time_ns": "202", "payload": {"state": "completed"},
                "identity": {"session_id": "session-b"},
            },), member_b, "file-cursor-b"),
            PollBatch(source_id, "session-global-gap", ({
                "event_id": "session-gap-a", "revision": 0, "kind": "diagnostic",
                "time_ns": "203", "payload": {"code": "possible_source_gap",
                                                   "severity": "warning"},
            },), member_a, ""),
        ]
        events: list[str] = []

        class SyntheticProcess:
            def __init__(self, _manifest, *, source_id, **kwargs):
                self.manifest = _manifest
                self.source_id = source_id
                self.required_features = ("source-discovery/1",)
                self.pending = list(batches)
                self.acks: list[str] = []
                self.process = None

            def start(self):
                events.append("worker_start")
                return self

            def poll_commit(self, cursor, commit, *, members=None):
                events.append("poll")
                batch = self.pending.pop(0)
                if batch.cursor == cursor:
                    raise AssertionError("synthetic batch must advance the global cursor")
                accepted = commit(batch)
                if accepted != batch.cursor:
                    raise AssertionError("session acknowledged a different cursor")
                with closing(sqlite3.connect(clone_path)) as db:
                    durable = db.execute(
                        "SELECT cursor FROM plugin_source_cursor_v1 WHERE source_id=?",
                        (source_id,)).fetchone()[0]
                if durable != batch.cursor:
                    raise AssertionError("ACK attempted before Rust cursor was durable")
                self.acks.append(batch.cursor)
                events.append("ack:" + batch.cursor)
                return batch

            def close(self, *, force=False):
                events.append("worker_close")
                self.process = None

        adapter_run = clone_adapter._run

        def tracked_run(mode, request):
            before = (logical_clone_snapshot(clone_path)
                      if mode in ("--v2-clone-member-cursor-path",
                                  "--v2-clone-diagnostic-path") else None)
            events.append(mode)
            reply = adapter_run(mode, request)
            if before is not None:
                self.assertEqual(logical_clone_snapshot(clone_path), before)
            return reply

        clone_adapter._run = tracked_run
        port = RustMarkedCloneSessionPortSubset(clone_adapter)
        with patch("observatory.plugin_runtime.PluginProcess", SyntheticProcess):
            session = PluginSession(clone_store, manifest, source_id=source_id,
                                    admission_port=port)
            session.start()
            self.assertEqual(session.cursor, "")
            self.assertEqual(session.members, {})
            self.assertEqual(session.last_diagnostic, {})
            # Session start currently launches the process before its read-only
            # cursor/diagnostic refresh, but both reads finish before first poll.
            self.assertLess(events.index("--v2-activation-clone-path"),
                            events.index("worker_start"))

            python_lease = activate_source(
                oracle, plugin_id=manifest.id, source_id=source_id,
                generation=manifest.artifact_sha256)

        def semantic_source_snapshot(path):
            with closing(sqlite3.connect(path)) as db:
                def rows(sql):
                    return tuple(tuple(row) for row in db.execute(sql, (source_id,)))
                return {
                    "cursor": rows(
                        "SELECT receiver_id,plugin_id,generation,cursor,prior_cursor,"
                        "last_batch_digest,dirty_seq,policy_epoch,capture_enabled "
                        "FROM plugin_source_cursor_v1 WHERE source_id=?"),
                    "evidence": rows(
                        "SELECT event_id_hash,revision,kind,time_ns,payload_json,record_digest,"
                        "native_session_id,native_worker_id,native_turn_id,native_operation_id,"
                        "member_id,policy_epoch FROM plugin_evidence_v1 WHERE source_id=? "
                        "ORDER BY event_id_hash,revision"),
                    "dirty": rows(
                        "SELECT dirty_seq FROM plugin_evidence_dirty_v1 WHERE source_id=?"),
                    "member_cursors": rows(
                        "SELECT member_id,cursor FROM plugin_source_member_cursor_v1 "
                        "WHERE source_id=? ORDER BY member_id"),
                    "session_members": rows(
                        "SELECT native_session_id,member_id FROM plugin_session_member_v1 "
                        "WHERE source_id=? ORDER BY native_session_id"),
                    "gaps": rows(
                        "SELECT native_session_id FROM "
                        "plugin_source_gap_invalidated_v1 WHERE source_id=? "
                        "ORDER BY native_session_id"),
                    "projection": rows(
                        "SELECT native_session_id,session_key,state,reason_code,state_time_ns,"
                        "policy_epoch,source_dirty_seq,usage_json "
                        "FROM plugin_session_projection_v1 WHERE source_id=? "
                        "ORDER BY native_session_id"),
                    "projection_cursor": rows(
                        "SELECT projected_seq,policy_epoch FROM plugin_projection_cursor_v1 "
                        "WHERE source_id=?"),
                }

        expected = ""
        for batch in batches:
            self.assertEqual(commit_batch(oracle, python_lease, expected, batch),
                             batch.cursor)
            received = session.poll_once()
            self.assertEqual(received, batch)
            expected = batch.cursor
            self.assertEqual(session.cursor, expected)
            self.assertEqual(session.process.acks[-1], expected)
        self.assertLess(events.index("--v2-clone-member-cursor-path"),
                        events.index("poll"))
        self.assertLess(events.index("--v2-clone-diagnostic-path"),
                        events.index("poll"))

        self.assertEqual(session.members, {member_b: "file-cursor-b"})
        self.assertEqual(session.last_diagnostic, {
            "code": "possible_source_gap", "severity": "warning", "time_ns": "203"})
        self.assertEqual(session.process.acks,
                         ["session-global-1", "session-global-2", "session-global-gap"])
        # Use the same Python projector on each detached DB and compare its
        # resulting session rows; session A is gapped while sibling B stays current.
        project_source(clone_store, source_id)
        project_source(oracle, source_id)
        self.assertEqual(project_source(clone_store, source_id),
                         project_source(oracle, source_id))
        rust_state = semantic_source_snapshot(clone_path)
        python_state = semantic_source_snapshot(oracle_path)
        for key in rust_state:
            with self.subTest(state=key):
                self.assertEqual(rust_state[key], python_state[key])
        for path in (clone_path, oracle_path):
            with closing(sqlite3.connect(path)) as db:
                self.assertGreaterEqual(db.execute(
                    "SELECT last_record_ns FROM plugin_source_gap_invalidated_v1 "
                    "WHERE source_id=? AND native_session_id='session-a'",
                    (source_id,)).fetchone()[0], 203)
        with clone_store._connect() as db:
            projected = {row[0]: (row[1], row[2]) for row in db.execute(
                "SELECT native_session_id,state,reason_code "
                "FROM plugin_session_projection_v1 WHERE source_id=?", (source_id,))}
        self.assertEqual(projected["session-a"], ("unknown", "source_gap"))
        self.assertEqual(projected["session-b"], ("completed", ""))
        with closing(sqlite3.connect(clone_path)) as db:
            self.assertEqual(db.execute(
                "SELECT cursor FROM plugin_source_cursor_v1 WHERE source_id=?",
                (source_id,)).fetchone()[0], "session-global-gap")
        session.close()
        self.assertTrue(clone_adapter._closed)
        self.assertIn("worker_close", events)
        with closing(sqlite3.connect(clone_path)) as db:
            self.assertEqual(db.execute(
                "SELECT cursor FROM plugin_source_cursor_v1 WHERE source_id=?",
                (source_id,)).fetchone()[0], "session-global-gap")

        # Reopen the same marked clone and resume the durable global/member
        # cursors and gap diagnostic through a new PluginSession.
        reopened_adapter = RustMarkedCloneMetadataAdapter(
            os.environ["AGENTURM_RUST_METADATA_ADAPTER_BIN"], clone_path)
        resumed_port = RustMarkedCloneSessionPortSubset(reopened_adapter)
        with closing(sqlite3.connect(clone_path)) as db:
            self.assertEqual(db.execute(
                "SELECT cursor FROM plugin_source_cursor_v1 WHERE source_id=?",
                (source_id,)).fetchone()[0], "session-global-gap")
        with patch("observatory.plugin_runtime.PluginProcess", SyntheticProcess):
            resumed = PluginSession(clone_store, manifest, source_id=source_id,
                                    admission_port=resumed_port).start()
            self.assertEqual(resumed.cursor, "session-global-gap")
            self.assertEqual(resumed.members, {member_b: "file-cursor-b"})
            self.assertEqual(resumed.last_diagnostic["code"], "possible_source_gap")
            resumed.close()
        self.assertTrue(reopened_adapter._closed)
        self.assertEqual(canonical_files(), canonical_before)

    def test_session_port_subset_rejects_before_worker_start(self):
        from unittest.mock import patch

        from tests.test_plugin_process import ProcessHostTests

        adapter = self.new_adapter("preworker-reject-source")
        store = Store(str(adapter.clone_path))
        self.addCleanup(store.close)
        fixture = ProcessHostTests("runTest")
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        manifest = fixture.manifest()
        events: list[str] = []

        class NeverStartedProcess:
            def __init__(self, _manifest, **kwargs):
                self.manifest = _manifest
                self.source_id = kwargs["source_id"]
                self.required_features = ()

            def start(self):
                events.append("worker_start")

            def close(self, *, force=False):
                events.append("worker_close")

        def reject_install(_registry):
            events.append("install_rejected")
            raise UnsupportedMetadataSubset("custom reference providers are not admitted")

        adapter.install_reference_schema = reject_install
        port = RustMarkedCloneSessionPortSubset(adapter)
        with patch("observatory.plugin_runtime.PluginProcess", NeverStartedProcess):
            session = PluginSession(store, manifest, source_id="preworker-reject-source",
                                    admission_port=port)
            with self.assertRaisesRegex(UnsupportedMetadataSubset,
                                        "custom reference providers"):
                session.start()
        self.assertEqual(events, ["install_rejected", "worker_close"])

    def test_supervisor_factory_closes_preflight_port_and_rejects_source_before_worker(self):
        from unittest.mock import patch

        from observatory.plugin_supervisor import PluginSource, PluginSupervisor
        from tests.test_plugin_process import ProcessHostTests

        source_id = "supervised-clone-source"
        seed_adapter = self.new_adapter(source_id)
        clone_path = seed_adapter.clone_path
        seed_adapter.close()
        store = Store(str(clone_path))
        self.addCleanup(store.close)
        fixture = ProcessHostTests("runTest")
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        manifest = fixture.manifest()
        starts: list[str] = []
        ports: list[RustMarkedCloneSessionPortSubset] = []

        class SupervisorProcess:
            def __init__(self, _manifest, *, source_id, **kwargs):
                self.manifest = _manifest
                self.source_id = source_id
                self.required_features = ()
                self.process = None

            def start(self):
                starts.append(self.source_id)
                return self

            def poll_commit(self, cursor, commit, *, members=None):
                idle = PollBatch(self.source_id, cursor, ())
                if commit(idle) != cursor:
                    raise AssertionError("idle supervisor commit changed cursor")
                return idle

            def close(self, *, force=False):
                self.process = None

        def factory(_store):
            port = RustMarkedCloneSessionPortSubset(
                RustMarkedCloneMetadataAdapter(
                    os.environ["AGENTURM_RUST_METADATA_ADAPTER_BIN"], clone_path))
            if ports:
                self.assertTrue(ports[-1].adapter._closed,
                                "preflight adapter must close before worker factory call")
            ports.append(port)
            return port

        valid_source = PluginSource(manifest, source_id,
                                    poll_interval=0.05, timeout=0.5)
        with patch("observatory.plugin_runtime.PluginProcess", SupervisorProcess):
            manager = PluginSupervisor(store, (valid_source,), backoff_seconds=0.05,
                                       admission_port_factory=factory)
            self.addCleanup(manager.close)
            self.assertEqual(len(ports), 1)
            self.assertTrue(ports[0].adapter._closed)
            manager.start()
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                if manager.snapshot()[0]["state"] == "running":
                    break
                time.sleep(0.01)
            else:
                self.fail(f"supervisor did not start worker: {manager.snapshot()}")
            manager.close()
            self.assertEqual(starts, [source_id])
            self.assertGreaterEqual(len(ports), 2)
            self.assertTrue(all(port.adapter._closed for port in ports))

            bad_source = PluginSource(
                manifest, "rejected-clone-source", poll_interval=0.05,
                timeout=0.5, capture_mode="unregistered-mode")
            starts_before_rejection = list(starts)
            with self.assertRaisesRegex(ValueError, "unsupported plugin capture mode"):
                PluginSupervisor(store, (bad_source,), admission_port_factory=factory)
            self.assertEqual(starts, starts_before_rejection)
            self.assertTrue(ports[-1].adapter._closed)

    def test_unsupported_reference_member_and_gap_histories_fail_closed(self):
        histories = (
            ("provider reference owner",
             "INSERT INTO plugin_reference_owner_v1(source_id,provider_id) VALUES(?,?)",
             ("pi-content",)),
            ("malformed member cursor",
             "INSERT INTO plugin_source_member_cursor_v1(source_id,member_id,cursor) VALUES(?,?,?)",
             ("bad-member", "member-cursor")),
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
                before = logical_clone_snapshot(self.adapter.clone_path)
                with self.assertRaises(RustFixtureError) as raised:
                    self.adapter.send_normal_dispatch(request)
                expected_message = {
                    "AdmitBatch": "mixed admission is unavailable through the storage service path",
                    "SetSourceCapture": "real-schema clone accepts only activation or admission",
                    "RevokeSource": "clone revocation is unavailable through the storage service path",
                }[label]
                self.assertIn(expected_message, str(raised.exception))
                self.assertEqual(logical_clone_snapshot(self.adapter.clone_path), before)


if __name__ == "__main__":
    unittest.main()
