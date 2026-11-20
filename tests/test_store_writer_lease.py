"""Opt-in cooperating telemetry writer lease behavior (Linux only)."""
from __future__ import annotations

import os
from pathlib import Path
import select
import sqlite3
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from observatory.core import Store
from observatory.store_authority import StoreAuthorityError
from observatory.store_writer_lease import (
    StoreWriterLease,
    StoreWriterLeaseError,
    retained_lease_count,
)


ROOT = Path(__file__).resolve().parents[1]
HOLDER = r'''
import sys
from observatory.store_writer_lease import StoreWriterLease
lease = StoreWriterLease.acquire(sys.argv[1])
print("READY", flush=True)
sys.stdin.readline()
lease.release_after_quiescence()
'''
PROBE = r'''
import sys
from observatory.core import Store
from observatory.store_writer_lease import StoreWriterLeaseError
try:
    store = Store(sys.argv[1], writer_lease=True)
except StoreWriterLeaseError as exc:
    print("DENIED:" + str(exc))
else:
    store.close(timeout=10)
    print("ACQUIRED")
'''
LEASE_PROBE = r'''
import sys
from observatory.store_writer_lease import StoreWriterLease, StoreWriterLeaseError
try:
    lease = StoreWriterLease.acquire(sys.argv[1])
except StoreWriterLeaseError as exc:
    print("DENIED:" + str(exc))
else:
    lease.release_after_quiescence()
    print("ACQUIRED")
'''


