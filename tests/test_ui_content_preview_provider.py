"""Trusted Activity preview providers keep shell selection and text transient."""
import time
import unittest
from types import SimpleNamespace
from urllib.parse import urlencode

from observatory.terminal_ui import ObservatoryApp, ObservatoryScreen, copy_event_text
from observatory.ui_export import investigation_json
from observatory.ui_plugin_content import (
    ContentPreviewProvider, ContentPreviewRegistry, PI_USER_TEXT, accepted_preview,
)


EVENT_ID = 'plugin-event:' + 'a' * 64


def tool_match(record):
    return (record.get('source') == 'plugin' and record.get('domain') == 'tool'
            and record.get('action') == 'started' and record.get('id') == EVENT_ID)


TOOL_PREVIEW = ContentPreviewProvider(
    'fixture-tool', tool_match, '/api/plugin-command-content',
    'Requested shell command', 'z Check captured requested command for this event.',
    '… Excerpt clipped; source command remains in the original Pi file.',
    '(empty command)')


class PreviewRegistryTests(unittest.TestCase):
    def setUp(self):
        self.row = {'source': 'plugin', 'domain': 'tool', 'action': 'started',
                    'id': EVENT_ID, 'source_id': EVENT_ID, 'receiver_id': 'receiver',
                    'session_id': 'session', 'agent_id': 'agent', 'service': 'fixture.pi',
                    'time_unix_nano': 1, 'subject': 'Shell call requested', 'detail': ''}

    def test_exact_provider_and_local_route(self):
        registry = ContentPreviewRegistry((PI_USER_TEXT, TOOL_PREVIEW))
        self.assertIs(registry.for_record(self.row), TOOL_PREVIEW)
        self.assertEqual(TOOL_PREVIEW.path(self.row),
                         '/api/plugin-command-content?' + urlencode({'event_id': EVENT_ID}))
        response = {'receiver_id': 'receiver', 'event_id': EVENT_ID,
                    'redacted_text': 'echo [REDACTED]', 'clipped': False,
                    'revision': [7, 1, 2]}
        self.assertEqual(accepted_preview(response, self.row, 'receiver', (7, 1, 2),
                                          (7, 1, 2), matches=TOOL_PREVIEW.matches),
                         ('echo [REDACTED]', False))
        self.assertIsNone(accepted_preview(response, self.row, 'receiver', (7, 1, 2),
                                           (7, 1, 3), matches=TOOL_PREVIEW.matches))
        self.assertIsNone(accepted_preview(response, {**self.row, 'source': 'native'},
                                           'receiver', (7, 1, 2), (7, 1, 2),
                                           matches=lambda _row: True))
        self.assertNotIn('echo [REDACTED]', copy_event_text(self.row))
        self.assertNotIn('echo [REDACTED]', investigation_json('activity', self.row))

    def test_ambiguous_or_invalid_provider_fails_closed(self):
        duplicate = ContentPreviewProvider('other-tool', tool_match,
                                           '/api/plugin-other-content', 'Other text',
                                           'z Check text.', '… Clipped.')
        self.assertIsNone(ContentPreviewRegistry((TOOL_PREVIEW, duplicate)).for_record(self.row))
        with self.assertRaisesRegex(ValueError, 'invalid content preview provider'):
            ContentPreviewRegistry((TOOL_PREVIEW, TOOL_PREVIEW))
        with self.assertRaisesRegex(ValueError, 'invalid content preview provider'):
            ContentPreviewRegistry((ContentPreviewProvider('remote', tool_match,
                'https://example.org/content', 'Text', 'z Check.', '… Clipped.'),))
        invalid = ContentPreviewProvider('invalid-result', lambda _row: 'yes',
                                         '/api/plugin-other-content', 'Text',
                                         'z Check.', '… Clipped.')
        self.assertIsNone(ContentPreviewRegistry((invalid,)).for_record(self.row))
        throwing = ContentPreviewProvider('throwing', lambda _row: 1 / 0,
                                          '/api/plugin-other-content', 'Text',
                                          'z Check.', '… Clipped.')
        self.assertIsNone(ContentPreviewRegistry((TOOL_PREVIEW, throwing)).for_record(self.row))
        self.assertIsNone(ContentPreviewRegistry((TOOL_PREVIEW,)).for_record(
            {**self.row, 'source': 'native'}))
        with self.assertRaises(ValueError):
            TOOL_PREVIEW.path({**self.row, 'id': 'bad'})

    def test_provider_change_discards_late_result_and_visible_text(self):
        state = {'record': self.row, 'receiver_id': 'receiver',
                 'provider_id': TOOL_PREVIEW.id, 'status': 'loading',
                 'text': '', 'clipped': False, 'revision': (7, 1, 2)}
        updates = []
        screen = SimpleNamespace(
            is_mounted=True, screen=ObservatoryScreen(), plugin_text_generation=4,
            plugin_text_state=state, content_preview_registry=ContentPreviewRegistry((PI_USER_TEXT,)),
            feed=SimpleNamespace(snapshot=SimpleNamespace(receiver_id='receiver')),
            table_data={'activity-table': {EVENT_ID: self.row}}, selected_activity=EVENT_ID,
            query_one=lambda *_args: SimpleNamespace(active='activity'),
            show_activity=lambda: updates.append('shown'),
            set_timer=lambda *_args: updates.append('timer'),
            _clear_plugin_text_visual=lambda: updates.append('cleared'))
        ObservatoryApp.finish_plugin_text(screen, 4, self.row, 'receiver',
                                          ('PRIVATE_COMMAND', False, (7, 1, 2)))
        self.assertEqual(state['status'], 'loading')
        self.assertEqual(updates, [])
        state.update(status='available', text='PRIVATE_COMMAND')
        ObservatoryApp.finish_plugin_text_recheck(screen, 4, self.row, 'receiver', (7, 1, 2))
        self.assertEqual(state['status'], 'unavailable')
        self.assertEqual(state['text'], '')
        self.assertEqual(updates, ['cleared', 'shown'])


