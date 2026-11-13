"""Leased metadata reads stay fresh, exclusive, bounded and replaceable."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import threading
import unittest

from observatory.read_connections import ReadConnections


class ReadConnectionTests(unittest.TestCase):
    def setUp(self):
        tmp=tempfile.TemporaryDirectory();self.addCleanup(tmp.cleanup)
        self.path=Path(tmp.name)/'test.db'
        with sqlite3.connect(self.path) as db:
            db.execute('PRAGMA journal_mode=WAL')
            db.execute('CREATE TABLE facts(value)');db.execute('INSERT INTO facts VALUES(1)')
        self.pool=ReadConnections(self.path);self.addCleanup(self.pool.close)

    def test_reuse_never_pins_snapshot_or_allows_writes(self):
        with self.pool.read() as db:
            first=db._db;db.execute('BEGIN')
            self.assertEqual(db.execute('SELECT value FROM facts').fetchone()[0],1)
        with sqlite3.connect(self.path) as writer:writer.execute('UPDATE facts SET value=2')
        with self.pool.read() as db:
            self.assertIs(db._db,first)
            self.assertFalse(db.in_transaction)
            self.assertEqual(db.execute('SELECT value FROM facts').fetchone()[0],2)
            with self.assertRaises(sqlite3.OperationalError):db.execute('DELETE FROM facts')

    def test_errors_discard_leases_and_close_during_active_read_defers_cleanup(self):
        with self.assertRaises(RuntimeError):
            with self.pool.read() as db:
                abandoned=db;db.execute('BEGIN');raise RuntimeError('cancelled')
        with self.assertRaises(sqlite3.ProgrammingError):abandoned.execute('SELECT 1')
        with self.pool.read() as active:
            self.pool.close()
            self.assertEqual(active.execute('SELECT COUNT(*) FROM facts').fetchone()[0],1)
        with self.assertRaises(sqlite3.ProgrammingError):active.execute('SELECT 1')
        with self.assertRaises(sqlite3.ProgrammingError):
            with self.pool.read():pass

    def replacement(self):
        replacement=self.path.with_suffix('.new')
        with sqlite3.connect(replacement) as db:
            db.execute('CREATE TABLE facts(value)');db.execute('INSERT INTO facts VALUES(9)')
        replacement.replace(self.path)

    def test_atomic_replacement_discards_idle_and_inflight_connections(self):
        with self.pool.read() as old:old.execute('SELECT * FROM facts').fetchall()
        # A checkpoint before replacement prevents old fixture WAL sidecars
        # from being interpreted as part of the new database.
        with sqlite3.connect(self.path) as db:db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        self.replacement()
        with self.pool.read() as new:
            self.assertEqual(new.execute('SELECT value FROM facts').fetchone()[0],9)
        with self.assertRaises(sqlite3.ProgrammingError):old.execute('SELECT 1')
        with self.assertRaisesRegex(sqlite3.OperationalError,'replaced'):
            with self.pool.read() as active:
                active.execute('SELECT * FROM facts').fetchall();self.replacement()
        with self.assertRaises(sqlite3.ProgrammingError):active.execute('SELECT 1')
        with self.pool.read() as latest:self.assertEqual(latest.execute('SELECT value FROM facts').fetchone()[0],9)

    def test_concurrent_leases_are_exclusive_and_idle_handles_bounded(self):
        barrier=threading.Barrier(8)
        live=set();lock=threading.Lock()
        def read(_):
            with self.pool.read() as db:
                with lock:
                    self.assertNotIn(id(db._db),live);live.add(id(db._db))
                barrier.wait(timeout=5)
                self.assertEqual(db.execute('SELECT value FROM facts').fetchone()[0],1)
                with lock:live.remove(id(db._db))
                return db._db
        with ThreadPoolExecutor(max_workers=8) as executor:leases=list(executor.map(read,range(8)))
        open_count=0
        for db in leases:
            try:db.execute('SELECT 1');open_count+=1
            except sqlite3.ProgrammingError:pass
        self.assertEqual(open_count,4)
        self.pool.close()
        for db in leases:
            with self.assertRaises(sqlite3.ProgrammingError):db.execute('SELECT 1')

    def test_unfinished_cursor_closes_before_reuse_and_does_not_pin_wal(self):
        with sqlite3.connect(self.path) as db:
            db.executemany('INSERT INTO facts VALUES(?)',[(i,) for i in range(100)])
        with self.pool.read() as lease:
            cursor=lease.execute('SELECT * FROM facts')
            cursor.fetchone()  # Deliberately retain an unfinished cursor.
        with self.assertRaises(sqlite3.ProgrammingError):cursor.fetchone()
        with self.assertRaises(sqlite3.ProgrammingError):lease.execute('SELECT 1')
        with sqlite3.connect(self.path) as writer:
            writer.execute('UPDATE facts SET value=3');writer.commit()
            self.assertEqual(writer.execute('PRAGMA wal_checkpoint(TRUNCATE)').fetchone()[0],0)

    def test_other_process_commit_is_visible_without_reconnecting(self):
        with self.pool.read() as lease:
            connection=lease._db
            self.assertEqual(lease.execute('SELECT value FROM facts').fetchone()[0],1)
        subprocess.run([sys.executable,'-c',
            "import sqlite3,sys; db=sqlite3.connect(sys.argv[1]); db.execute('UPDATE facts SET value=7'); db.commit(); db.close()",
            str(self.path)],check=True)
        with self.pool.read() as lease:
            self.assertIs(lease._db,connection)
            self.assertEqual(lease.execute('SELECT value FROM facts').fetchone()[0],7)

    def test_symlink_retarget_never_keeps_previous_receiver(self):
        alias=self.path.with_name('current ?#.db')
        try:alias.symlink_to(self.path)
        except (OSError,NotImplementedError):self.skipTest('symlink creation unavailable')
        pool=ReadConnections(alias);self.addCleanup(pool.close)
        with pool.read() as lease:
            original=lease._db
            self.assertEqual(lease.execute('SELECT value FROM facts').fetchone()[0],1)
        other=self.path.with_name('other.db')
        with sqlite3.connect(other) as db:
            db.execute('CREATE TABLE facts(value)');db.execute('INSERT INTO facts VALUES(8)')
        replacement=alias.with_suffix('.link');replacement.symlink_to(other);replacement.replace(alias)
        with pool.read() as lease:self.assertEqual(lease.execute('SELECT value FROM facts').fetchone()[0],8)
        with self.assertRaises(sqlite3.ProgrammingError):original.execute('SELECT 1')
        with self.assertRaisesRegex(sqlite3.OperationalError,'replaced'):
            with pool.read() as lease:
                lease.execute('SELECT value FROM facts').fetchone()
                replacement.symlink_to(self.path);replacement.replace(alias)
        with pool.read() as lease:self.assertEqual(lease.execute('SELECT value FROM facts').fetchone()[0],1)
