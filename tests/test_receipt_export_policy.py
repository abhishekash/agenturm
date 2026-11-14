"""Receipt policy is checked at preview, confirmation and queued worker egress."""
import asyncio
from pathlib import Path
from tempfile import TemporaryDirectory
import threading
import unittest

from textual.app import App
from textual.widgets import Static,RichLog,Input,Button
from observatory.ui_export import ReceiptExportScreen


class ReceiptHarness(App):
    def __init__(self,valid):super().__init__();self.valid=valid
    def compose(self):yield Static('Review')
    def on_mount(self):
        self.push_screen(ReceiptExportScreen(dict(markdown='PEER_PRIVATE_TEXT',html='<p>PEER_PRIVATE_TEXT</p>',json='{}'),valid=self.valid))


class ReceiptPolicyJourney(unittest.IsolatedAsyncioTestCase):
    async def test_observed_denial_clears_preview_and_cannot_revive_after_restore(self):
        allowed=[True];app=ReceiptHarness(lambda:allowed[0])
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();screen=app.screen
            allowed[0]=False;self.assertFalse(screen.check_receipt_policy())
            self.assertEqual(screen.formats,{});self.assertEqual(screen.content,'')
            self.assertFalse(screen.query_one('#export-preview',RichLog).lines)
            allowed[0]=True
            self.assertFalse(screen.receipt_allowed())
            self.assertTrue(screen.query_one('#export-confirm',Button).disabled)

    async def test_queued_worker_rechecks_policy_before_any_file_is_created(self):
        allowed=[True];entered=threading.Event();release=threading.Event()
        def valid():
            if threading.current_thread() is not threading.main_thread():
                entered.set();release.wait(5)
            return allowed[0]
        app=ReceiptHarness(valid)
        with TemporaryDirectory() as directory:
            path=Path(directory)/'receipt.md'
            async with app.run_test(size=(80,24)) as pilot:
                await pilot.pause();screen=app.screen
                screen.query_one('#export-path',Input).value=str(path)
                screen.confirm()
                self.assertTrue(await asyncio.to_thread(entered.wait,3))
                allowed[0]=False;release.set()
                for _ in range(20):
                    await pilot.pause(.03)
                    if screen._export_invalidated:break
                self.assertTrue(screen._export_invalidated)
                self.assertFalse(path.exists());self.assertFalse(path.with_suffix('.html').exists())
                allowed[0]=True;self.assertFalse(screen.receipt_allowed())


if __name__=='__main__':unittest.main()
