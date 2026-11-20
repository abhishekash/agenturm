"""Explicit auxiliary-store handoff never reports a live handle as drained."""
import sqlite3
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

from observatory.saved_tasks import TaskError, TaskStore, backup_task_database
from observatory.native_lifecycle import record_uncertain
from observatory.workflow_store import WorkflowStore, WorkflowStoreError


class AuxiliaryStoreHandoffTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def _hold(self, open_context):
        entered, release = threading.Event(), threading.Event()
        errors = []

        def worker():
            try:
                with open_context() as db:
                    self.assertEqual(db.execute('SELECT 1').fetchone()[0], 1)
                    entered.set()
                    self.assertTrue(release.wait(5), 'held SQLite handle was not released')
                    self.assertEqual(db.execute('SELECT 1').fetchone()[0], 1)
            except BaseException as exc:
                errors.append(exc)

        thread = threading.Thread(target=worker, daemon=True)
        thread.start()
        self.assertTrue(entered.wait(5), 'SQLite context did not open')
        self.addCleanup(thread.join, 5)
        self.addCleanup(release.set)
        return release, thread, errors

    def test_workflow_handoff_waits_for_existing_transaction(self):
        store = WorkflowStore(self.root / 'workflow.sqlite3')
        release, thread, errors = self._hold(lambda: store._transaction(write=False))
        self.assertFalse(store.quiesce_for_writer_handoff(0.02))
        with self.assertRaisesRegex(WorkflowStoreError, 'handoff'):
            with store._transaction():
                pass
        with self.assertRaisesRegex(WorkflowStoreError, 'handoff'):
            record_uncertain(store, {'sender_id': 'sender', 'receiver_id': 'receiver'}, 'closed')
        release.set()
        thread.join(5)
        self.assertFalse(errors)
        self.assertTrue(store.quiesce_for_writer_handoff(0.5))
        # An instance-local fence does not stop another instance at this path.
        with WorkflowStore(self.root / 'workflow.sqlite3')._transaction(write=False) as db:
            self.assertEqual(db.execute('SELECT 1').fetchone()[0], 1)

    def test_task_handoff_waits_for_existing_connection(self):
        store = TaskStore(self.root / 'tasks.sqlite3')
        release, thread, errors = self._hold(store._connect)
        self.assertFalse(store.quiesce_for_writer_handoff(0.02))
        with self.assertRaisesRegex(TaskError, 'handoff'):
            with store._connect():
                pass
        release.set()
        thread.join(5)
        self.assertFalse(errors)
        self.assertTrue(store.quiesce_for_writer_handoff(0.5))
        with self.assertRaisesRegex(TaskError, 'handoff'):
            store.backup(self.root / 'after-handoff.sqlite3')

    def test_task_handoff_waits_for_instance_backup(self):
        store = TaskStore(self.root / 'tasks.sqlite3')
        entered, release = threading.Event(), threading.Event()
        errors = []

        def held_backup(_path, _destination, *, on_close_failure):
            entered.set()
            self.assertTrue(release.wait(5), 'backup was never released')
            return 'backup complete'

        def worker():
            try:
                self.assertEqual(store.backup(self.root / 'backup.sqlite3'), 'backup complete')
            except BaseException as exc:
                errors.append(exc)

        with patch('observatory.saved_tasks._backup_task_database', side_effect=held_backup):
            thread = threading.Thread(target=worker, daemon=True)
            thread.start()
            self.assertTrue(entered.wait(5), 'instance backup did not start')
            try:
                self.assertFalse(store.quiesce_for_writer_handoff(0.02))
                with self.assertRaisesRegex(TaskError, 'handoff'):
                    store.backup(self.root / 'second.sqlite3')
            finally:
                release.set()
                thread.join(5)
        self.assertFalse(errors)
        self.assertTrue(store.quiesce_for_writer_handoff(0.5))

    def test_connection_cleanup_failure_stays_failed(self):
        class FailingClose:
            def __init__(self, real):
                self.real = real

            def __getattr__(self, name):
                return getattr(self.real, name)

            def close(self):
                self.real.close()
                raise OSError('injected close failure')

            def __enter__(self):
                self.real.__enter__()
                return self

            def __exit__(self, *args):
                return self.real.__exit__(*args)

        for store_type, path, module in (
            (WorkflowStore, self.root / 'workflow.sqlite3', 'observatory.workflow_store'),
            (TaskStore, self.root / 'tasks.sqlite3', 'observatory.saved_tasks'),
        ):
            with self.subTest(store=store_type.__name__):
                store = store_type(path)
                real_connect = sqlite3.connect
                with patch(module + '.sqlite3.connect',
                           side_effect=lambda *args, **kwargs: FailingClose(
                               real_connect(*args, **kwargs))):
                    with self.assertRaisesRegex(OSError, 'injected close failure'):
                        with (store._transaction(write=False) if store_type is WorkflowStore
                              else store._connect()) as db:
                            self.assertEqual(db.execute('SELECT 1').fetchone()[0], 1)
                self.assertFalse(store.quiesce_for_writer_handoff(0.1))
                self.assertFalse(store.quiesce_for_writer_handoff(0.1))

    def test_backup_closes_both_handles_and_unlinks_on_close_failure(self):
        source = TaskStore(self.root / 'tasks.sqlite3')
        closed = []

        class FakeSource:
            def backup(self, _target):
                pass

            def close(self):
                closed.append('source')

        class FakeTarget:
            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def close(self):
                closed.append('target')
                raise OSError('target close failed')

        with patch('observatory.saved_tasks.sqlite3.connect',
                   side_effect=(FakeSource(), FakeTarget())):
            with self.assertRaisesRegex(OSError, 'target close failed'):
                source.backup(self.root / 'backup.sqlite3')
        self.assertEqual(closed, ['target', 'source'])
        self.assertFalse((self.root / 'backup.sqlite3').exists())
        self.assertFalse(source.quiesce_for_writer_handoff(0.1))

    def test_backup_source_close_failure_also_fails_handoff(self):
        source = TaskStore(self.root / 'tasks.sqlite3')
        closed = []

        class FakeSource:
            def backup(self, _target):
                pass

            def close(self):
                closed.append('source')
                raise OSError('source close failed')

        class FakeTarget:
            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def close(self):
                closed.append('target')

        with patch('observatory.saved_tasks.sqlite3.connect',
                   side_effect=(FakeSource(), FakeTarget())):
            with self.assertRaisesRegex(OSError, 'source close failed'):
                source.backup(self.root / 'source-failed.sqlite3')
        self.assertEqual(closed, ['target', 'source'])
        self.assertFalse((self.root / 'source-failed.sqlite3').exists())
        self.assertFalse(source.quiesce_for_writer_handoff(0.1))

    def test_timeout_validation_and_standalone_backup_is_separate(self):
        tasks = TaskStore(self.root / 'tasks.sqlite3')
        workflow = WorkflowStore(self.root / 'workflow.sqlite3')
        for store in (tasks, workflow):
            for invalid in (None, True, 0, -1, 61, float('inf'), float('nan'), 1e308):
                with self.subTest(store=type(store).__name__, invalid=invalid):
                    with self.assertRaises(ValueError):
                        store.quiesce_for_writer_handoff(invalid)
        self.assertTrue(tasks.quiesce_for_writer_handoff(0.1))
        # This module-level function has no instance fence; migration must
        # account for it separately even after TaskStore has quiesced.
        target = self.root / 'standalone.sqlite3'
        backup_task_database(tasks.path, target)
        self.assertTrue(target.exists())


if __name__ == '__main__':
    unittest.main()
