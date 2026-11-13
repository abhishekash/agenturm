"""Actual UI teardown yields to pending publications while durable writes drain."""

import asyncio
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
from tests.test_attention import item, snapshot


class ReceiptShutdownTests(unittest.IsolatedAsyncioTestCase):
    async def test_textual_shutdown_with_pending_publication_preserves_accepted_receipts(
        self,
    ):
        with tempfile.TemporaryDirectory() as directory:
            entered = threading.Event()
            release = threading.Event()
            path = Path(directory) / "workflow.db"

            class Harness(App):
                def __init__(self):
                    super().__init__()
                    self.feed = SimpleNamespace(
                        snapshot=snapshot(time.time()),
                        api=SimpleNamespace(endpoint="http://localhost:14318"),
                    )
                    self.preferences = SimpleNamespace(enabled=True)
                    self.workflow_notify = False
                    self.workflow_snooze = 0
                    self.workflow_ledger = SimpleNamespace(rows=[])
                    self.last_update = 0
                    self.controller = WorkflowController(self, store_path=path)

                def compose(self):
                    yield Label("Receipt drain harness")

                def call_from_thread(self, callback, *args):
                    entered.set()
                    release.wait(3)
                    return super().call_from_thread(callback, *args)

                async def on_unmount(self):
                    await asyncio.to_thread(self.controller.close)

            app = Harness()
            done = item(state="Completed", waiting_for="")
            async with app.run_test(size=(60, 18)):
                options = dict(
                    endpoint=app.feed.api.endpoint, persist=True, notify=False, snooze=0
                )
                app.controller._enqueue(
                    ("update", ([done], app.feed.snapshot, {}, options))
                )
                self.assertTrue(await asyncio.to_thread(entered.wait, 2))
                app.controller.acknowledge(done, action="reviewed")
                event = dict(
                    service="codex",
                    agent_id="worker",
                    session_id="session",
                    trace_id="run",
                    id="displayed-error",
                    result="error",
                )
                app.controller.error_displayed([event], snapshot=app.feed.snapshot)
                asyncio.get_running_loop().call_later(0.05, release.set)
                app.exit()
            persisted = WorkflowStore(path)
            self.assertTrue(
                any(
                    record.get("acknowledgement") == "reviewed"
                    for record in persisted.load()
                )
            )
            self.assertIn(
                event_receipt(event, app.feed.snapshot, app.feed.api.endpoint),
                persisted.load_error_seen(),
            )
            self.assertFalse(app.controller._running)
            self.assertNotIn("timed out", app.controller.warning)