class PreviewLayoutTests(unittest.IsolatedAsyncioTestCase):
    async def test_contributed_preview_at_narrow_and_standard_width(self):
        from tests.test_terminal_search import ManualFeed

        row = {'source': 'plugin', 'domain': 'tool', 'action': 'started',
               'id': EVENT_ID, 'source_id': EVENT_ID, 'receiver_id': '',
               'session_id': 'session', 'agent_id': 'agent', 'service': 'fixture.pi',
               'time_unix_nano': 1, 'subject': 'Shell call requested', 'detail': ''}
        for size in ((60, 18), (80, 24)):
            with self.subTest(size=size):
                app = ObservatoryApp(ManualFeed(), persist_preferences=False,
                                     content_preview_providers=(TOOL_PREVIEW,))
                async with app.run_test(size=size) as pilot:
                    await pilot.pause()
                    app.refresh_timer.stop()
                    await pilot.press('2')
                    await pilot.pause()
                    app.table_data['activity-table'] = {EVENT_ID: row}
                    app.selected_activity = EVENT_ID
                    app.native_page_pending = lambda _page: False
                    header = app.query_one('#masthead').region
                    app.action_expand()
                    await pilot.pause()
                    app.show_activity()
                    await pilot.pause()
                    detail = app.query_one('#activity-detail').plain_text
                    self.assertIn('z Check captured requested command', detail)
                    await pilot.press('z')
                    await pilot.pause()
                    self.assertIn('Unavailable now', app.query_one('#activity-detail').plain_text)
                    def loaded(*, provider_id=TOOL_PREVIEW.id, expires_in=20):
                        return {'record': row, 'receiver_id': '',
                            'provider_id': provider_id, 'status': 'available',
                            'text': 'PRIVATE_COMMAND', 'clipped': False,
                            'expires_at': time.monotonic() + expires_in}

                    app.plugin_text_state = loaded()
                    app.show_activity()
                    await pilot.pause()
                    self.assertIn('PRIVATE_COMMAND', app.query_one('#activity-detail').plain_text)
                    app.plugin_text_state = loaded(provider_id='other-tool')
                    app.show_activity()
                    await pilot.pause()
                    self.assertIsNone(app.plugin_text_state)
                    self.assertNotIn('PRIVATE_COMMAND', app.query_one('#activity-detail').plain_text)
                    app.plugin_text_state = loaded(expires_in=-1)
                    app.show_activity()
                    await pilot.pause()
                    self.assertIsNone(app.plugin_text_state)
                    app.plugin_text_state = loaded()
                    app.show_activity()
                    await pilot.pause()
                    app.native_page_pending = lambda _page: True
                    app.selected_activity = 'plugin-event:' + 'b' * 64
                    app.show_activity()
                    await pilot.pause()
                    self.assertIsNone(app.plugin_text_state)
                    self.assertNotIn('PRIVATE_COMMAND', app.query_one('#activity-detail').plain_text)
                    self.assertEqual(app.query_one('#masthead').region, header)
