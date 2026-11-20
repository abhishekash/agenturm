"""Requested Pi command preview is exact, transient, and confined to Activity."""
import json
import time
import unittest
from types import SimpleNamespace
from unittest.mock import patch
from urllib.parse import urlencode

from observatory.terminal_ui import ObservatoryApp, ObservatoryScreen, copy_event_text
from observatory.terminal import _cockpit_app
from observatory.ui_export import investigation_json
from observatory.ui_pi_command_content import (
    PI_REQUESTED_COMMAND, eligible_requested_command,
)
from observatory.ui_plugin_content import accepted_preview


EVENT_ID = 'plugin-event:' + 'a' * 64
OPERATION_ID = 'plugin-operation:' + 'b' * 64
ROW = {'source': 'plugin', 'domain': 'tool', 'action': 'shell',
       'result': 'started', 'id': EVENT_ID, 'source_id': EVENT_ID,
       'operation_id': OPERATION_ID, 'receiver_id': 'receiver',
       'session_id': 'session', 'agent_id': 'agent', 'service': 'fixture.pi',
       'time_unix_nano': 1, 'subject': 'Shell operation recorded', 'detail': '',
       'metadata': {'plugin_source_id': 'pi-directory', 'capture': 'content-free',
                    'native_operation_observed': True}}
RESPONSE = {'receiver_id': 'receiver', 'event_id': EVENT_ID,
            'redacted_text': 'echo [REDACTED]', 'clipped': False,
            'revision': [7, 1, 2]}


class PiCommandProviderTests(unittest.TestCase):
    def test_interactive_composition_includes_command_provider(self):
        from tests.test_terminal_search import ManualFeed

        for panes in ((), ('pane-fixture',)):
            with self.subTest(panes=panes):
                if panes:
                    # The composition root forwards contributed panes without
                    # changing how a preview provider is registered.
                    with patch('observatory.terminal_ui.PaneRegistry') as registry:
                        app = _cockpit_app(ManualFeed(), ui_panes=panes)
                        registry.assert_called_once_with(panes)
                else:
                    app = _cockpit_app(ManualFeed())
                self.assertIs(app.content_preview_registry.get(
                    PI_REQUESTED_COMMAND.id), PI_REQUESTED_COMMAND)

    def test_real_pi_projection_is_eligible_without_command_in_activity(self):
        from observatory.plugin_admission import commit_batch
        from observatory.plugin_projection import project_source
        from tests.test_plugin_pi_command import PiCommandAdmissionTests

        fixture = PiCommandAdmissionTests('runTest')
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        batch, ref = fixture.bundle()
        commit_batch(fixture.store, fixture.lease, '', batch,
                     command_refs=(ref,), command_grant=fixture.grant)
        project_source(fixture.store, fixture.source.source_id)
        with fixture.store._read() as db:
            payload = db.execute("SELECT payload FROM activity_index_v1 WHERE kind='plugin'")
            record = json.loads(payload.fetchone()[0])
        self.assertTrue(eligible_requested_command(record))
        self.assertNotIn('highly-private', json.dumps(record))
        self.assertNotIn('password=', json.dumps(record))

    def test_projected_started_shell_only_and_no_content_export(self):
        self.assertTrue(eligible_requested_command(ROW))
        self.assertEqual(PI_REQUESTED_COMMAND.path(ROW),
                         '/api/plugin-command-content?' + urlencode({'event_id': EVENT_ID}))
        for change in ({'source': 'native'}, {'domain': 'check'},
                       {'action': 'other'}, {'result': 'completed'},
                       {'operation_id': None}, {'operation_id': 'plugin-operation:bad'},
                       {'metadata': {'plugin_source_id': 'pi-directory',
                                     'capture': 'content-free',
                                     'native_operation_observed': False}}):
            with self.subTest(change=change):
                self.assertFalse(eligible_requested_command({**ROW, **change}))
        self.assertNotIn('echo [REDACTED]', copy_event_text(ROW))
        self.assertNotIn('echo [REDACTED]', investigation_json('activity', ROW))

    def test_mocked_route_response_requires_current_exact_revision(self):
        self.assertEqual(accepted_preview(RESPONSE, ROW, 'receiver', (7, 1, 2),
                                          (7, 1, 2),
                                          matches=eligible_requested_command),
                         ('echo [REDACTED]', False))
        for response, before, after in (
                ({**RESPONSE, 'event_id': 'plugin-event:' + 'c' * 64}, (7, 1, 2), (7, 1, 2)),
                ({**RESPONSE, 'receiver_id': 'wrong'}, (7, 1, 2), (7, 1, 2)),
                ({**RESPONSE, 'redacted_text': '\x1bprivate'}, (7, 1, 2), (7, 1, 2)),
                (RESPONSE, (7, 1, 2), (7, 1, 3)),
                (RESPONSE, (7, 1, 3), (7, 1, 3))):
            with self.subTest(response=response, before=before, after=after):
                self.assertIsNone(accepted_preview(
                    response, ROW, 'receiver', before, after,
                    matches=eligible_requested_command))

    def test_worker_uses_only_the_registered_local_route(self):
        calls = []
        screen = SimpleNamespace(
            content_preview_registry=SimpleNamespace(
                get=lambda key: PI_REQUESTED_COMMAND if key == PI_REQUESTED_COMMAND.id else None,
                for_record=lambda record: PI_REQUESTED_COMMAND if record == ROW else None),
            feed=SimpleNamespace(api=object()), plugin_text_generation=4)
        screen.app = SimpleNamespace(call_from_thread=lambda *args: calls.append(args))
        screen.finish_plugin_text = lambda *_args: None
        with (patch('observatory.terminal_ui.current_plugin_activity_revision',
                    side_effect=[(7, 1, 2), (7, 1, 2)]),
              patch('observatory.terminal_ui.shared_get', return_value=RESPONSE) as get):
            ObservatoryApp.fetch_plugin_text.__wrapped__(
                screen, 4, ROW, 'receiver', PI_REQUESTED_COMMAND.id)
        self.assertEqual(get.call_args.args[1], PI_REQUESTED_COMMAND.path(ROW))
        self.assertEqual(calls[0][1:],
                         (4, ROW, 'receiver', ('echo [REDACTED]', False, (7, 1, 2))))

    def test_recheck_revocation_removes_visible_text(self):
        state = {'record': ROW, 'receiver_id': 'receiver',
                 'provider_id': PI_REQUESTED_COMMAND.id, 'status': 'available',
                 'text': 'PRIVATE_COMMAND', 'clipped': False, 'revision': (7, 1, 2)}
        updates = []
        screen = SimpleNamespace(
            is_mounted=True, screen=ObservatoryScreen(), plugin_text_generation=4,
            plugin_text_state=state, content_preview_registry=SimpleNamespace(
                for_record=lambda _: PI_REQUESTED_COMMAND),
            feed=SimpleNamespace(snapshot=SimpleNamespace(receiver_id='receiver')),
            table_data={'activity-table': {EVENT_ID: ROW}}, selected_activity=EVENT_ID,
            query_one=lambda *_args: SimpleNamespace(active='activity'),
            show_activity=lambda: updates.append('shown'),
            set_timer=lambda *_args: updates.append('timer'),
            _clear_plugin_text_visual=lambda: updates.append('cleared'))
        ObservatoryApp.finish_plugin_text_recheck(screen, 4, ROW, 'receiver', None)
        self.assertEqual((state['status'], state['text']), ('unavailable', ''))
        self.assertEqual(updates, ['cleared', 'shown'])


