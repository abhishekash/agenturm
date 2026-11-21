"""Provider-aware contract for the one fixed Rust clone audit fixture."""
from __future__ import annotations

import os
import hashlib
import json
import sqlite3
import tempfile
import unittest
from contextlib import closing
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory.plugin_batch_handlers import DEFAULT_BATCH_HANDLERS
from observatory.plugin_capture_modes import CaptureMode, DEFAULT_CAPTURE_MODES
from observatory.plugin_manifest import PluginManifest
from observatory.plugin_projection import project_source
from observatory.plugin_reference_admission import DEFAULT_REFERENCE_ADMISSIONS
from observatory.plugin_runtime import PluginSession
from observatory.plugin_supervisor import PluginSource
from observatory.plugin_wire import PollBatch
from observatory.plugin_wire import ReferencePollBatch
from tests.test_plugin_reference_admission_port_contract import (
    PluginReferenceAdmissionPortContract,
    PLUGIN_ID,
    SOURCE_ID,
    reference_batch,
)
from tests.support.rust_metadata_adapter import (
    RustFixtureError,
    RustMarkedCloneMetadataAdapter,
    RustMarkedCloneSessionPortSubset,
    RustMetadataLease,
    assert_clone_database_reader_current,
    assert_clone_database_quiescent,
    logical_clone_snapshot,
)


@unittest.skipUnless(os.environ.get("AGENTURM_RUST_METADATA_ADAPTER_BIN"),
                     "Rust marked-clone fixture binary required")
