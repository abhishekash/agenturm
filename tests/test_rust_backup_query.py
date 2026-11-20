"""Installed Rust exact query over a completed, isolated Python Store backup."""

import hashlib
import json
import os
import sqlite3
import subprocess
import tempfile
import unittest
from contextlib import closing
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
from observatory.plugin_projection import project_source
from observatory.plugin_wire import PollBatch, ProtocolError
from observatory.rust_backup_attestation import MAX_RESULT_BYTES, attest_exact
from observatory.rust_shadow_snapshot import capture, current


SOURCE = "backup-query-source"


class RustBackupQueryTests(unittest.TestCase):
    def setUp(self):
        binary = os.environ.get("AGENTURM_RUST_BACKUP_QUERY_BIN")
        if not binary:
            self.skipTest("installed Rust backup query binary required")
        self.binary = str(Path(binary).resolve())
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.store = Store(self.root / "store.db")
        self.addCleanup(self.store.close)
        self.backup_index = 0

    def backup(self):
        self.backup_index += 1
        path = self.root / f"query-{self.backup_index}.db"
        fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        os.close(fd)
        with self.store._connect() as source, closing(sqlite3.connect(path)) as destination:
            source.backup(destination)
        with closing(sqlite3.connect(path)) as db:
            self.assertEqual(db.execute("PRAGMA journal_mode=DELETE").fetchone()[0], "delete")
        return path

    def run_query(self, path, kind, identity, *, source=SOURCE):
        query = {"kind": kind, "source_id": source,
                 "native_session_id" if kind == "session" else "event_id": identity}
        request = {"schema_version": 1, "sqlite_path": str(path),
                   "source_id": source, "query": query}
        return subprocess.run([self.binary], input=json.dumps(request), text=True,
                              capture_output=True, timeout=5)

    def result(self, path, kind, identity, *, source=SOURCE):
        process = self.run_query(path, kind, identity, source=source)
        self.assertEqual(process.returncode, 0, process.stderr)
        return json.loads(process.stdout)

    def ready(self):
        lease = activate_source(self.store, plugin_id="fixture.plugin",
                                source_id=SOURCE, generation="a" * 64)
        commit_batch(self.store, lease, "", PollBatch(SOURCE, "one", (
            {"event_id": "start", "revision": 0, "kind": "lifecycle",
             "time_ns": "10", "payload": {"state": "started"},
             "identity": {"session_id": "native-one"}},
            {"event_id": "message", "revision": 0, "kind": "message",
             "time_ns": "11", "payload": {"role": "user"},
             "identity": {"session_id": "native-one"}},
        )))
        project_source(self.store, SOURCE)
        return lease, capture(self.store, SOURCE), self.backup()

    @staticmethod
    def exact(kind, identity):
        return {"kind": kind, "source_id": SOURCE,
                "native_session_id" if kind == "session" else "event_id": identity}

    def attest(self, path, kind, identity):
        process = self.run_query(path, kind, identity)
        self.assertEqual(process.returncode, 0, process.stderr)
        return attest_exact(self.store, self.exact(kind, identity), process.stdout)

    def test_exact_attestation_matches_session_activity_and_absence(self):
        _, snapshot, copy = self.ready()
        self.assertEqual(self.attest(copy, "session", "native-one")["row"],
                         snapshot.expected["sessions"][0])
        activity = snapshot.expected["activities"][0]
        self.assertEqual(self.attest(copy, "activity", activity["id"])["row"], activity)
        missing = self.attest(copy, "session", "native-missing")
        self.assertEqual(missing["query"], self.exact("session", "native-missing"))
        self.assertEqual((missing["coverage"], missing["row"]), ("partial", None))
        self.assertEqual(missing["receiver_id"], snapshot.revision[0])
        self.assertEqual(missing["revision"]["dirty_seq"], str(snapshot.revision[3]))
        self.assertIsNone(self.attest(copy, "activity", "plugin-event:" + "0" * 64)["row"])

    def test_attestation_preserves_conservative_gap_projection(self):
        lease, _, _ = self.ready()
        commit_batch(self.store, lease, "one", PollBatch(SOURCE, "two", (
            {"event_id": "gap", "revision": 0, "kind": "diagnostic",
             "time_ns": "0", "payload": {"code": "possible_source_gap",
                                           "severity": "warning"}},)))
        with self.assertRaises(ProtocolError):
            self.attest(self.backup(), "session", "native-one")
        project_source(self.store, SOURCE)
        snapshot = capture(self.store, SOURCE)
        copy = self.backup()
        row = self.attest(copy, "session", "native-one")["row"]
        self.assertEqual(row, snapshot.expected["sessions"][0])
        self.assertEqual((row["reported_state"], row["reason_code"]),
                         ("unknown", "source_gap"))
        self.assertIsNone(self.attest(copy, "session", "native-missing")["row"])

    def test_attestation_rejects_valid_backup_tamper_and_stale_live_source(self):
        lease, snapshot, copy = self.ready()
        query = self.exact("session", "native-one")
        original = self.run_query(copy, "session", "native-one").stdout
        self.assertEqual(attest_exact(self.store, query, original)["row"],
                         snapshot.expected["sessions"][0])
        with closing(sqlite3.connect(copy)) as db:
            db.execute("UPDATE plugin_session_projection_v1 SET state='completed' "
                       "WHERE source_id=? AND native_session_id='native-one'", (SOURCE,))
            db.commit()
        forged = self.run_query(copy, "session", "native-one").stdout
        with self.assertRaises(ProtocolError):
            attest_exact(self.store, query, forged)
        self.assertTrue(current(self.store, snapshot))
        commit_batch(self.store, lease, "one", PollBatch(SOURCE, "two", (
            {"event_id": "pause", "revision": 0, "kind": "lifecycle",
             "time_ns": "12", "payload": {"state": "paused"},
             "identity": {"session_id": "native-one"}},)))
        with self.assertRaises(ProtocolError):
            attest_exact(self.store, query, original)
        project_source(self.store, SOURCE)
        fresh = self.backup()
        fresh_result = self.run_query(fresh, "session", "native-one").stdout
        set_source_capture(self.store, source_id=SOURCE, enabled=False)
        with self.assertRaises(ProtocolError):
            attest_exact(self.store, query, fresh_result)

    def test_attestation_rejects_malformed_or_unavailable_rust_results(self):
        _, _, copy = self.ready()
        query = self.exact("session", "native-one")
        good = self.result(copy, "session", "native-one")
        for change in (
            {"unexpected": 1}, {"coverage": "unavailable"}, {"state": "unknown"},
            {"source_id": "other"}, {"receiver_id": "other"},
            {"query": self.exact("session", "native-other")},
            {"activity": None}, {"session": {**good["session"], "state_time_ns": True}},
            {"revision": {**good["revision"], "dirty_seq": 1}},
        ):
            with self.subTest(change=change), self.assertRaises(ProtocolError):
                attest_exact(self.store, query, json.dumps({**good, **change}))
        malformed = (
            json.dumps(good)[:-1] + ',"state":"ready"}',
            json.dumps(good)[:-1] + ',"session":null}',
            '{"state":NaN}',
            '{"state":1e400}',
            " " * (MAX_RESULT_BYTES + 1),
        )
        for raw in malformed:
            with self.subTest(raw=raw[:24]), self.assertRaises(ProtocolError):
                attest_exact(self.store, query, raw)

    def test_attestation_rechecks_after_snapshot_before_return(self):
        _, snapshot, copy = self.ready()
        query = self.exact("session", "native-one")
        raw = self.run_query(copy, "session", "native-one").stdout
        actual_current = current

        def revoke_during_check(store, captured):
            self.assertEqual(captured.revision, snapshot.revision)
            set_source_capture(store, source_id=SOURCE, enabled=False)
            return actual_current(store, captured)

        with patch("observatory.rust_backup_attestation.current",
                   side_effect=revoke_during_check), self.assertRaises(ProtocolError):
            attest_exact(self.store, query, raw)

    def test_attestation_rejects_same_revision_store_file_replacement(self):
        _, snapshot, copy = self.ready()
        query = self.exact("session", "native-one")
        raw = self.run_query(copy, "session", "native-one").stdout
        replacement = self.backup()
        with closing(sqlite3.connect(replacement)) as db:
            db.execute("UPDATE plugin_session_projection_v1 SET state='completed' "
                       "WHERE source_id=? AND native_session_id='native-one'", (SOURCE,))
            db.commit()
        actual_current = current

        def replace_during_check(store, captured):
            self.assertEqual(captured.revision, snapshot.revision)
            os.replace(replacement, store.path)
            current_after_replacement = actual_current(store, captured)
            self.assertFalse(current_after_replacement)
            return current_after_replacement

        with patch("observatory.rust_backup_attestation.current",
                   side_effect=replace_during_check), self.assertRaises(ProtocolError):
            attest_exact(self.store, query, raw)

    def test_attestation_accepts_symlinked_store_and_rejects_retarget(self):
        _, snapshot, copy = self.ready()
        query = self.exact("session", "native-one")
        raw = self.run_query(copy, "session", "native-one").stdout
        alias_path = self.root / "store-alias.db"
        alias_path.symlink_to(Path(self.store.path).name)
        alias = Store(alias_path)
        self.addCleanup(alias.close)
        self.assertEqual(attest_exact(alias, query, raw)["row"],
                         snapshot.expected["sessions"][0])
        replacement = self.backup()
        actual_current = current

        def retarget_during_check(store, captured):
            alias_path.unlink()
            alias_path.symlink_to(replacement.name)
            self.assertFalse(actual_current(store, captured))
            return False

        with patch("observatory.rust_backup_attestation.current",
                   side_effect=retarget_during_check), self.assertRaises(ProtocolError):
            attest_exact(alias, query, raw)

    def test_ready_exact_query_is_historical_and_matches_python_projection(self):
        unknown = self.result(self.backup(), "session", "native-one")
        self.assertEqual((unknown["state"], unknown["coverage"], unknown["revision"]),
                         ("unknown", "unavailable", None))
        self.assertNotIn("session", unknown)
        lease = activate_source(self.store, plugin_id="fixture.plugin",
                                source_id=SOURCE, generation="a" * 64)
        pending = self.result(self.backup(), "session", "native-one")
        self.assertEqual((pending["state"], pending["coverage"]),
                         ("pending", "unavailable"))
        self.assertNotIn("session", pending)
        huge = 2**63 - 1
        records = (
            {"event_id": "start", "revision": 0, "kind": "lifecycle",
             "time_ns": "10", "payload": {"state": "started"},
             "identity": {"session_id": "native-one"}},
            {"event_id": "usage-a", "revision": 0, "kind": "usage",
             "time_ns": "11", "payload": {"input_tokens": huge},
             "identity": {"session_id": "native-one"}},
            {"event_id": "usage-b", "revision": 0, "kind": "usage",
             "time_ns": "12", "payload": {"input_tokens": huge},
             "identity": {"session_id": "native-one"}},
            {"event_id": "message", "revision": 0, "kind": "message",
             "time_ns": "13", "payload": {"role": "user"},
             "identity": {"session_id": "native-one"}},
        )
        commit_batch(self.store, lease, "", PollBatch(SOURCE, "one", records))
        self.assertEqual(self.result(self.backup(), "session", "native-one")
                         ["coverage"], "unavailable")
        project_source(self.store, SOURCE)
        snapshot = capture(self.store, SOURCE)
        self.assertTrue(current(self.store, snapshot))
        copy = self.backup()
        digest = hashlib.sha256(copy.read_bytes()).digest()
        before_files = {p.name for p in self.root.iterdir()}
        found = self.result(copy, "session", "native-one")
        self.assertEqual(found["session"], snapshot.expected["sessions"][0])
        self.assertEqual(found["session"]["metadata_usage"]["input_tokens"], str(2 * huge))
        self.assertEqual((found["state"], found["coverage"]), ("ready", "partial"))
        self.assertEqual(found["receiver_id"], snapshot.revision[0])
        self.assertEqual(found["revision"], {
            "lease_epoch": str(snapshot.revision[1]),
            "policy_epoch": str(snapshot.revision[2]),
            "dirty_seq": str(snapshot.revision[3]),
            "capture_enabled": True,
            "projected_seq": str(snapshot.revision[5]),
            "projected_policy": str(snapshot.revision[6]),
        })
        absent = self.result(copy, "session", "native-absent")
        self.assertIn("session", absent)
        self.assertIsNone(absent["session"])
        activity = snapshot.expected["activities"][0]
        self.assertEqual(self.result(copy, "activity", activity["id"])["activity"], activity)
        absent_activity = self.result(copy, "activity", "plugin-event:" + "0" * 64)
        self.assertIn("activity", absent_activity)
        self.assertIsNone(absent_activity["activity"])
        self.assertEqual(hashlib.sha256(copy.read_bytes()).digest(), digest)
        self.assertEqual({p.name for p in self.root.iterdir()}, before_files)
        wrong_source = self.result(copy, "session", "native-one", source="wrong-source")
        self.assertEqual((wrong_source["state"], wrong_source["coverage"]),
                         ("unknown", "unavailable"))
        commit_batch(self.store, lease, "one", PollBatch(SOURCE, "two", (
            {"event_id": "gap", "revision": 0, "kind": "diagnostic",
             "time_ns": "0", "payload": {"code": "possible_source_gap",
                                           "severity": "warning"}},)))
        self.assertFalse(current(self.store, snapshot))
        self.assertEqual(self.result(copy, "session", "native-one"), found)
        project_source(self.store, SOURCE)
        gapped = capture(self.store, SOURCE)
        self.assertTrue(current(self.store, gapped))
        gap_copy = self.backup()
        self.assertEqual(self.result(gap_copy, "session", "native-one")
                         ["session"], gapped.expected["sessions"][0])
        self.assertEqual(self.result(gap_copy, "session", "native-one")
                         ["session"]["reason_code"], "source_gap")
        self.assertIsNone(self.result(gap_copy, "activity", activity["id"])["activity"])
        set_source_capture(self.store, source_id=SOURCE, enabled=False)
        self.assertFalse(current(self.store, gapped))
        disabled = self.result(self.backup(), "session", "native-one")
        self.assertEqual((disabled["state"], disabled["coverage"]),
                         ("disabled", "unavailable"))
        self.assertNotIn("session", disabled)

    def test_malformed_queries_fail_closed(self):
        copy = self.backup()
        self.assertEqual(self.run_query(copy, "session", "bad id").returncode, 2)
        self.assertEqual(self.run_query(copy, "activity", "bad-id").returncode, 2)
        request = {"schema_version": 1, "sqlite_path": str(copy),
                   "source_id": SOURCE, "query": {"kind": "session",
                   "source_id": "other", "native_session_id": "native-one"}}
        process = subprocess.run([self.binary], input=json.dumps(request), text=True,
                                 capture_output=True, timeout=5)
        self.assertEqual((process.returncode, process.stdout), (2, ""))

    def test_revision_fence_does_not_authenticate_a_structurally_valid_backup(self):
        lease = activate_source(self.store, plugin_id="fixture.plugin",
                                source_id=SOURCE, generation="a" * 64)
        commit_batch(self.store, lease, "", PollBatch(SOURCE, "one", (
            {"event_id": "start", "revision": 0, "kind": "lifecycle",
             "time_ns": "10", "payload": {"state": "started"},
             "identity": {"session_id": "native-one"}},
        )))
        project_source(self.store, SOURCE)
        snapshot = capture(self.store, SOURCE)
        self.assertTrue(current(self.store, snapshot))
        altered = self.backup()
        with closing(sqlite3.connect(altered)) as db:
            db.execute("UPDATE plugin_session_projection_v1 SET state='completed' "
                       "WHERE source_id=? AND native_session_id='native-one'", (SOURCE,))
            db.commit()
        result = self.result(altered, "session", "native-one")
        self.assertEqual((result["state"], result["coverage"]), ("ready", "partial"))
        self.assertEqual(result["receiver_id"], snapshot.revision[0])
        self.assertEqual(result["revision"], {
            "lease_epoch": str(snapshot.revision[1]),
            "policy_epoch": str(snapshot.revision[2]),
            "dirty_seq": str(snapshot.revision[3]),
            "capture_enabled": True,
            "projected_seq": str(snapshot.revision[5]),
            "projected_policy": str(snapshot.revision[6]),
        })
        self.assertTrue(current(self.store, snapshot))
        self.assertEqual(result["session"]["reported_state"], "completed")
        self.assertNotEqual(result["session"], snapshot.expected["sessions"][0])

    def test_unrequested_corrupt_projection_does_not_block_exact_query(self):
        lease = activate_source(self.store, plugin_id="fixture.plugin",
                                source_id=SOURCE, generation="a" * 64)
        commit_batch(self.store, lease, "", PollBatch(SOURCE, "one", (
            {"event_id": "start", "revision": 0, "kind": "lifecycle",
             "time_ns": "10", "payload": {"state": "started"},
             "identity": {"session_id": "native-one"}},
            {"event_id": "message", "revision": 0, "kind": "message",
             "time_ns": "11", "payload": {"role": "user"},
             "identity": {"session_id": "native-one"}},
        )))
        project_source(self.store, SOURCE)
        snapshot = capture(self.store, SOURCE)
        activity_id = snapshot.expected["activities"][0]["id"]
        broken_activity = self.backup()
        with closing(sqlite3.connect(broken_activity)) as db:
            db.execute("UPDATE activity_index_v1 SET payload=? WHERE kind='plugin' "
                       "AND owner_key=?", ('{"prompt":"untrusted"}', SOURCE))
            db.commit()
        self.assertEqual(self.result(broken_activity, "session", "native-one")
                         ["session"], snapshot.expected["sessions"][0])
        self.assertEqual(self.run_query(broken_activity, "activity", activity_id).returncode, 2)
        broken_session = self.backup()
        with closing(sqlite3.connect(broken_session)) as db:
            db.execute("UPDATE plugin_session_projection_v1 SET usage_json=? "
                       "WHERE source_id=?", ('{"input_tokens":-1}', SOURCE))
            db.commit()
        self.assertEqual(self.result(broken_session, "activity", activity_id)
                         ["activity"], snapshot.expected["activities"][0])
        self.assertEqual(self.run_query(broken_session, "session", "native-one")
                         .returncode, 2)


if __name__ == "__main__":
    unittest.main()
