"""Headless regressions for lazy views, narrow tables and explicit local state."""
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from textual.widgets import Input, RichLog

from observatory.terminal import Snapshot
from observatory.terminal_ui import ObservatoryApp, attention_key
from observatory.ui_diagnostics import DiagnosticsScreen, diagnostics_text
from observatory.ui_export import ExportScreen, investigation_json, write_investigation
from observatory.ui_layout import column_layout
from observatory.ui_preferences import Preferences
from tests.test_terminal_search import ManualFeed


class LocalHelpersTest(unittest.TestCase):
    def test_dismissals_persist_only_bounded_fingerprints(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'prefs.json'
            keys=[attention_key({'id':f'session-{i}','state':'Error','stamp':i}) for i in range(105)]
            Preferences(path).save({'dismissed_attention':keys,'session_id':'private-session'})
            saved=Preferences(path).load()['dismissed_attention']
            self.assertEqual(saved,keys[-100:])
            self.assertNotIn('session-',path.read_text())
            self.assertNotIn('private-session',path.read_text())

    def test_failure_fingerprint_changes_for_a_new_trace_or_timestamp(self):
        original={'id':'session-1','state':'Error','latest_trace_id':'trace-1','stamp':100}
        self.assertEqual(attention_key(original),attention_key(dict(original)))
        self.assertNotEqual(attention_key(original),attention_key({**original,'latest_trace_id':'trace-2'}))
        self.assertNotEqual(attention_key(original),attention_key({**original,'stamp':101}))

    def test_preferences_allowlist_and_private_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'prefs.json'
            store = Preferences(path)
            store.save({'theme': 'Solarized', 'view': 'metrics', 'reviews': True,
                        'errors': ['mcp', 'secret'], 'search': 'secret',
                        'session_id': 'secret', 'endpoint': 'https://secret'})
            self.assertEqual(store.load(), {'theme': 'Solarized', 'view': 'metrics',
                                           'reviews': True, 'errors': ['mcp']})
            self.assertNotIn('secret', path.read_text())
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            path.write_text('{broken')
            self.assertEqual(store.load(), {})
            path.write_text('[]')
            self.assertEqual(store.load(), {})

    def test_preference_opt_out_never_touches_files(self):
        with patch.dict(os.environ, {'AGENTURM_UI_PREFERENCES': '0'}):
            store = Preferences('/no/such/path/preferences.json')
            with patch.object(Path, 'stat', side_effect=AssertionError('read')), \
                    patch.object(Path, 'mkdir', side_effect=AssertionError('write')):
                self.assertEqual(store.load(), {})
                store.save({'view': 'agents'})

    def test_narrow_columns_keep_subject_and_result(self):
        specs = [('Task / source', 34, 2), ('State', 17, 0), ('Updated', 8, 0)]
        visible, widths = column_layout('agent-table', specs, 60)
        self.assertIn(0, visible)
        self.assertIn(1, visible)
        self.assertNotIn(2, visible)
        self.assertLessEqual(sum(widths) + 2 * len(widths) + 1, 60)
        self.assertEqual(column_layout('agent-table', specs, 180)[0], tuple(range(3)))

    def test_diagnostics_only_include_allowlisted_health(self):
        text = diagnostics_text({'enabled': True, 'token': 'SECRET', 'endpoint': 'SECRET',
                                 'sessions': [{'diagnostics': {'source_path': '/local/source.jsonl',
                                     'last_success_ns': 123, 'backlog_bytes': 100,
                                     'skipped_oversized_records': 2, 'schema_errors': 3,
                                     'credentials': 'SECRET'}}]})
        for expected in ('/local/source.jsonl', '100', '123', 'Schema errors: 3',
                         'skipped oversized: 2', 'not secure erasure', 'backups'):
            self.assertIn(expected, text)
        self.assertNotIn('SECRET', text)

    def test_export_is_allowlisted_and_refuses_overwrite(self):
        content = investigation_json('activity', {'subject': 'selected', 'detail': 'sensitive',
                                                  'endpoint': 'SECRET', 'token': 'SECRET'})
        self.assertNotIn('SECRET', content)
        self.assertEqual(json.loads(content)['evidence']['detail'], 'sensitive')
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'evidence.json'
            write_investigation(path, content)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            with self.assertRaises(FileExistsError):
                write_investigation(path, 'overwritten')
            self.assertEqual(path.read_text(), content)


class CockpitPolishTest(unittest.IsolatedAsyncioTestCase):
    async def test_span_cursor_is_initialized_before_the_new_pane_is_painted(self):
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(160,48)) as pilot:
            await pilot.pause()
            app.refresh_timer.stop()
            tree=app.query_one('#span-tree')
            # Rebuild while Traces is still hidden. Selection must not depend
            # on a later paint/idle callback that can race with tab activation.
            app.span_signature=None
            with patch.object(app,'call_after_refresh'):
                app.show_spans()
                self.assertEqual(tree.cursor_node.data['span_id'],'span-1')
            await pilot.press('4')
            await pilot.pause()
            self.assertEqual(tree.cursor_node.data['span_id'],'span-1')

    async def test_keyboard_tab_navigation_preserves_selected_trace_span(self):
        feed = ManualFeed()
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(160, 48)) as pilot:
            await pilot.pause()
            app.refresh_timer.stop()
            await pilot.press('4')
            await pilot.pause()
            tree = app.query_one('#span-tree')
            self.assertEqual(tree.cursor_node.data['span_id'], 'span-1')
            self.assertTrue(tree.cursor_node.is_expanded)
            await pilot.press('enter', 'down')
            await pilot.pause()
            self.assertEqual(tree.cursor_node.data['span_id'], 'span-2')
            await pilot.press('1')
            await pilot.pause()
            feed.refresh()
            app.refresh_data()
            await pilot.press('4')
            await pilot.pause()
            self.assertEqual(feed.selected_trace, 'trace-1')
            self.assertEqual(tree.cursor_node.data['span_id'], 'span-2')
            self.assertEqual(app.inspector_signatures['span-info'][0], 'shell command')

    async def test_hidden_views_do_no_render_work_and_activation_uses_latest(self):
        feed = ManualFeed()
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(140, 42)) as pilot:
            await pilot.pause()
            app.refresh_timer.stop()
            self.assertNotIn('activity-table', app.table_data)
            self.assertNotIn('metrics-table', app.table_data)
            with patch.object(app, 'render_activity', wraps=app.render_activity) as render:
                feed.api.activity[0]['subject'] = 'newest evidence'
                feed.refresh()
                app.refresh_data()
                render.assert_not_called()
                app.action_page('activity')
                await pilot.pause()
                self.assertTrue(render.called)
            self.assertIn('newest evidence', str(app.query_one('#activity-table').get_row_at(0)))
            with patch.object(app, 'render_activity') as render:
                app.refresh_data()
                render.assert_not_called()

    async def test_resize_hides_metadata_restores_full_values_and_selection(self):
        app = ObservatoryApp(ManualFeed(), persist_preferences=False)
        async with app.run_test(size=(180, 48)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all')
            await pilot.pause()
            table = app.query_one('#agent-table')
            original = table.get_row_at(0)
            key = table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value
            await pilot.resize_terminal(70, 30)
            await pilot.pause()
            self.assertNotIn(2, table.visible_columns)
            self.assertIn(0, table.visible_columns)
            self.assertIn(1, table.visible_columns)
            await pilot.resize_terminal(180, 48)
            await pilot.pause()
            self.assertEqual(table.visible_columns, tuple(range(3)))
            self.assertEqual(table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value, key)
            # Recorded age may advance while the semantic task identity stays fixed.
            self.assertEqual(table.get_row_at(0)[0].plain.splitlines()[0],original[0].plain.splitlines()[0])

    async def test_preferences_restore_safe_state_without_search_or_scope(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'prefs.json'
            Preferences(path).save({'theme': 'Dark', 'view': 'mcp', 'errors': ['mcp'], 'reviews': True})
            app = ObservatoryApp(ManualFeed(), preferences_path=path)
            async with app.run_test(size=(140, 42)) as pilot:
                await pilot.pause()
                self.assertEqual(app.theme, 'Dark')
                self.assertEqual(app.query_one('#pages').active, 'mcp')
                self.assertEqual(app.errors_only, {'mcp'})
                self.assertEqual(app.scope_value, 'all')
                app.query_one('#search-mcp', Input).value = 'SENSITIVE_SEARCH'
                await pilot.pause()
            self.assertEqual(Preferences(path).load()['view'], 'mcp')
            self.assertNotIn('SENSITIVE_SEARCH', path.read_text())

    async def test_diagnostics_and_export_require_explicit_confirmation(self):
        app = ObservatoryApp(ManualFeed(), persist_preferences=False)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'investigation.json'
            async with app.run_test(size=(140, 42)) as pilot:
                await pilot.pause()
                await pilot.press('d')
                await pilot.pause()
                self.assertIsInstance(app.screen, DiagnosticsScreen)
                self.assertIn('logical deletion', '\n'.join(line.text for line in app.screen.query_one(RichLog).lines))
                await pilot.resize_terminal(100, 36)
                await pilot.pause()
                self.assertIsInstance(app.screen, DiagnosticsScreen)
                await pilot.press('escape')
                await pilot.pause()
                app.action_page('activity')
                await pilot.pause()
                await pilot.press('x')
                await pilot.pause()
                self.assertIsInstance(app.screen, ExportScreen)
                content = app.screen.content
                self.assertFalse(path.exists())
                app.screen.query_one('#export-path', Input).value = str(path)
                await pilot.click('#export-confirm')
                await pilot.pause()
                self.assertEqual(path.read_text(), content)

    async def test_empty_first_run_guidance_visible_even_in_compact_mode(self):
        feed = ManualFeed()
        feed.snapshot = Snapshot(updated=1)
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(80, 24)) as pilot:
            await pilot.pause()
            text = str(app.query_one('#search-status-agents').render())
            self.assertIn('Start an agent', text)
            self.assertIn('diagnostics', text)


if __name__ == '__main__':
    unittest.main()
