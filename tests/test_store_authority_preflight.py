"""Python Store refuses foreign storage authority before its startup writes."""
import os
import sqlite3
import stat
import tempfile
import unittest
from contextlib import closing
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory import store_authority
from observatory.store_authority import StoreAuthorityError


class StoreAuthorityPreflightTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def store(self, name="store.db"):
        store = Store(self.root / name)
        self.addCleanup(store.close)
        return store

    @staticmethod
    def marker(path):
        with sqlite3.connect(path) as db:
            return db.execute("SELECT value FROM schema_meta WHERE key=?",
                              (store_authority.MARKER,)).fetchone()[0]

    def test_new_existing_empty_and_legacy_stores_get_python_marker(self):
        path = self.root / "new" / "store.db"
        with closing(Store(path)) as store:
            self.assertTrue(store.created_new)
            self.assertEqual(self.marker(path), "python-v1")
        with closing(Store(path)) as store:
            self.assertFalse(store.created_new)
            self.assertEqual(self.marker(path), "python-v1")
        empty = self.root / "empty.db"
        empty.touch()
        with closing(Store(empty)) as store:
            self.assertFalse(store.created_new)
            self.assertEqual(self.marker(empty), "python-v1")
        legacy = self.root / "legacy.db"
        with sqlite3.connect(legacy) as db:
            db.execute("CREATE TABLE legacy_marker(value TEXT)")
        with closing(Store(legacy)) as store:
            self.assertFalse(store.created_new)
            self.assertEqual(self.marker(legacy), "python-v1")
        with sqlite3.connect(legacy) as db:
            db.execute("DELETE FROM schema_meta WHERE key=?", (store_authority.MARKER,))
        with closing(Store(legacy)):
            self.assertEqual(self.marker(legacy), "python-v1")

    def test_foreign_unknown_and_oversized_markers_refuse_without_python_mutation(self):
        path = self.root / "store.db"
        with closing(Store(path)):
            pass
        for value in ("rust-v1", "future-owner-v9", "x" * 100_000):
            with sqlite3.connect(path) as db:
                db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?) "
                           "ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                           (store_authority.MARKER, value))
            os.chmod(path, 0o640)
            before = (path.read_bytes(), path.stat().st_mtime_ns,
                      stat.S_IMODE(path.stat().st_mode))
            with patch.object(Store, "_finish_migration",
                              side_effect=AssertionError("worker started")):
                with self.assertRaises(StoreAuthorityError):
                    Store(path, async_derived_refresh=False)
            after = (path.read_bytes(), path.stat().st_mtime_ns,
                     stat.S_IMODE(path.stat().st_mode))
            self.assertEqual(after, before)

    def test_preflight_sees_uncheckpointed_wal_authority_marker(self):
        path = self.root / "live-wal.db"
        with closing(Store(path)):
            pass
        with closing(sqlite3.connect(path)) as writer:
            self.assertEqual(writer.execute("PRAGMA journal_mode=WAL").fetchone()[0], "wal")
            writer.execute("PRAGMA wal_checkpoint(TRUNCATE)")
            writer.execute("UPDATE schema_meta SET value='rust-v1' WHERE key=?",
                           (store_authority.MARKER,))
            writer.commit()
            self.assertGreater((self.root / "live-wal.db-wal").stat().st_size, 0)
            before = (path.read_bytes(), path.stat().st_mtime_ns)
            with self.assertRaises(StoreAuthorityError):
                Store(path)
            self.assertEqual((path.read_bytes(), path.stat().st_mtime_ns), before)

    def test_corrupt_nonregular_and_broken_symlink_targets_refuse(self):
        corrupt = self.root / "corrupt.db"
        corrupt.write_bytes(b"not a SQLite database")
        before = corrupt.read_bytes()
        with self.assertRaises(StoreAuthorityError):
            Store(corrupt)
        self.assertEqual(corrupt.read_bytes(), before)
        directory = self.root / "directory.db"
        directory.mkdir()
        with self.assertRaises(StoreAuthorityError):
            Store(directory)
        broken = self.root / "broken.db"
        broken.symlink_to("missing.db")
        with self.assertRaises(StoreAuthorityError):
            Store(broken)
        self.assertTrue(broken.is_symlink())
        self.assertFalse((self.root / "missing.db").exists())

    def test_ambiguous_and_malformed_authority_metadata_refuse(self):
        ambiguous = self.root / "ambiguous.db"
        with sqlite3.connect(ambiguous) as db:
            db.execute("CREATE TABLE schema_meta(key TEXT,value TEXT)")
            db.executemany("INSERT INTO schema_meta VALUES(?,?)", [
                (store_authority.MARKER, "python-v1"),
                (store_authority.MARKER, "rust-v1")])
        before = ambiguous.read_bytes()
        with self.assertRaisesRegex(StoreAuthorityError, "ambiguous"):
            Store(ambiguous)
        self.assertEqual(ambiguous.read_bytes(), before)
        malformed = self.root / "malformed.db"
        with sqlite3.connect(malformed) as db:
            db.execute("CREATE TABLE schema_meta(key TEXT)")
            db.execute("INSERT INTO schema_meta VALUES(?)", (store_authority.MARKER,))
        before = malformed.read_bytes()
        with self.assertRaisesRegex(StoreAuthorityError, "unreadable"):
            Store(malformed)
        self.assertEqual(malformed.read_bytes(), before)

    def test_explicit_symlink_target_and_retarget_during_preflight(self):
        first = self.store("first.db")
        second = self.store("second.db")
        self.assertNotEqual(Path(first.path).stat().st_ino,
                            Path(second.path).stat().st_ino)
        alias = self.root / "alias.db"
        alias.symlink_to("first.db")
        with closing(Store(alias)) as store:
            self.assertFalse(store.created_new)
        new_link = self.root / "new-alias.db"
        new_link.symlink_to("second.db")
        original_connect = sqlite3.connect

        def retarget(database, *args, **kwargs):
            os.replace(new_link, alias)
            return original_connect(database, *args, **kwargs)

        with patch.object(store_authority.sqlite3, "connect", side_effect=retarget):
            with self.assertRaisesRegex(StoreAuthorityError, "changed during preflight"):
                Store(alias)
        self.assertTrue(alias.is_symlink())
        self.assertEqual(alias.resolve(), Path(second.path))

    def test_concurrent_creation_is_not_adopted_after_absent_preflight(self):
        path = self.root / "raced.db"
        real_open = os.open

        def create_then_open(target, flags, mode=0o777):
            if str(target) == str(path) and flags & os.O_EXCL:
                with sqlite3.connect(path) as db:
                    db.execute("CREATE TABLE schema_meta(key TEXT PRIMARY KEY,value TEXT)")
                    db.execute("INSERT INTO schema_meta VALUES(?, 'rust-v1')",
                               (store_authority.MARKER,))
            return real_open(target, flags, mode)

        with patch("observatory.core.os.open", side_effect=create_then_open):
            with self.assertRaisesRegex(StoreAuthorityError, "appeared"):
                Store(path)
        with sqlite3.connect(path) as db:
            self.assertEqual(db.execute("SELECT value FROM schema_meta WHERE key=?",
                                        (store_authority.MARKER,)).fetchone()[0], "rust-v1")

    def test_symlink_retarget_before_chmod_cannot_change_new_target_mode(self):
        first = self.store("first.db")
        second = self.store("second.db")
        alias = self.root / "alias.db"
        alias.symlink_to(Path(first.path).name)
        next_alias = self.root / "next-alias.db"
        next_alias.symlink_to(Path(second.path).name)
        os.chmod(second.path, 0o640)
        before = (Path(second.path).read_bytes(),
                  stat.S_IMODE(Path(second.path).stat().st_mode))
        actual_open = os.open
        retargeted = False

        def retarget_before_chmod(target, flags, mode=0o777):
            nonlocal retargeted
            if (str(target) == str(alias) and not flags & os.O_EXCL and not retargeted):
                os.replace(next_alias, alias)
                retargeted = True
            return actual_open(target, flags, mode)

        with patch.object(store_authority.os, "open", side_effect=retarget_before_chmod):
            with self.assertRaisesRegex(StoreAuthorityError, "before chmod"):
                Store(alias)
        self.assertTrue(retargeted)
        self.assertEqual((Path(second.path).read_bytes(),
                          stat.S_IMODE(Path(second.path).stat().st_mode)), before)

    def test_missing_path_before_first_sqlite_connect_is_not_recreated(self):
        path = self.root / "disappeared.db"
        actual_connect = sqlite3.connect
        removed = False

        def remove_before_connect(database, *args, **kwargs):
            nonlocal removed
            if str(database).endswith("?mode=rw") and not removed:
                path.unlink()
                removed = True
            return actual_connect(database, *args, **kwargs)

        with patch("observatory.core.sqlite3.connect", side_effect=remove_before_connect):
            with self.assertRaises(sqlite3.OperationalError):
                Store(path)
        self.assertTrue(removed)
        self.assertFalse(path.exists())

    def test_relative_path_remains_bound_after_cwd_changes(self):
        original_cwd = Path.cwd()
        origin = self.root / "origin"
        elsewhere = self.root / "elsewhere"
        origin.mkdir()
        (elsewhere / "nested").mkdir(parents=True)
        store = None
        try:
            os.chdir(origin)
            store = Store("nested/store.db")
            self.assertEqual(Path(store.path), origin / "nested" / "store.db")
            os.chdir(elsewhere)
            self.assertEqual(store.plugin_sessions()["sessions"], [])
            self.assertFalse((elsewhere / "nested" / "store.db").exists())
        finally:
            if store is not None:
                store.close()
            os.chdir(original_cwd)


if __name__ == "__main__":
    unittest.main()
