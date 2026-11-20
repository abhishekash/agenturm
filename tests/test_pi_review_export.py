"""Transient Pi user-text parity in Review JSON exports."""
import json
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace

from observatory.ui_export import review_investigation_json
from observatory.ui_review import pi_review_request_record


class PiReviewExportTests(unittest.TestCase):
    def setUp(self):
        self.projection = {'format': 'agenturm-review-v1', 'request': {
            'text': 'content-free request label', 'source': {'id': 'plugin-event:' + 'a' * 64}},
            'agent_update': None, 'checks': []}
        self.pi_text = {
            'source': {'id': 'plugin-event:' + 'a' * 64,
                       'source_id': 'plugin-event:' + 'a' * 64,
                       'service': 'pi', 'session_id': 'pi:s', 'turn_id': 'turn:1',
                       'receiver_id': 'receiver:1', 'time_unix_nano': 123},
            'text': 'password=[REDACTED]', 'revision': (3, 4, 5), 'clipped': False,
        }

    def test_opt_out_omits_private_text_and_requested_command(self):
        encoded = review_investigation_json(self.projection)
        self.assertNotIn('pi_user_text', encoded)
        self.assertNotIn('redacted_text', encoded)
        self.assertNotIn('requested_command', encoded)
        self.assertNotIn('password=', encoded)

    def test_opt_in_exports_only_exact_redacted_pi_user_text_reference(self):
        payload = json.loads(review_investigation_json(self.projection, self.pi_text))
        self.assertEqual(payload['pi_user_text']['source'], self.pi_text['source'])
        self.assertEqual(payload['pi_user_text']['redacted_text'], 'password=[REDACTED]')
        self.assertEqual(payload['pi_user_text']['revision'], [3, 4, 5])
        self.assertEqual(payload['pi_user_text']['capture'], 'opt-in source-backed redacted text')
        self.assertNotIn('requested_command', json.dumps(payload))

    def test_malformed_or_unbound_text_is_rejected(self):
        for bad in ({'text': 'x', 'revision': (1, 2, 3)},
                    {'source': {}, 'text': 'x', 'revision': (1, 2)},
                    {'source': {}, 'text': 'x', 'revision': 'stale'}):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                review_investigation_json(self.projection, bad)

    def test_review_selects_only_exact_scoped_pi_user_message(self):
        wanted={'source':'plugin','domain':'message','action':'user',
                'id':'plugin-event:'+'a'*64,'source_id':'plugin-event:'+'a'*64,
                'service':'pi','agent_id':'agent','session_id':'session',
                'turn_id':'turn:1','time_unix_nano':10}
        later={**wanted,'id':'plugin-event:'+'b'*64,'source_id':'plugin-event:'+'b'*64,
               'time_unix_nano':20}
        other_turn={**later,'id':'plugin-event:'+'c'*64,
                    'source_id':'plugin-event:'+'c'*64,'turn_id':'turn:2',
                    'time_unix_nano':30}
        native={**later,'id':'ordinary','source':'native','time_unix_nano':40}
        scope=SimpleNamespace(matches=lambda row: row.get('turn_id')=='turn:1')
        history=SimpleNamespace(events={row['id']:row for row in
                                        (wanted,later,other_turn,native)})
        # An exact request reference wins; absent one, only the latest event
        # matching the active user turn is eligible.
        self.assertEqual(pi_review_request_record(history,self.projection,scope),wanted)
        self.assertEqual(pi_review_request_record(history,{},scope),later)


class PiReviewExportScreenTests(unittest.IsolatedAsyncioTestCase):
    async def test_revoke_after_preview_proof_withholds_json_and_clipboard(self):
        from textual.app import App
        from textual.widgets import Input, Static
        from observatory.ui_export import ExportScreen

        allowed = [True]
        class Harness(App):
            def compose(self):
                yield Static('Review')
            def on_mount(self):
                self.push_screen(ExportScreen('review', {}, content='REDACTED_PI_TEXT',
                    can_export=lambda: allowed[0], poll_policy=False, allow_clipboard=False))

        with tempfile.TemporaryDirectory() as directory:
            target=Path(directory)/'review.json'
            async with Harness().run_test(size=(80,24)) as pilot:
                await pilot.pause()
                screen=pilot.app.screen
                self.assertIsInstance(screen,ExportScreen)
                self.assertIn('REDACTED_PI_TEXT',screen.content)
                allowed[0]=False
                screen.query_one('#export-path',Input).value=str(target)
                screen.confirm()
                self.assertFalse(target.exists())
                self.assertEqual(screen.content,'')
                self.assertTrue(screen.query_one('#export-confirm').disabled)

    async def test_expiry_while_json_preview_is_open_clears_and_disables_it(self):
        from textual.app import App
        from textual.widgets import Static
        from observatory.ui_export import ExportScreen

        expires_at=time.monotonic()+.15
        class Harness(App):
            def compose(self):yield Static('Review')
            def on_mount(self):
                self.push_screen(ExportScreen('review',{},content='REDACTED_PI_TEXT',
                    can_export=lambda:time.monotonic()<expires_at,poll_policy=True,
                    preview_valid=lambda:time.monotonic()<expires_at,allow_clipboard=False))
        async with Harness().run_test(size=(80,24)) as pilot:
            await pilot.pause(.4)
            screen=pilot.app.screen
            self.assertEqual(screen.content,'')
            self.assertTrue(screen.query_one('#export-confirm').disabled)

    async def test_revocation_while_json_preview_is_open_clears_and_disables_it(self):
        from textual.app import App
        from textual.widgets import Static
        from observatory.ui_export import ExportScreen

        permitted=[True]
        class Harness(App):
            def compose(self):yield Static('Review')
            def on_mount(self):
                self.push_screen(ExportScreen('review',{},content='REDACTED_PI_TEXT',
                    can_export=lambda:permitted[0],poll_policy=True,
                    preview_valid=lambda:permitted[0],allow_clipboard=False))
        async with Harness().run_test(size=(80,24)) as pilot:
            await pilot.pause()
            screen=pilot.app.screen
            self.assertIn('REDACTED_PI_TEXT',screen.content)
            permitted[0]=False
            await pilot.pause(.4)
            self.assertEqual(screen.content,'')
            self.assertTrue(screen.query_one('#export-confirm').disabled)


if __name__ == '__main__':
    unittest.main()
