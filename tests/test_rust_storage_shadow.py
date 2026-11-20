"""Installed Rust shadow reader over isolated, consistent Python Store backups."""
import hashlib
import json
import os
import sqlite3
import subprocess
import tempfile
import unittest
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
from observatory.plugin_projection import project_source
from observatory.plugin_wire import PollBatch
from observatory.rust_shadow_snapshot import _ready, _revision, capture, current


SOURCE = "shadow-source"


class RustStorageShadowTests(unittest.TestCase):
    def setUp(self):
        binary = os.environ.get("AGENTURM_RUST_STORAGE_SHADOW_BIN")
        if not binary:
            self.skipTest("installed Rust storage shadow binary required")
        self.binary = str(Path(binary).resolve())
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.store = Store(self.root / "store.db")
        self.addCleanup(self.store.close)
        self.backup_index = 0

    def backup(self):
        self.backup_index += 1
        path = self.root / f"shadow-{self.backup_index}.db"
        descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        os.close(descriptor)
        with self.store._connect() as source, sqlite3.connect(path) as destination:
            source.backup(destination)
        # Freeze the backup into one main file so later test mutations are
        # visible to SQLite immutable mode, rather than parked in WAL.
        with sqlite3.connect(path) as db:
            self.assertEqual(db.execute("PRAGMA journal_mode=DELETE").fetchone()[0],
                             "delete")
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        return path

    def inspect(self, path, source=SOURCE):
        request = {"schema_version": 1, "sqlite_path": str(path), "source_id": source}
        return subprocess.run([self.binary], input=json.dumps(request), text=True,
                              capture_output=True, timeout=5)

    def certificate(self, path, source=SOURCE):
        result = self.inspect(path, source)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_revision_states_match_python_and_old_copy_is_not_live_authority(self):
        unknown = self.certificate(self.backup())
        self.assertEqual((unknown["state"], unknown["revision"]), ("unknown", None))
        lease = activate_source(self.store, plugin_id="fixture.plugin",
                                source_id=SOURCE, generation="a" * 64)
        self.assertEqual(self.certificate(self.backup())["state"], "pending")
        fact = {"event_id": "start", "revision": 0, "kind": "lifecycle",
                "time_ns": "100", "payload": {"state": "started"},
                "identity": {"session_id": "native-one"}}
        commit_batch(self.store, lease, "", PollBatch(SOURCE, "one", (fact,)))
        pending = self.certificate(self.backup())
        self.assertEqual(pending["state"], "pending")
        project_source(self.store, SOURCE)
        copy = self.backup()
        ready = self.certificate(copy)
        self.assertEqual(ready["state"], "ready")
        with self.store._connect() as db:
            db.execute("BEGIN")
            revision = _revision(db, SOURCE)
        self.assertTrue(_ready(revision))
        self.assertEqual(ready["receiver_id"], revision[0])
        self.assertEqual(
            ready["revision"],
            {"lease_epoch": str(revision[1]), "policy_epoch": str(revision[2]),
             "dirty_seq": str(revision[3]), "capture_enabled": True,
             "projected_seq": str(revision[5]), "projected_policy": str(revision[6])})
        snapshot = capture(self.store, SOURCE)
        self.assertTrue(current(self.store, snapshot))
        commit_batch(self.store, lease, "one", PollBatch(SOURCE, "two", (
            {"event_id": "end", "revision": 0, "kind": "lifecycle",
             "time_ns": "200", "payload": {"state": "completed"},
             "identity": {"session_id": "native-one"}},)))
        self.assertFalse(current(self.store, snapshot))
        self.assertEqual(self.certificate(copy)["state"], "ready")
        self.assertEqual(self.certificate(self.backup())["state"], "pending")
        set_source_capture(self.store, source_id=SOURCE, enabled=False)
        self.assertEqual(self.certificate(self.backup())["state"], "disabled")

    def test_read_only_copy_has_no_file_or_sidecar_changes(self):
        activate_source(self.store, plugin_id="fixture.plugin",
                        source_id=SOURCE, generation="a" * 64)
        copy = self.backup()
        before_files = {item.name for item in self.root.iterdir()}
        before_bytes = hashlib.sha256(copy.read_bytes()).digest()
        self.assertEqual(self.certificate(copy)["state"], "pending")
        self.assertEqual(before_files, {item.name for item in self.root.iterdir()})
        self.assertEqual(before_bytes, hashlib.sha256(copy.read_bytes()).digest())
        (self.root / f"{copy.name}-wal").touch()
        self.assertEqual(self.inspect(copy).returncode, 2)

    def test_malformed_schema_types_and_receiver_mismatch_fail_closed(self):
        activate_source(self.store, plugin_id="fixture.plugin",
                        source_id=SOURCE, generation="a" * 64)
        for column, value in (("dirty_seq", "bad"), ("lease_epoch", "bad"),
                              ("policy_epoch", "bad"), ("capture_enabled", 3),
                              ("receiver_id", "different"), ("receiver_id", 3)):
            with self.subTest(column=column, value=value):
                copy = self.backup()
                with sqlite3.connect(copy) as db:
                    db.execute(f"UPDATE plugin_source_cursor_v1 SET {column}=? "
                               "WHERE source_id=?", (value, SOURCE))
                self.assertEqual(self.inspect(copy).returncode, 2)
        copy = self.backup()
        with sqlite3.connect(copy) as db:
            db.execute("DROP TABLE plugin_projection_cursor_v1")
            db.execute("CREATE TABLE plugin_projection_cursor_v1 "
                       "(source_id TEXT PRIMARY KEY, projected_seq INTEGER, "
                       "policy_epoch INTEGER)")
            db.execute("INSERT INTO plugin_projection_cursor_v1 VALUES (?, 0, NULL)",
                       (SOURCE,))
        self.assertEqual(self.inspect(copy).returncode, 2)
        empty = self.root / "empty.db"
        sqlite3.connect(empty).close()
        self.assertEqual(self.inspect(empty).returncode, 2)
        self.assertFalse((self.root / "missing.db").exists())
        self.assertEqual(self.inspect(self.root / "missing.db").returncode, 2)
        self.assertFalse((self.root / "missing.db").exists())


if __name__ == "__main__":
    unittest.main()