class PiCommandLayoutTests(unittest.IsolatedAsyncioTestCase):
    async def test_explicit_preview_at_60_and_80_columns(self):
        from tests.test_terminal_search import ManualFeed

        for size in ((60, 18), (80, 24)):
            with self.subTest(size=size):
                app = ObservatoryApp(ManualFeed(), persist_preferences=False,
                                     content_preview_providers=(PI_REQUESTED_COMMAND,))
                async with app.run_test(size=size) as pilot:
                    await pilot.pause()
                    app.refresh_timer.stop()
                    await pilot.press('2')
                    await pilot.pause()
                    app.table_data['activity-table'] = {EVENT_ID: ROW}
                    app.selected_activity = EVENT_ID
                    app.native_page_pending = lambda _page: False
                    header = app.query_one('#masthead').region
                    app.action_expand()
                    await pilot.pause()
                    app.show_activity()
                    await pilot.pause()
                    detail = app.query_one('#activity-detail').plain_text
                    self.assertIn('z Check for a redacted requested command', detail)
                    await pilot.press('z')
                    await pilot.pause()
                    self.assertIn('Unavailable now', app.query_one('#activity-detail').plain_text)
                    self.assertNotIn('PRIVATE_COMMAND', app.query_one('#activity-detail').plain_text)
                    app.plugin_text_state = {
                        'record': ROW, 'receiver_id': '',
                        'provider_id': PI_REQUESTED_COMMAND.id, 'status': 'available',
                        'text': 'PRIVATE_COMMAND', 'clipped': False,
                        'expires_at': time.monotonic() + 20}
                    app.show_activity()
                    await pilot.pause()
                    self.assertIn('Requested shell command · redacted',
                                  app.query_one('#activity-detail').plain_text)
                    self.assertIn('PRIVATE_COMMAND', app.query_one('#activity-detail').plain_text)
                    app.plugin_text_state['expires_at'] = time.monotonic() - 1
                    app.show_activity()
                    await pilot.pause()
                    self.assertIsNone(app.plugin_text_state)
                    self.assertNotIn('PRIVATE_COMMAND', app.query_one('#activity-detail').plain_text)
                    app.plugin_text_state = {
                        'record': ROW, 'receiver_id': '',
                        'provider_id': PI_REQUESTED_COMMAND.id, 'status': 'available',
                        'text': 'PRIVATE_COMMAND', 'clipped': False,
                        'expires_at': time.monotonic() + 20}
                    app.selected_activity = 'plugin-event:' + 'c' * 64
                    app.show_activity()
                    await pilot.pause()
                    self.assertIsNone(app.plugin_text_state)
                    self.assertNotIn('PRIVATE_COMMAND', app.query_one('#activity-detail').plain_text)
                    self.assertEqual(app.query_one('#masthead').region, header)
