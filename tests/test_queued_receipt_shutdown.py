"""Accepted receipts survive Textual cancellation of queued thread workers."""
import asyncio
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from textual.app import App
from textual.widgets import Label
from observatory.ui_workflow_controller import WorkflowController
from observatory.workflow_store import WorkflowStore
from observatory.error_reminders import event_receipt
from tests.test_attention import snapshot


class QueuedReceiptShutdownTests(unittest.IsolatedAsyncioTestCase):
    async def test_cancelled_not_started_worker_flushes_accepted_error_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'workflow.db'
            class Harness(App):
                def __init__(self):
                    super().__init__()
                    self.feed = SimpleNamespace(snapshot=snapshot(time.time()),
                        api=SimpleNamespace(endpoint='http://localhost:14318'))
                    self.preferences = SimpleNamespace(enabled=True)
                    self.workflow_notify = False
                    self.workflow_snooze = 0
                    self.workflow_ledger = SimpleNamespace(rows=[])
                    self.last_update = 0
                    self.controller = WorkflowController(self, store_path=path)
                def compose(self):
                    yield Label('Queued receipt shutdown')
                async def on_unmount(self):
                    await asyncio.to_thread(self.controller.close)
            app = Harness()
            started, release = threading.Event(), threading.Event()
            event = dict(service='codex', agent_id='worker', session_id='session',
                         trace_id='run', id='displayed-error', result='error')
            loop = asyncio.get_running_loop()
            executor = ThreadPoolExecutor(max_workers=1)
            loop.set_default_executor(executor)
            blocker = None
            try:
                async with app.run_test(size=(60, 18)):
                    # run_test is headless; explicitly establish durable
                    # storage before accepting the receipt under test.
                    app.controller._open_storage(True)
                    self.assertIsNotNone(app.controller.store)
                    self.assertFalse(app.controller._memory)
                    # Occupy the only executor slot: the Textual worker has
                    # accepted the job but cannot enter the controller drain.
                    blocker = loop.run_in_executor(None, lambda: (started.set(), release.wait(5)))
                    while not started.is_set():
                        await asyncio.sleep(.001)
                    app.controller.error_displayed([event], snapshot=app.feed.snapshot)
                    await asyncio.sleep(.05)
                    self.assertTrue(app.controller._running)
                    self.assertEqual(len(app.controller._queue), 1)
                    loop.call_later(.1, release.set)
                    app.exit()
                persisted = WorkflowStore(path)
                self.assertIn(event_receipt(event, app.feed.snapshot, app.feed.api.endpoint),
                              persisted.load_error_seen())
                self.assertFalse(app.controller._running)
                self.assertEqual(app.controller._queue, [])
                self.assertNotIn('timed out', app.controller.warning)
            finally:
                release.set()
                if blocker is not None:
                    await blocker
                executor.shutdown(wait=True)


class DuplicateDrainShutdownTests(unittest.TestCase):
    def test_cancelled_worker_start_racing_rescue_never_toggles_receipt_twice(self):
        from tests.test_attention import item
        from tests.test_workflow_controller import Owner
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as directory:
            owner = Owner()
            owner.workflow_notify = False
            controller = WorkflowController(owner, store_path=Path(directory) / 'workflow.db')
            done = item(state='Completed', waiting_for='')
            controller.update([done], owner.feed.snapshot, {})
            owner.wait()
            accepted = []
            entered, release = threading.Event(), threading.Event()
            original = controller._ack
            def ack(*args):
                entered.set()
                if not release.wait(3):
                    raise AssertionError('Generated acknowledgement barrier expired')
                return original(*args)
            with patch.object(owner, 'run_worker', side_effect=lambda callback, **kwargs: accepted.append(callback)), \
                 patch.object(controller, '_ack', side_effect=ack) as write:
                controller.acknowledge(done, action='reviewed')
                self.assertEqual(len(accepted), 1)
                close = threading.Thread(target=controller.close)
                close.start()
                late = None
                try:
                    self.assertTrue(entered.wait(2))
                    # The original Textual callback starts after the shutdown
                    # drainer claimed the queue but before its write completes.
                    late = threading.Thread(target=accepted[0])
                    late.start()
                    late.join(1)
                    self.assertFalse(late.is_alive())
                finally:
                    release.set()
                    close.join(4)
                    if late is not None:
                        late.join(3)
                self.assertFalse(close.is_alive())
                self.assertEqual(write.call_count, 1)
            self.assertFalse(controller._running)
            self.assertEqual(controller._queue, [])
            self.assertNotIn('timed out', controller.warning)
            records = WorkflowStore(controller.store_path).load()
            self.assertEqual(sum(row.get('acknowledgement') == 'reviewed' for row in records), 1)


if __name__ == '__main__':
    unittest.main()
