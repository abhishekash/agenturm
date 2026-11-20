"""Explicit Pi user-text delivery remains exact, transient, and content-free elsewhere."""
import json
import threading
import time
import unittest
from types import SimpleNamespace
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

from observatory.plugin_admission import set_source_capture
from observatory.plugin_pi_content_read import Available, read_exact_pi_user_text
from observatory.plugin_projection import project_source
from observatory.plugin_supervisor import PluginSupervisor
from observatory.server import BoundedHTTPServer, handler_for
from observatory.terminal_ui import ObservatoryApp, ObservatoryScreen, copy_event_text
from observatory.ui_export import investigation_json
from observatory.ui_plugin_content import accepted_preview, eligible


class PluginContentDeliveryTests(unittest.TestCase):
    def setUp(self):
        from tests.test_plugin_pi_content import PiContentTests
        self.fixture = PiContentTests('runTest')
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.store = self.fixture.store
        supervisor = object.__new__(PluginSupervisor)
        supervisor.store = self.store
        supervisor.sources = (self.fixture.source,)
        supervisor.stop = threading.Event()
        self.store.plugin_supervisor = supervisor
        batch, ref = self.fixture.bundle()
        self.fixture.admit(batch, ref)
        project_source(self.store, 'pi-directory')
        with self.store._read() as db:
            self.event_id = db.execute("SELECT event_key FROM activity_index_v1 "
                                       "WHERE kind='plugin'").fetchone()[0]
        self.handler = handler_for(self.store, 'secret', quiet=True)
        self.addCleanup(self.handler.response_cache.close)
        self.server = BoundedHTTPServer(('127.0.0.1', 0), self.handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)

    def get(self, suffix, *, authorized=True):
        request = Request(f'http://127.0.0.1:{self.server.server_port}' + suffix,
                          headers={'Authorization': 'Bearer secret'} if authorized else {})
        try:
            with urlopen(request, timeout=5) as response:
                return response.status, json.load(response)
        except HTTPError as exc:
            return exc.code, json.loads(exc.read())

    def path(self):
        return '/api/plugin-content?' + urlencode({'event_id': self.event_id})

    def test_exact_redacted_response_and_no_cache_replay_after_revoke(self):
        self.assertEqual(self.get(self.path(), authorized=False)[0], 401)
        status, response = self.get(self.path())
        self.assertEqual(status, 200)
        self.assertEqual(response['event_id'], self.event_id)
        self.assertEqual(response['redacted_text'], 'password=[REDACTED]')
        self.assertEqual(len(response['revision']), 3)
        self.assertNotIn('highly-private', json.dumps(response))
        self.assertNotIn(str(self.fixture.sources), json.dumps(response))
        set_source_capture(self.store, source_id='pi-directory', enabled=False)
        status, denied = self.get(self.path())
        self.assertEqual(status, 404)
        self.assertNotIn('password=', json.dumps(denied))

    def test_exact_query_and_source_grant_are_required(self):
        self.assertEqual(self.get('/api/plugin-content')[0], 400)
        self.assertEqual(self.get(self.path() + '&event_id=duplicate')[0], 400)
        self.assertEqual(self.get('/api/plugin-content?event_id=invalid')[0], 400)
        with patch('observatory.receiver_state.loopback_host', return_value=False):
            self.assertEqual(self.get(self.path())[0], 403)
        with patch('observatory.receiver_state.loopback_host', side_effect=[True, False]):
            self.assertEqual(self.get(self.path())[0], 403)
        self.store.plugin_supervisor = None
        status, denied = self.get(self.path())
        self.assertEqual(status, 404)
        self.assertNotIn('password=', json.dumps(denied))

    def test_revoke_between_first_proof_and_send_withholds_body(self):
        from observatory import plugin_pi_content_read as reader
        original = reader.read_exact_pi_user_text
        count = 0

        def revoke(store, event_id):
            nonlocal count
            result = original(store, event_id)
            count += 1
            if count == 1 and isinstance(result, Available):
                set_source_capture(store, source_id='pi-directory', enabled=False)
            return result

        with patch.object(reader, 'read_exact_pi_user_text', side_effect=revoke):
            status, denied = self.get(self.path())
        self.assertEqual(status, 409)
        self.assertGreaterEqual(count, 2)
        self.assertNotIn('password=', json.dumps(denied))

    def test_same_inode_rewrite_between_proofs_withholds_body(self):
        from observatory import plugin_pi_content_read as reader
        original = reader.read_exact_pi_user_text
        count = 0
        inode = self.fixture.file.stat().st_ino

        def rewrite(store, event_id):
            nonlocal count
            result = original(store, event_id)
            count += 1
            if count == 1 and isinstance(result, Available):
                self.fixture.file.write_bytes(self.fixture.header + self.fixture.user.replace(
                    b'highly-private', b'another-secret'))
                self.assertEqual(self.fixture.file.stat().st_ino, inode)
            return result

        with patch.object(reader, 'read_exact_pi_user_text', side_effect=rewrite):
            status, denied = self.get(self.path())
        self.assertEqual(status, 409)
        self.assertGreaterEqual(count, 2)
        self.assertNotIn('password=', json.dumps(denied))

    def test_source_rewrite_closes(self):
        self.fixture.file.write_bytes(self.fixture.header +
                                      self.fixture.user.replace(b'highly-private', b'another-secret'))
        self.assertEqual(self.get(self.path())[0], 404)

    def deliver_new_text(self, text):
        from tests.test_plugin_pi_content import PiContentTests
        fixture = PiContentTests('runTest')
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        fixture.user = fixture.user_line('user-1', text)
        fixture.file.write_bytes(fixture.header + fixture.user)
        batch, ref = fixture.bundle()
        fixture.admit(batch, ref)
        project_source(fixture.store, 'pi-directory')
        supervisor = object.__new__(PluginSupervisor)
        supervisor.store = fixture.store
        supervisor.sources = (fixture.source,)
        supervisor.stop = threading.Event()
        fixture.store.plugin_supervisor = supervisor
        with fixture.store._read() as db:
            event_id = db.execute("SELECT event_key FROM activity_index_v1 "
                                  "WHERE kind='plugin'").fetchone()[0]
        handler = handler_for(fixture.store, 'secret', quiet=True)
        self.addCleanup(handler.response_cache.close)
        server = BoundedHTTPServer(('127.0.0.1', 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        request = Request(f'http://127.0.0.1:{server.server_port}/api/plugin-content?'
                          + urlencode({'event_id': event_id}),
                          headers={'Authorization': 'Bearer secret'})
        with urlopen(request, timeout=5) as response:
            return json.load(response)

    def test_empty_user_text_is_delivered_as_available(self):
        self.assertEqual(self.deliver_new_text('')['redacted_text'], '')

    def test_response_text_is_bounded(self):
        response = self.deliver_new_text('x' * 5000)
        self.assertTrue(response['clipped'])
        self.assertEqual(len(response['redacted_text'].encode('utf-8')), 4096)


class PluginContentUiTests(unittest.TestCase):
    def setUp(self):
        self.row = {'source': 'plugin', 'domain': 'message', 'action': 'user',
                    'id': 'plugin-event:' + 'a' * 64, 'source_id': 'plugin-event:' + 'a' * 64,
                    'receiver_id': 'receiver', 'session_id': 'session',
                    'agent_id': 'agent', 'service': 'fixture.pi', 'time_unix_nano': 1,
                    'subject': 'User message recorded', 'detail': '',
                    'metadata': {'plugin_source_id': 'pi-directory'}}
        self.response = {'receiver_id': 'receiver', 'event_id': self.row['id'],
                         'redacted_text': 'password=[REDACTED]', 'clipped': False,
                         'revision': [7, 1, 2]}

    def test_exact_response_validation_and_copy_export_isolation(self):
        self.assertTrue(eligible(self.row))
        self.assertEqual(accepted_preview(self.response, self.row, 'receiver',
                                          (7, 1, 2), (7, 1, 2)),
                         ('password=[REDACTED]', False))
        self.assertNotIn('password=', copy_event_text(self.row))
        self.assertNotIn('password=', investigation_json('activity', self.row))
        self.assertIsNone(accepted_preview(self.response, self.row, 'receiver',
                                           (7, 1, 2), (7, 1, 3)))
        self.assertIsNone(accepted_preview({**self.response, 'receiver_id': 'other'},
                                           self.row, 'receiver', (7, 1, 2), (7, 1, 2)))
        self.assertIsNone(accepted_preview({**self.response, 'redacted_text': '\x1bsecret'},
                                           self.row, 'receiver', (7, 1, 2), (7, 1, 2)))

    def test_selection_change_discards_async_result(self):
        current = {**self.row, 'id': 'plugin-event:' + 'b' * 64}
        screen = SimpleNamespace()
        screen.is_mounted = True
        screen.screen = ObservatoryScreen()
        screen.plugin_text_generation = 4
        screen.plugin_text_state = {'record': self.row, 'receiver_id': 'receiver',
                                    'status': 'loading', 'text': '', 'clipped': False}
        screen.feed = SimpleNamespace(snapshot=SimpleNamespace(receiver_id='receiver'))
        screen.table_data = {'activity-table': {current['id']: current}}
        screen.selected_activity = current['id']
        screen.query_one = lambda *_args: SimpleNamespace(active='activity')
        updated = []
        screen.show_activity = lambda: updated.append('shown')
        screen.set_timer = lambda *_args: updated.append('timer')
        ObservatoryApp.finish_plugin_text(screen, 4, self.row, 'receiver',
                                          ('password=[REDACTED]', False))
        self.assertEqual(screen.plugin_text_state['status'], 'loading')
        self.assertEqual(updated, [])

    def test_revoke_after_display_clears_transient_text(self):
        screen = SimpleNamespace()
        screen.is_mounted = True
        screen.screen = ObservatoryScreen()
        screen.plugin_text_generation = 4
        screen.plugin_text_state = {'record': self.row, 'receiver_id': 'receiver',
                                    'status': 'available', 'text': 'password=[REDACTED]',
                                    'clipped': False, 'revision': (7, 1, 2)}
        screen.feed = SimpleNamespace(snapshot=SimpleNamespace(receiver_id='receiver'))
        screen.table_data = {'activity-table': {self.row['id']: self.row}}
        screen.selected_activity = self.row['id']
        screen.query_one = lambda *_args: SimpleNamespace(active='activity')
        updated = []
        screen.show_activity = lambda: updated.append('shown')
        screen.set_timer = lambda *_args: updated.append('timer')
        screen._clear_plugin_text_visual = lambda: updated.append('cleared')
        ObservatoryApp.finish_plugin_text_recheck(screen, 4, self.row, 'receiver', None)
        self.assertEqual(screen.plugin_text_state['status'], 'unavailable')
        self.assertEqual(screen.plugin_text_state['text'], '')
        self.assertEqual(updated, ['cleared', 'shown'])

    def test_pending_activity_invalidates_selection_and_receiver_before_return(self):
        for selected_id, receiver_id in ((('plugin-event:' + 'b' * 64), 'receiver'),
                                         (self.row['id'], 'different')):
            with self.subTest(selected_id=selected_id, receiver_id=receiver_id):
                screen = SimpleNamespace(
                    selected_activity=selected_id,
                    table_data={'activity-table': {selected_id: {**self.row, 'id': selected_id}}},
                    feed=SimpleNamespace(snapshot=SimpleNamespace(receiver_id=receiver_id)),
                    plugin_text_generation=4,
                    plugin_text_state={'record': self.row, 'receiver_id': 'receiver',
                                       'status': 'available', 'text': 'password=[REDACTED]',
                                       'expires_at': time.monotonic() + 20},
                    native_page_pending=lambda _page: True)
                visual = []
                screen.clear_plugin_text = lambda: ObservatoryApp.clear_plugin_text(screen)
                screen._clear_plugin_text_visual = lambda: visual.append('cleared')
                ObservatoryApp.show_activity(screen)
                self.assertIsNone(screen.plugin_text_state)
                self.assertEqual(screen.plugin_text_generation, 5)
                self.assertEqual(visual, ['cleared'])


class PluginContentLayoutTests(unittest.IsolatedAsyncioTestCase):
    async def test_minimum_activity_inspector_keeps_shell_and_explicit_hint(self):
        from tests.test_terminal_search import ManualFeed
        feed = ManualFeed()
        app = ObservatoryApp(feed, persist_preferences=False)
        row = {'source': 'plugin', 'domain': 'message', 'action': 'user',
               'id': 'plugin-event:' + 'a' * 64, 'source_id': 'plugin-event:' + 'a' * 64,
               'receiver_id': 'receiver', 'session_id': 'session', 'agent_id': 'agent',
               'service': 'fixture.pi', 'time_unix_nano': 1,
               'subject': 'User message recorded', 'detail': ''}
        async with app.run_test(size=(60, 18)) as pilot:
            await pilot.pause()
            app.refresh_timer.stop()
            await pilot.press('2')
            await pilot.pause()
            app.table_data['activity-table'] = {row['id']: row}
            app.selected_activity = row['id']
            app.native_page_pending = lambda _page: False
            header = app.query_one('#masthead').region
            app.action_expand()
            await pilot.pause()
            app.show_activity()
            await pilot.pause()
            self.assertIn('z Check for captured', app.query_one('#activity-detail').plain_text)
            await pilot.press('z')
            await pilot.pause()
            self.assertIn('Unavailable now', app.query_one('#activity-detail').plain_text)
            self.assertEqual(app.query_one('#masthead').region, header)
            app.plugin_text_state = {'record': row, 'receiver_id': '',
                                     'status': 'available', 'text': 'PRIVATE_PREVIEW',
                                     'clipped': False, 'expires_at': time.monotonic() + 20}
            app.show_activity()
            await pilot.pause()
            self.assertIn('PRIVATE_PREVIEW', app.query_one('#activity-detail').plain_text)
            app.native_page_pending = lambda _page: True
            app.selected_activity = 'plugin-event:' + 'b' * 64
            app.show_activity()
            await pilot.pause()
            self.assertIsNone(app.plugin_text_state)
            self.assertNotIn('PRIVATE_PREVIEW', app.query_one('#activity-detail').plain_text)
            self.assertEqual(app.query_one('#masthead').region, header)

    async def test_direct_tab_click_discards_preview_before_return(self):
        from tests.test_terminal_search import ManualFeed
        feed = ManualFeed()
        app = ObservatoryApp(feed, persist_preferences=False)
        row = {'source': 'plugin', 'domain': 'message', 'action': 'user',
               'id': 'plugin-event:' + 'a' * 64, 'source_id': 'plugin-event:' + 'a' * 64,
               'receiver_id': '', 'session_id': 'session', 'agent_id': 'agent',
               'service': 'fixture.pi', 'time_unix_nano': 1,
               'subject': 'User message recorded', 'detail': ''}
        async with app.run_test(size=(120, 32)) as pilot:
            await pilot.pause()
            app.refresh_timer.stop()
            await pilot.click('#--content-tab-activity')
            await pilot.pause()
            app.native_page_pending = lambda _page: False
            app.table_data['activity-table'] = {row['id']: row}
            app.selected_activity = row['id']
            app.plugin_text_generation = 4
            app.plugin_text_state = {'record': row, 'receiver_id': '',
                                     'status': 'available', 'text': 'PRIVATE_PREVIEW',
                                     'clipped': False, 'expires_at': time.monotonic() + 20}
            app.show_activity()
            await pilot.pause()
            self.assertIn('PRIVATE_PREVIEW', app.query_one('#activity-detail').plain_text)
            await pilot.click('#--content-tab-agents')
            await pilot.pause()
            self.assertIsNone(app.plugin_text_state)
            await pilot.click('#--content-tab-activity')
            await pilot.pause()
            app.table_data['activity-table'] = {row['id']: row}
            app.selected_activity = row['id']
            app.show_activity()
            await pilot.pause()
            self.assertNotIn('PRIVATE_PREVIEW', app.query_one('#activity-detail').plain_text)


if __name__ == '__main__':
    unittest.main()
