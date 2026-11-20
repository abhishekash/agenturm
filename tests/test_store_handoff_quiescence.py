"""Feature-off Store handoff keeps a fence until tracked work has drained."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import sqlite3
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from observatory.core import Store


class StoreHandoffQuiescenceTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.store = Store(Path(temporary.name) / 'store.db')
        self.addCleanup(self.store.close)

    def test_held_writer_fences_new_admission_until_context_closes(self):
        entered, release = threading.Event(), threading.Event()

        def hold():
            with self.store._connect() as db:
                db.execute('BEGIN')
                db.execute('SELECT 1').fetchone()
                entered.set()
                self.assertTrue(release.wait(5))
                return db

        with ThreadPoolExecutor(max_workers=1) as executor:
            future = executor.submit(hold)
            self.assertTrue(entered.wait(5))
            try:
                self.assertFalse(self.store.quiesce_for_writer_handoff(.02))
                with self.assertRaises(sqlite3.ProgrammingError):
                    with self.store._connect():
                        pass
                with self.assertRaises(sqlite3.ProgrammingError):
                    with self.store._read():
                        pass
            finally:
                release.set()
            connection = future.result(timeout=5)
        self.assertTrue(self.store.quiesce_for_writer_handoff(1))
        with self.assertRaises(sqlite3.ProgrammingError):
            connection.execute('SELECT 1')

    def test_held_read_lease_delays_success_without_revocation(self):
        entered, release = threading.Event(), threading.Event()

        def hold():
            with self.store._read() as lease:
                cursor = lease.execute('SELECT 1')
                entered.set()
                self.assertTrue(release.wait(5))
                return cursor.fetchone()[0]

        with ThreadPoolExecutor(max_workers=1) as executor:
            future = executor.submit(hold)
            self.assertTrue(entered.wait(5))
            try:
                self.assertFalse(self.store.quiesce_for_writer_handoff(.02))
            finally:
                release.set()
            self.assertEqual(future.result(timeout=5), 1)
        self.assertTrue(self.store.quiesce_for_writer_handoff(1))

    def test_held_worker_remains_visible_after_deadline_and_retry(self):
        release = threading.Event()
        worker = threading.Thread(target=lambda: release.wait(5), daemon=True)
        worker.start()
        self.store._derived_thread = worker
        try:
            self.assertFalse(self.store.quiesce_for_writer_handoff(.02))
            self.assertIs(self.store._derived_thread, worker)
            self.assertTrue(worker.is_alive())
            with self.assertRaises(sqlite3.ProgrammingError):
                with self.store._connect():
                    pass
        finally:
            release.set()
        worker.join(timeout=5)
        self.assertTrue(self.store.quiesce_for_writer_handoff(1))

    def test_direct_incoming_status_read_is_counted(self):
        entered, release = threading.Event(), threading.Event()
        self.store._async_derived_refresh = True
        self.store.migration_status = {**self.store.migration_status, 'ready': True}
        original_connect = sqlite3.connect

        def delayed_connect(*args, **kwargs):
            entered.set()
            self.assertTrue(release.wait(5))
            return original_connect(*args, **kwargs)

        with patch('observatory.core.sqlite3.connect', side_effect=delayed_connect):
            with ThreadPoolExecutor(max_workers=1) as executor:
                future = executor.submit(self.store._incoming_status, time.monotonic() + 5)
                self.assertTrue(entered.wait(5))
                try:
                    self.assertFalse(self.store.quiesce_for_writer_handoff(.02))
                finally:
                    release.set()
                future.result(timeout=5)
        self.assertTrue(self.store.quiesce_for_writer_handoff(1))

    def test_pool_lock_timeout_keeps_handoff_fenced_for_retry(self):
        pool = self.store._read_connections
        with pool.lock:
            self.assertFalse(self.store.quiesce_for_writer_handoff(.02))
            self.assertFalse(pool.closed)
        with self.assertRaises(sqlite3.ProgrammingError):
            with self.store._read():
                pass
        self.assertTrue(self.store.quiesce_for_writer_handoff(1))

    def test_synchronous_idle_close_overrun_cannot_report_success(self):
        class SlowClose:
            def close(self):
                time.sleep(.05)

        self.store._read_connections.idle.append(SlowClose())
        self.assertFalse(self.store.quiesce_for_writer_handoff(.01))
        self.assertTrue(self.store._read_connections.closed)
        self.assertTrue(self.store.quiesce_for_writer_handoff(1))

    def test_writer_close_failure_never_reports_success(self):
        connection = sqlite3.connect(self.store.path, check_same_thread=False)
        self.addCleanup(connection.close)

        class FailedClose:
            def __init__(self, real):
                object.__setattr__(self, 'real', real)

            def __getattr__(self, name):
                return getattr(self.real, name)

            def __setattr__(self, name, value):
                setattr(self.real, name, value)

            def close(self):
                raise OSError('injected close failure')

        with patch('observatory.core.sqlite3.connect', return_value=FailedClose(connection)):
            with self.assertRaisesRegex(OSError, 'injected close failure'):
                with self.store._connect() as db:
                    db.execute('SELECT 1')
        self.assertFalse(self.store.quiesce_for_writer_handoff(.01))
        self.assertFalse(self.store.quiesce_for_writer_handoff(1))

    def test_ordinary_close_retains_reference_to_worker_that_outlives_join(self):
        class HeldWorker:
            alive = True

            def is_alive(self):
                return self.alive

            def join(self, timeout=None):
                pass

        worker = HeldWorker()
        self.store._derived_thread = worker
        try:
            with patch('observatory.core.time.monotonic', side_effect=[0, 20]):
                self.store._close_workers()
            self.assertIs(self.store._derived_thread, worker)
        finally:
            worker.alive = False

    def test_rejects_unbounded_handoff_deadlines(self):
        for timeout in (None, True, -1, 0, 61, float('inf'), float('nan'), 1e308):
            with self.subTest(timeout=timeout), self.assertRaises(ValueError):
                self.store.quiesce_for_writer_handoff(timeout)
        with self.store._connect() as db:
            self.assertEqual(db.execute('SELECT 1').fetchone()[0], 1)
