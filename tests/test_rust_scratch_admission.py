"""Feature-off Rust scratch transaction against disposable Python Store fixtures."""
import json
import hashlib
import os
import select
import sqlite3
import stat
import subprocess
import sys
import tempfile
import unittest
from contextlib import closing, contextmanager, nullcontext
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import (_validated, activate_source, commit_batch,
                                          last_source_diagnostic, set_source_capture)
from observatory.plugin_wire import PollBatch, ProtocolError


SOURCE = "scratch-source"
PLUGIN = "fixture.plugin"
GENERATION = "a" * 64


def fact(event="first", state="started"):
    return {"event_id": event, "revision": 0, "kind": "lifecycle",
            "time_ns": "9007199254740993", "payload": {"state": state},
            "identity": {"session_id": "native-one"}}


@unittest.skipUnless(sys.platform.startswith("linux"), "scratch writer lease is Linux-only")
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

    def request(self, batch, expected="", *, lease=None, path=None, source_id=SOURCE):
        digest, rows, checked = _validated(batch, source_id)
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

    def v2_request(self, batch, expected="", *, request_id="1" * 32,
                   lease=None, source_id=SOURCE):
        prepared = self.request(batch, expected, lease=lease, source_id=source_id)
        return {"version": 2, "request_id": request_id,
                "receiver_id": prepared["lease"]["receiver_id"],
                "operation": {"op": "admit_lifecycle", "lease": prepared["lease"],
                              "expected_cursor": prepared["expected_cursor"],
                              "cursor": prepared["cursor"],
                              "batch_digest": prepared["batch_digest"],
                              "records": prepared["records"]}}

    def v2_revoke_request(self, *, cursor, dirty_seq, request_id="3" * 32, lease=None):
        owner = lease or self.rust_lease
        lease = {"receiver_id": owner.receiver_id,
                 "plugin_id": owner.plugin_id,
                 "source_id": owner.source_id,
                 "generation": owner.generation,
                 "epoch": owner.epoch,
                 "policy_epoch": owner.policy_epoch}
        return {"version": 2, "request_id": request_id,
                "receiver_id": self.rust_lease.receiver_id,
                "operation": {"op": "revoke_source", "lease": lease,
                              "cursor": cursor, "expected_dirty_seq": dirty_seq}}

    def v2_activate_request(self, source_id, *, request_id, plugin_id=PLUGIN,
                            generation=GENERATION):
        return {"version": 2, "request_id": request_id,
                "receiver_id": self.rust_lease.receiver_id,
                "operation": {"op": "activate_source", "plugin_id": plugin_id,
                              "source_id": source_id, "generation": generation}}

    @staticmethod
    def v2_capture_request(lease, *, enabled, expected_enabled, cursor, dirty_seq,
                           request_id):
        lease_data = {"receiver_id": lease.receiver_id, "plugin_id": lease.plugin_id,
                      "source_id": lease.source_id, "generation": lease.generation,
                      "epoch": lease.epoch, "policy_epoch": lease.policy_epoch}
        return {"version": 2, "request_id": request_id,
                "receiver_id": lease.receiver_id,
                "operation": {"op": "set_source_capture", "lease": lease_data,
                              "enabled": enabled, "expected_cursor": cursor,
                              "expected_dirty_seq": dirty_seq,
                              "expected_capture_enabled": expected_enabled}}

    @staticmethod
    def clone_capture_request(lease, *, enabled, expected_enabled, cursor, dirty_seq,
                              request_id):
        return {"version": 2, "request_id": request_id,
                "receiver_id": lease["receiver_id"],
                "operation": {"op": "set_source_capture", "lease": dict(lease),
                              "enabled": enabled, "expected_cursor": cursor,
                              "expected_dirty_seq": dirty_seq,
                              "expected_capture_enabled": expected_enabled}}

    @staticmethod
    def clone_revoke_request(lease, *, cursor, dirty_seq, request_id):
        return {"version": 2, "request_id": request_id,
                "receiver_id": lease["receiver_id"],
                "operation": {"op": "revoke_source", "lease": dict(lease),
                              "cursor": cursor, "expected_dirty_seq": dirty_seq}}

    def apply_v2(self, request, *, path=None, success=True):
        raw = request if isinstance(request, str) else json.dumps(request)
        result = subprocess.run([self.binary, "--v2-scratch-path",
                                 str(path or self.rust_store.path)],
                                input=raw, text=True, capture_output=True, timeout=10)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("scratch admission rejected", result.stderr)
        return result.stderr

    def apply_activation_clone_v2(self, request, *, path, success=True):
        raw = request if isinstance(request, str) else json.dumps(request)
        result = subprocess.run([self.binary, "--v2-activation-clone-path", str(path)],
                                input=raw, text=True, capture_output=True, timeout=10)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("activation-clone admission rejected", result.stderr)
        return result.stderr

    def apply_mixed_clone_batch(self, request, *, path, success=True):
        raw = request if isinstance(request, str) else json.dumps(request)
        result = subprocess.run([self.binary, "--v2-mixed-clone-batch", str(path)],
                                input=raw, text=True, capture_output=True, timeout=10)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("activation-clone admission rejected", result.stderr)
        return result.stderr

    def apply_clone_policy(self, request, *, path, success=True):
        raw = request if isinstance(request, str) else json.dumps(request)
        result = subprocess.run([self.binary, "--v2-clone-policy-path", str(path)],
                                input=raw, text=True, capture_output=True, timeout=10)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("activation-clone policy rejected", result.stderr)
        return result.stderr

    def apply_clone_diagnostic_read(self, request, *, path, success=True):
        raw = request if isinstance(request, str) else json.dumps(request)
        result = subprocess.run([self.binary, "--v2-clone-diagnostic-path", str(path)],
                                input=raw, text=True, capture_output=True, timeout=10)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("activation-clone diagnostic read rejected", result.stderr)
        return result.stderr

    def apply_clone_member_cursor_read(self, request, *, path, success=True):
        raw = request if isinstance(request, str) else json.dumps(request)
        result = subprocess.run(
            [self.binary, "--v2-clone-member-cursor-path", str(path)],
            input=raw, text=True, capture_output=True, timeout=10)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("member cursor read rejected", result.stderr)
        return result.stderr

    @staticmethod
    def clone_diagnostic_read_request(lease, request_id):
        return {"version": 2, "request_id": request_id,
                "receiver_id": lease["receiver_id"],
                "operation": {"op": "read_last_source_diagnostic",
                              "lease": dict(lease)}}

    @staticmethod
    def clone_member_cursor_read_request(lease, request_id):
        return {"version": 2, "request_id": request_id,
                "receiver_id": lease["receiver_id"],
                "operation": {"op": "read_source_member_cursors",
                              "lease": dict(lease)}}

    @staticmethod
    def mixed_batch_request(batch, lease, request_id, expected_cursor):
        from observatory.plugin_admission import _validated

        digest, rows, _ = _validated(batch, batch.source_id)
        operation_lease = {key: lease[key] for key in
                           ("receiver_id", "plugin_id", "source_id", "generation",
                            "epoch", "policy_epoch")}
        records = [{
            "event_id_hash": row[1], "revision": row[2], "kind": row[3],
            "time_ns": row[4], "payload_json": row[5], "record_digest": row[6],
            "native_session_id": row[7], "native_worker_id": row[8],
            "native_turn_id": row[9], "native_operation_id": row[10],
        } for row in rows]
        operation = {"op": "admit_batch", "lease": operation_lease,
                     "expected_cursor": expected_cursor, "cursor": batch.cursor,
                     "batch_digest": digest, "records": records}
        if batch.member_id:
            operation["member_update"] = {"member_id": batch.member_id,
                                           "cursor": batch.member_cursor}
        return {"version": 2, "request_id": request_id,
                "receiver_id": lease["receiver_id"],
                "operation": operation}

    @staticmethod
    def batch_source_snapshot(store, source_id):
        def read(db):
            source = tuple(db.execute(
                "SELECT receiver_id,plugin_id,generation,cursor,prior_cursor,last_batch_digest,"
                "dirty_seq,lease_epoch,policy_epoch,capture_enabled,content_fingerprint "
                "FROM plugin_source_cursor_v1 WHERE source_id=?", (source_id,)).fetchone())
            evidence = tuple(tuple(row) for row in db.execute(
                "SELECT event_id_hash,revision,kind,time_ns,payload_json,record_digest,"
                "native_session_id,native_worker_id,native_turn_id,native_operation_id,"
                "member_id,policy_epoch FROM plugin_evidence_v1 WHERE source_id=? "
                "ORDER BY event_id_hash", (source_id,)))
            dirty = tuple(tuple(row) for row in db.execute(
                "SELECT source_id,dirty_seq FROM plugin_evidence_dirty_v1 WHERE source_id=?",
                (source_id,)))
            projection = tuple(tuple(row) for row in db.execute(
                "SELECT native_session_id,session_key,state,reason_code,state_time_ns,"
                "policy_epoch,source_dirty_seq,usage_json,"
                "parent_native_session_id,relationship_kind,relationship_state "
                "FROM plugin_session_projection_v1 WHERE source_id=? ORDER BY native_session_id",
                (source_id,)))
            activity = tuple(tuple(row) for row in db.execute(
                "SELECT event_key,stamp,trace_id,session_id,agent_id,service,domain,action,"
                "result,turn_id,payload FROM activity_index_v1 "
                "WHERE kind='plugin' AND owner_key=? ORDER BY event_key", (source_id,)))
            projected = tuple(tuple(row) for row in db.execute(
                "SELECT projected_seq,policy_epoch FROM plugin_projection_cursor_v1 "
                "WHERE source_id=?", (source_id,)))
            member_cursors = tuple(tuple(row) for row in db.execute(
                "SELECT member_id,cursor FROM plugin_source_member_cursor_v1 "
                "WHERE source_id=? ORDER BY member_id", (source_id,)))
            session_members = tuple(tuple(row) for row in db.execute(
                "SELECT native_session_id,member_id FROM plugin_session_member_v1 "
                "WHERE source_id=? ORDER BY native_session_id", (source_id,)))
            source_gaps = tuple(tuple(row) for row in db.execute(
                "SELECT native_session_id,last_record_ns FROM plugin_source_gap_invalidated_v1 "
                "WHERE source_id=? ORDER BY native_session_id", (source_id,)))
            return (source, evidence, dirty, projection, activity, projected,
                    member_cursors, session_members, source_gaps)

        if hasattr(store, "_connect"):
            with store._connect() as db:
                return read(db)
        with closing(sqlite3.connect(str(store))) as db:
            db.row_factory = sqlite3.Row
            return read(db)

    @staticmethod
    def batch_binding_snapshot(path, source_id):
        with closing(sqlite3.connect(str(path))) as db:
            exists = db.execute(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND "
                "name='storage_admission_batch_binding_v1'").fetchone()
            if exists is None:
                return None
            return tuple(db.execute(
                "SELECT generation,policy_epoch,expected_cursor,cursor,batch_digest,"
                "prepared_digest,dirty_seq FROM storage_admission_batch_binding_v1 "
                "WHERE source_id=?", (source_id,)).fetchone() or ())

    @staticmethod
    def clone_policy_state_snapshot(path, source_id):
        with closing(sqlite3.connect(str(path))) as db:
            tables = (
                ("plugin_source_cursor_v1", "source_id=?", (source_id,)),
                ("plugin_evidence_v1", "source_id=?", (source_id,)),
                ("plugin_evidence_dirty_v1", "source_id=?", (source_id,)),
                ("plugin_session_projection_v1", "source_id=?", (source_id,)),
                ("plugin_projection_cursor_v1", "source_id=?", (source_id,)),
                ("activity_index_v1", "kind='plugin' AND owner_key=?", (source_id,)),
                ("investigation_dirty_v1", "kind='plugin' AND owner_key=?", (source_id,)),
                ("plugin_content_v1", "source_id=?", (source_id,)),
                ("plugin_content_batch_v1", "source_id=?", (source_id,)),
                ("plugin_content_status_v1", "source_id=?", (source_id,)),
                ("plugin_reference_owner_v1", "source_id=?", (source_id,)),
                ("plugin_source_member_cursor_v1", "source_id=?", (source_id,)),
                ("plugin_session_member_v1", "source_id=?", (source_id,)),
                ("plugin_source_gap_invalidated_v1", "source_id=?", (source_id,)),
                ("plugin_retention_v1", "source_id=?", (source_id,)),
                ("plugin_retention_invalidated_v1", "source_id=?", (source_id,)),
                ("plugin_command_content_v1", "source_id=?", (source_id,)),
                ("plugin_command_batch_v1", "source_id=?", (source_id,)),
            )
            result = []
            for table, where, params in tables:
                exists = db.execute(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", (table,)
                ).fetchone()
                if exists is None:
                    result.append(None)
                    continue
                result.append(tuple(tuple(row) for row in db.execute(
                    f"SELECT * FROM {table} WHERE {where} ORDER BY rowid", params)))
            return tuple(result)

    @staticmethod
    def native_lifecycle_snapshot(path, source_id):
        with closing(sqlite3.connect(str(path))) as db:
            activity = tuple(tuple(row) for row in db.execute(
                "SELECT event_key,payload FROM activity_index_v1 "
                "WHERE kind='plugin' AND owner_key=? ORDER BY event_key", (source_id,)))
            native_index = tuple(tuple(row) for row in db.execute(
                "SELECT * FROM native_lifecycle_index_v1 ORDER BY event_key"))
            native_dirty = tuple(tuple(row) for row in db.execute(
                "SELECT * FROM native_lifecycle_dirty_v1 ORDER BY kind,owner_key"))
            return activity, native_index, native_dirty

    @classmethod
    def comparable_clone_policy_state(cls, path, source_id):
        snapshot = cls.batch_source_snapshot(path, source_id)
        refs = cls.clone_policy_state_snapshot(path, source_id)
        source = snapshot[0]
        source_owned_refs = refs[7:]
        # Python records installed provider ownership even for an empty
        # reference batch. Rust clone mode rejects provider-owned sources;
        # compare actual content/receipts and supported history separately.
        comparable_refs = source_owned_refs[:3] + source_owned_refs[4:]
        return (source[:7] + source[8:], *snapshot[1:], comparable_refs)

    @staticmethod
    def admission_receipts_snapshot(path, source_id):
        with closing(sqlite3.connect(str(path))) as db:
            exists = db.execute(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND "
                "name='storage_admission_request_v2'").fetchone()
            if exists is None:
                return ()
            return tuple(tuple(row) for row in db.execute(
                "SELECT request_id,receiver_id,request_digest,cursor,batch_digest,"
                "dirty_seq,disposition FROM storage_admission_request_v2 "
                "WHERE source_id=? ORDER BY request_id", (source_id,)))

    @staticmethod
    def policy_receipts_snapshot(path, table, source_id):
        with closing(sqlite3.connect(str(path))) as db:
            exists = db.execute(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", (table,)
            ).fetchone()
            if exists is None:
                return None
            return tuple(tuple(row) for row in db.execute(
                f"SELECT * FROM {table} WHERE source_id=? ORDER BY request_id",
                (source_id,)))

    def test_mixed_batch_is_atomic_and_matches_python_on_detached_clone(self):
        from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
        from observatory.plugin_projection import project_source

        source_id = "mixed-batch-source"
        generation = "d" * 64
        canonical_path = self.root / "canonical.db"
        clone_path = self.root / "telemetry-clone.db"
        oracle_path = self.root / "python-oracle.db"

        canonical = Store(str(canonical_path))
        receiver_id = canonical.receiver_id
        activate_source(canonical, plugin_id=PLUGIN, source_id="clone-seed",
                        generation=GENERATION)
        canonical.close()
        with closing(sqlite3.connect(canonical_path)) as original:
            for target in (clone_path, oracle_path):
                with closing(sqlite3.connect(target)) as copied:
                    original.backup(copied)
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                       ("storage_activation_clone_v1", "enabled"))
            db.commit()

        def canonical_file_state():
            paths = (canonical_path, Path(f"{canonical_path}-wal"),
                     Path(f"{canonical_path}-shm"))
            return tuple((path.stat().st_ino, path.stat().st_mtime_ns,
                          hashlib.sha256(path.read_bytes()).hexdigest()) if path.exists() else None
                         for path in paths)

        canonical_before = canonical_file_state()
        activation = {"version": 2, "request_id": "1" * 32,
                      "receiver_id": receiver_id,
                      "operation": {"op": "activate_source", "plugin_id": PLUGIN,
                                    "source_id": source_id, "generation": generation}}
        rust_activation = self.apply_activation_clone_v2(
            activation, path=clone_path)["result"]
        rust_lease = rust_activation["lease"]
        oracle = Store(str(oracle_path))
        self.addCleanup(oracle.close)
        python_lease = activate_source(oracle, plugin_id=PLUGIN, source_id=source_id,
                                       generation=generation)

        def mirror_projection_to_clone():
            project_source(oracle, source_id)
            tables = ("plugin_session_projection_v1", "activity_index_v1",
                      "plugin_projection_cursor_v1")
            rows = {}
            with oracle._connect() as source_db:
                rows[tables[0]] = tuple(tuple(row) for row in source_db.execute(
                    "SELECT * FROM plugin_session_projection_v1 WHERE source_id=?",
                    (source_id,)))
                rows[tables[1]] = tuple(tuple(row) for row in source_db.execute(
                    "SELECT * FROM activity_index_v1 WHERE kind='plugin' AND owner_key=?",
                    (source_id,)))
                rows[tables[2]] = tuple(tuple(row) for row in source_db.execute(
                    "SELECT * FROM plugin_projection_cursor_v1 WHERE source_id=?",
                    (source_id,)))
            with closing(sqlite3.connect(clone_path)) as clone_db:
                clone_db.execute("BEGIN IMMEDIATE")
                clone_db.execute("DELETE FROM plugin_session_projection_v1 WHERE source_id=?",
                                 (source_id,))
                clone_db.execute("DELETE FROM activity_index_v1 "
                                  "WHERE kind='plugin' AND owner_key=?", (source_id,))
                clone_db.execute("DELETE FROM investigation_dirty_v1 "
                                  "WHERE kind='plugin' AND owner_key=?", (source_id,))
                clone_db.execute("DELETE FROM plugin_projection_cursor_v1 WHERE source_id=?",
                                 (source_id,))
                clone_db.executemany(
                    "INSERT INTO plugin_session_projection_v1 VALUES(" + "?," * 12 + "?)",
                    rows[tables[0]])
                clone_db.executemany(
                    "INSERT INTO activity_index_v1 VALUES(" + "?," * 12 + "?)",
                    rows[tables[1]])
                clone_db.executemany(
                    "INSERT INTO plugin_projection_cursor_v1 VALUES(?,?,?)", rows[tables[2]])
                clone_db.execute("DELETE FROM investigation_dirty_v1 "
                                 "WHERE kind='plugin' AND owner_key=?", (source_id,))
                clone_db.commit()

        batch = PollBatch(source_id, "mixed-one", (
            {"event_id": "mixed-lifecycle", "revision": 0, "kind": "lifecycle",
             "time_ns": "100", "payload": {"state": "completed"},
             "identity": {"session_id": "mixed-session"}},
            {"event_id": "mixed-message", "revision": 0, "kind": "message",
             "time_ns": "110", "payload": {"role": "assistant"},
             "identity": {"session_id": "fact-only-session"}},
        ))
        mixed = self.mixed_batch_request(batch, rust_lease, "2" * 32, "")
        rust_reply = self.apply_mixed_clone_batch(mixed, path=clone_path)["result"]
        self.assertEqual(rust_reply["kind"], "batch_admission")
        self.assertEqual(rust_reply["disposition"], "committed")
        self.assertEqual(rust_reply["cursor"], "mixed-one")
        with closing(sqlite3.connect(clone_path)) as db:
            admission_receipt = tuple(db.execute(
                "SELECT request_id,receiver_id,request_digest,source_id,cursor,batch_digest,"
                "dirty_seq,disposition FROM storage_admission_request_v2 WHERE request_id=?",
                (mixed["request_id"],)).fetchone())
        self.assertEqual(admission_receipt[:2], (mixed["request_id"], receiver_id))
        self.assertEqual(admission_receipt[3:],
                         (source_id, "mixed-one", mixed["operation"]["batch_digest"],
                          1, "committed"))
        expected_request_digest = hashlib.sha256(json.dumps(
            mixed, sort_keys=True, separators=(",", ":"), ensure_ascii=False
        ).encode()).hexdigest()
        self.assertEqual(admission_receipt[2], expected_request_digest)
        prepared_rows = [[row[key] for key in (
            "event_id_hash", "revision", "kind", "time_ns", "payload_json",
            "record_digest", "native_session_id", "native_worker_id",
            "native_turn_id", "native_operation_id")] for row in mixed["operation"]["records"]]
        expected_prepared_digest = hashlib.sha256(
            b"agenturm-prepared-rows-v1\0" + json.dumps(
                prepared_rows, separators=(",", ":"), ensure_ascii=False
            ).encode()).hexdigest()
        committed_binding = self.batch_binding_snapshot(clone_path, source_id)
        self.assertEqual(committed_binding,
                         (generation, rust_lease["policy_epoch"], "", "mixed-one",
                          mixed["operation"]["batch_digest"], expected_prepared_digest, 1))
        binding_before_new_id_replay = committed_binding
        same_rows_new_request = json.loads(json.dumps(mixed))
        same_rows_new_request["request_id"] = "7" * 32
        self.assertEqual(self.apply_mixed_clone_batch(
            same_rows_new_request, path=clone_path)["result"]["disposition"], "replay")
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id),
                         binding_before_new_id_replay)

        altered_rows_new_request = json.loads(json.dumps(mixed))
        altered_rows_new_request["request_id"] = "8" * 32
        altered_rows_new_request["operation"]["records"][1]["payload_json"] = '{"role":"user"}'
        altered_rows_new_request["operation"]["records"][1]["record_digest"] = "e" * 64
        before_altered_rows = self.batch_source_snapshot(clone_path, source_id)
        receipts_before_altered_rows = self.admission_receipts_snapshot(clone_path, source_id)
        self.assertIn("conflicting prepared batch replay identity",
                      self.apply_mixed_clone_batch(altered_rows_new_request,
                                                   path=clone_path, success=False))
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id),
                         before_altered_rows)
        self.assertEqual(self.admission_receipts_snapshot(clone_path, source_id),
                         receipts_before_altered_rows)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id),
                         binding_before_new_id_replay)
        with closing(sqlite3.connect(clone_path)) as db:
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
                (altered_rows_new_request["request_id"],)).fetchone()[0], 0)

        tampered_clone_path = self.root / "tampered-binding-clone.db"
        with closing(sqlite3.connect(clone_path)) as original:
            with closing(sqlite3.connect(tampered_clone_path)) as tampered:
                original.backup(tampered)
        with closing(sqlite3.connect(tampered_clone_path)) as tampered:
            tampered.execute(
                "UPDATE storage_admission_batch_binding_v1 SET prepared_digest=? "
                "WHERE source_id=?", ("f" * 64, source_id))
            tampered.commit()
        tampered_before = self.batch_source_snapshot(tampered_clone_path, source_id)
        tampered_binding = self.batch_binding_snapshot(tampered_clone_path, source_id)
        self.assertIn("conflicting prepared batch replay identity",
                      self.apply_mixed_clone_batch(mixed, path=tampered_clone_path,
                                                   success=False))
        self.assertEqual(self.batch_source_snapshot(tampered_clone_path, source_id),
                         tampered_before)
        self.assertEqual(self.batch_binding_snapshot(tampered_clone_path, source_id),
                         tampered_binding)

        unbound_clone_path = self.root / "unbound-binding-clone.db"
        with closing(sqlite3.connect(clone_path)) as original:
            with closing(sqlite3.connect(unbound_clone_path)) as unbound:
                original.backup(unbound)
        with closing(sqlite3.connect(unbound_clone_path)) as unbound:
            unbound.execute("DROP TABLE storage_admission_batch_binding_v1")
            unbound.commit()
        unbound_before = self.batch_source_snapshot(unbound_clone_path, source_id)
        self.assertIn("prepared batch replay binding is missing",
                      self.apply_mixed_clone_batch(mixed, path=unbound_clone_path,
                                                   success=False))
        self.assertEqual(self.batch_source_snapshot(unbound_clone_path, source_id),
                         unbound_before)
        with closing(sqlite3.connect(unbound_clone_path)) as unbound:
            self.assertEqual(unbound.execute(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND "
                "name='storage_admission_batch_binding_v1'").fetchone(), None)
        self.assertEqual(commit_batch(oracle, python_lease, "", batch), "mixed-one")
        with closing(sqlite3.connect(clone_path)) as check_db:
            self.assertEqual(check_db.execute("PRAGMA integrity_check").fetchone()[0], "ok")
        normal_dispatch_before = self.batch_source_snapshot(clone_path, source_id)
        normal_dispatch_error = self.apply_activation_clone_v2(
            mixed, path=clone_path, success=False)
        self.assertIn("mixed admission is unavailable through the storage service path",
                      normal_dispatch_error)
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id),
                         normal_dispatch_before)
        projection_path = self.root / "projection-clone.db"
        with closing(sqlite3.connect(clone_path)) as original:
            with closing(sqlite3.connect(projection_path)) as projected:
                original.backup(projected)
        committed_snapshot = self.batch_source_snapshot(clone_path, source_id)
        projection_store = Store(str(projection_path))
        self.addCleanup(projection_store.close)
        self.assertEqual(project_source(projection_store, source_id), 2)
        self.assertEqual(project_source(oracle, source_id), 2)
        empty_refs_digest = hashlib.sha256(b'{"refs":[]}').hexdigest()
        with projection_store._connect() as db:
            self.assertEqual(tuple(db.execute(
                "SELECT cursor,refs_digest FROM plugin_content_batch_v1 WHERE source_id=?",
                (source_id,)).fetchone()), ("mixed-one", empty_refs_digest))
            self.assertEqual(tuple(db.execute(
                "SELECT cursor,reason,refs_count FROM plugin_content_status_v1 WHERE source_id=?",
                (source_id,)).fetchone()), ("mixed-one", "not_requested", 0))
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM plugin_content_v1 WHERE source_id=?",
                (source_id,)).fetchone()[0], 0)
        rust_projected = self.batch_source_snapshot(projection_store, source_id)
        python_projected = self.batch_source_snapshot(oracle, source_id)
        self.assertEqual(rust_projected[0][:7] + rust_projected[0][8:],
                         python_projected[0][:7] + python_projected[0][8:])
        self.assertEqual(rust_projected[1:], python_projected[1:])
        self.assertEqual({row[0] for row in rust_projected[3]},
                         {"mixed-session", "fact-only-session"})

        # Build marked clone-only fixtures from Python's projection rows, but
        # copy no content/reference-owner rows and never open these clones via
        # Store. This isolates the supported fact projection from provider
        # owner backfill in the Python fixture.
        projected_clone_path = self.root / "projected-rust-clone.db"
        with closing(sqlite3.connect(clone_path)) as original:
            with closing(sqlite3.connect(projected_clone_path)) as projected_clone:
                original.backup(projected_clone)
        with closing(sqlite3.connect(projected_clone_path)) as projected_clone:
            projected_clone.execute("ATTACH DATABASE ? AS projection_src",
                                    (str(projection_path),))
            for table in ("plugin_session_projection_v1", "plugin_session_member_v1",
                          "plugin_source_member_cursor_v1", "plugin_projection_cursor_v1",
                          "plugin_content_batch_v1", "plugin_content_status_v1"):
                projected_clone.execute(
                    f"INSERT OR REPLACE INTO main.{table} SELECT * FROM projection_src.{table} "
                    "WHERE source_id=?", (source_id,))
            projected_clone.execute(
                "INSERT OR REPLACE INTO main.activity_index_v1 "
                "SELECT * FROM projection_src.activity_index_v1 "
                "WHERE kind='plugin' AND owner_key=?", (source_id,))
            projected_clone.execute(
                "INSERT OR IGNORE INTO main.investigation_dirty_v1 "
                "SELECT * FROM projection_src.investigation_dirty_v1 "
                "WHERE kind='plugin' AND owner_key=?", (source_id,))
            projected_clone.execute(
                "INSERT OR IGNORE INTO main.investigation_terms_v1 "
                "SELECT * FROM projection_src.investigation_terms_v1 WHERE event_key IN "
                "(SELECT event_key FROM projection_src.activity_index_v1 "
                "WHERE kind='plugin' AND owner_key=?)", (source_id,))
            projected_clone.commit()
            for table in ("plugin_content_v1", "plugin_reference_owner_v1"):
                self.assertEqual(projected_clone.execute(
                    f"SELECT COUNT(*) FROM {table} WHERE source_id=?", (source_id,)
                    ).fetchone()[0], 0, f"projection clone includes private {table} rows")
            self.assertEqual(tuple(projected_clone.execute(
                "SELECT cursor,refs_digest FROM plugin_content_batch_v1 WHERE source_id=?",
                (source_id,)).fetchone()), ("mixed-one", empty_refs_digest))
            self.assertEqual(tuple(projected_clone.execute(
                "SELECT cursor,reason,refs_count FROM plugin_content_status_v1 WHERE source_id=?",
                (source_id,)).fetchone()), ("mixed-one", "not_requested", 0))
        projected_before_revoke = self.batch_source_snapshot(projected_clone_path, source_id)
        self.assertTrue(projected_before_revoke[3], "projected clone has no session rows")
        self.assertTrue(projected_before_revoke[4], "projected clone has no Activity rows")

        # A subsequent supported batch on a fact-only projected source remains
        # admissible without Python Store backfill state.
        next_batch = PollBatch(source_id, "mixed-two", (
            {"event_id": "after-projection", "revision": 0, "kind": "message",
             "time_ns": "120", "payload": {"role": "assistant"},
             "identity": {"session_id": "fact-only-session"}},
        ))
        next_request = self.mixed_batch_request(next_batch, rust_lease, "b" * 32, "mixed-one")
        next_reply = self.apply_mixed_clone_batch(
            next_request, path=projected_clone_path)["result"]
        self.assertEqual((next_reply["disposition"], next_reply["cursor"],
                          next_reply["dirty_seq"]), ("committed", "mixed-two", 2))
        next_binding = self.batch_binding_snapshot(projected_clone_path, source_id)
        self.assertEqual(next_binding[2:5], ("mixed-one", "mixed-two",
                                              next_request["operation"]["batch_digest"]))
        with closing(sqlite3.connect(projected_clone_path)) as db:
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM storage_admission_batch_binding_v1 WHERE source_id=?",
                (source_id,)).fetchone()[0], 1)
        next_oracle_path = self.root / "next-python-oracle.db"
        with closing(sqlite3.connect(oracle_path)) as original:
            with closing(sqlite3.connect(next_oracle_path)) as copied:
                original.backup(copied)
        next_oracle = Store(str(next_oracle_path))
        self.addCleanup(next_oracle.close)
        self.assertEqual(commit_batch(next_oracle, python_lease, "mixed-one", next_batch),
                         "mixed-two")
        next_rust_state = self.batch_source_snapshot(projected_clone_path, source_id)
        next_python_state = self.batch_source_snapshot(next_oracle, source_id)
        self.assertEqual(next_rust_state[0][:7] + next_rust_state[0][8:],
                         next_python_state[0][:7] + next_python_state[0][8:])
        self.assertEqual(next_rust_state[1:], next_python_state[1:])

        replay = self.apply_mixed_clone_batch(mixed, path=clone_path)["result"]
        self.assertEqual(replay["disposition"], "replay")
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id),
                         committed_snapshot)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id),
                         committed_binding)
        with closing(sqlite3.connect(clone_path)) as db:
            self.assertEqual(tuple(db.execute(
                "SELECT request_id,receiver_id,request_digest,source_id,cursor,batch_digest,"
                "dirty_seq,disposition FROM storage_admission_request_v2 WHERE request_id=?",
                (mixed["request_id"],)).fetchone()), admission_receipt)
        changed_batch = PollBatch(source_id, "mixed-one", (
            {"event_id": "mixed-lifecycle", "revision": 0, "kind": "lifecycle",
             "time_ns": "100", "payload": {"state": "completed"},
             "identity": {"session_id": "mixed-session"}},
            {"event_id": "mixed-message", "revision": 0, "kind": "message",
             "time_ns": "110", "payload": {"role": "user"},
             "identity": {"session_id": "mixed-session"}},
        ))
        conflicting_replay = self.mixed_batch_request(
            changed_batch, rust_lease, "2" * 32, "")
        self.assertIn("conflicting v2 request replay identity",
                      self.apply_mixed_clone_batch(conflicting_replay,
                                                   path=clone_path, success=False))
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id),
                         committed_snapshot)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id),
                         committed_binding)

        invalid_batch = PollBatch(source_id, "mixed-two", (
            {"event_id": "insert-before-conflict", "revision": 0,
             "kind": "lifecycle", "time_ns": "120",
             "payload": {"state": "completed"},
             "identity": {"session_id": "another-session"}},
            {"event_id": "mixed-lifecycle", "revision": 0,
             "kind": "lifecycle", "time_ns": "100",
             "payload": {"state": "failed"},
             "identity": {"session_id": "mixed-session"}},
        ))
        rollback_request = self.mixed_batch_request(
            invalid_batch, rust_lease, "3" * 32, "mixed-one")
        oracle_before_invalid = self.batch_source_snapshot(oracle, source_id)
        with self.assertRaisesRegex(ProtocolError, "conflicting plugin event revision"):
            commit_batch(oracle, python_lease, "mixed-one", invalid_batch)
        self.assertEqual(self.batch_source_snapshot(oracle, source_id),
                         oracle_before_invalid)
        self.assertIn("conflicting or unsupported event revision",
                      self.apply_mixed_clone_batch(rollback_request,
                                                   path=clone_path, success=False))
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id),
                         committed_snapshot)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id),
                         committed_binding)
        with closing(sqlite3.connect(clone_path)) as db:
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
                (rollback_request["request_id"],)).fetchone()[0], 0)

        renewed_activation = json.loads(json.dumps(activation))
        renewed_activation["request_id"] = "4" * 32
        renewed = self.apply_activation_clone_v2(
            renewed_activation, path=clone_path)["result"]["lease"]
        stale_request = self.mixed_batch_request(
            PollBatch(source_id, "mixed-two", ({
                "event_id": "after-fence", "revision": 0, "kind": "lifecycle",
                "time_ns": "130", "payload": {"state": "completed"},
            },)), rust_lease, "5" * 32, "mixed-one")
        after_renewal = self.batch_source_snapshot(clone_path, source_id)
        self.assertIn("stale or unsupported source lease",
                      self.apply_mixed_clone_batch(stale_request,
                                                   path=clone_path, success=False))
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id), after_renewal)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id),
                         committed_binding)
        with closing(sqlite3.connect(clone_path)) as db:
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
                (stale_request["request_id"],)).fetchone()[0], 0)

        revoke = {"version": 2, "request_id": "6" * 32,
                  "receiver_id": receiver_id,
                  "operation": {"op": "revoke_source", "lease": renewed,
                                "cursor": "mixed-one", "expected_dirty_seq": 1}}
        before_revoke = self.batch_source_snapshot(clone_path, source_id)
        normal_revoke_error = self.apply_activation_clone_v2(
            revoke, path=clone_path, success=False)
        self.assertIn("clone revocation is unavailable through the storage service path",
                      normal_revoke_error)
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id), before_revoke)
        # Python Store remains the production writer; its revoke clears the
        # projected oracle. Rust revoke cleanup/fencing is separately covered
        # by revoke_atomically_clears_core_projections_and_fences_old_admission.
        oracle_before_revoke = self.batch_source_snapshot(oracle, source_id)
        self.assertTrue(oracle_before_revoke[3])
        set_source_capture(oracle, source_id=source_id, enabled=False)
        oracle_revoked = self.batch_source_snapshot(oracle, source_id)
        self.assertEqual(oracle_revoked[1], ())
        self.assertEqual(oracle_revoked[3], ())
        self.assertEqual(oracle_revoked[4], ())
        self.assertEqual(canonical_file_state(), canonical_before,
                         "detached-clone admission changed the canonical Store")

    def test_empty_mixed_batch_cursor_policy_on_detached_clone(self):
        from observatory.plugin_admission import activate_source

        source_id = "empty-batch-source"
        generation = "d" * 64
        canonical_path = self.root / "empty-canonical.db"
        clone_path = self.root / "empty-clone.db"
        canonical = Store(str(canonical_path))
        receiver_id = canonical.receiver_id
        activate_source(canonical, plugin_id=PLUGIN, source_id="clone-seed",
                        generation=GENERATION)
        canonical.close()
        with closing(sqlite3.connect(canonical_path)) as original:
            with closing(sqlite3.connect(clone_path)) as clone:
                original.backup(clone)
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                       ("storage_activation_clone_v1", "enabled"))
            db.commit()
        activation = {"version": 2, "request_id": "a" * 32,
                      "receiver_id": receiver_id,
                      "operation": {"op": "activate_source", "plugin_id": PLUGIN,
                                    "source_id": source_id, "generation": generation}}
        lease = self.apply_activation_clone_v2(activation, path=clone_path)["result"]["lease"]

        def receipts():
            with closing(sqlite3.connect(clone_path)) as db:
                exists = db.execute("SELECT 1 FROM sqlite_master WHERE type='table' "
                                    "AND name='storage_admission_request_v2'").fetchone()
                if exists is None:
                    return ()
                return tuple(tuple(row) for row in db.execute(
                    "SELECT request_id,request_digest,cursor,batch_digest,dirty_seq,disposition "
                    "FROM storage_admission_request_v2 WHERE source_id=? ORDER BY request_id",
                    (source_id,)))

        initial = self.batch_source_snapshot(clone_path, source_id)
        idle = self.mixed_batch_request(PollBatch(source_id, "", ()), lease,
                                        "1" * 32, "")
        self.assertIn("mixed admission is unavailable through the storage service path",
                      self.apply_activation_clone_v2(idle, path=clone_path, success=False))
        self.assertEqual(self.apply_mixed_clone_batch(
            idle, path=clone_path)["result"]["disposition"], "idle")
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id), initial)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id), ())

        cursor_only = self.mixed_batch_request(PollBatch(source_id, "cursor-one", ()),
                                               lease, "2" * 32, "")
        committed = self.apply_mixed_clone_batch(cursor_only, path=clone_path)["result"]
        self.assertEqual((committed["disposition"], committed["cursor"],
                          committed["dirty_seq"]), ("committed", "cursor-one", 0))
        after_commit = self.batch_source_snapshot(clone_path, source_id)
        self.assertEqual((after_commit[0][3], after_commit[0][4]), ("cursor-one", ""))
        self.assertEqual(after_commit[1], ())
        committed_binding = self.batch_binding_snapshot(clone_path, source_id)
        self.assertEqual(committed_binding,
                         (generation, lease["policy_epoch"], "", "cursor-one",
                          cursor_only["operation"]["batch_digest"],
                          hashlib.sha256(b"agenturm-prepared-rows-v1\0[]").hexdigest(), 0))
        committed_receipts = receipts()
        self.assertEqual(self.apply_mixed_clone_batch(
            cursor_only, path=clone_path)["result"]["disposition"], "replay")
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id), after_commit)
        self.assertEqual(receipts(), committed_receipts)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id),
                         committed_binding)

        same_cursor_idle = self.mixed_batch_request(
            PollBatch(source_id, "cursor-one", ()), lease, "3" * 32, "cursor-one")
        self.assertEqual(self.apply_mixed_clone_batch(
            same_cursor_idle, path=clone_path)["result"]["disposition"], "idle")
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id), after_commit)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id),
                         committed_binding)
        before_rewind_receipts = receipts()
        rewind = self.mixed_batch_request(PollBatch(source_id, "", ()), lease,
                                          "4" * 32, "cursor-one")
        self.assertIn("source cursor cannot return to empty",
                      self.apply_mixed_clone_batch(rewind, path=clone_path, success=False))
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id), after_commit)
        self.assertEqual(receipts(), before_rewind_receipts)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id),
                         committed_binding)

        stale_cursor = self.mixed_batch_request(
            PollBatch(source_id, "cursor-two", ()), lease, "6" * 32, "stale")
        self.assertIn("stale source cursor",
                      self.apply_mixed_clone_batch(stale_cursor, path=clone_path,
                                                   success=False))
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id), after_commit)
        self.assertEqual(receipts(), before_rewind_receipts)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id),
                         committed_binding)

        renewed = json.loads(json.dumps(activation))
        renewed["request_id"] = "b" * 32
        self.apply_activation_clone_v2(renewed, path=clone_path)
        after_renewal = self.batch_source_snapshot(clone_path, source_id)
        stale = self.mixed_batch_request(PollBatch(source_id, "cursor-two", ()),
                                         lease, "5" * 32, "cursor-one")
        self.assertIn("stale or unsupported source lease",
                      self.apply_mixed_clone_batch(stale, path=clone_path, success=False))
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id), after_renewal)
        self.assertEqual(receipts(), before_rewind_receipts)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id),
                         committed_binding)

    def test_normal_clone_dispatch_rejects_all_metadata_subset_writes(self):
        from tests.support.rust_metadata_adapter import logical_clone_snapshot

        source_id = "normal-subset-dispatch-source"
        generation = "c" * 64
        canonical_path = self.root / "normal-subset-canonical.db"
        clone_path = self.root / "normal-subset-clone.db"
        canonical = Store(str(canonical_path))
        receiver_id = canonical.receiver_id
        activate_source(canonical, plugin_id=PLUGIN, source_id="normal-subset-seed",
                        generation=GENERATION)
        canonical.close()
        with closing(sqlite3.connect(canonical_path)) as original:
            with closing(sqlite3.connect(clone_path)) as clone:
                original.backup(clone)
        with closing(sqlite3.connect(clone_path)) as clone:
            clone.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                          ("storage_activation_clone_v1", "enabled"))
            clone.commit()
        activation = {"version": 2, "request_id": "a" * 32,
                      "receiver_id": receiver_id,
                      "operation": {"op": "activate_source", "plugin_id": PLUGIN,
                                    "source_id": source_id, "generation": generation}}
        lease = self.apply_activation_clone_v2(
            activation, path=clone_path)["result"]["lease"]
        requests = (
            ("AdmitBatch", self.mixed_batch_request(
                PollBatch(source_id, "cursor-1", (fact("normal-dispatch"),)),
                lease, "b" * 32, ""),
             "mixed admission is unavailable through the storage service path"),
            ("SetSourceCapture", self.clone_capture_request(
                lease, enabled=False, expected_enabled=True, cursor="", dirty_seq=0,
                request_id="c" * 32),
             "real-schema clone accepts only activation or admission"),
            ("RevokeSource", self.clone_revoke_request(
                lease, cursor="", dirty_seq=0, request_id="d" * 32),
             "clone revocation is unavailable through the storage service path"),
        )
        for label, request, reason in requests:
            with self.subTest(operation=label):
                full_before = logical_clone_snapshot(clone_path)
                source_before = self.batch_source_snapshot(clone_path, source_id)
                state_before = self.clone_policy_state_snapshot(clone_path, source_id)
                binding_before = self.batch_binding_snapshot(clone_path, source_id)
                admission_before = self.admission_receipts_snapshot(clone_path, source_id)
                capture_before = self.policy_receipts_snapshot(
                    clone_path, "storage_source_capture_set_v2", source_id)
                revoke_before = self.policy_receipts_snapshot(
                    clone_path, "storage_source_revoke_v2", source_id)
                failure = self.apply_activation_clone_v2(
                    request, path=clone_path, success=False)
                self.assertIn(reason, failure)
                self.assertEqual(self.batch_source_snapshot(clone_path, source_id), source_before)
                self.assertEqual(self.clone_policy_state_snapshot(clone_path, source_id), state_before)
                self.assertEqual(self.batch_binding_snapshot(clone_path, source_id), binding_before)
                self.assertEqual(self.admission_receipts_snapshot(clone_path, source_id),
                                 admission_before)
                self.assertEqual(self.policy_receipts_snapshot(
                    clone_path, "storage_source_capture_set_v2", source_id), capture_before)
                self.assertEqual(self.policy_receipts_snapshot(
                    clone_path, "storage_source_revoke_v2", source_id), revoke_before)
                self.assertEqual(logical_clone_snapshot(clone_path), full_before)

    def test_marked_clone_capture_revoke_and_reactivation_match_python(self):
        from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
        from observatory.plugin_projection import project_source

        source_id = "clone-policy-source"
        generation = "e" * 64
        canonical_path = self.root / "policy-canonical.db"
        clone_path = self.root / "policy-clone.db"
        oracle_path = self.root / "policy-oracle.db"
        canonical = Store(str(canonical_path))
        receiver_id = canonical.receiver_id
        activate_source(canonical, plugin_id=PLUGIN, source_id="clone-policy-seed",
                        generation=GENERATION)
        canonical.close()
        with closing(sqlite3.connect(canonical_path)) as original:
            for target in (clone_path, oracle_path):
                with closing(sqlite3.connect(target)) as copied:
                    original.backup(copied)
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                       ("storage_activation_clone_v1", "enabled"))
            db.commit()

        def canonical_file_state():
            paths = (canonical_path, Path(f"{canonical_path}-wal"),
                     Path(f"{canonical_path}-shm"))
            return tuple((path.stat().st_ino, path.stat().st_mtime_ns,
                          hashlib.sha256(path.read_bytes()).hexdigest()) if path.exists() else None
                         for path in paths)

        canonical_before = canonical_file_state()
        activation = {"version": 2, "request_id": "a" * 32,
                      "receiver_id": receiver_id,
                      "operation": {"op": "activate_source", "plugin_id": PLUGIN,
                                    "source_id": source_id, "generation": generation}}
        rust_lease = self.apply_activation_clone_v2(
            activation, path=clone_path)["result"]["lease"]
        oracle = Store(str(oracle_path))
        self.addCleanup(oracle.close)
        python_lease = activate_source(oracle, plugin_id=PLUGIN, source_id=source_id,
                                       generation=generation)

        idle_capture = self.clone_capture_request(
            rust_lease, enabled=True, expected_enabled=True, cursor="", dirty_seq=0,
            request_id="0" * 32)
        idle_state = self.clone_policy_state_snapshot(clone_path, source_id)
        self.assertEqual(self.apply_clone_policy(
            idle_capture, path=clone_path)["result"]["disposition"], "idle")
        idle_receipt = self.policy_receipts_snapshot(
            clone_path, "storage_source_capture_set_v2", source_id)
        idle_replay = self.apply_clone_policy(idle_capture, path=clone_path)
        self.assertEqual(idle_replay["result"]["disposition"], "replay")
        self.assertEqual(self.clone_policy_state_snapshot(clone_path, source_id), idle_state)
        self.assertEqual(self.policy_receipts_snapshot(
            clone_path, "storage_source_capture_set_v2", source_id), idle_receipt)

        def mirror_projection_to_clone():
            project_source(oracle, source_id)
            tables = ("plugin_session_projection_v1", "activity_index_v1",
                      "plugin_projection_cursor_v1")
            rows = {}
            with oracle._connect() as source_db:
                rows[tables[0]] = tuple(tuple(row) for row in source_db.execute(
                    "SELECT * FROM plugin_session_projection_v1 WHERE source_id=?",
                    (source_id,)))
                rows[tables[1]] = tuple(tuple(row) for row in source_db.execute(
                    "SELECT * FROM activity_index_v1 WHERE kind='plugin' AND owner_key=?",
                    (source_id,)))
                rows[tables[2]] = tuple(tuple(row) for row in source_db.execute(
                    "SELECT * FROM plugin_projection_cursor_v1 WHERE source_id=?",
                    (source_id,)))
            with closing(sqlite3.connect(clone_path)) as clone_db:
                clone_db.execute("BEGIN IMMEDIATE")
                clone_db.execute("DELETE FROM plugin_session_projection_v1 WHERE source_id=?",
                                 (source_id,))
                clone_db.execute("DELETE FROM activity_index_v1 "
                                 "WHERE kind='plugin' AND owner_key=?", (source_id,))
                clone_db.execute("DELETE FROM investigation_dirty_v1 "
                                 "WHERE kind='plugin' AND owner_key=?", (source_id,))
                clone_db.execute("DELETE FROM plugin_projection_cursor_v1 WHERE source_id=?",
                                 (source_id,))
                clone_db.executemany(
                    "INSERT INTO plugin_session_projection_v1 VALUES(" + "?," * 12 + "?)",
                    rows[tables[0]])
                clone_db.executemany(
                    "INSERT INTO activity_index_v1 VALUES(" + "?," * 12 + "?)",
                    rows[tables[1]])
                clone_db.executemany(
                    "INSERT INTO plugin_projection_cursor_v1 VALUES(?,?,?)", rows[tables[2]])
                clone_db.execute("DELETE FROM investigation_dirty_v1 "
                                 "WHERE kind='plugin' AND owner_key=?", (source_id,))
                clone_db.commit()

        first_batch = PollBatch(source_id, "policy-one", (
            {"event_id": "policy-life", "revision": 0, "kind": "lifecycle",
             "time_ns": "200", "payload": {"state": "completed"},
             "identity": {"session_id": "policy-session"}},
            {"event_id": "policy-message", "revision": 0, "kind": "message",
             "time_ns": "210", "payload": {"role": "assistant"},
             "identity": {"session_id": "policy-fact-session"}},
        ))
        first = self.mixed_batch_request(first_batch, rust_lease, "b" * 32, "")
        self.assertEqual(self.apply_mixed_clone_batch(first, path=clone_path)["result"]["disposition"],
                         "committed")
        self.assertEqual(self.apply_mixed_clone_batch(first, path=clone_path)["result"]["disposition"],
                         "replay")
        commit_batch(oracle, python_lease, "", first_batch)
        self.assertEqual(commit_batch(oracle, python_lease, "", first_batch), "policy-one")
        mirror_projection_to_clone()
        self.assertEqual(self.comparable_clone_policy_state(clone_path, source_id),
                         self.comparable_clone_policy_state(oracle_path, source_id))
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id)[3], "policy-one")
        with closing(sqlite3.connect(clone_path)) as db:
            clone_owners = tuple(db.execute(
                "SELECT provider_id FROM plugin_reference_owner_v1 WHERE source_id=? ORDER BY provider_id",
                (source_id,)))
        with oracle._connect() as db:
            python_owners = tuple(tuple(row) for row in db.execute(
                "SELECT provider_id FROM plugin_reference_owner_v1 WHERE source_id=? ORDER BY provider_id",
                (source_id,)))
        self.assertEqual(clone_owners, ())
        self.assertEqual(python_owners, (("pi-command",), ("pi-content",)))

        disable = self.clone_capture_request(
            rust_lease, enabled=False, expected_enabled=True, cursor="policy-one",
            dirty_seq=1, request_id="c" * 32)
        for index, attack in enumerate(("native-lifecycle-payload", "native-index-overlap"),
                                       start=1):
            with self.subTest(capture_delete_preflight=attack):
                attacked_path = self.root / f"policy-{attack}-{index}.db"
                with closing(sqlite3.connect(clone_path)) as original:
                    with closing(sqlite3.connect(attacked_path)) as attacked:
                        original.backup(attacked)
                with closing(sqlite3.connect(attacked_path)) as db:
                    event_key, payload = db.execute(
                        "SELECT event_key,payload FROM activity_index_v1 "
                        "WHERE kind='plugin' AND owner_key=? ORDER BY event_key LIMIT 1",
                        (source_id,)).fetchone()
                    if attack == "native-lifecycle-payload":
                        activity_payload = json.loads(payload)
                        activity_payload["native_lifecycle"] = {"state": "started"}
                        db.execute("UPDATE activity_index_v1 SET payload=? WHERE event_key=?",
                                   (json.dumps(activity_payload, sort_keys=True,
                                               separators=(",", ":")), event_key))
                        # Leave a clean baseline so a delete-trigger dirty row would
                        # be an observable side effect if preflight did not stop it.
                        db.execute("DELETE FROM native_lifecycle_dirty_v1 "
                                   "WHERE kind='plugin' AND owner_key=?", (source_id,))
                    else:
                        db.execute(
                            "INSERT INTO native_lifecycle_index_v1 "
                            "(event_key,kind,owner_key,receiver_id,sender_id,request_id,sequence,"
                            "scope_key,state_hash,payload,event_kind) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                            (event_key, "native", "native-owner", receiver_id, "sender",
                             "request", 1, "scope", "a" * 64,
                             json.dumps({"native_lifecycle": {"state": "started"}},
                                        sort_keys=True, separators=(",", ":")), "started"))
                    db.commit()
                before_attack = self.clone_policy_state_snapshot(attacked_path, source_id)
                native_before_attack = self.native_lifecycle_snapshot(attacked_path, source_id)
                binding_before_attack = self.batch_binding_snapshot(attacked_path, source_id)
                admission_before_attack = self.admission_receipts_snapshot(
                    attacked_path, source_id)
                capture_before_attack = self.policy_receipts_snapshot(
                    attacked_path, "storage_source_capture_set_v2", source_id)
                failure = self.apply_clone_policy(disable, path=attacked_path, success=False)
                self.assertIn("real clone", failure)
                self.assertEqual(self.clone_policy_state_snapshot(attacked_path, source_id),
                                 before_attack)
                self.assertEqual(self.native_lifecycle_snapshot(attacked_path, source_id),
                                 native_before_attack)
                self.assertEqual(self.batch_binding_snapshot(attacked_path, source_id),
                                 binding_before_attack)
                self.assertEqual(self.admission_receipts_snapshot(attacked_path, source_id),
                                 admission_before_attack)
                self.assertEqual(self.policy_receipts_snapshot(
                    attacked_path, "storage_source_capture_set_v2", source_id),
                    capture_before_attack)
        before_normal_disable = self.clone_policy_state_snapshot(clone_path, source_id)
        normal_disable = self.apply_activation_clone_v2(
            disable, path=clone_path, success=False)
        self.assertIn("real-schema clone accepts only activation or admission", normal_disable)
        self.assertEqual(self.clone_policy_state_snapshot(clone_path, source_id),
                         before_normal_disable)
        before_disable = self.clone_policy_state_snapshot(clone_path, source_id)
        before_binding = self.batch_binding_snapshot(clone_path, source_id)
        before_admission_receipts = self.admission_receipts_snapshot(clone_path, source_id)
        self.assertEqual(self.apply_clone_policy(disable, path=clone_path)["result"]["disposition"],
                         "committed")
        after_disable = self.clone_policy_state_snapshot(clone_path, source_id)
        capture_receipt = self.policy_receipts_snapshot(
            clone_path, "storage_source_capture_set_v2", source_id)
        disabled = self.apply_clone_policy(disable, path=clone_path)
        self.assertEqual(disabled["result"]["disposition"], "replay")
        self.assertEqual(after_disable, self.clone_policy_state_snapshot(clone_path, source_id))
        self.assertEqual(capture_receipt, self.policy_receipts_snapshot(
            clone_path, "storage_source_capture_set_v2", source_id))
        self.assertEqual(before_binding[0], "e" * 64)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id), ())
        self.assertEqual(self.admission_receipts_snapshot(clone_path, source_id),
                         before_admission_receipts)
        python_policy = set_source_capture(oracle, source_id=source_id, enabled=False)
        self.assertEqual(python_policy, 2)
        rust_state = self.batch_source_snapshot(clone_path, source_id)
        python_state = self.batch_source_snapshot(oracle, source_id)
        self.assertEqual(rust_state[0][:7] + rust_state[0][8:],
                         python_state[0][:7] + python_state[0][8:])
        self.assertEqual(self.comparable_clone_policy_state(clone_path, source_id),
                         self.comparable_clone_policy_state(oracle_path, source_id))
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id), ())
        disable_receipt = next(row for row in self.policy_receipts_snapshot(
            clone_path, "storage_source_capture_set_v2", source_id)
                               if row[0] == disable["request_id"])
        self.assertEqual(disable_receipt[8], "committed")
        self.assertEqual(canonical_file_state(), canonical_before)
        disabled_lease = disabled["result"]["lease"]

        stale_after_disable = self.mixed_batch_request(
            PollBatch(source_id, "policy-two", ({
                "event_id": "after-disable", "revision": 0, "kind": "lifecycle",
                "time_ns": "220", "payload": {"state": "completed"},
            },)), rust_lease, "d" * 32, "policy-one")
        before_stale = self.clone_policy_state_snapshot(clone_path, source_id)
        before_stale_receipts = self.admission_receipts_snapshot(clone_path, source_id)
        self.assertIn("stale or unsupported source lease",
                      self.apply_mixed_clone_batch(stale_after_disable, path=clone_path,
                                                   success=False))
        self.assertEqual(self.clone_policy_state_snapshot(clone_path, source_id), before_stale)
        self.assertEqual(self.admission_receipts_snapshot(clone_path, source_id),
                         before_stale_receipts)

        enable = self.clone_capture_request(
            disabled_lease, enabled=True, expected_enabled=False, cursor="policy-one",
            dirty_seq=2, request_id="e" * 32)
        self.assertEqual(self.apply_clone_policy(enable, path=clone_path)["result"]["disposition"],
                         "committed")
        enabled = self.apply_clone_policy(enable, path=clone_path)
        self.assertEqual(enabled["result"]["disposition"], "replay")
        self.assertEqual(set_source_capture(oracle, source_id=source_id, enabled=True), 3)
        self.assertEqual(self.comparable_clone_policy_state(clone_path, source_id),
                         self.comparable_clone_policy_state(oracle_path, source_id))
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id), ())
        before_historical_capture_replay = self.clone_policy_state_snapshot(
            clone_path, source_id)
        historical_capture = self.apply_clone_policy(disable, path=clone_path)
        self.assertEqual(historical_capture["result"]["disposition"], "replay")
        self.assertEqual(self.clone_policy_state_snapshot(clone_path, source_id),
                         before_historical_capture_replay)

        reactivation = {"version": 2, "request_id": "f" * 32,
                        "receiver_id": receiver_id,
                        "operation": {"op": "activate_source", "plugin_id": PLUGIN,
                                      "source_id": source_id, "generation": generation}}
        rust_lease = self.apply_activation_clone_v2(
            reactivation, path=clone_path)["result"]["lease"]
        python_lease = activate_source(oracle, plugin_id=PLUGIN, source_id=source_id,
                                       generation=generation)
        self.assertEqual((enabled["result"]["cursor"], rust_lease["policy_epoch"]),
                         (python_lease.cursor, python_lease.policy_epoch))
        self.assertEqual((enabled["result"]["cursor"], rust_lease["policy_epoch"]),
                         ("policy-one", 3))

        second_batch = PollBatch(source_id, "policy-two", ({
            "event_id": "policy-next", "revision": 0, "kind": "lifecycle",
            "time_ns": "220", "payload": {"state": "failed"},
            "identity": {"session_id": "policy-session"},
        },))
        second = self.mixed_batch_request(second_batch, rust_lease, "9" * 32, "policy-one")
        self.assertEqual(self.apply_mixed_clone_batch(second, path=clone_path)["result"]["disposition"],
                         "committed")
        self.assertEqual(commit_batch(oracle, python_lease, "policy-one", second_batch),
                         "policy-two")
        mirror_projection_to_clone()
        self.assertEqual(self.comparable_clone_policy_state(clone_path, source_id),
                         self.comparable_clone_policy_state(oracle_path, source_id))

        revoke = self.clone_revoke_request(
            rust_lease, cursor="policy-two", dirty_seq=4, request_id="7" * 32)
        before_normal_revoke = self.clone_policy_state_snapshot(clone_path, source_id)
        normal_revoke = self.apply_activation_clone_v2(
            revoke, path=clone_path, success=False)
        self.assertIn("clone revocation is unavailable through the storage service path",
                      normal_revoke)
        self.assertEqual(self.clone_policy_state_snapshot(clone_path, source_id),
                         before_normal_revoke)
        before_revoke = self.clone_policy_state_snapshot(clone_path, source_id)
        before_binding = self.batch_binding_snapshot(clone_path, source_id)
        before_admission_receipts = self.admission_receipts_snapshot(clone_path, source_id)
        self.assertEqual(self.apply_clone_policy(revoke, path=clone_path)["result"]["disposition"],
                         "committed")
        after_revoke = self.clone_policy_state_snapshot(clone_path, source_id)
        revoke_receipt = self.policy_receipts_snapshot(
            clone_path, "storage_source_revoke_v2", source_id)
        revoked = self.apply_clone_policy(revoke, path=clone_path)
        self.assertEqual(revoked["result"]["disposition"], "replay")
        self.assertEqual(after_revoke, self.clone_policy_state_snapshot(clone_path, source_id))
        self.assertEqual(revoke_receipt, self.policy_receipts_snapshot(
            clone_path, "storage_source_revoke_v2", source_id))
        self.assertEqual(before_binding[0], "e" * 64)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id), ())
        self.assertEqual(self.admission_receipts_snapshot(clone_path, source_id),
                         before_admission_receipts)
        self.assertEqual(set_source_capture(oracle, source_id=source_id, enabled=False), 4)
        self.assertEqual(self.comparable_clone_policy_state(clone_path, source_id),
                         self.comparable_clone_policy_state(oracle_path, source_id))
        self.assertEqual(canonical_file_state(), canonical_before)
        revoked_lease = revoked["result"]
        self.assertEqual((revoked_lease["capture_enabled"], revoked_lease["cursor"],
                          revoked_lease["policy_epoch"], revoked_lease["dirty_seq"]),
                         (False, "policy-two", 4, 5))
        self.assertEqual(self.policy_receipts_snapshot(
            clone_path, "storage_source_revoke_v2", source_id)[0][8], 0)

        reenable_lease = {
            "receiver_id": receiver_id, "plugin_id": PLUGIN, "source_id": source_id,
            "generation": generation, "epoch": revoked_lease["lease_epoch"],
            "policy_epoch": revoked_lease["policy_epoch"],
        }
        reenable_after_revoke = self.clone_capture_request(
            reenable_lease, enabled=True, expected_enabled=False, cursor="policy-two",
            dirty_seq=5, request_id="6" * 32)
        self.assertEqual(self.apply_clone_policy(
            reenable_after_revoke, path=clone_path)["result"]["disposition"], "committed")
        self.assertEqual(set_source_capture(oracle, source_id=source_id, enabled=True), 5)
        self.assertEqual(self.comparable_clone_policy_state(clone_path, source_id),
                         self.comparable_clone_policy_state(oracle_path, source_id))
        before_historical_revoke_replay = self.clone_policy_state_snapshot(
            clone_path, source_id)
        historical_revoke = self.apply_clone_policy(revoke, path=clone_path)
        self.assertEqual(historical_revoke["result"]["disposition"], "replay")
        self.assertEqual(self.clone_policy_state_snapshot(clone_path, source_id),
                         before_historical_revoke_replay)

        stale_after_revoke = self.mixed_batch_request(
            PollBatch(source_id, "policy-three", ({
                "event_id": "after-revoke", "revision": 0, "kind": "lifecycle",
                "time_ns": "230", "payload": {"state": "completed"},
            },)), rust_lease, "8" * 32, "policy-two")
        final_before_stale = self.clone_policy_state_snapshot(clone_path, source_id)
        final_receipts_before_stale = self.admission_receipts_snapshot(clone_path, source_id)
        self.assertIn("stale or unsupported source lease",
                      self.apply_mixed_clone_batch(stale_after_revoke, path=clone_path,
                                                   success=False))
        self.assertEqual(self.clone_policy_state_snapshot(clone_path, source_id),
                         final_before_stale)
        self.assertEqual(self.admission_receipts_snapshot(clone_path, source_id),
                         final_receipts_before_stale)

    def test_clone_policy_rejects_unsupported_history_before_mutation(self):
        from observatory.plugin_admission import activate_source

        source_id = "clone-policy-unsupported"
        generation = "f" * 64
        canonical_path = self.root / "unsupported-canonical.db"
        canonical = Store(str(canonical_path))
        receiver_id = canonical.receiver_id
        activate_source(canonical, plugin_id=PLUGIN, source_id="unsupported-seed",
                        generation=GENERATION)
        canonical.close()
        unsupported_rows = {
            "provider reference owner": (
                "INSERT INTO plugin_reference_owner_v1(source_id,provider_id) VALUES(?,?)",
                (source_id, "pi-content")),
            "member cursor": (
                "INSERT INTO plugin_source_member_cursor_v1(source_id,member_id,cursor) VALUES(?,?,?)",
                (source_id, "member-one", "member-cursor")),
            "source gap": (
                "INSERT INTO plugin_source_gap_invalidated_v1(source_id,native_session_id,last_record_ns) VALUES(?,?,?)",
                (source_id, "*", 0)),
        }
        for index, (label, (sql, values)) in enumerate(unsupported_rows.items(), start=1):
            with self.subTest(history=label):
                clone_path = self.root / f"unsupported-clone-{index}.db"
                with closing(sqlite3.connect(canonical_path)) as original:
                    with closing(sqlite3.connect(clone_path)) as clone:
                        original.backup(clone)
                with closing(sqlite3.connect(clone_path)) as db:
                    db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                               ("storage_activation_clone_v1", "enabled"))
                    db.commit()
                activation = {"version": 2, "request_id": "a" * 32,
                              "receiver_id": receiver_id,
                              "operation": {"op": "activate_source", "plugin_id": PLUGIN,
                                            "source_id": source_id, "generation": generation}}
                lease = self.apply_activation_clone_v2(
                    activation, path=clone_path)["result"]["lease"]
                with closing(sqlite3.connect(clone_path)) as db:
                    db.execute(sql, values)
                    db.commit()
                before = self.clone_policy_state_snapshot(clone_path, source_id)
                binding_before = self.batch_binding_snapshot(clone_path, source_id)
                capture_receipts_before = self.policy_receipts_snapshot(
                    clone_path, "storage_source_capture_set_v2", source_id)
                request = self.clone_capture_request(
                    lease, enabled=False, expected_enabled=True, cursor="", dirty_seq=0,
                    request_id="b" * 32)
                self.apply_clone_policy(request, path=clone_path, success=False)
                self.assertEqual(self.clone_policy_state_snapshot(clone_path, source_id), before)
                self.assertEqual(self.batch_binding_snapshot(clone_path, source_id),
                                 binding_before)
                self.assertEqual(self.policy_receipts_snapshot(
                    clone_path, "storage_source_capture_set_v2", source_id),
                    capture_receipts_before)

    def test_clone_policy_replay_rejects_orphan_committed_receipts(self):
        from observatory.plugin_admission import activate_source

        canonical_path = self.root / "orphan-replay-canonical.db"
        canonical = Store(str(canonical_path))
        receiver_id = canonical.receiver_id
        activate_source(canonical, plugin_id=PLUGIN, source_id="orphan-replay-seed",
                        generation=GENERATION)
        canonical.close()

        for operation in ("capture", "revoke"):
            with self.subTest(operation=operation):
                source_id = f"orphan-replay-{operation}"
                clone_path = self.root / f"orphan-replay-{operation}.db"
                with closing(sqlite3.connect(canonical_path)) as original:
                    with closing(sqlite3.connect(clone_path)) as clone:
                        original.backup(clone)
                with closing(sqlite3.connect(clone_path)) as db:
                    db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                               ("storage_activation_clone_v1", "enabled"))
                    db.commit()
                activation = {"version": 2, "request_id": "a" * 32,
                              "receiver_id": receiver_id,
                              "operation": {"op": "activate_source", "plugin_id": PLUGIN,
                                            "source_id": source_id,
                                            "generation": GENERATION}}
                lease = self.apply_activation_clone_v2(
                    activation, path=clone_path)["result"]["lease"]
                if operation == "capture":
                    request = self.clone_capture_request(
                        lease, enabled=False, expected_enabled=True, cursor="", dirty_seq=0,
                        request_id="b" * 32)
                    table = "storage_source_capture_set_v2"
                    receipt_epoch = lease["epoch"] + 1 if lease["epoch"] < 2**63 - 1 else lease["epoch"] - 1
                    receipt = (request["request_id"], receiver_id,
                               hashlib.sha256(json.dumps(
                                   request, sort_keys=True, separators=(",", ":"),
                                   ensure_ascii=False).encode()).hexdigest(),
                               source_id, PLUGIN, GENERATION, 1, 0, "committed",
                               receipt_epoch, 2, "", 1)
                    create_and_insert = (
                        "CREATE TABLE storage_source_capture_set_v2("
                        "request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,"
                        "request_digest TEXT NOT NULL,source_id TEXT NOT NULL,plugin_id TEXT NOT NULL,"
                        "generation TEXT NOT NULL,expected_capture_enabled INTEGER NOT NULL,"
                        "enabled INTEGER NOT NULL,disposition TEXT NOT NULL,lease_epoch INTEGER NOT NULL,"
                        "policy_epoch INTEGER NOT NULL,cursor TEXT NOT NULL,dirty_seq INTEGER NOT NULL)",
                        "INSERT INTO storage_source_capture_set_v2 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    )
                else:
                    request = self.clone_revoke_request(
                        lease, cursor="", dirty_seq=0, request_id="c" * 32)
                    table = "storage_source_revoke_v2"
                    receipt_epoch = lease["epoch"] + 1 if lease["epoch"] < 2**63 - 1 else lease["epoch"] - 1
                    receipt = (request["request_id"], receiver_id,
                               hashlib.sha256(json.dumps(
                                   request, sort_keys=True, separators=(",", ":"),
                                   ensure_ascii=False).encode()).hexdigest(),
                               source_id, "", 1, receipt_epoch, 2, 0)
                    create_and_insert = (
                        "CREATE TABLE storage_source_revoke_v2("
                        "request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,"
                        "request_digest TEXT NOT NULL,source_id TEXT NOT NULL,cursor TEXT NOT NULL,"
                        "dirty_seq INTEGER NOT NULL,lease_epoch INTEGER NOT NULL,"
                        "policy_epoch INTEGER NOT NULL,capture_enabled INTEGER NOT NULL)",
                        "INSERT INTO storage_source_revoke_v2 VALUES(?,?,?,?,?,?,?,?,?)",
                    )
                with closing(sqlite3.connect(clone_path)) as db:
                    db.execute(create_and_insert[0])
                    db.execute(create_and_insert[1], receipt)
                    db.commit()
                before = self.clone_policy_state_snapshot(clone_path, source_id)
                receipts_before = self.policy_receipts_snapshot(clone_path, table, source_id)
                binding_before = self.batch_binding_snapshot(clone_path, source_id)
                failure = self.apply_clone_policy(request, path=clone_path, success=False)
                self.assertIn("clone policy replay is not fenced by the current source state",
                              failure)
                self.assertEqual(self.clone_policy_state_snapshot(clone_path, source_id), before)
                self.assertEqual(self.policy_receipts_snapshot(clone_path, table, source_id),
                                 receipts_before)
                self.assertEqual(self.batch_binding_snapshot(clone_path, source_id),
                                 binding_before)

    def test_clone_capture_transition_rolls_back_late_sql_failure(self):
        from observatory.plugin_admission import activate_source

        source_id = "clone-policy-rollback"
        generation = "9" * 64
        canonical_path = self.root / "rollback-canonical.db"
        clone_path = self.root / "rollback-clone.db"
        canonical = Store(str(canonical_path))
        receiver_id = canonical.receiver_id
        activate_source(canonical, plugin_id=PLUGIN, source_id="rollback-seed",
                        generation=GENERATION)
        canonical.close()
        with closing(sqlite3.connect(canonical_path)) as original:
            with closing(sqlite3.connect(clone_path)) as clone:
                original.backup(clone)
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                       ("storage_activation_clone_v1", "enabled"))
            db.commit()

        def canonical_file_state():
            paths = (canonical_path, Path(f"{canonical_path}-wal"),
                     Path(f"{canonical_path}-shm"))
            return tuple((path.stat().st_ino, path.stat().st_mtime_ns,
                          hashlib.sha256(path.read_bytes()).hexdigest()) if path.exists() else None
                         for path in paths)

        canonical_before = canonical_file_state()
        activation = {"version": 2, "request_id": "a" * 32,
                      "receiver_id": receiver_id,
                      "operation": {"op": "activate_source", "plugin_id": PLUGIN,
                                    "source_id": source_id, "generation": generation}}
        lease = self.apply_activation_clone_v2(
            activation, path=clone_path)["result"]["lease"]
        batch = PollBatch(source_id, "rollback-one", ({
            "event_id": "rollback-message", "revision": 0, "kind": "message",
            "time_ns": "250", "payload": {"role": "assistant"},
            "identity": {"session_id": "rollback-session"},
        },))
        mixed = self.mixed_batch_request(batch, lease, "b" * 32, "")
        self.assertEqual(self.apply_mixed_clone_batch(mixed, path=clone_path)["result"]["disposition"],
                         "committed")
        before = self.clone_policy_state_snapshot(clone_path, source_id)
        binding_before = self.batch_binding_snapshot(clone_path, source_id)
        admissions_before = self.admission_receipts_snapshot(clone_path, source_id)
        capture_before = self.policy_receipts_snapshot(
            clone_path, "storage_source_capture_set_v2", source_id)

        # Force a failure after the transition has already deleted evidence,
        # projections and Activity rows, at the content-status epoch update.
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("ALTER TABLE plugin_content_status_v1 "
                       "RENAME TO plugin_content_status_v1_unconstrained")
            db.execute("CREATE TABLE plugin_content_status_v1("
                       "source_id TEXT PRIMARY KEY,cursor TEXT NOT NULL,reason TEXT NOT NULL,"
                       "refs_count INTEGER NOT NULL,policy_epoch INTEGER NOT NULL CHECK(policy_epoch=1))")
            db.execute("INSERT INTO plugin_content_status_v1 "
                       "SELECT source_id,cursor,reason,refs_count,policy_epoch "
                       "FROM plugin_content_status_v1_unconstrained")
            db.execute("DROP TABLE plugin_content_status_v1_unconstrained")
            db.commit()
        request = self.clone_capture_request(
            lease, enabled=False, expected_enabled=True, cursor="rollback-one",
            dirty_seq=1, request_id="c" * 32)
        failure = self.apply_clone_policy(request, path=clone_path, success=False)
        self.assertIn("CHECK constraint failed", failure)
        self.assertEqual(self.clone_policy_state_snapshot(clone_path, source_id), before)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id), binding_before)
        self.assertEqual(self.admission_receipts_snapshot(clone_path, source_id), admissions_before)
        self.assertEqual(self.policy_receipts_snapshot(
            clone_path, "storage_source_capture_set_v2", source_id), capture_before)
        self.assertEqual(canonical_file_state(), canonical_before)

    def test_v2_durable_receipt_exact_replay_and_conflict(self):
        first = self.v2_request(PollBatch(SOURCE, "one", (fact(),)))
        digest = hashlib.sha256(json.dumps(first, sort_keys=True, separators=(",", ":"),
                                           ensure_ascii=False).encode()).hexdigest()
        reply = self.apply_v2(first)
        self.assertEqual(reply, {"version": 2, "request_id": first["request_id"],
                                 "receiver_id": first["receiver_id"],
                                 "request_digest": digest,
                                 "result": {"kind": "lifecycle_admission",
                                            "disposition": "committed", "cursor": "one",
                                            "batch_digest": first["operation"]["batch_digest"],
                                            "dirty_seq": 1}})
        committed = self.snapshot(self.rust_store)
        with self.rust_store._connect() as db:
            receipt = tuple(db.execute(
                "SELECT request_digest,cursor,batch_digest,dirty_seq,disposition "
                "FROM storage_admission_request_v2 WHERE request_id=?",
                (first["request_id"],)).fetchone())
        self.assertEqual(receipt, (digest, "one", first["operation"]["batch_digest"],
                                   1, "committed"))
        replay = self.apply_v2(first)
        self.assertEqual(replay["result"]["disposition"], "replay")
        self.assertEqual(replay["request_digest"], digest)
        self.assertEqual(self.snapshot(self.rust_store), committed)
        # The same request ID cannot be reused for a different valid batch.
        changed = self.v2_request(PollBatch(SOURCE, "two", (fact("second"),)), "one")
        self.assertIn("conflicting v2 request replay identity",
                      self.apply_v2(changed, success=False))
        self.assertEqual(self.snapshot(self.rust_store), committed)
        changed["request_id"] = "2" * 32
        self.assertEqual(self.apply_v2(changed)["result"]["disposition"], "committed")
        # A much older exact request cannot replay after the source advanced.
        self.apply_v2(first, success=False)

    def test_v2_rejects_hostile_frame_and_unmarked_path_before_write(self):
        request = self.v2_request(PollBatch(SOURCE, "one", (fact(),)))
        unknown = json.loads(json.dumps(request))
        unknown["operation"]["records"][0]["output"] = "secret"
        self.apply_v2(unknown, success=False)
        raw = json.dumps(request).replace('"version": 2',
                                          '"version": 2, "version": 2', 1)
        self.apply_v2(raw, success=False)
        self.assertIn("not an explicit scratch",
                      self.apply_v2(request, path=self.python_store.path, success=False))
        self.assertFalse((self.root / ".python.db.agenturm-writer.lock").exists())
        with self.rust_store._connect() as db:
            self.assertIsNone(db.execute(
                "SELECT name FROM sqlite_master WHERE name='storage_admission_request_v2'"
            ).fetchone())
        self.assertEqual(self.snapshot(self.rust_store)[1], [])

    def test_v2_receipt_tamper_and_revoked_source_fail_closed(self):
        request = self.v2_request(PollBatch(SOURCE, "one", (fact(),)))
        self.apply_v2(request)
        before = self.snapshot(self.rust_store)
        with self.rust_store._connect() as db:
            db.execute("UPDATE storage_admission_request_v2 SET dirty_seq=99 WHERE request_id=?",
                       (request["request_id"],))
        self.assertIn("conflicting v2 request replay identity",
                      self.apply_v2(request, success=False))
        self.assertEqual(self.snapshot(self.rust_store), before)
        with self.rust_store._connect() as db:
            db.execute("UPDATE storage_admission_request_v2 SET dirty_seq=1 WHERE request_id=?",
                       (request["request_id"],))
        set_source_capture(self.rust_store, source_id=SOURCE, enabled=False)
        self.assertIn("stale or unsupported source lease",
                      self.apply_v2(request, success=False))

    def test_v2_revoke_cleans_projects_fences_and_replays_across_process_restart(self):
        from observatory.plugin_projection import project_source

        admission = self.v2_request(PollBatch(SOURCE, "one", (fact(),)), request_id="4" * 32)
        self.apply_v2(admission)
        project_source(self.rust_store, SOURCE)
        with self.rust_store._connect() as db:
            source = db.execute("SELECT cursor,dirty_seq,policy_epoch,lease_epoch,capture_enabled,"
                                "content_fingerprint FROM plugin_source_cursor_v1 WHERE source_id=?",
                                (SOURCE,)).fetchone()
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_session_projection_v1 "
                                        "WHERE source_id=?", (SOURCE,)).fetchone()[0], 1)
        cursor, dirty_seq, policy_epoch, lease_epoch, enabled, fingerprint = tuple(source)
        self.assertEqual((cursor, enabled), ("one", 1))
        revoke = self.v2_revoke_request(cursor=cursor, dirty_seq=dirty_seq)
        request_digest = hashlib.sha256(json.dumps(
            revoke, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()
        reply = self.apply_v2(revoke)
        self.assertEqual(reply, {"version": 2, "request_id": revoke["request_id"],
                                 "receiver_id": revoke["receiver_id"],
                                 "request_digest": request_digest,
                                 "result": {"kind": "source_revoked", "disposition": "committed",
                                            "cursor": cursor, "dirty_seq": dirty_seq + 1,
                                            "lease_epoch": lease_epoch + 1,
                                            "policy_epoch": policy_epoch + 1,
                                            "capture_enabled": False}})
        with self.rust_store._connect() as db:
            revoked = tuple(db.execute("SELECT cursor,dirty_seq,policy_epoch,lease_epoch,"
                                       "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                                       "WHERE source_id=?", (SOURCE,)).fetchone())
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id=?",
                                        (SOURCE,)).fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_session_projection_v1 WHERE source_id=?",
                                        (SOURCE,)).fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT COUNT(*) FROM activity_index_v1 "
                                        "WHERE kind='plugin' AND owner_key=?", (SOURCE,)).fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_content_batch_v1 WHERE source_id=?",
                                        (SOURCE,)).fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_content_status_v1 WHERE source_id=?",
                                        (SOURCE,)).fetchone()[0], 0)
            stored = tuple(db.execute("SELECT request_digest,source_id,cursor,dirty_seq,lease_epoch,"
                                      "policy_epoch,capture_enabled FROM storage_source_revoke_v2 "
                                      "WHERE request_id=?", (revoke["request_id"],)).fetchone())
        self.assertEqual(revoked, (cursor, dirty_seq + 1, policy_epoch + 1,
                                   lease_epoch + 1, 0, ""))
        self.assertEqual(stored, (request_digest, SOURCE, cursor, dirty_seq + 1,
                                  lease_epoch + 1, policy_epoch + 1, 0))

        # Each call launches a new process, exercising the persisted receipt after restart.
        replay = self.apply_v2(revoke)
        self.assertEqual(replay["result"]["disposition"], "replay")
        self.assertEqual(replay["result"]["policy_epoch"], policy_epoch + 1)
        self.assertIn("stale or unsupported source lease", self.apply_v2(admission, success=False))
        conflict = json.loads(json.dumps(revoke))
        conflict["operation"]["expected_dirty_seq"] += 1
        self.assertIn("conflicting v2 source revocation request",
                      self.apply_v2(conflict, success=False))
        reused = json.loads(json.dumps(admission))
        reused["request_id"] = revoke["request_id"]
        self.assertIn("conflicting v2 request replay identity",
                      self.apply_v2(reused, success=False))
        with self.rust_store._connect() as db:
            final = tuple(db.execute(
                "SELECT cursor,dirty_seq,policy_epoch,lease_epoch,capture_enabled,content_fingerprint "
                "FROM plugin_source_cursor_v1 WHERE source_id=?", (SOURCE,)).fetchone())
        self.assertEqual(final, revoked)

    def test_v2_activation_defaults_retry_reactivation_and_revocation_fencing(self):
        from observatory.plugin_projection import project_source

        source_id = "new-scratch-source"
        activation = self.v2_activate_request(
            source_id, request_id="a123456789abcdef0123456789abcdef")
        first = self.apply_v2(activation)
        self.assertEqual(first["result"]["kind"], "source_activated")
        self.assertEqual(first["result"]["disposition"], "committed")
        initial_lease = first["result"]["lease"]
        self.assertEqual(first["result"]["cursor"], "")
        self.assertEqual(first["result"]["dirty_seq"], 0)
        with self.rust_store._connect() as db:
            defaults = tuple(db.execute(
                "SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,capture_enabled,"
                "cursor,prior_cursor,last_batch_digest,dirty_seq,content_fingerprint "
                "FROM plugin_source_cursor_v1 WHERE source_id=?", (source_id,)).fetchone())
        self.assertEqual(defaults, (self.rust_lease.receiver_id, PLUGIN, GENERATION,
                                    initial_lease["epoch"], 1, 1, "", "", "", 0, ""))
        before_retry = defaults
        replay = self.apply_v2(activation)
        self.assertEqual(replay["result"]["disposition"], "replay")
        self.assertEqual(replay["result"]["lease"], initial_lease)
        with self.rust_store._connect() as db:
            self.assertEqual(tuple(db.execute(
                "SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,capture_enabled,"
                "cursor,prior_cursor,last_batch_digest,dirty_seq,content_fingerprint "
                "FROM plugin_source_cursor_v1 WHERE source_id=?", (source_id,)).fetchone()),
                before_retry)
        conflict = self.v2_activate_request(
            source_id, request_id=activation["request_id"], generation="b" * 64)
        self.assertIn("conflicting v2 source activation request",
                      self.apply_v2(conflict, success=False))

        batch = PollBatch(source_id, "cursor-one", (fact(),))
        admission = self.v2_request(batch, lease=type("Lease", (), initial_lease)(),
                                    source_id=source_id, request_id="b123456789abcdef0123456789abcdef")
        self.apply_v2(admission)
        project_source(self.rust_store, source_id)
        with self.rust_store._connect() as db:
            before = tuple(db.execute(
                "SELECT cursor,policy_epoch,dirty_seq,lease_epoch,capture_enabled "
                "FROM plugin_source_cursor_v1 WHERE source_id=?", (source_id,)).fetchone())
        self.assertEqual(before[:3], ("cursor-one", 1, 1))

        renewed_request = self.v2_activate_request(
            source_id, request_id="c123456789abcdef0123456789abcdef")
        renewed = self.apply_v2(renewed_request)["result"]["lease"]
        self.assertNotEqual(renewed["epoch"], initial_lease["epoch"])
        with self.rust_store._connect() as db:
            after = tuple(db.execute(
                "SELECT cursor,policy_epoch,dirty_seq,lease_epoch,capture_enabled "
                "FROM plugin_source_cursor_v1 WHERE source_id=?", (source_id,)).fetchone())
        self.assertEqual(after, (before[0], before[1], before[2], renewed["epoch"], 1))
        stale = self.v2_request(PollBatch(source_id, "cursor-two", (fact("second"),)),
                                "cursor-one", lease=type("Lease", (), initial_lease)(),
                                source_id=source_id, request_id="d123456789abcdef0123456789abcdef")
        self.assertIn("stale or unsupported source lease", self.apply_v2(stale, success=False))

        prior_activation_replay = self.apply_v2(activation)
        self.assertEqual(prior_activation_replay["result"]["lease"], initial_lease)
        with self.rust_store._connect() as db:
            self.assertEqual(tuple(db.execute(
                "SELECT cursor,policy_epoch,dirty_seq,lease_epoch,capture_enabled "
                "FROM plugin_source_cursor_v1 WHERE source_id=?", (source_id,)).fetchone()), after)

        revoke = self.v2_revoke_request(cursor="cursor-one", dirty_seq=1,
                                        request_id="e123456789abcdef0123456789abcdef",
                                        lease=type("Lease", (), renewed)())
        self.apply_v2(revoke)
        revoked_activation_replay = self.apply_v2(activation)
        self.assertEqual(revoked_activation_replay["result"]["disposition"], "replay")
        self.assertEqual(revoked_activation_replay["result"]["lease"], initial_lease)
        with self.rust_store._connect() as db:
            revoked_state = tuple(db.execute(
                "SELECT lease_epoch,policy_epoch,capture_enabled,cursor "
                "FROM plugin_source_cursor_v1 WHERE source_id=?", (source_id,)).fetchone())
        self.assertEqual(revoked_state, (renewed["epoch"] + 1, 2, 0, "cursor-one"))
        fresh_after_revoke = self.v2_activate_request(
            source_id, request_id="f123456789abcdef0123456789abcdef")
        self.assertIn("revoked", self.apply_v2(fresh_after_revoke, success=False))

    def test_v2_capture_noop_disable_enable_and_replay_fence_supported_history(self):
        from observatory.plugin_projection import project_source

        no_op = self.v2_capture_request(
            self.rust_lease, enabled=True, expected_enabled=True, cursor="", dirty_seq=0,
            request_id="8123456789abcdef0123456789abcdef")
        idle = self.apply_v2(no_op)
        self.assertEqual(idle["result"]["kind"], "source_capture_set")
        self.assertEqual(idle["result"]["disposition"], "idle")
        self.assertEqual(idle["result"]["lease"]["epoch"], self.rust_lease.epoch)
        with self.rust_store._connect() as db:
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM plugin_evidence_dirty_v1 WHERE source_id=?", (SOURCE,)
            ).fetchone()[0], 0)
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM plugin_projection_cursor_v1 WHERE source_id=?", (SOURCE,)
            ).fetchone()[0], 0)
        self.assertEqual(self.apply_v2(no_op)["result"]["disposition"], "replay")

        admission = self.v2_request(
            PollBatch(SOURCE, "cursor-one", (fact(),)), request_id="9123456789abcdef0123456789abcdef")
        self.apply_v2(admission)
        project_source(self.rust_store, SOURCE)
        disable = self.v2_capture_request(
            self.rust_lease, enabled=False, expected_enabled=True, cursor="cursor-one",
            dirty_seq=1, request_id="a123456789abcdef0123456789abcdef")
        disabled = self.apply_v2(disable)["result"]
        self.assertEqual(disabled["disposition"], "committed")
        self.assertFalse(disabled["enabled"])
        self.assertEqual(disabled["cursor"], "cursor-one")
        self.assertEqual(disabled["dirty_seq"], 2)
        disabled_lease = type("Lease", (), disabled["lease"])()
        with self.rust_store._connect() as db:
            source = tuple(db.execute(
                "SELECT cursor,dirty_seq,policy_epoch,lease_epoch,capture_enabled,content_fingerprint "
                "FROM plugin_source_cursor_v1 WHERE source_id=?", (SOURCE,)).fetchone())
            self.assertEqual(source, ("cursor-one", 2, 2, disabled["lease"]["epoch"], 0, ""))
            for table in ("plugin_evidence_v1", "plugin_session_projection_v1",
                          "activity_index_v1", "investigation_dirty_v1"):
                self.assertEqual(db.execute(
                    f"SELECT COUNT(*) FROM {table} WHERE source_id=?" if table not in
                    ("activity_index_v1", "investigation_dirty_v1") else
                    f"SELECT COUNT(*) FROM {table} WHERE kind='plugin' AND owner_key=?",
                    (SOURCE,)).fetchone()[0], 0, table)
        stale = self.v2_request(PollBatch(SOURCE, "cursor-two", (fact("second"),)),
                                "cursor-one", request_id="b123456789abcdef0123456789abcdef")
        self.assertIn("stale or unsupported source lease", self.apply_v2(stale, success=False))

        enable = self.v2_capture_request(
            disabled_lease, enabled=True, expected_enabled=False, cursor="cursor-one",
            dirty_seq=2, request_id="c123456789abcdef0123456789abcdef")
        enabled = self.apply_v2(enable)["result"]
        self.assertEqual(enabled["disposition"], "committed")
        self.assertTrue(enabled["enabled"])
        self.assertEqual(enabled["dirty_seq"], 3)
        self.assertEqual(enabled["lease"]["policy_epoch"], 3)
        with self.rust_store._connect() as db:
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id=?", (SOURCE,)
            ).fetchone()[0], 0)
            current = tuple(db.execute(
                "SELECT cursor,dirty_seq,policy_epoch,lease_epoch,capture_enabled "
                "FROM plugin_source_cursor_v1 WHERE source_id=?", (SOURCE,)).fetchone())
        # Both old operations replay their first committed replies after later
        # transitions; the command starts a fresh process for every invocation.
        no_op_replay = self.apply_v2(no_op)["result"]
        self.assertEqual(no_op_replay["disposition"], "replay")
        self.assertEqual(no_op_replay["lease"]["epoch"], self.rust_lease.epoch)
        disable_replay = self.apply_v2(disable)["result"]
        self.assertEqual(disable_replay["disposition"], "replay")
        self.assertEqual(disable_replay["lease"], disabled["lease"])
        with self.rust_store._connect() as db:
            self.assertEqual(tuple(db.execute(
                "SELECT cursor,dirty_seq,policy_epoch,lease_epoch,capture_enabled "
                "FROM plugin_source_cursor_v1 WHERE source_id=?", (SOURCE,)).fetchone()), current)
        changed = self.v2_capture_request(
            self.rust_lease, enabled=False, expected_enabled=True, cursor="different-cursor",
            dirty_seq=1, request_id=disable["request_id"])
        self.assertIn("conflicting v2 source capture request",
                      self.apply_v2(changed, success=False))

    def test_v2_capture_explicitly_reenables_revoked_source_without_restoring_evidence(self):
        revoke = self.v2_revoke_request(cursor="", dirty_seq=0,
                                        request_id="d123456789abcdef0123456789abcdef")
        revoked = self.apply_v2(revoke)["result"]
        self.assertEqual(revoked["disposition"], "committed")
        revoked_lease = type("Lease", (), {
            "receiver_id": self.rust_lease.receiver_id,
            "plugin_id": self.rust_lease.plugin_id,
            "source_id": self.rust_lease.source_id,
            "generation": self.rust_lease.generation,
            "epoch": revoked["lease_epoch"],
            "policy_epoch": revoked["policy_epoch"],
        })()
        enabled = self.apply_v2(self.v2_capture_request(
            revoked_lease, enabled=True, expected_enabled=False, cursor="", dirty_seq=1,
            request_id="e123456789abcdef0123456789abcdef"))["result"]
        self.assertEqual(enabled["disposition"], "committed")
        self.assertTrue(enabled["enabled"])
        self.assertNotEqual(enabled["lease"]["epoch"], revoked["lease_epoch"])
        self.assertEqual(enabled["lease"]["policy_epoch"], revoked["policy_epoch"] + 1)
        self.assertEqual(enabled["dirty_seq"], 2)
        with self.rust_store._connect() as db:
            source = tuple(db.execute(
                "SELECT capture_enabled,cursor,dirty_seq,policy_epoch,lease_epoch "
                "FROM plugin_source_cursor_v1 WHERE source_id=?", (SOURCE,)).fetchone())
            self.assertEqual(source, (1, "", 2, 3, enabled["lease"]["epoch"]))
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id=?", (SOURCE,)
            ).fetchone()[0], 0)
        stale = self.v2_request(PollBatch(SOURCE, "next", (fact(),)),
                                request_id="f123456789abcdef0123456789abcdef")
        self.assertIn("stale or unsupported source lease", self.apply_v2(stale, success=False))

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
        lock = self.root / ".rust.db.agenturm-writer.lock"
        self.assertTrue(lock.is_file())
        self.assertEqual(stat.S_IMODE(lock.stat().st_mode), 0o600)
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
        unmarked_lock = self.root / ".python.db.agenturm-writer.lock"
        self.assertFalse(unmarked_lock.exists())
        self.assertIn("not an explicit scratch", self.apply(no_mark, success=False))
        self.assertFalse(unmarked_lock.exists())
        self.assertEqual(self.snapshot(self.python_store)[1], [])
        set_source_capture(self.rust_store, source_id=SOURCE, enabled=False)
        self.assertIn("stale", self.apply(request, success=False))
        self.assertEqual(self.snapshot(self.rust_store)[1], [])

    def test_unsupported_correction_rejected_before_write(self):
        request = self.request(PollBatch(SOURCE, "one", (fact(),)))
        request["records"][0]["revision"] = 1
        self.assertIn("unsupported", self.apply(request, success=False))
        self.assertEqual(self.snapshot(self.rust_store)[0][0], "")

    def test_replay_rejects_present_mismatched_receipt(self):
        request = self.request(PollBatch(SOURCE, "one", (fact(),)))
        self.apply(request)
        with self.rust_store._connect() as db:
            db.execute("DELETE FROM plugin_content_batch_v1 WHERE source_id=?", (SOURCE,))
        self.assertEqual(self.apply(request)["disposition"], "replay")
        with self.rust_store._connect() as db:
            db.execute("INSERT INTO plugin_content_batch_v1 VALUES(?,?,?)",
                       (SOURCE, "one", "f" * 64))
        before = self.snapshot(self.rust_store)
        self.assertIn("receipt mismatch", self.apply(request, success=False))
        self.assertEqual(self.snapshot(self.rust_store), before)

    def test_retention_watermark_and_member_map_are_unsupported(self):
        request = self.request(PollBatch(SOURCE, "one", (fact(),)))
        with self.rust_store._connect() as db:
            db.execute("INSERT INTO plugin_retention_v1 VALUES(?,?,?,?)",
                       (SOURCE, 1, 10, 1))
        self.assertIn("unsupported history", self.apply(request, success=False))
        with self.rust_store._connect() as db:
            db.execute("DELETE FROM plugin_retention_v1 WHERE source_id=?", (SOURCE,))
            db.execute("INSERT INTO plugin_session_member_v1 VALUES(?,?,?)",
                       (SOURCE, "native-one", "member"))
        self.assertIn("unsupported history", self.apply(request, success=False))
        self.assertEqual(self.snapshot(self.rust_store)[0][0], "")

    def test_lifecycle_payload_is_closed_to_free_text(self):
        request = self.request(PollBatch(SOURCE, "one", (fact(),)))
        request["records"][0]["payload_json"] = '{"state":"started","text":"prompt"}'
        self.assertIn("unsupported prepared lifecycle", self.apply(request, success=False))
        self.assertEqual(self.snapshot(self.rust_store)[0][0], "")

    def test_other_process_lock_refuses_scratch_commit_until_release(self):
        request = self.request(PollBatch(SOURCE, "one", (fact(),)))
        lock = self.root / ".rust.db.agenturm-writer.lock"
        lock.touch(mode=0o600)
        holder = subprocess.Popen([sys.executable, "-c", """
import fcntl, sys
with open(sys.argv[1], 'r+b') as stream:
    fcntl.flock(stream, fcntl.LOCK_EX)
    print('ready', flush=True)
    sys.stdin.read(1)
""", str(lock)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, text=True)
        try:
            self.assertTrue(select.select([holder.stdout], [], [], 5)[0],
                            "lock holder did not become ready")
            self.assertEqual(holder.stdout.readline().strip(), "ready")
            before = self.snapshot(self.rust_store)
            self.apply(request, success=False)
            self.assertEqual(self.snapshot(self.rust_store), before)
        finally:
            holder.stdin.close()
            self.assertEqual(holder.wait(timeout=5), 0,
                             holder.stderr.read() if holder.stderr else "")
            holder.stdout.close()
            holder.stderr.close()
        self.assertEqual(self.apply(request)["disposition"], "committed")

    def test_hardlink_alias_refuses_scratch_write(self):
        request = self.request(PollBatch(SOURCE, "one", (fact(),)))
        alias = self.root / "alias.db"
        os.link(self.rust_store.path, alias)
        try:
            self.apply(request, success=False)
            self.assertEqual(self.snapshot(self.rust_store)[0][0], "")
        finally:
            alias.unlink()
        self.assertEqual(self.apply(request)["disposition"], "committed")

    def test_symlink_final_path_refuses_before_lease_file(self):
        alias = self.root / "alias.db"
        alias.symlink_to(self.rust_store.path)
        request = self.request(PollBatch(SOURCE, "one", (fact(),)), path=alias)
        self.assertIn("existing regular file", self.apply(request, success=False))
        self.assertFalse((self.root / ".rust.db.agenturm-writer.lock").exists())
        self.assertEqual(self.snapshot(self.rust_store)[0][0], "")

    def test_clone_diagnostic_admission_read_and_policy_filtering_match_store(self):
        from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
        from observatory.plugin_wire import encode_message

        source_id = "diagnostic-clone-source"
        generation = "f" * 64
        canonical_path = self.root / "diagnostic-canonical.db"
        clone_path = self.root / "diagnostic-clone.db"
        oracle_path = self.root / "diagnostic-oracle.db"
        canonical = Store(str(canonical_path))
        receiver_id = canonical.receiver_id
        activate_source(canonical, plugin_id=PLUGIN, source_id="diagnostic-seed",
                        generation=GENERATION)
        canonical.close()
        with closing(sqlite3.connect(canonical_path)) as original:
            for target in (clone_path, oracle_path):
                with closing(sqlite3.connect(target)) as copied:
                    original.backup(copied)
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                       ("storage_activation_clone_v1", "enabled"))
            db.commit()

        def canonical_file_state():
            paths = (canonical_path, Path(f"{canonical_path}-wal"),
                     Path(f"{canonical_path}-shm"))
            return tuple((path.stat().st_ino, path.stat().st_mtime_ns,
                          hashlib.sha256(path.read_bytes()).hexdigest()) if path.exists() else None
                         for path in paths)

        canonical_before = canonical_file_state()

        def logical_clone_snapshot(path):
            with closing(sqlite3.connect(path)) as db:
                schema = tuple(tuple(row) for row in db.execute(
                    "SELECT type,name,sql FROM sqlite_master ORDER BY type,name"))
                tables = [row[0] for row in db.execute(
                    "SELECT name FROM sqlite_master WHERE type='table' "
                    "AND name NOT LIKE 'sqlite_%' ORDER BY name")]
                data = []
                for table in tables:
                    quoted_table = table.replace('"', '""')
                    rows = tuple(sorted(
                        (tuple(row) for row in db.execute(
                            f'SELECT * FROM "{quoted_table}"')),
                        key=repr))
                    data.append((table, rows))
                return schema, tuple(data)

        def assert_supported_state_matches_python():
            rust_state = self.batch_source_snapshot(clone_path, source_id)
            python_state = self.batch_source_snapshot(oracle_path, source_id)
            self.assertEqual(rust_state[0][:7] + rust_state[0][8:],
                             python_state[0][:7] + python_state[0][8:])
            self.assertEqual(rust_state[1:], python_state[1:])

        def activate_request(request_id):
            return {"version": 2, "request_id": request_id,
                    "receiver_id": receiver_id,
                    "operation": {"op": "activate_source", "plugin_id": PLUGIN,
                                  "source_id": source_id, "generation": generation}}

        activation = activate_request("9" * 32)
        activated = self.apply_activation_clone_v2(activation, path=clone_path)["result"]
        rust_lease = activated["lease"]
        oracle = Store(str(oracle_path))
        self.addCleanup(oracle.close)
        python_lease = activate_source(oracle, plugin_id=PLUGIN, source_id=source_id,
                                       generation=generation)

        def read_request(lease, request_id):
            return self.clone_diagnostic_read_request(lease, request_id)

        def rust_read(lease, request_id):
            request = read_request(lease, request_id)
            before = logical_clone_snapshot(clone_path)
            reply = self.apply_clone_diagnostic_read(request, path=clone_path)
            self.assertEqual(logical_clone_snapshot(clone_path), before)
            self.assertEqual(set(reply), {
                "version", "request_id", "receiver_id", "request_digest", "result"})
            self.assertEqual(set(reply["result"]), {"kind", "diagnostic"})
            if reply["result"]["diagnostic"] is not None:
                self.assertEqual(set(reply["result"]["diagnostic"]),
                                 {"code", "severity", "time_ns"})
            self.assertEqual(reply["request_id"], request["request_id"])
            self.assertEqual(reply["receiver_id"], receiver_id)
            expected_digest = hashlib.sha256(json.dumps(
                request, ensure_ascii=False, sort_keys=True, separators=(",", ":")
            ).encode()).hexdigest()
            self.assertEqual(reply["request_digest"], expected_digest)
            return reply["result"]["diagnostic"]

        self.assertIsNone(rust_read(rust_lease, "a" * 32))
        self.assertEqual(last_source_diagnostic(oracle, python_lease), {})
        before_normal_read = self.clone_policy_state_snapshot(clone_path, source_id)
        before_normal_read_logical = logical_clone_snapshot(clone_path)
        normal_read = self.apply_activation_clone_v2(
            read_request(rust_lease, "b" * 32), path=clone_path, success=False)
        self.assertIn("real-schema clone accepts only activation or admission", normal_read)
        self.assertEqual(logical_clone_snapshot(clone_path), before_normal_read_logical)
        self.assertEqual(self.clone_policy_state_snapshot(clone_path, source_id),
                         before_normal_read)

        def diagnostic_batch(event_id, cursor, code="source_timeout", severity="warning",
                             time_ns="9007199254740993"):
            return PollBatch(source_id, cursor, ({
                "event_id": event_id, "revision": 0, "kind": "diagnostic",
                "time_ns": time_ns,
                "payload": {"code": code, "severity": severity},
            },))

        first_batch = diagnostic_batch("timeout-one", "diag-one")
        first_request = self.mixed_batch_request(first_batch, rust_lease, "c" * 32, "")
        before_normal_admit = self.clone_policy_state_snapshot(clone_path, source_id)
        before_normal_admit_logical = logical_clone_snapshot(clone_path)
        normal_admit = self.apply_activation_clone_v2(
            first_request, path=clone_path, success=False)
        self.assertIn("mixed admission is unavailable", normal_admit)
        self.assertEqual(logical_clone_snapshot(clone_path), before_normal_admit_logical)
        self.assertEqual(self.clone_policy_state_snapshot(clone_path, source_id),
                         before_normal_admit)
        first_reply = self.apply_mixed_clone_batch(first_request, path=clone_path)
        self.assertEqual(first_reply["result"]["disposition"], "committed")
        self.assertEqual(commit_batch(oracle, python_lease, "", first_batch), "diag-one")
        assert_supported_state_matches_python()
        self.assertEqual(rust_read(rust_lease, "d" * 32),
                         {"code": "source_timeout", "severity": "warning",
                          "time_ns": "9007199254740993"})
        self.assertEqual(last_source_diagnostic(oracle, python_lease),
                         {"code": "source_timeout", "severity": "warning",
                          "time_ns": "9007199254740993"})

        committed_state = self.batch_source_snapshot(clone_path, source_id)
        committed_receipts = self.admission_receipts_snapshot(clone_path, source_id)
        committed_binding = self.batch_binding_snapshot(clone_path, source_id)
        self.assertEqual(self.apply_mixed_clone_batch(first_request, path=clone_path)
                         ["result"]["disposition"], "replay")
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id), committed_state)
        self.assertEqual(self.admission_receipts_snapshot(clone_path, source_id),
                         committed_receipts)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id), committed_binding)
        altered = json.loads(json.dumps(first_request))
        altered["operation"]["records"][0]["payload_json"] = (
            '{"code":"source_timeout","severity":"error"}')
        self.apply_mixed_clone_batch(altered, path=clone_path, success=False)
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id), committed_state)
        self.assertEqual(self.admission_receipts_snapshot(clone_path, source_id),
                         committed_receipts)
        self.assertEqual(self.batch_binding_snapshot(clone_path, source_id), committed_binding)

        second_batch = diagnostic_batch("timeout-two", "diag-two", severity="error")
        second_request = self.mixed_batch_request(second_batch, rust_lease,
                                                  "e" * 32, "diag-one")
        self.apply_mixed_clone_batch(second_request, path=clone_path)
        commit_batch(oracle, python_lease, "diag-one", second_batch)
        assert_supported_state_matches_python()
        # Pin admitted_ns to a tie; rowid DESC is the deterministic tiebreaker.
        for db_path in (clone_path, oracle_path):
            with closing(sqlite3.connect(db_path)) as db:
                db.execute("UPDATE plugin_evidence_v1 SET admitted_ns=777 WHERE source_id=?",
                           (source_id,))
                db.commit()
        self.assertEqual(rust_read(rust_lease, "f" * 32),
                         {"code": "source_timeout", "severity": "error",
                          "time_ns": "9007199254740993"})
        self.assertEqual(last_source_diagnostic(oracle, python_lease),
                         {"code": "source_timeout", "severity": "error",
                          "time_ns": "9007199254740993"})

        renewed = self.apply_activation_clone_v2(
            activate_request("1" * 32), path=clone_path)["result"]["lease"]
        python_lease = activate_source(oracle, plugin_id=PLUGIN, source_id=source_id,
                                       generation=generation)
        self.assertEqual(rust_read(renewed, "2" * 32),
                         {"code": "source_timeout", "severity": "error",
                          "time_ns": "9007199254740993"})
        self.assertEqual(last_source_diagnostic(oracle, python_lease)["severity"], "error")

        for case, admitted_ns, revision, member_id, worker_id, payload_json in (
                ("malformed-latest", 999999, 0, "bad-member", "",
                 '{"code":"member_gap","severity":"warning"}'),
                ("malformed-older", 1, 0, "", "",
                 '{"code":"BadCode","severity":"warning"}'),
                ("malformed-worker", 10, 0, "", "worker-one",
                 '{"code":"identity_gap","severity":"warning"}'),
                ("malformed-revision", 20, 1, "", "",
                 '{"code":"revision_gap","severity":"warning"}')):
            attacked_path = self.root / f"{case}.db"
            with closing(sqlite3.connect(clone_path)) as original:
                with closing(sqlite3.connect(attacked_path)) as copied:
                    original.backup(copied)
            event_id = f"{case}-event"
            event_hash = hashlib.sha256((source_id + "\0" + event_id).encode()).hexdigest()
            record_digest = hashlib.sha256(event_id.encode()).hexdigest()
            with closing(sqlite3.connect(attacked_path)) as db:
                db.execute(
                    "INSERT INTO plugin_evidence_v1(source_id,event_id_hash,revision,kind,"
                    "time_ns,payload_json,record_digest,native_session_id,native_worker_id,"
                    "native_turn_id,native_operation_id,member_id,policy_epoch,admitted_ns) "
                    "VALUES(?,?,?,'diagnostic',8,?,?, '', ?, '', '', ?,1,?)",
                    (source_id, event_hash, revision, payload_json, record_digest, worker_id,
                     member_id, admitted_ns))
                db.commit()
            before_bad_read = logical_clone_snapshot(attacked_path)
            failure = self.apply_clone_diagnostic_read(
                read_request(renewed, "9" * 32 if case == "malformed-latest" else "0" * 32),
                path=attacked_path, success=False)
            self.assertIn("stored source diagnostic is invalid", failure)
            self.assertEqual(logical_clone_snapshot(attacked_path), before_bad_read)

        disable = self.clone_capture_request(
            renewed, enabled=False, expected_enabled=True, cursor="diag-two",
            dirty_seq=2, request_id="3" * 32)
        disabled = self.apply_clone_policy(disable, path=clone_path)["result"]
        self.assertEqual(disabled["disposition"], "committed")
        self.assertIsNone(rust_read(disabled["lease"], "4" * 32))
        self.assertEqual(set_source_capture(oracle, source_id=source_id, enabled=False), 2)
        python_disabled_lease = type(python_lease)(
            python_lease.receiver_id, python_lease.plugin_id, python_lease.source_id,
            python_lease.generation, python_lease.epoch, python_lease.cursor, 2,
            python_lease.content_fingerprint)
        self.assertEqual(last_source_diagnostic(oracle, python_disabled_lease), {})

        enable = self.clone_capture_request(
            disabled["lease"], enabled=True, expected_enabled=False, cursor="diag-two",
            dirty_seq=3, request_id="5" * 32)
        enabled = self.apply_clone_policy(enable, path=clone_path)["result"]
        self.assertEqual(enabled["disposition"], "committed")
        self.assertIsNone(rust_read(enabled["lease"], "6" * 32))
        self.assertEqual(set_source_capture(oracle, source_id=source_id, enabled=True), 3)
        python_enabled_lease = type(python_lease)(
            python_lease.receiver_id, python_lease.plugin_id, python_lease.source_id,
            python_lease.generation, python_lease.epoch, python_lease.cursor, 3,
            python_lease.content_fingerprint)
        self.assertEqual(last_source_diagnostic(oracle, python_enabled_lease), {})

        # A fresh but stale-policy diagnostic must not become visible after re-enable.
        stale_hash = hashlib.sha256((source_id + "\0stale-diagnostic").encode()).hexdigest()
        stale_record = {"code": "stale_policy", "severity": "error"}
        stale_json = encode_message(stale_record)
        stale_digest = hashlib.sha256(encode_message({
            "event_id": "stale-diagnostic", "revision": 0, "kind": "diagnostic",
            "time_ns": "8", "payload": stale_record,
        }).encode()).hexdigest()
        for db_path in (clone_path, oracle_path):
            with closing(sqlite3.connect(db_path)) as db:
                db.execute("INSERT INTO plugin_evidence_v1(source_id,event_id_hash,revision,kind,"
                           "time_ns,payload_json,record_digest,policy_epoch,admitted_ns) "
                           "VALUES(?,?,0,'diagnostic',8,?,?,1,999999)",
                           (source_id, stale_hash, stale_json, stale_digest))
                db.commit()
        self.assertIsNone(rust_read(enabled["lease"], "7" * 32))
        self.assertEqual(last_source_diagnostic(oracle, python_enabled_lease), {})

        # All diagnostic evidence remains non-projecting, and the source of truth
        # stayed untouched by every operation against its detached clone.
        final = self.batch_source_snapshot(clone_path, source_id)
        self.assertEqual(final[4], ())
        self.assertEqual(final[3], ())
        self.assertEqual(canonical_file_state(), canonical_before)

        for invalid_payload, invalid_identity in (
                ({"code": "bad_identity", "severity": "info"}, "session-one"),
                ({"code": "possible_source_gap", "severity": "warning"}, ""),
                ({"code": "bad_severity", "severity": "fatal"}, ""),
                ({"code": "BadCode", "severity": "warning"}, ""),
                ({"code": "free_text", "severity": "warning", "message": "private"}, ""),
        ):
            valid = self.mixed_batch_request(
                diagnostic_batch("invalid-input", "invalid-cursor"),
                enabled["lease"], "8" * 32, "diag-two")
            invalid = json.loads(json.dumps(valid))
            invalid["operation"]["records"][0]["payload_json"] = encode_message(
                invalid_payload)
            invalid["operation"]["records"][0]["native_session_id"] = invalid_identity
            invalid_before = self.clone_policy_state_snapshot(clone_path, source_id)
            invalid_receipts = self.admission_receipts_snapshot(clone_path, source_id)
            invalid_binding = self.batch_binding_snapshot(clone_path, source_id)
            self.apply_mixed_clone_batch(invalid, path=clone_path, success=False)
            self.assertEqual(self.clone_policy_state_snapshot(clone_path, source_id),
                             invalid_before)
            self.assertEqual(self.admission_receipts_snapshot(clone_path, source_id),
                             invalid_receipts)
            self.assertEqual(self.batch_binding_snapshot(clone_path, source_id),
                             invalid_binding)
        self.assertEqual(canonical_file_state(), canonical_before)

    def test_v1_binding_migrates_only_in_clone_fixture_and_replays(self):
        from observatory.plugin_admission import PollBatch, activate_source, commit_batch

        source_id = "member-migration-source"
        generation = "e" * 64
        canonical_path = self.root / "member-migration-canonical.db"
        clone_path = self.root / "member-migration-clone.db"
        failed_path = self.root / "member-migration-failure.db"
        oracle_path = self.root / "member-migration-oracle.db"
        canonical = Store(str(canonical_path))
        receiver_id = canonical.receiver_id
        activate_source(canonical, plugin_id=PLUGIN, source_id="clone-seed",
                        generation=GENERATION)
        canonical.close()
        with closing(sqlite3.connect(canonical_path)) as original:
            for target in (clone_path, failed_path, oracle_path):
                with closing(sqlite3.connect(target)) as copied:
                    original.backup(copied)
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                       ("storage_activation_clone_v1", "enabled"))
            db.commit()
        with closing(sqlite3.connect(failed_path)) as db:
            db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                       ("storage_activation_clone_v1", "enabled"))
            db.commit()
        oracle = Store(str(oracle_path))
        self.addCleanup(oracle.close)
        activation = {"version": 2, "request_id": "1" * 32,
                      "receiver_id": receiver_id,
                      "operation": {"op": "activate_source", "plugin_id": PLUGIN,
                                    "source_id": source_id, "generation": generation}}
        lease = self.apply_activation_clone_v2(
            activation, path=clone_path)["result"]["lease"]
        self.apply_activation_clone_v2(activation, path=failed_path)
        python_lease = activate_source(oracle, plugin_id=PLUGIN, source_id=source_id,
                                       generation=generation)

        initial = PollBatch(source_id, "migration-one", ({
            "event_id": "migration-event", "revision": 0, "kind": "usage",
            "time_ns": "101", "payload": {"input_tokens": 4},
        },))
        mixed = self.mixed_batch_request(initial, lease, "2" * 32, "")
        facts = json.loads(json.dumps(mixed))
        facts["request_id"] = "3" * 32
        facts["operation"]["op"] = "admit_facts"
        result = self.apply_activation_clone_v2(facts, path=clone_path)
        self.assertEqual(result["result"]["disposition"], "committed")
        self.assertEqual(commit_batch(oracle, python_lease, "", initial), initial.cursor)
        with closing(sqlite3.connect(clone_path)) as db:
            legacy_columns = tuple(row[1] for row in db.execute(
                "PRAGMA table_info(storage_admission_batch_binding_v1)"))
            self.assertEqual(len(legacy_columns), 8)
            v1_digest = db.execute(
                "SELECT prepared_digest FROM storage_admission_batch_binding_v1 "
                "WHERE source_id=?", (source_id,)).fetchone()[0]

        # A failed late-row admission must roll back the ALTER itself as well
        # as the first row inserted in that transaction.
        failure_request = self.mixed_batch_request(PollBatch(
            source_id, "migration-two", (
                {"event_id": "migration-new", "revision": 0, "kind": "usage",
                 "time_ns": "102", "payload": {"output_tokens": 3}},
                {"event_id": "migration-event", "revision": 0, "kind": "usage",
                 "time_ns": "101", "payload": {"input_tokens": 9}},
            )), lease, "4" * 32, "migration-one")
        with closing(sqlite3.connect(clone_path)) as original:
            with closing(sqlite3.connect(failed_path)) as failed:
                original.backup(failed)
        before_failed_migration = self.batch_source_snapshot(failed_path, source_id)
        self.apply_mixed_clone_batch(failure_request, path=failed_path, success=False)
        self.assertEqual(self.batch_source_snapshot(failed_path, source_id),
                         before_failed_migration)
        with closing(sqlite3.connect(failed_path)) as db:
            self.assertEqual(len(tuple(row[1] for row in db.execute(
                "PRAGMA table_info(storage_admission_batch_binding_v1)"))), 8)

        # V1 no-member digest remains usable after the marked fixture performs
        # its transactional 8→11 column migration.
        replay = self.apply_mixed_clone_batch(mixed, path=clone_path)["result"]
        self.assertEqual(replay["disposition"], "replay")
        with closing(sqlite3.connect(clone_path)) as db:
            columns = tuple(row[1] for row in db.execute(
                "PRAGMA table_info(storage_admission_batch_binding_v1)"))
            self.assertEqual(len(columns), 11)
            binding = db.execute(
                "SELECT prepared_version,member_id,member_cursor,prepared_digest "
                "FROM storage_admission_batch_binding_v1 WHERE source_id=?",
                (source_id,)).fetchone()
            self.assertEqual(binding, (1, "", "", v1_digest))
        migrated_snapshot = self.batch_source_snapshot(clone_path, source_id)
        replay_again = self.apply_mixed_clone_batch(mixed, path=clone_path)["result"]
        self.assertEqual(replay_again["disposition"], "replay")
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id),
                         migrated_snapshot)

    def test_member_cursor_keeps_legacy_session_owner_ambiguous(self):
        from observatory.plugin_admission import PollBatch, activate_source, commit_batch

        source_id = "member-legacy-source"
        generation = "e" * 64
        canonical_path = self.root / "member-legacy-canonical.db"
        clone_path = self.root / "member-legacy-clone.db"
        oracle_path = self.root / "member-legacy-oracle.db"
        canonical = Store(str(canonical_path))
        receiver_id = canonical.receiver_id
        activate_source(canonical, plugin_id=PLUGIN, source_id="clone-seed",
                        generation=GENERATION)
        canonical.close()
        with closing(sqlite3.connect(canonical_path)) as original:
            for target in (clone_path, oracle_path):
                with closing(sqlite3.connect(target)) as copied:
                    original.backup(copied)
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                       ("storage_activation_clone_v1", "enabled"))
            db.commit()
        oracle = Store(str(oracle_path))
        self.addCleanup(oracle.close)
        activation = {"version": 2, "request_id": "1" * 32,
                      "receiver_id": receiver_id,
                      "operation": {"op": "activate_source", "plugin_id": PLUGIN,
                                    "source_id": source_id, "generation": generation}}
        rust_lease = self.apply_activation_clone_v2(
            activation, path=clone_path)["result"]["lease"]
        python_lease = activate_source(oracle, plugin_id=PLUGIN, source_id=source_id,
                                       generation=generation)
        legacy = PollBatch(source_id, "legacy-one", ({
            "event_id": "legacy-event", "revision": 0, "kind": "lifecycle",
            "time_ns": "100", "payload": {"state": "started"},
            "identity": {"session_id": "legacy-session"},
        },))
        legacy_req = self.mixed_batch_request(legacy, rust_lease, "2" * 32, "")
        self.apply_mixed_clone_batch(legacy_req, path=clone_path)
        commit_batch(oracle, python_lease, "", legacy)
        next_batch = PollBatch(source_id, "legacy-two", ({
            "event_id": "member-event", "revision": 0, "kind": "usage",
            "time_ns": "101", "payload": {"input_tokens": 5},
            "identity": {"session_id": "legacy-session"},
        },), "a" * 64, "member-file-two")
        member_req = self.mixed_batch_request(
            next_batch, rust_lease, "3" * 32, "legacy-one")
        self.apply_mixed_clone_batch(member_req, path=clone_path)
        commit_batch(oracle, python_lease, "legacy-one", next_batch)
        rust = self.batch_source_snapshot(clone_path, source_id)
        python = self.batch_source_snapshot(oracle, source_id)
        self.assertEqual(rust[1:], python[1:])
        self.assertEqual(rust[6], (("a" * 64, "member-file-two"),))
        self.assertEqual(rust[7], (("legacy-session", ""),))
        member_hash = hashlib.sha256(
            (source_id + "\0member-event").encode()).hexdigest()
        self.assertEqual(next(row for row in rust[1] if row[0] == member_hash)[10], "")

    def test_member_cursor_mixed_batches_match_python_and_stay_clone_only(self):
        from observatory.plugin_admission import (
            PollBatch, SourceLease, activate_source, commit_batch,
            last_source_diagnostic, member_cursors, set_source_capture,
        )

        source_id = "member-cursor-source"
        generation = "e" * 64
        canonical_path = self.root / "member-canonical.db"
        clone_path = self.root / "member-clone.db"
        oracle_path = self.root / "member-oracle.db"
        canonical = Store(str(canonical_path))
        receiver_id = canonical.receiver_id
        activate_source(canonical, plugin_id=PLUGIN, source_id="clone-seed",
                        generation=GENERATION)
        canonical.close()
        with closing(sqlite3.connect(canonical_path)) as original:
            for target in (clone_path, oracle_path):
                with closing(sqlite3.connect(target)) as copied:
                    original.backup(copied)
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                       ("storage_activation_clone_v1", "enabled"))
            db.commit()
        oracle = Store(str(oracle_path))
        self.addCleanup(oracle.close)

        def canonical_files():
            return tuple((path.stat().st_ino, path.stat().st_mtime_ns,
                          hashlib.sha256(path.read_bytes()).hexdigest()) if path.exists() else None
                         for path in (canonical_path, Path(f"{canonical_path}-wal"),
                                      Path(f"{canonical_path}-shm")))

        def logical_snapshot(path):
            with closing(sqlite3.connect(path)) as db:
                schema = tuple(tuple(row) for row in db.execute(
                    "SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name"))
                tables = [row[0] for row in db.execute(
                    "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")]
                rows = []
                for name in tables:
                    quoted = '"' + name.replace('"', '""') + '"'
                    values = tuple(sorted((tuple(row) for row in db.execute(
                        f"SELECT * FROM {quoted}")), key=repr))
                    rows.append((name, values))
                return schema, tuple(rows)

        before_canonical = canonical_files()
        activation = {"version": 2, "request_id": "1" * 32,
                      "receiver_id": receiver_id,
                      "operation": {"op": "activate_source", "plugin_id": PLUGIN,
                                    "source_id": source_id, "generation": generation}}
        rust_lease = self.apply_activation_clone_v2(
            activation, path=clone_path)["result"]["lease"]
        python_lease = activate_source(oracle, plugin_id=PLUGIN, source_id=source_id,
                                       generation=generation)

        def parity():
            rust = self.batch_source_snapshot(clone_path, source_id)
            python = self.batch_source_snapshot(oracle, source_id)
            self.assertEqual(rust[0][:7] + rust[0][8:],
                             python[0][:7] + python[0][8:])
            self.assertEqual(rust[1:8], python[1:8])
            self.assertEqual(tuple(row[0] for row in rust[8]),
                             tuple(row[0] for row in python[8]))

        class CloneProjectionStore:
            def __init__(self, path):
                self.path = str(path)

            @contextmanager
            def _connect(self):
                db = sqlite3.connect(self.path, timeout=10)
                db.row_factory = sqlite3.Row
                try:
                    yield db
                    db.commit()
                except BaseException:
                    db.rollback()
                    raise
                finally:
                    db.close()

            def _source_write(self):
                return nullcontext()

            def receiver_namespace(self):
                with self._connect() as db:
                    row = db.execute(
                        "SELECT value FROM schema_meta WHERE key='receiver_id'").fetchone()
                    return row[0]

        clone_projection_store = CloneProjectionStore(clone_path)

        def mirror_projection_to_clone():
            from observatory.plugin_projection import project_source
            oracle_projected = project_source(oracle, source_id)
            clone_projected = project_source(clone_projection_store, source_id)
            self.assertEqual(oracle_projected, clone_projected,
                             (self.batch_source_snapshot(oracle, source_id)[0],
                              self.batch_source_snapshot(clone_path, source_id)[0]))

        def batch(cursor, member, checkpoint, records=()):
            return PollBatch(source_id, cursor, tuple(records), member, checkpoint)

        member_one = "a" * 64
        first = batch("member-global-one", member_one, "file-checkpoint-one", (
            {"event_id": "member-life", "revision": 0, "kind": "lifecycle",
             "time_ns": "101", "payload": {"state": "started"},
             "identity": {"session_id": "member-session"}},
            {"event_id": "member-usage", "revision": 0, "kind": "usage",
             "time_ns": "102", "payload": {"input_tokens": 11}},
            {"event_id": "member-diagnostic", "revision": 0, "kind": "diagnostic",
             "time_ns": "103", "payload": {"code": "source_timeout",
                                                "severity": "warning"}},
        ))
        request = self.mixed_batch_request(first, rust_lease, "2" * 32, "")
        other_activation = {"version": 2, "request_id": "d" * 32,
                            "receiver_id": receiver_id,
                            "operation": {"op": "activate_source", "plugin_id": PLUGIN,
                                          "source_id": "member-trigger-target",
                                          "generation": "f" * 64}}
        self.apply_activation_clone_v2(other_activation, path=clone_path)
        for table in ("plugin_session_member_v1", "plugin_source_member_cursor_v1"):
            trigger = f"member_cross_source_{table}"
            with closing(sqlite3.connect(clone_path)) as db:
                db.execute(
                    f"CREATE TRIGGER {trigger} AFTER INSERT ON {table} BEGIN "
                    "UPDATE plugin_source_cursor_v1 SET cursor='trigger-write' "
                    "WHERE source_id='member-trigger-target'; END")
                db.commit()
            trigger_before = logical_snapshot(clone_path)
            self.assertIn("real clone admission write tables",
                          self.apply_mixed_clone_batch(request, path=clone_path,
                                                       success=False))
            self.assertEqual(logical_snapshot(clone_path), trigger_before)
            with closing(sqlite3.connect(clone_path)) as db:
                db.execute(f"DROP TRIGGER {trigger}")
                db.commit()
        before_normal = logical_snapshot(clone_path)
        self.assertIn("mixed admission is unavailable",
                      self.apply_activation_clone_v2(request, path=clone_path,
                                                     success=False))
        self.assertEqual(logical_snapshot(clone_path), before_normal)
        self.assertEqual(self.apply_mixed_clone_batch(
            request, path=clone_path)["result"]["disposition"], "committed")
        self.assertEqual(commit_batch(oracle, python_lease, "", first), first.cursor)
        parity()
        with closing(sqlite3.connect(clone_path)) as db:
            self.assertEqual(db.execute(
                "SELECT member_id,cursor FROM plugin_source_member_cursor_v1 "
                "WHERE source_id=?", (source_id,)).fetchall(),
                [(member_one, "file-checkpoint-one")])
            self.assertEqual(db.execute(
                "SELECT prepared_version,member_id,member_cursor FROM "
                "storage_admission_batch_binding_v1 WHERE source_id=?",
                (source_id,)).fetchone(), (2, member_one, "file-checkpoint-one"))

        # The next process reopens the same clone, exact replay is mutation-free,
        # and the read envelope is bound to the request without writes.
        replay_before = logical_snapshot(clone_path)
        self.assertEqual(self.apply_mixed_clone_batch(
            request, path=clone_path)["result"]["disposition"], "replay")
        self.assertEqual(logical_snapshot(clone_path), replay_before)
        read = self.clone_member_cursor_read_request(rust_lease, "3" * 32)
        read_before = logical_snapshot(clone_path)
        reply = self.apply_clone_member_cursor_read(read, path=clone_path)
        self.assertEqual(reply["result"]["cursors"], [
            {"member_id": member_one, "cursor": "file-checkpoint-one"}])
        self.assertEqual(reply["request_digest"], hashlib.sha256(json.dumps(
            read, ensure_ascii=False, sort_keys=True, separators=(",", ":")
        ).encode()).hexdigest())
        self.assertEqual(logical_snapshot(clone_path), read_before)
        normal_read_before = logical_snapshot(clone_path)
        self.apply_activation_clone_v2(read, path=clone_path, success=False)
        self.assertEqual(logical_snapshot(clone_path), normal_read_before)
        self.assertEqual(member_cursors(oracle, python_lease),
                         {member_one: "file-checkpoint-one"})
        self.assertEqual(last_source_diagnostic(oracle, python_lease),
                         {"code": "source_timeout", "severity": "warning",
                          "time_ns": "103"})

        malformed_read = self.clone_member_cursor_read_request(rust_lease, "f" * 32)
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("INSERT INTO plugin_source_member_cursor_v1 "
                       "(source_id,member_id,cursor) VALUES(?,?,?)",
                       (source_id, "bad-member", "bad-checkpoint"))
            db.commit()
        malformed_before = logical_snapshot(clone_path)
        self.apply_clone_member_cursor_read(
            malformed_read, path=clone_path, success=False)
        self.assertEqual(logical_snapshot(clone_path), malformed_before)
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("DELETE FROM plugin_source_member_cursor_v1 "
                       "WHERE source_id=? AND member_id='bad-member'", (source_id,))
            db.commit()

        dirty_before = self.batch_source_snapshot(clone_path, source_id)[0][6]
        member_two = "b" * 64
        member_only = batch("member-global-two", member_two, "file-checkpoint-two")
        member_only_req = self.mixed_batch_request(
            member_only, rust_lease, "4" * 32, "member-global-one")
        self.assertEqual(self.apply_mixed_clone_batch(
            member_only_req, path=clone_path)["result"]["dirty_seq"], dirty_before)
        self.assertEqual(commit_batch(oracle, python_lease, "member-global-one", member_only),
                         member_only.cursor)
        parity()

        for unsupported in (
            batch("member-global-three", member_one, ""),
        ):
            unsupported_req = self.mixed_batch_request(
                unsupported, rust_lease, "e" * 32, "member-global-two")
            unsupported_before = logical_snapshot(clone_path)
            self.apply_mixed_clone_batch(
                unsupported_req, path=clone_path, success=False)
            self.assertEqual(logical_snapshot(clone_path), unsupported_before)

        tampered = json.loads(json.dumps(member_only_req))
        tampered["request_id"] = "5" * 32
        tampered["operation"]["member_update"]["cursor"] = "changed-checkpoint"
        tamper_before = logical_snapshot(clone_path)
        self.apply_mixed_clone_batch(tampered, path=clone_path, success=False)
        self.assertEqual(logical_snapshot(clone_path), tamper_before)

        cross_member = batch("member-global-three", member_two, "new-checkpoint", ({
            "event_id": "cross-member", "revision": 0, "kind": "lifecycle",
            "time_ns": "104", "payload": {"state": "running"},
            "identity": {"session_id": "member-session"},
        },))
        cross_req = self.mixed_batch_request(
            cross_member, rust_lease, "6" * 32, "member-global-two")
        cross_before = logical_snapshot(clone_path)
        with self.assertRaises(ProtocolError):
            commit_batch(oracle, python_lease, "member-global-two", cross_member)
        self.apply_mixed_clone_batch(cross_req, path=clone_path, success=False)
        self.assertEqual(logical_snapshot(clone_path), cross_before)

        late_failure = batch("member-global-three", member_one, "late-checkpoint", (
            {"event_id": "late-new", "revision": 0, "kind": "lifecycle",
             "time_ns": "105", "payload": {"state": "running"},
             "identity": {"session_id": "late-session"}},
            {"event_id": "member-life", "revision": 0, "kind": "lifecycle",
             "time_ns": "101", "payload": {"state": "failed"},
             "identity": {"session_id": "member-session"}},
        ))
        late_req = self.mixed_batch_request(
            late_failure, rust_lease, "7" * 32, "member-global-two")
        late_before = logical_snapshot(clone_path)
        with self.assertRaises(ProtocolError):
            commit_batch(oracle, python_lease, "member-global-two", late_failure)
        self.apply_mixed_clone_batch(late_req, path=clone_path, success=False)
        self.assertEqual(logical_snapshot(clone_path), late_before)

        # Cursor-only member updates count toward the actual 48-member bound;
        # the 49th fails before any state or receipt is written.
        for index in range(1, 47):
            member = f"{index:064x}"
            next_global = f"member-global-cap-{index}"
            cap_batch = batch(next_global, member, f"member-checkpoint-{index}")
            expected = "member-global-two" if index == 1 else f"member-global-cap-{index - 1}"
            cap_req = self.mixed_batch_request(
                cap_batch, rust_lease, f"{index + 16:032x}", expected)
            self.apply_mixed_clone_batch(cap_req, path=clone_path)
            commit_batch(oracle, python_lease, expected, cap_batch)
        self.assertEqual(len(member_cursors(oracle, python_lease)), 48)
        at_limit = logical_snapshot(clone_path)
        over_limit = batch("member-global-over-limit", f"{47:064x}", "too-many")
        over_req = self.mixed_batch_request(
            over_limit, rust_lease, "8" * 32, "member-global-cap-46")
        with self.assertRaises(ProtocolError):
            commit_batch(oracle, python_lease, "member-global-cap-46", over_limit)
        self.apply_mixed_clone_batch(over_req, path=clone_path, success=False)
        self.assertEqual(logical_snapshot(clone_path), at_limit)

        sibling_batch = batch("member-global-sibling", member_two,
                              "file-checkpoint-two-updated", ({
            "event_id": "sibling-life", "revision": 0, "kind": "lifecycle",
            "time_ns": "108", "payload": {"state": "completed"},
            "identity": {"session_id": "sibling-session"},
        },))
        sibling_req = self.mixed_batch_request(
            sibling_batch, rust_lease, "9" * 32, "member-global-cap-46")
        self.apply_mixed_clone_batch(sibling_req, path=clone_path)
        commit_batch(oracle, python_lease, "member-global-cap-46", sibling_batch)
        parity()
        mirror_projection_to_clone()
        before_gap_projection = self.batch_source_snapshot(clone_path, source_id)[3]
        self.assertTrue(any(row[0] == "sibling-session" for row in before_gap_projection),
                        before_gap_projection)
        sibling_before = next(row for row in before_gap_projection
                              if row[0] == "sibling-session")
        gap_batch = batch("member-global-gap", member_one, "", ({
            "event_id": "member-one-gap", "revision": 0, "kind": "diagnostic",
            "time_ns": "109", "payload": {"code": "possible_source_gap",
                                               "severity": "warning"},
        },))
        gap_req = self.mixed_batch_request(
            gap_batch, rust_lease, "c" * 32, "member-global-sibling")
        stale_gap = json.loads(json.dumps(gap_req))
        stale_gap["operation"]["expected_cursor"] = "stale-member-global"
        stale_before = logical_snapshot(clone_path)
        self.apply_mixed_clone_batch(stale_gap, path=clone_path, success=False)
        self.assertEqual(logical_snapshot(clone_path), stale_before)

        for malformed_records in (
            list(gap_req["operation"]["records"]) * 2,
            list(gap_req["operation"]["records"]) + [{
                **gap_req["operation"]["records"][0],
                "event_id_hash": hashlib.sha256(b"extra-gap-record").hexdigest(),
                "record_digest": hashlib.sha256(b"extra-gap-record").hexdigest(),
                "payload_json": '{"code":"source_timeout","severity":"warning"}',
            }],
        ):
            malformed = json.loads(json.dumps(gap_req))
            malformed["request_id"] = "a" * 32
            malformed["operation"]["records"] = malformed_records
            malformed_before = logical_snapshot(clone_path)
            self.apply_mixed_clone_batch(malformed, path=clone_path, success=False)
            self.assertEqual(logical_snapshot(clone_path), malformed_before)

        with closing(sqlite3.connect(clone_path)) as db:
            db.execute(
                "CREATE TRIGGER gap_cross_source AFTER INSERT ON plugin_source_gap_invalidated_v1 "
                "BEGIN UPDATE plugin_source_cursor_v1 SET cursor='trigger-write' "
                "WHERE source_id='member-trigger-target'; END")
            db.commit()
        trigger_before = logical_snapshot(clone_path)
        self.assertIn("real clone admission write tables",
                      self.apply_mixed_clone_batch(gap_req, path=clone_path, success=False))
        self.assertEqual(logical_snapshot(clone_path), trigger_before)
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("DROP TRIGGER gap_cross_source")
            db.commit()

        # A late receipt constraint failure occurs after evidence, cursor and
        # tombstone writes have begun; SQLite must roll back the whole batch.
        failed_gap_clone = self.root / "gap-late-failure.db"
        with closing(sqlite3.connect(clone_path)) as src, closing(sqlite3.connect(failed_gap_clone)) as dst:
            src.backup(dst)
        with closing(sqlite3.connect(failed_gap_clone)) as db:
            db.execute("PRAGMA foreign_keys=OFF")
            db.execute("ALTER TABLE storage_admission_request_v2 RENAME TO old_request_receipts")
            db.execute("CREATE TABLE storage_admission_request_v2("
                       "request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,"
                       "request_digest TEXT NOT NULL,source_id TEXT NOT NULL,cursor TEXT NOT NULL,"
                       "batch_digest TEXT NOT NULL,dirty_seq INTEGER NOT NULL,"
                       "disposition TEXT NOT NULL CHECK(request_id<>'" + "c" * 32 + "'))")
            db.execute("INSERT INTO storage_admission_request_v2 SELECT * FROM old_request_receipts")
            db.execute("DROP TABLE old_request_receipts")
            db.commit()
        failure_before = logical_snapshot(failed_gap_clone)
        self.apply_mixed_clone_batch(gap_req, path=failed_gap_clone, success=False)
        self.assertEqual(logical_snapshot(failed_gap_clone), failure_before)

        corrupt_clone = self.root / "gap-missing-owner.db"
        with closing(sqlite3.connect(clone_path)) as src, closing(sqlite3.connect(corrupt_clone)) as dst:
            src.backup(dst)
        with closing(sqlite3.connect(corrupt_clone)) as db:
            db.execute("DELETE FROM plugin_session_member_v1 WHERE source_id=? AND native_session_id='member-session'",
                       (source_id,))
            db.commit()
        corrupt_before = logical_snapshot(corrupt_clone)
        self.apply_mixed_clone_batch(gap_req, path=corrupt_clone, success=False)
        self.assertEqual(logical_snapshot(corrupt_clone), corrupt_before)
        ambiguous_clone = self.root / "gap-ambiguous-owner.db"
        with closing(sqlite3.connect(clone_path)) as src, \
                closing(sqlite3.connect(ambiguous_clone)) as dst:
            src.backup(dst)
        with closing(sqlite3.connect(ambiguous_clone)) as db:
            db.execute("UPDATE plugin_session_member_v1 SET member_id='' "
                       "WHERE source_id=? AND native_session_id='member-session'",
                       (source_id,))
            db.commit()
        ambiguous_before = logical_snapshot(ambiguous_clone)
        self.apply_mixed_clone_batch(gap_req, path=ambiguous_clone, success=False)
        self.assertEqual(logical_snapshot(ambiguous_clone), ambiguous_before)

        service_before = logical_snapshot(clone_path)
        self.apply_activation_clone_v2(gap_req, path=clone_path, success=False)
        self.assertEqual(logical_snapshot(clone_path), service_before)
        self.assertEqual(self.apply_mixed_clone_batch(
            gap_req, path=clone_path)["result"]["disposition"], "committed")
        commit_batch(oracle, python_lease, "member-global-sibling", gap_batch)
        parity()
        member_cursors_after_gap = dict(self.batch_source_snapshot(clone_path, source_id)[6])
        self.assertNotIn(member_one, member_cursors_after_gap)
        self.assertEqual(member_cursors_after_gap[member_two], "file-checkpoint-two-updated")
        gap_rows = self.batch_source_snapshot(clone_path, source_id)[8]
        self.assertEqual(tuple(row[0] for row in gap_rows), ("member-session",))
        self.assertGreaterEqual(gap_rows[0][1], 109)

        # Exercise the projector's synthetic tombstone path from a durable
        # owner+tombstone with no retained evidence, then validate read/admit.
        with oracle._connect() as db:
            db.execute("INSERT INTO plugin_session_member_v1(source_id,native_session_id,member_id) VALUES(?,?,?)",
                       (source_id, "projection-only-session", member_one))
            db.execute("INSERT INTO plugin_source_gap_invalidated_v1(source_id,native_session_id,last_record_ns) VALUES(?,?,?)",
                       (source_id, "projection-only-session", 111))
            db.commit()
        with closing(sqlite3.connect(clone_path)) as db:
            db.execute("INSERT INTO plugin_session_member_v1(source_id,native_session_id,member_id) VALUES(?,?,?)",
                       (source_id, "projection-only-session", member_one))
            db.execute("INSERT INTO plugin_source_gap_invalidated_v1(source_id,native_session_id,last_record_ns) VALUES(?,?,?)",
                       (source_id, "projection-only-session", 111))
            db.commit()
        mirror_projection_to_clone()
        synthetic = next(row for row in self.batch_source_snapshot(clone_path, source_id)[3]
                         if row[0] == "projection-only-session")
        self.assertEqual((synthetic[2], synthetic[3]), ("unknown", "source_gap"))
        parity()
        replay_before = logical_snapshot(clone_path)
        self.assertEqual(self.apply_mixed_clone_batch(
            gap_req, path=clone_path)["result"]["disposition"], "replay")
        self.assertEqual(logical_snapshot(clone_path), replay_before)
        conflict = json.loads(json.dumps(gap_req))
        conflict["request_id"] = "c" * 32
        conflict["operation"]["records"][0]["record_digest"] = "0" * 64
        self.apply_mixed_clone_batch(conflict, path=clone_path, success=False)
        self.assertEqual(logical_snapshot(clone_path), replay_before)
        python_gap_diag = last_source_diagnostic(oracle, python_lease)
        self.assertEqual(python_gap_diag, {"code": "possible_source_gap",
                                           "severity": "warning", "time_ns": "109"})
        read_gap = self.clone_diagnostic_read_request(rust_lease, "d" * 32)
        read_before = logical_snapshot(clone_path)
        rust_gap_diag = self.apply_clone_diagnostic_read(read_gap, path=clone_path)[
            "result"]["diagnostic"]
        self.assertEqual(rust_gap_diag, python_gap_diag)
        self.assertEqual(logical_snapshot(clone_path), read_before)
        mirror_projection_to_clone()
        projected = self.batch_source_snapshot(clone_path, source_id)[3]
        gap_session = next(row for row in projected if row[0] == "member-session")
        sibling_after = next(row for row in projected if row[0] == "sibling-session")
        self.assertEqual((gap_session[2], gap_session[3]), ("unknown", "source_gap"))
        self.assertEqual(sibling_after[:6] + sibling_after[7:],
                         sibling_before[:6] + sibling_before[7:])
        parity()

        # Reappearing under the same stable member ID preserves old tombstones;
        # its prior session stays Unknown while a newly owned session projects.
        reappear = batch("member-global-reappear", member_one,
                         "file-checkpoint-reappeared", ({
            "event_id": "member-one-reappeared", "revision": 0, "kind": "lifecycle",
            "time_ns": "110", "payload": {"state": "started"},
            "identity": {"session_id": "member-new-session"},
        },))
        reappear_req = self.mixed_batch_request(
            reappear, rust_lease, "e" * 32, "member-global-gap")
        self.apply_mixed_clone_batch(reappear_req, path=clone_path)
        commit_batch(oracle, python_lease, "member-global-gap", reappear)
        parity()
        mirror_projection_to_clone()
        projected = self.batch_source_snapshot(clone_path, source_id)[3]
        self.assertEqual((next(row for row in projected if row[0] == "member-session")[2:4]),
                         ("unknown", "source_gap"))
        sibling_reappeared = next(row for row in projected if row[0] == "sibling-session")
        self.assertEqual(sibling_reappeared[:6] + sibling_reappeared[7:],
                         sibling_before[:6] + sibling_before[7:])
        parity()

        regap = batch("member-global-regap", member_one, "", ({
            "event_id": "member-one-regap", "revision": 0, "kind": "diagnostic",
            "time_ns": "112", "payload": {"code": "possible_source_gap",
                                               "severity": "warning"},
        },))
        regap_req = self.mixed_batch_request(
            regap, rust_lease, "f" * 32, "member-global-reappear")
        old_gap_time = dict((row[0], row[1]) for row in
                            self.batch_source_snapshot(clone_path, source_id)[8])["member-session"]
        self.apply_mixed_clone_batch(regap_req, path=clone_path)
        commit_batch(oracle, python_lease, "member-global-reappear", regap)
        parity()
        regap_rows = self.batch_source_snapshot(clone_path, source_id)[8]
        regap_sessions = {row[0] for row in regap_rows}
        self.assertTrue({"member-session", "projection-only-session",
                         "member-new-session"}.issubset(regap_sessions))
        self.assertGreaterEqual(dict(regap_rows)["member-session"], old_gap_time)
        self.assertNotIn(member_one,
                         dict(self.batch_source_snapshot(clone_path, source_id)[6]))
        mirror_projection_to_clone()
        regap_projection = self.batch_source_snapshot(clone_path, source_id)[3]
        self.assertEqual((next(row for row in regap_projection
                               if row[0] == "member-new-session")[2:4]),
                         ("unknown", "source_gap"))
        parity()

        # Revoke uses a separate clone-policy transaction and must erase every
        # gap/evidence/projection/owner row while preserving member cursors.
        revoke_clone = self.root / "gap-revoke-clone.db"
        revoke_oracle_path = self.root / "gap-revoke-oracle.db"
        with closing(sqlite3.connect(clone_path)) as src, \
                closing(sqlite3.connect(revoke_clone)) as dst, \
                closing(sqlite3.connect(revoke_oracle_path)) as py_dst:
            src.backup(dst)
            src.backup(py_dst)
        revoke_oracle = Store(str(revoke_oracle_path))
        self.addCleanup(revoke_oracle.close)
        with closing(sqlite3.connect(revoke_clone)) as db:
            revoke_cursor, revoke_dirty = db.execute(
                "SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 WHERE source_id=?",
                (source_id,)).fetchone()
        revoke = self.clone_revoke_request(
            rust_lease, cursor=revoke_cursor, dirty_seq=revoke_dirty,
            request_id="12" * 16)
        cursors_before_revoke = self.batch_source_snapshot(revoke_clone, source_id)[6]
        rust_revoked = self.apply_clone_policy(revoke, path=revoke_clone)["result"]
        set_source_capture(revoke_oracle, source_id=source_id, enabled=False)
        revoked_snapshot = self.batch_source_snapshot(revoke_clone, source_id)
        python_revoked_snapshot = self.batch_source_snapshot(revoke_oracle, source_id)
        self.assertEqual(revoked_snapshot[0][:7] + revoked_snapshot[0][8:],
                         python_revoked_snapshot[0][:7] + python_revoked_snapshot[0][8:])
        for index in (1, 3, 4, 8):
            self.assertEqual(revoked_snapshot[index], python_revoked_snapshot[index])
        self.assertEqual(revoked_snapshot[1], ())
        self.assertEqual(revoked_snapshot[3], ())
        self.assertEqual(revoked_snapshot[4], ())
        self.assertEqual(revoked_snapshot[7], ())
        self.assertEqual(revoked_snapshot[8], ())
        self.assertEqual(revoked_snapshot[6], cursors_before_revoke)
        self.assertEqual(self.batch_binding_snapshot(revoke_clone, source_id), ())
        self.assertFalse(rust_revoked["capture_enabled"])
        stale_after_revoke = batch("member-global-after-revoke", member_two,
                                   "stale-after-revoke")
        stale_revoke_req = self.mixed_batch_request(
            stale_after_revoke, rust_lease, "13" * 16, revoke_cursor)
        revoke_before_stale = logical_snapshot(revoke_clone)
        self.apply_mixed_clone_batch(stale_revoke_req, path=revoke_clone, success=False)
        self.assertEqual(logical_snapshot(revoke_clone), revoke_before_stale)

        with closing(sqlite3.connect(clone_path)) as db:
            current_cursor, dirty_seq = db.execute(
                "SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 WHERE source_id=?",
                (source_id,)).fetchone()
        disable = self.clone_capture_request(
            rust_lease, enabled=False, expected_enabled=True, cursor=current_cursor,
            dirty_seq=dirty_seq, request_id="a" * 32)
        self.apply_clone_policy(disable, path=clone_path)
        set_source_capture(oracle, source_id=source_id, enabled=False)
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id)[6],
                         self.batch_source_snapshot(oracle, source_id)[6])
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id)[1], ())
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id)[8], ())
        stale_batch = batch("member-global-stale", member_one, "stale-checkpoint")
        stale_request = self.mixed_batch_request(
            stale_batch, rust_lease, "14" * 16, current_cursor)
        stale_before = logical_snapshot(clone_path)
        self.assertIn("stale or unsupported source lease",
                      self.apply_mixed_clone_batch(stale_request, path=clone_path,
                                                   success=False))
        self.assertEqual(logical_snapshot(clone_path), stale_before)
        self.apply_clone_member_cursor_read(
            self.clone_member_cursor_read_request(rust_lease, "15" * 16),
            path=clone_path, success=False)
        disabled_lease = self.apply_clone_policy(disable, path=clone_path)["result"]["lease"]
        enable = self.clone_capture_request(
            disabled_lease, enabled=True, expected_enabled=False, cursor=current_cursor,
            dirty_seq=dirty_seq + 1, request_id="b" * 32)
        enabled = self.apply_clone_policy(enable, path=clone_path)["result"]
        set_source_capture(oracle, source_id=source_id, enabled=True)
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id)[6],
                         self.batch_source_snapshot(oracle, source_id)[6])
        with oracle._connect() as db:
            row = db.execute(
                "SELECT receiver_id,plugin_id,source_id,generation,lease_epoch,cursor,"
                "policy_epoch,content_fingerprint FROM plugin_source_cursor_v1 "
                "WHERE source_id=?", (source_id,)).fetchone()
        python_enabled = SourceLease(*row)
        resumed = batch("member-global-resumed", member_one,
                        "file-checkpoint-resumed", ({
            "event_id": "after-resume", "revision": 0, "kind": "usage",
            "time_ns": "106", "payload": {"output_tokens": 9},
        },))
        resume_req = self.mixed_batch_request(
            resumed, enabled["lease"], "16" * 16, current_cursor)
        self.apply_mixed_clone_batch(resume_req, path=clone_path)
        commit_batch(oracle, python_enabled, current_cursor, resumed)
        parity()

        # Explicit revoke fences the lease, removes member ownership/evidence,
        # and retains per-member checkpoints. Re-enabling must not resurrect
        # any erased evidence.
        with closing(sqlite3.connect(clone_path)) as db:
            revoke_cursor, revoke_dirty = db.execute(
                "SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 WHERE source_id=?",
                (source_id,)).fetchone()
        revoke = self.clone_revoke_request(
            enabled["lease"], cursor=revoke_cursor, dirty_seq=revoke_dirty,
            request_id="17" * 16)
        revoked = self.apply_clone_policy(revoke, path=clone_path)["result"]
        set_source_capture(oracle, source_id=source_id, enabled=False)
        revoked_state = self.batch_source_snapshot(clone_path, source_id)
        oracle_revoked = self.batch_source_snapshot(oracle, source_id)
        self.assertEqual(revoked_state[1], ())
        self.assertEqual(revoked_state[7], ())
        self.assertEqual(revoked_state[6], oracle_revoked[6])
        self.assertEqual(revoked_state[0][:7] + revoked_state[0][8:],
                         oracle_revoked[0][:7] + oracle_revoked[0][8:])
        revoked_lease = dict(enabled["lease"])
        revoked_lease["epoch"] = revoked["lease_epoch"]
        revoked_lease["policy_epoch"] = revoked["policy_epoch"]
        reenable = self.clone_capture_request(
            revoked_lease, enabled=True, expected_enabled=False, cursor=revoke_cursor,
            dirty_seq=revoke_dirty + 1, request_id="18" * 16)
        reenabling = self.apply_clone_policy(reenable, path=clone_path)["result"]
        set_source_capture(oracle, source_id=source_id, enabled=True)
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id)[1], ())
        self.assertEqual(self.batch_source_snapshot(clone_path, source_id)[6],
                         self.batch_source_snapshot(oracle, source_id)[6])
        self.assertEqual(len(revoked["cursor"]), len(revoke_cursor))
        self.assertEqual(len(reenabling["lease"]["generation"]), 64)
        self.assertEqual(canonical_files(), before_canonical)


if __name__ == "__main__":
    unittest.main()