@unittest.skipUnless(sys.platform.startswith("linux"), "opt-in writer lease is Linux-only")
class StoreWriterLeaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="agenturm-writer-lease-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.root.chmod(0o700)

    def database(self, name="observatory.db"):
        return self.root / name

    def probe(self, path):
        environment = dict(os.environ, PYTHONPATH=str(ROOT))
        return subprocess.run(
            [sys.executable, "-c", PROBE, str(path)], cwd=ROOT, env=environment,
            text=True, capture_output=True, timeout=10,
        )

    def lease_probe(self, path):
        environment = dict(os.environ, PYTHONPATH=str(ROOT))
        return subprocess.run(
            [sys.executable, "-c", LEASE_PROBE, str(path)], cwd=ROOT, env=environment,
            text=True, capture_output=True, timeout=10,
        )

    def hold_in_child(self, path):
        environment = dict(os.environ, PYTHONPATH=str(ROOT))
        process = subprocess.Popen(
            [sys.executable, "-c", HOLDER, str(path)], cwd=ROOT, env=environment,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True,
        )
        readable, _, _ = select.select([process.stdout], [], [], 10)
        if not readable:
            process.kill()
            process.wait(timeout=5)
            self.fail("writer lease child did not become ready")
        line = process.stdout.readline().strip()
        if line != "READY":
            stderr = process.stderr.read()
            process.kill()
            process.wait(timeout=5)
            self.fail(f"writer lease child failed: {line} {stderr}")
        return process

    def stop_child(self, process):
        if process.poll() is None:
            process.stdin.write("release\n")
            process.stdin.flush()
            process.wait(timeout=10)
        process.stdin.close()
        process.stdout.close()
        process.stderr.close()

    @staticmethod
    def snapshot(path):
        paths = [path, Path(f"{path}-wal"), Path(f"{path}-shm"), Path(f"{path}-journal")]
        result = {}
        for candidate in paths:
            if os.path.lexists(candidate):
                info = candidate.lstat()
                result[str(candidate)] = (candidate.read_bytes(), stat.S_IMODE(info.st_mode),
                                          info.st_dev, info.st_ino)
            else:
                result[str(candidate)] = None
        return result

    def test_subprocess_contention_fails_before_creating_database_or_sidecars(self):
        path = self.database()
        holder = StoreWriterLease.acquire(path)
        before = self.snapshot(path)
        denied = self.probe(path)
        self.assertEqual(denied.returncode, 0, denied.stderr)
        self.assertIn("DENIED:another cooperating telemetry writer is active", denied.stdout)
        self.assertEqual(self.snapshot(path), before)
        self.assertTrue(holder.release_after_quiescence())
        self.assertEqual(self.probe(path).stdout.strip(), "ACQUIRED")

    def test_symlink_alias_contends_on_canonical_target_lock(self):
        target = self.database("target.db")
        alias = self.root / "alias.db"
        alias.symlink_to(target.name)
        holder = StoreWriterLease.acquire(target)
        before = self.snapshot(target)
        denied = self.probe(alias)
        self.assertEqual(denied.returncode, 0, denied.stderr)
        self.assertIn("DENIED:another cooperating telemetry writer is active", denied.stdout)
        self.assertEqual(self.snapshot(target), before)
        self.assertFalse(alias.exists())
        self.assertTrue(holder.release_after_quiescence())

    def test_same_process_aliases_share_reference_counted_lock(self):
        target = self.database()
        seed = Store(target)
        seed.close()
        alias = self.root / "alias.db"
        alias.symlink_to(target.name)
        first = StoreWriterLease.acquire(target)
        second = StoreWriterLease.acquire(alias)
        self.assertEqual(retained_lease_count(), 1)
        self.assertTrue(first.release_after_quiescence())
        self.assertIn("DENIED:", self.probe(target).stdout)
        self.assertTrue(second.release_after_quiescence())
        self.assertEqual(retained_lease_count(), 0)
        self.assertEqual(self.probe(alias).stdout.strip(), "ACQUIRED")

    def test_lock_descriptor_is_close_on_exec_and_fork_child_cannot_release_parent(self):
        import fcntl

        path = self.database()
        lease = StoreWriterLease.acquire(path)
        flags = fcntl.fcntl(lease._state.fd, fcntl.F_GETFD)
        self.assertTrue(flags & fcntl.FD_CLOEXEC)
        read_fd, write_fd = os.pipe()
        pid = os.fork()
        if pid == 0:
            try:
                os.close(read_fd)
                try:
                    lease.verify()
                except StoreWriterLeaseError:
                    os.write(write_fd, b"child-detached")
                else:
                    os.write(write_fd, b"child-still-bound")
            finally:
                os.close(write_fd)
                os._exit(0)
        os.close(write_fd)
        try:
            self.assertEqual(os.read(read_fd, 64), b"child-detached")
            _, status = os.waitpid(pid, 0)
            self.assertTrue(os.WIFEXITED(status))
            self.assertIn("DENIED:", self.probe(path).stdout)
        finally:
            os.close(read_fd)
            self.assertTrue(lease.release_after_quiescence())

    def test_store_refcount_does_not_unlock_until_last_store_quiesces(self):
        path = self.database()
        first = Store(path, writer_lease=True)
        second = Store(path, writer_lease=True)
        self.addCleanup(first.close)
        self.addCleanup(second.close)
        first.close(timeout=2)
        self.assertIn("DENIED:", self.probe(path).stdout)
        second.close(timeout=2)
        self.assertEqual(self.probe(path).stdout.strip(), "ACQUIRED")

    def test_close_timeout_retains_lease_until_active_writer_exits(self):
        path = self.database()
        store = Store(path, writer_lease=True)
        self.addCleanup(store.close)
        entered = threading.Event()
        release = threading.Event()
        failures = []

        def hold_writer():
            try:
                with store._connect() as db:
                    db.execute("INSERT INTO schema_meta(key,value) VALUES('held_writer_test','1')")
                    entered.set()
                    release.wait(5)
            except BaseException as exc:
                failures.append(exc)

        worker = threading.Thread(target=hold_writer)
        worker.start()
        self.assertTrue(entered.wait(3))
        store.close(timeout=0.05)
        self.assertTrue(store._writer_lease.held)
        self.assertIn("DENIED:", self.probe(path).stdout)
        with self.assertRaises(sqlite3.ProgrammingError):
            with store._connect():
                pass
        release.set()
        worker.join(5)
        self.assertFalse(worker.is_alive())
        self.assertEqual(failures, [])
        store.close(timeout=3)
        self.assertIsNone(store._writer_lease)
        self.assertEqual(self.probe(path).stdout.strip(), "ACQUIRED")

    def test_close_timeout_retains_lease_until_active_read_lease_exits(self):
        path = self.database()
        store = Store(path, writer_lease=True)
        self.addCleanup(store.close)
        entered = threading.Event()
        release = threading.Event()
        failures = []

        def hold_reader():
            try:
                with store._read() as db:
                    db.execute("SELECT value FROM schema_meta WHERE key='receiver_id'").fetchone()
                    entered.set()
                    release.wait(5)
            except BaseException as exc:
                failures.append(exc)

        worker = threading.Thread(target=hold_reader)
        worker.start()
        self.assertTrue(entered.wait(3))
        self.assertFalse(store.close(timeout=0.05))
        self.assertTrue(store._writer_lease.held)
        self.assertIn("DENIED:", self.probe(path).stdout)
        release.set()
        worker.join(5)
        self.assertFalse(worker.is_alive())
        self.assertEqual(failures, [])
        self.assertTrue(store.close(timeout=3))
        self.assertIsNone(store._writer_lease)

    def test_lock_symlink_and_bad_mode_are_rejected_without_database_mutation(self):
        path = self.database()
        lock = self.root / ".observatory.db.agenturm-writer.lock"
        target = self.root / "sentinel"
        target.write_bytes(b"do not alter")
        lock.symlink_to(target.name)
        before = self.snapshot(path)
        with self.assertRaises(StoreWriterLeaseError):
            Store(path, writer_lease=True)
        self.assertEqual(target.read_bytes(), b"do not alter")
        self.assertEqual(self.snapshot(path), before)
        lock.unlink()
        lock.write_bytes(b"")
        lock.chmod(0o640)
        before_lock = (lock.read_bytes(), stat.S_IMODE(lock.stat().st_mode))
        with self.assertRaises(StoreWriterLeaseError):
            Store(path, writer_lease=True)
        self.assertEqual((lock.read_bytes(), stat.S_IMODE(lock.stat().st_mode)), before_lock)
        self.assertEqual(self.snapshot(path), before)

    def test_lock_path_replacement_rejects_new_store_work_without_mutation(self):
        path = self.database()
        store = Store(path, writer_lease=True)
        self.addCleanup(store.close)
        lock = self.root / ".observatory.db.agenturm-writer.lock"
        lock.unlink()
        lock.write_bytes(b"")
        lock.chmod(0o600)
        before = self.snapshot(path)
        with self.assertRaises(StoreWriterLeaseError):
            with store._connect() as db:
                db.execute("INSERT INTO schema_meta(key,value) VALUES('must_not_write','1')")
        self.assertEqual(self.snapshot(path), before)
        store.close(timeout=3)
        self.assertIsNone(store._writer_lease)

    def test_database_replacement_and_symlink_retarget_fail_closed(self):
        first = self.database("first.db")
        second = self.database("second.db")
        replacement = self.database("replacement.db")
        for path in (first, second):
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE sentinel(value TEXT)")
                db.execute("INSERT INTO sentinel VALUES(?)", (path.name,))

        store = Store(first, writer_lease=True)
        self.addCleanup(store.close)
        before = first.read_bytes()
        os.replace(first, replacement)
        with self.assertRaises(StoreWriterLeaseError):
            with store._connect() as db:
                db.execute("INSERT INTO sentinel VALUES('must-not-write')")
        self.assertEqual(replacement.read_bytes(), before)
        store.close(timeout=3)

        link = self.root / "linked.db"
        link.symlink_to(second.name)
        linked_store = Store(link, writer_lease=True)
        self.addCleanup(linked_store.close)
        link.unlink()
        link.symlink_to(replacement.name)
        before_replacement = self.snapshot(replacement)
        with self.assertRaises(StoreWriterLeaseError):
            with linked_store._connect() as db:
                db.execute("INSERT INTO sentinel VALUES('must-not-write')")
        self.assertEqual(self.snapshot(replacement), before_replacement)
        linked_store.close(timeout=3)

    def test_database_replacement_during_open_write_aborts_before_commit(self):
        path = self.database("race.db")
        moved = self.database("moved.db")
        store = Store(path, writer_lease=True)
        self.addCleanup(store.close)
        before = path.read_bytes()
        with self.assertRaisesRegex(StoreWriterLeaseError, "database target"):
            with store._connect() as db:
                db.execute("INSERT INTO schema_meta(key,value) VALUES('inflight_must_rollback','1')")
                os.replace(path, moved)
        self.assertEqual(moved.read_bytes(), before)
        with sqlite3.connect(moved) as db:
            self.assertIsNone(db.execute(
                "SELECT 1 FROM schema_meta WHERE key='inflight_must_rollback'"
            ).fetchone())
        self.assertFalse(path.exists())

    def test_hardlink_alias_refuses_store_admission(self):
        path = self.database("hardlink-source.db")
        alias = self.database("hardlink-alias.db")
        store = Store(path, writer_lease=True)
        self.addCleanup(store.close)
        before = self.snapshot(path)
        os.link(path, alias)
        with self.assertRaisesRegex(StoreWriterLeaseError, "database target changed"):
            with store._connect() as db:
                db.execute("INSERT INTO schema_meta(key,value) VALUES('hardlink_write','1')")
        self.assertEqual(self.snapshot(path), before)
        with self.assertRaises(StoreWriterLeaseError):
            StoreWriterLease.acquire(alias)

    def test_hardlink_alias_refuses_store_admission(self):
        path = self.database("hardlink-source.db")
        alias = self.database("hardlink-alias.db")
        store = Store(path, writer_lease=True)
        self.addCleanup(store.close)
        before = self.snapshot(path)
        os.link(path, alias)
        with self.assertRaisesRegex(StoreWriterLeaseError, "database target changed"):
            with store._connect() as db:
                db.execute("INSERT INTO schema_meta(key,value) VALUES('hardlink_write','1')")
        self.assertEqual(self.snapshot(path), before)
        with self.assertRaises(StoreWriterLeaseError):
            StoreWriterLease.acquire(alias)

    def test_constructor_failure_releases_only_after_tracked_work_drains(self):
        path = self.database()
        with sqlite3.connect(path) as db:
            db.execute("CREATE TABLE schema_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL)")
            db.execute("INSERT INTO schema_meta VALUES('storage_authority_v1','rust-v1')")
        with self.assertRaises(StoreAuthorityError):
            Store(path, writer_lease=True)
        self.assertEqual(retained_lease_count(), 0)
        self.assertEqual(self.lease_probe(path).stdout.strip(), "ACQUIRED")

    def test_constructor_failure_retains_lock_until_started_worker_drains(self):
        path = self.database()
        migration_started = threading.Event()
        quiesce_started = threading.Event()
        release_worker = threading.Event()
        constructor_done = threading.Event()
        result = []
        original_quiesce = Store.quiesce_for_writer_handoff

        def hold_migration(store):
            migration_started.set()
            release_worker.wait(5)

        def injected_failure(*_args, **_kwargs):
            raise RuntimeError("injected initialization failure after worker start")

        def observed_quiesce(store, timeout):
            quiesce_started.set()
            return original_quiesce(store, timeout)

        def construct():
            try:
                Store(path, async_derived_refresh=True, workflow_patterns=True,
                      writer_lease=True)
            except BaseException as exc:
                result.append(exc)
            finally:
                constructor_done.set()

        with patch.object(Store, "_finish_migration", hold_migration), \
                patch.object(Store, "_refresh_summaries", side_effect=injected_failure), \
                patch.object(Store, "quiesce_for_writer_handoff", observed_quiesce):
            constructor = threading.Thread(target=construct)
            constructor.start()
            try:
                self.assertTrue(migration_started.wait(5))
                self.assertTrue(quiesce_started.wait(5))
                self.assertFalse(constructor_done.wait(0.1))
                self.assertIn("DENIED:", self.lease_probe(path).stdout)
            finally:
                release_worker.set()
                constructor.join(10)
            self.assertFalse(constructor.is_alive())

        self.assertEqual(len(result), 1)
        self.assertIsInstance(result[0], RuntimeError)
        self.assertIn("injected initialization failure", str(result[0]))
        self.assertEqual(retained_lease_count(), 0)
        self.assertEqual(self.lease_probe(path).stdout.strip(), "ACQUIRED")

    def test_database_created_after_initial_lease_is_not_adopted(self):
        path = self.database()
        acquire = StoreWriterLease.acquire

        def race_after_acquire(database):
            lease = acquire(database)
            with sqlite3.connect(database) as db:
                db.execute("CREATE TABLE concurrent_creator(value TEXT)")
                db.execute("INSERT INTO concurrent_creator VALUES('external')")
            return lease

        with patch.object(StoreWriterLease, "acquire", side_effect=race_after_acquire):
            with self.assertRaisesRegex(StoreAuthorityError, "appeared after writer lease"):
                Store(path, writer_lease=True)
        self.assertEqual(retained_lease_count(), 0)
        with sqlite3.connect(path) as db:
            self.assertEqual(db.execute("SELECT value FROM concurrent_creator").fetchone()[0], "external")

    def test_constructor_uses_one_lexical_path_across_process_cwd_change(self):
        first = self.root / "first-cwd"
        second = self.root / "second-cwd"
        first.mkdir(mode=0o700)
        second.mkdir(mode=0o700)
        original_cwd = Path.cwd()
        acquire = StoreWriterLease.acquire

        def acquire_then_change_cwd(database):
            lease = acquire(database)
            os.chdir(second)
            return lease

        try:
            os.chdir(first)
            with patch.object(StoreWriterLease, "acquire", side_effect=acquire_then_change_cwd):
                store = Store("observatory.db", writer_lease=True)
            store.close(timeout=3)
        finally:
            os.chdir(original_cwd)
        self.assertTrue((first / "observatory.db").is_file())
        self.assertFalse((second / "observatory.db").exists())

    def test_sigkill_releases_process_lock(self):
        path = self.database()
        process = self.hold_in_child(path)
        process.kill()
        process.wait(timeout=5)
        process.stdin.close()
        process.stdout.close()
        process.stderr.close()
        self.assertEqual(self.probe(path).stdout.strip(), "ACQUIRED")


class StoreWriterLeasePlatformTests(unittest.TestCase):
    def test_non_linux_opt_in_fails_but_feature_off_mode_stays_available(self):
        with tempfile.TemporaryDirectory(prefix="agenturm-writer-platform-") as temporary:
            root = Path(temporary)
            root.chmod(0o700)
            path = root / "store.db"
            with patch("observatory.store_writer_lease.sys.platform", "darwin"):
                with self.assertRaisesRegex(StoreWriterLeaseError, "Linux-only"):
                    StoreWriterLease.acquire(path)
            store = Store(path, writer_lease=False)
            store.close()
            self.assertFalse((root / ".store.db.agenturm-writer.lock").exists())


if __name__ == "__main__":
    unittest.main()