class RustAuditReferenceAdmissionPortContractTests(
        PluginReferenceAdmissionPortContract, unittest.TestCase):
    def make_backend(self, directory: Path, registry):
        canonical_path = directory / "canonical.db"
        canonical = Store(str(canonical_path))
        self.canonical_path = canonical_path
        canonical.close()
        self.canonical_before = logical_clone_snapshot(canonical_path)
        self.canonical_files_before = self._file_snapshot(canonical_path)

        clone_path = directory / "reference-clone.db"
        with closing(sqlite3.connect(canonical_path)) as source:
            with closing(sqlite3.connect(clone_path)) as clone:
                source.backup(clone)
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                       ("storage_activation_clone_v1", "enabled"))
            db.commit()

        adapter_ref = [RustMarkedCloneMetadataAdapter(
            os.environ["AGENTURM_RUST_METADATA_ADAPTER_BIN"], clone_path)]
        adapter_ref[0].install_reference_schema(registry)

        def new_port():
            adapter_ref[0] = RustMarkedCloneMetadataAdapter(
                os.environ["AGENTURM_RUST_METADATA_ADAPTER_BIN"], clone_path)
            return RustMarkedCloneSessionPortSubset(adapter_ref[0])

        def set_capture_enabled(source_id, enabled):
            with closing(sqlite3.connect(clone_path)) as db:
                source = db.execute(
                    "SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,cursor,dirty_seq "
                    "FROM plugin_source_cursor_v1 WHERE source_id=?",
                    (source_id,)).fetchone()
            if source is None:
                raise AssertionError("fixture source lease is unavailable")
            lease = RustMetadataLease(
                source[0], source[1], source_id, source[2], source[3], source[4], source[5])
            result = adapter_ref[0].set_capture_enabled(
                lease, enabled=enabled, expected_enabled=not enabled,
                expected_cursor=source[5], expected_dirty_seq=source[6],
                request_id="d" * 32)
            return result

        def durable_snapshot(source_id):
            with closing(sqlite3.connect(clone_path)) as db:
                source = db.execute(
                    "SELECT receiver_id,plugin_id,generation,cursor,dirty_seq,lease_epoch,"
                    "policy_epoch,capture_enabled FROM plugin_source_cursor_v1 WHERE source_id=?",
                    (source_id,)).fetchone()
                evidence = db.execute(
                    "SELECT event_id_hash,revision,record_digest FROM plugin_evidence_v1 "
                    "WHERE source_id=? ORDER BY event_id_hash,revision", (source_id,)).fetchall()
                audit = db.execute(
                    "SELECT source_id,cursor,digest FROM audit_fixture WHERE source_id=?",
                    (source_id,)).fetchall()
                owners = db.execute(
                    "SELECT provider_id FROM plugin_reference_owner_v1 WHERE source_id=? "
                    "ORDER BY provider_id", (source_id,)).fetchall()
                dirty = db.execute(
                    "SELECT dirty_seq FROM plugin_evidence_dirty_v1 WHERE source_id=?",
                    (source_id,)).fetchall()
                projection = db.execute(
                    "SELECT projected_seq,policy_epoch FROM plugin_projection_cursor_v1 "
                    "WHERE source_id=?", (source_id,)).fetchall()
                binding_exists = db.execute(
                    "SELECT 1 FROM sqlite_master WHERE type='table' "
                    "AND name='storage_admission_batch_binding_v1'").fetchone()
                binding = (() if binding_exists is None else db.execute(
                    "SELECT * FROM storage_admission_batch_binding_v1 WHERE source_id=?",
                    (source_id,)).fetchall())
                requests_exists = db.execute(
                    "SELECT 1 FROM sqlite_master WHERE type='table' "
                    "AND name='storage_admission_request_v2'").fetchone()
                requests = (() if requests_exists is None else db.execute(
                    "SELECT request_id,receiver_id,request_digest,source_id,cursor,batch_digest,"
                    "dirty_seq,disposition FROM storage_admission_request_v2 WHERE source_id=? "
                    "ORDER BY request_id", (source_id,)).fetchall())
                content = db.execute(
                    "SELECT cursor,refs_digest FROM plugin_content_batch_v1 WHERE source_id=?",
                    (source_id,)).fetchall()
                status = db.execute(
                    "SELECT cursor,reason,refs_count,policy_epoch FROM plugin_content_status_v1 "
                    "WHERE source_id=?", (source_id,)).fetchall()
                captures_exists = db.execute(
                    "SELECT 1 FROM sqlite_master WHERE type='table' "
                    "AND name='storage_source_capture_set_v2'").fetchone()
                captures = (() if captures_exists is None else tuple(db.execute(
                    "SELECT request_id,source_id,enabled,disposition FROM "
                    "storage_source_capture_set_v2 WHERE source_id=? ORDER BY request_id",
                    (source_id,))))
            return (tuple(source) if source else None,
                    tuple(tuple(row) for row in evidence),
                    tuple(tuple(row) for row in audit),
                    tuple(tuple(row) for row in owners),
                    tuple(tuple(row) for row in dirty),
                    tuple(tuple(row) for row in projection),
                    tuple(tuple(row) for row in binding),
                    tuple(tuple(row) for row in requests),
                    tuple(tuple(row) for row in content),
                    tuple(tuple(row) for row in status),
                    tuple(tuple(row) for row in captures))

        def set_failure_mode(mode, enabled):
            table = {
                "provider_commit_after_write": "storage_admission_request_v2",
                "provider_erase_after_write": "storage_source_capture_set_v2",
            }.get(mode)
            if table is None:
                raise ValueError("unknown provider failure mode")
            with closing(sqlite3.connect(clone_path)) as db:
                if not enabled:
                    db.execute(f'DROP TABLE IF EXISTS "{table}"')
                elif table == "storage_admission_request_v2":
                    db.execute("CREATE TABLE storage_admission_request_v2("
                               "request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,"
                               "request_digest TEXT NOT NULL,source_id TEXT NOT NULL,"
                               "cursor TEXT NOT NULL,batch_digest TEXT NOT NULL,"
                               "dirty_seq INTEGER NOT NULL,"
                               "disposition TEXT NOT NULL CHECK(disposition='forced-failure'))")
                else:
                    db.execute("CREATE TABLE storage_source_capture_set_v2("
                               "request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,"
                               "request_digest TEXT NOT NULL,source_id TEXT NOT NULL,"
                               "plugin_id TEXT NOT NULL,generation TEXT NOT NULL,"
                               "expected_capture_enabled INTEGER NOT NULL,enabled INTEGER NOT NULL,"
                               "disposition TEXT NOT NULL CHECK(disposition='forced-failure'),"
                               "lease_epoch INTEGER NOT NULL,policy_epoch INTEGER NOT NULL,"
                               "cursor TEXT NOT NULL,dirty_seq INTEGER NOT NULL)")
                db.commit()

        return (new_port, set_capture_enabled, durable_snapshot, set_failure_mode)

    @staticmethod
    def _file_snapshot(path: Path):
        output = []
        for candidate in (path, Path(str(path) + "-wal"), Path(str(path) + "-shm")):
            output.append((candidate.name, candidate.read_bytes() if candidate.exists() else None))
        return tuple(output)

    def tearDown(self):
        self._close_port()
        self.assertEqual(logical_clone_snapshot(self.canonical_path), self.canonical_before)
        self.assertEqual(self._file_snapshot(self.canonical_path), self.canonical_files_before)

    def test_missing_provider_fails_closed_before_persisting_batch(self):
        with closing(sqlite3.connect(self.port.clone_path)) as db:
            db.execute("DROP TABLE audit_fixture")
            db.commit()
        before = logical_clone_snapshot(self.port.clone_path)
        with self.assertRaises(Exception):
            self.commit()
        self.assertEqual(logical_clone_snapshot(self.port.clone_path), before)

    def test_audit_submission_conflict_and_normal_dispatch_rejection_are_no_write(self):
        batch = reference_batch("cursor-1")
        request = self.port.batch_request(
            self.lease, "", batch, request_id="1" * 32,
            reference_submission=self.submission(), reference_admissions=self.registry)
        before = logical_clone_snapshot(self.port.clone_path)
        with self.assertRaises(RustFixtureError):
            self.port.send_normal_dispatch(request)
        self.assertEqual(logical_clone_snapshot(self.port.clone_path), before)

        self.assertEqual(self.commit(request_id="1" * 32), "cursor-1")
        committed = logical_clone_snapshot(self.port.clone_path)
        with self.assertRaises(Exception):
            self.commit(request_id="1" * 32, ref="audit-b")
        self.assertEqual(logical_clone_snapshot(self.port.clone_path), committed)

    def test_forged_grant_is_rejected_before_rust_subprocess(self):
        from unittest.mock import patch

        before = logical_clone_snapshot(self.port.clone_path)
        with patch.object(self.port.adapter, "_run", wraps=self.port.adapter._run) as run:
            with self.assertRaises(Exception):
                self.port.batch_request(
                    self.lease, "", reference_batch("cursor-1"),
                    request_id="2" * 32,
                    reference_submission=self.submission(grant="forged-grant"),
                    reference_admissions=self.registry)
            run.assert_not_called()
        self.assertEqual(logical_clone_snapshot(self.port.clone_path), before)

    def test_post_provider_write_constraint_failure_rolls_back_full_clone(self):
        self.set_failure_mode("provider_commit_after_write", True)
        before = logical_clone_snapshot(self.port.clone_path)
        try:
            with self.assertRaises(RustFixtureError):
                self.commit(request_id="9" * 32)
            self.assertEqual(logical_clone_snapshot(self.port.clone_path), before)
        finally:
            self.set_failure_mode("provider_commit_after_write", False)

    def test_audit_write_trigger_is_rejected_before_any_clone_change(self):
        with closing(sqlite3.connect(self.port.clone_path)) as db:
            db.execute("CREATE TRIGGER audit_cross_source AFTER INSERT ON audit_fixture "
                       "BEGIN UPDATE plugin_source_cursor_v1 SET dirty_seq=dirty_seq+1 "
                       "WHERE source_id='other-source'; END")
            db.commit()
        before = logical_clone_snapshot(self.port.clone_path)
        with self.assertRaises(RustFixtureError):
            self.commit()
        self.assertEqual(logical_clone_snapshot(self.port.clone_path), before)

    def test_audit_history_rejects_noncanonical_receipt_digest(self):
        with closing(sqlite3.connect(self.port.clone_path)) as db:
            for provider_id in ("audit-fixture", "pi-command", "pi-content"):
                db.execute("INSERT INTO plugin_reference_owner_v1(source_id,provider_id) "
                           "VALUES(?,?)", (SOURCE_ID, provider_id))
            db.execute("INSERT INTO audit_fixture(source_id,cursor,digest) VALUES(?,?,?)",
                       (SOURCE_ID, "", "f" * 64))
            db.commit()
        before = logical_clone_snapshot(self.port.clone_path)
        with self.assertRaises(RustFixtureError):
            self.commit()
        self.assertEqual(logical_clone_snapshot(self.port.clone_path), before)

    def test_unknown_owner_and_private_pi_content_history_fail_closed(self):
        with closing(sqlite3.connect(self.port.clone_path)) as db:
            db.execute("INSERT INTO plugin_reference_owner_v1(source_id,provider_id) "
                       "VALUES(?,?)", (SOURCE_ID, "untrusted-provider"))
            db.commit()
        before_unknown = logical_clone_snapshot(self.port.clone_path)
        with self.assertRaises(RustFixtureError):
            self.commit()
        self.assertEqual(logical_clone_snapshot(self.port.clone_path), before_unknown)

        with closing(sqlite3.connect(self.port.clone_path)) as db:
            db.execute("DELETE FROM plugin_reference_owner_v1 WHERE source_id=?", (SOURCE_ID,))
            for provider_id in ("audit-fixture", "pi-command", "pi-content"):
                db.execute("INSERT INTO plugin_reference_owner_v1 VALUES(?,?)",
                           (SOURCE_ID, provider_id))
            db.execute("INSERT INTO audit_fixture VALUES(?,?,?)",
                       (SOURCE_ID, "", "a" * 64))
            db.execute("INSERT INTO plugin_content_v1 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                       (SOURCE_ID, "b" * 64, 0, "message", "private-session", "", "",
                        0, 1, "c" * 64, 1, 1, "private text", 1, 1, 0))
            db.commit()
        before_private = logical_clone_snapshot(self.port.clone_path)
        with self.assertRaises(RustFixtureError):
            self.commit()
        self.assertEqual(logical_clone_snapshot(self.port.clone_path), before_private)

    def test_direct_revoke_erases_audit_state_and_rolls_back_late_failure(self):
        self.commit(request_id="e" * 32)
        with closing(sqlite3.connect(self.port.clone_path)) as db:
            row = db.execute(
                "SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 WHERE source_id=?",
                (SOURCE_ID,)).fetchone()
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM audit_fixture WHERE source_id=?", (SOURCE_ID,)
            ).fetchone()[0], 1)

        with closing(sqlite3.connect(self.port.clone_path)) as db:
            db.execute("CREATE TABLE storage_source_revoke_v2("
                       "request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,"
                       "request_digest TEXT NOT NULL,source_id TEXT NOT NULL,cursor TEXT NOT NULL,"
                       "dirty_seq INTEGER NOT NULL,lease_epoch INTEGER NOT NULL,"
                       "policy_epoch INTEGER NOT NULL,capture_enabled INTEGER NOT NULL "
                       "CHECK(capture_enabled=1))")
            db.commit()
        before_failure = logical_clone_snapshot(self.port.clone_path)
        with self.assertRaises(RustFixtureError):
            self.port.revoke_source(
                self.lease, cursor=row[0], expected_dirty_seq=row[1],
                request_id="f" * 32)
        self.assertEqual(logical_clone_snapshot(self.port.clone_path), before_failure)

        with closing(sqlite3.connect(self.port.clone_path)) as db:
            db.execute("DROP TABLE storage_source_revoke_v2")
            db.commit()
        result = self.port.revoke_source(
            self.lease, cursor=row[0], expected_dirty_seq=row[1],
            request_id="f" * 32)
        self.assertEqual(result["disposition"], "committed")
        revoked = logical_clone_snapshot(self.port.clone_path)
        with closing(sqlite3.connect(self.port.clone_path)) as db:
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM audit_fixture WHERE source_id=?", (SOURCE_ID,)
            ).fetchone()[0], 0)
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM plugin_reference_owner_v1 WHERE source_id=?",
                (SOURCE_ID,)).fetchone()[0], 0)
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id=?",
                (SOURCE_ID,)).fetchone()[0], 0)
        self.assertTrue(self.port.capture_revoked(SOURCE_ID))
        with self.assertRaises(RustFixtureError) as raised:
            self.commit(cursor="cursor-2", expected="cursor-1", ref="audit-b")
        self.assertEqual(self.port.classify_lease_failure(raised.exception, SOURCE_ID),
                         "capture_revoked")
        self.assertEqual(logical_clone_snapshot(self.port.clone_path), revoked)

    def test_idle_preserves_audit_provider_state_and_rejects_selected_submission(self):
        empty = PollBatch(SOURCE_ID, "", ())
        initial_idle = self.port.adapter.commit_batch(
            self.lease, "", empty, request_id="a" * 32,
            reference_admissions=self.registry)
        self.assertEqual(initial_idle["disposition"], "idle")
        with closing(sqlite3.connect(self.port.clone_path)) as db:
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM plugin_reference_owner_v1 WHERE source_id=?",
                (SOURCE_ID,)).fetchone()[0], 0)
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM audit_fixture WHERE source_id=?",
                (SOURCE_ID,)).fetchone()[0], 0)
            self.assertEqual(db.execute(
                "SELECT disposition FROM storage_admission_request_v2 WHERE request_id=?",
                ("a" * 32,)).fetchone()[0], "idle")
        initial_idle_snapshot = logical_clone_snapshot(self.port.clone_path)
        initial_idle_replay = self.port.adapter.commit_batch(
            self.lease, "", empty, request_id="a" * 32,
            reference_admissions=self.registry)
        self.assertEqual(initial_idle_replay["disposition"], "replay")
        self.assertEqual(logical_clone_snapshot(self.port.clone_path), initial_idle_snapshot)

        self.commit(request_id="b" * 32)
        before_idle = self.durable_snapshot(SOURCE_ID)
        empty_at_cursor = PollBatch(SOURCE_ID, "cursor-1", ())
        no_selection_idle = self.port.adapter.commit_batch(
            self.lease, "cursor-1", empty_at_cursor, request_id="c" * 32,
            reference_admissions=self.registry)
        self.assertEqual(no_selection_idle["disposition"], "idle")
        after_idle = self.durable_snapshot(SOURCE_ID)
        self.assertEqual(tuple(value for index, value in enumerate(after_idle) if index != 7),
                         tuple(value for index, value in enumerate(before_idle) if index != 7))
        added_receipts = set(after_idle[7]) - set(before_idle[7])
        self.assertEqual(len(added_receipts), 1)
        added_receipt = next(iter(added_receipts))
        self.assertEqual(added_receipt[0], "c" * 32)
        self.assertEqual(added_receipt[3], SOURCE_ID)
        self.assertEqual(added_receipt[4], "cursor-1")
        self.assertEqual(added_receipt[7], "idle")
        self.assertEqual(set(after_idle[7]), set(before_idle[7]) | {added_receipt})
        with closing(sqlite3.connect(self.port.clone_path)) as db:
            self.assertEqual(db.execute(
                "SELECT disposition FROM storage_admission_request_v2 WHERE request_id=?",
                ("c" * 32,)).fetchone()[0], "idle")
        after_idle_snapshot = logical_clone_snapshot(self.port.clone_path)
        no_selection_replay = self.port.adapter.commit_batch(
            self.lease, "cursor-1", empty_at_cursor, request_id="c" * 32,
            reference_admissions=self.registry)
        self.assertEqual(no_selection_replay["disposition"], "replay")
        self.assertEqual(logical_clone_snapshot(self.port.clone_path), after_idle_snapshot)

        before_rejected = logical_clone_snapshot(self.port.clone_path)
        with self.assertRaises(RustFixtureError):
            self.port.adapter.commit_batch(
                self.lease, "cursor-1", empty_at_cursor, request_id="d" * 32,
                reference_submission=self.submission(),
                reference_admissions=self.registry)
        self.assertEqual(logical_clone_snapshot(self.port.clone_path), before_rejected)

    def test_plugin_session_acks_only_after_audit_receipt_and_cursor_are_durable(self):
        source_id = "audit-session-source"
        clone_path = self.port.clone_path.parent / f"{source_id}.db"
        with closing(sqlite3.connect(self.canonical_path)) as canonical, \
                closing(sqlite3.connect(clone_path)) as clone:
            canonical.backup(clone)
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                       ("storage_activation_clone_v1", "enabled"))
            db.commit()
        clone_adapter = RustMarkedCloneMetadataAdapter(
            os.environ["AGENTURM_RUST_METADATA_ADAPTER_BIN"], clone_path)

        plugin_manifest = PluginManifest(
            id="fixture.audit-session", version="1.0.0", protocol_min=1,
            protocol_max=1, entrypoint=("/bin/true",), artifact_sha256="a" * 64,
            provides=("source-adapter/1", "fixture-audit-ref/1"),
            requires=("evidence-ingest/1",), depends_on=(), grants=(),
            origin=Path("/tmp/audit-session-manifest.toml"))
        source = PluginSource(
            plugin_manifest, source_id, required_features=("fixture-audit-ref/1",),
            capture_mode="audit-reference-v1")

        class AuditHostGrant:
            @staticmethod
            def fingerprint_for_source(_source):
                return ""

            @staticmethod
            def from_source(_source, lease):
                if lease.source_id != source_id:
                    raise ValueError("audit fixture lease mismatch")
                return "host-owned-audit"

        modes = DEFAULT_CAPTURE_MODES.with_mode(CaptureMode(
            "audit-reference-v1", frozenset({"fixture-audit-ref/1"}),
            reference_feature="fixture-audit-ref/1", reference_family="reference",
            grant_type=AuditHostGrant, reference_provider_id="audit-fixture"))

        def validate_refs(refs, batch):
            if refs != ({"token": "audit-a"},) or batch.source_id != source_id:
                raise ValueError("unsupported audit fixture token")
            return ("audit-a",)

        handlers = DEFAULT_BATCH_HANDLERS.with_reference_handler(
            "fixture-audit-ref/1", validate_refs)
        audit_registry = DEFAULT_REFERENCE_ADMISSIONS.with_provider(self.provider)
        clone_adapter.install_reference_schema(audit_registry)
        assert_clone_database_quiescent(clone_path)
        clone_store = Store(str(clone_adapter.clone_path))
        self.addCleanup(clone_store.close)

        def assert_idle_reader_current():
            readers = clone_store._read_connections.idle
            self.assertEqual(len(readers), 1)
            self.assertFalse(readers[0].in_transaction)
            assert_clone_database_reader_current(clone_path)

        # Keep one idle Python Store reader open across Rust activation,
        # diagnostic read, and commit subprocesses. The projection worker is
        # inert; Python projects afterward. This covers an idle reader only,
        # not active-read concurrency or full production-port conformance.
        assert_idle_reader_current()
        events = []
        batch_requests = []
        raw_run = clone_adapter._run

        def record_batch_request(mode, request):
            assert_idle_reader_current()
            if mode == "--v2-mixed-clone-batch":
                batch_requests.append(request)
            result = raw_run(mode, request)
            assert_idle_reader_current()
            return result

        clone_adapter._run = record_batch_request
        response = {
            "type": "batch", "protocol": 1, "source_id": source_id,
            "cursor": "audit-session-cursor", "records": [{
                "event_id": "audit-session-event", "revision": 0,
                "kind": "lifecycle", "time_ns": "9007199254740993",
                "payload": {"state": "completed"},
                "identity": {"session_id": "audit-session"},
            }],
            "reference_refs": {"schema_version": 1, "refs": [{"token": "audit-a"}]},
        }

        class SyntheticProcess:
            def __init__(self, manifest, *, source_id, batch_handlers, **_kwargs):
                self.manifest = manifest
                self.source_id = source_id
                self.required_features = ("fixture-audit-ref/1",)
                self.batch_validator = batch_handlers.select(self.required_features)
                self.process = None

            def start(self):
                events.append("worker-start")
                return self

            def poll_commit(self, cursor, commit, *, members=None):
                events.append("poll")
                if cursor != "":
                    raise AssertionError("session should start from empty cursor")
                batch = self.batch_validator(response, source_id)
                if not isinstance(batch, ReferencePollBatch):
                    raise AssertionError("registered reference handler was not used")
                if commit(batch) != batch.cursor:
                    raise AssertionError("session commit returned the wrong cursor")
                if len(batch_requests) != 1:
                    raise AssertionError("session did not dispatch exactly one mixed batch")
                request = batch_requests[0]
                request_digest = hashlib.sha256(json.dumps(
                    request, sort_keys=True, separators=(",", ":"),
                    ensure_ascii=False).encode("utf-8")).hexdigest()
                request_id = request["request_id"]
                operation = request["operation"]
                batch_digest = operation["batch_digest"]
                lease = operation["lease"]
                with closing(sqlite3.connect(clone_adapter.clone_path)) as db:
                    persisted_source = db.execute(
                        "SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 WHERE source_id=?",
                        (source_id,)).fetchone()
                    receipt = db.execute(
                        "SELECT receiver_id,request_digest,source_id,cursor,batch_digest,dirty_seq,disposition "
                        "FROM storage_admission_request_v2 WHERE request_id=?",
                        (request_id,)).fetchone()
                    audit_row = db.execute(
                        "SELECT cursor,digest FROM audit_fixture WHERE source_id=?",
                        (source_id,)).fetchone()
                    owners = tuple(row[0] for row in db.execute(
                        "SELECT provider_id FROM plugin_reference_owner_v1 WHERE source_id=? "
                        "ORDER BY provider_id", (source_id,)))
                assert_idle_reader_current()
                expected_receipt = (lease["receiver_id"], request_digest, source_id,
                                    batch.cursor, batch_digest, 1, "committed")
                expected_audit = (batch.cursor,
                                  hashlib.sha256(b"audit-a").hexdigest())
                if (persisted_source != (batch.cursor, 1)
                        or receipt != expected_receipt
                        or audit_row != expected_audit
                        or owners != ("audit-fixture", "pi-command", "pi-content")):
                    raise AssertionError("ACK was attempted before Rust state was durable")
                events.append("ack")
                return batch

            def close(self, *, force=False):
                events.append("worker-close")
                self.process = None

        class InertProjectionThread:
            def __init__(self, *_args, **_kwargs):
                pass

            def start(self):
                pass

            def join(self, timeout=None):
                pass

        port = RustMarkedCloneSessionPortSubset(clone_adapter)
        with patch("observatory.plugin_runtime.PluginProcess", SyntheticProcess), \
                patch("observatory.plugin_runtime.threading.Thread", InertProjectionThread):
            session = PluginSession(
                clone_store, plugin_manifest, source_id=source_id,
                required_features=source.runtime_features,
                capture_source=source, capture_modes=modes,
                reference_admissions=audit_registry, batch_handlers=handlers,
                admission_port=port)
            session.start()
            try:
                assert_idle_reader_current()
                accepted = session.poll_once()
                self.assertEqual(accepted.cursor, "audit-session-cursor")
                self.assertLess(events.index("worker-start"), events.index("ack"))
                self.assertEqual(events.count("ack"), 1)
            finally:
                session.close()
        projected = project_source(clone_store, source_id)
        with clone_store._connect() as db:
            projection = db.execute(
                "SELECT state FROM plugin_session_projection_v1 WHERE source_id=?",
                (source_id,)).fetchone()
            evidence = db.execute(
                "SELECT kind,payload_json,policy_epoch FROM plugin_evidence_v1 WHERE source_id=?",
                (source_id,)).fetchall()
            self.assertIsNotNone(projection, (projected, evidence))
            self.assertEqual(projection[0], "completed")
