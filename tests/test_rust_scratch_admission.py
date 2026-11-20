"""Feature-off Rust scratch transaction against disposable Python Store fixtures."""
import json
import hashlib
import os
import select
import stat
import subprocess
import sys
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
