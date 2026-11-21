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
from contextlib import closing
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import (_validated, activate_source, commit_batch,
                                          set_source_capture)
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
        return {"version": 2, "request_id": request_id,
                "receiver_id": lease["receiver_id"],
                "operation": {"op": "admit_batch", "lease": operation_lease,
                              "expected_cursor": expected_cursor, "cursor": batch.cursor,
                              "batch_digest": digest, "records": records}}

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
            return source, evidence, dirty, projection, activity, projected

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


if __name__ == "__main__":
    unittest.main()
