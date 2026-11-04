"""Saved-task journeys exercise real persistence without a receiver or agent."""
from dataclasses import replace
from pathlib import Path
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from textual.widgets import Button, DataTable, Input, RichLog, Static, TextArea

from observatory.saved_tasks import TaskStore
from observatory.terminal_ui import ObservatoryApp
from observatory.ui_saved_tasks import (HandoffScreen, SavedTasksScreen, TaskDetailScreen,
    TaskEditor, TaskRunsScreen, capture_selected_run, task_overview)
from tests.test_terminal_search import ManualFeed


TRACE = 'a' * 32


class SelectedRunTests(unittest.TestCase):
    def test_exact_run_and_agent_scope_never_capture_session_totals_or_other_checks(self):
        feed = ManualFeed()
        item = {'id': 'worker', 'session_id': 'shared', 'agent_id': 'worker',
                'runtime': 'Codex', 'task': 'Fix refresh expiry', 'state': 'Completed',
                'working_directory': 'file:///tmp/local%20repo'}
        usage = {'trace_id': TRACE, 'input_tokens': 200, 'output_tokens': 20}
        event = {'session_id': 'shared', 'agent_id': 'worker', 'trace_id': TRACE,
                 'domain': 'shell', 'subject': 'python -m unittest', 'result': 'success',
                 'time_unix_nano': time.time_ns()}
        feed.snapshot = replace(feed.snapshot, sessions=[{'agent_id': 'worker', 'session_id': 'shared',
            'input_tokens': 999999, 'latest_run': usage}], activity=[event,
            {**event, 'agent_id': 'other', 'subject': 'pytest other-agent'},
            {**event, 'trace_id': 'b' * 32, 'subject': 'pytest other-run'}])
        owner = SimpleNamespace(feed=feed, selected_source='worker', workflow_items=lambda: [item])
        result = capture_selected_run(owner)
        self.assertEqual(result['usage']['input_tokens'], 200)
        self.assertEqual([check['command'] for check in result['checks']], ['python -m unittest'])
        self.assertEqual(result['working_directory'], '/tmp/local repo')
        self.assertIn("cd -- '/tmp/local repo'", result['resume_command'])
        self.assertEqual(result['local_workspace'], '')  # A remote path is not a local checkout.
        feed.api.endpoint = 'http://127.0.0.1:4318'
        self.assertEqual(capture_selected_run(owner)['local_workspace'], '/tmp/local repo')
        feed.snapshot.sessions[0]['latest_run'] = {'input_tokens': 999999}
        result = capture_selected_run(owner)
        self.assertEqual(result['usage'], {})
        self.assertEqual(result['checks'], [])
        self.assertEqual(result['trace_id'], '')

    def test_zero_partial_usage_is_not_presented_as_complete(self):
        text = task_overview({'usage_summary': {'input_tokens': 0, 'output_tokens': None,
            'usage_complete': False, 'known_cost_usd': 0, 'cost_complete': False}})
        self.assertIn('Tokens 0 (partial)', text)
        self.assertIn('$0.0000 (partial)', text)


class SavedTaskJourney(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.path = self.root / 'tasks.sqlite3'

    def app(self):
        return ObservatoryApp(ManualFeed(), persist_preferences=False, task_store_path=self.path)

    async def ready(self, pilot, predicate, message='task job did not finish'):
        deadline = time.monotonic() + 6
        while not predicate() and time.monotonic() < deadline:
            await pilot.pause(.05)
        self.assertTrue(predicate(), message)

    async def open_list(self, app, pilot):
        await pilot.pause()
        app.refresh_timer.stop()
        app.query_one('#agent-table', DataTable).focus()
        await pilot.press('b')
        await self.ready(pilot, lambda: isinstance(app.screen, SavedTasksScreen) and not app.screen.busy)

    async def test_save_edit_reopen_handoff_export_and_escape_ladder_at_floor(self):
        app = self.app()
        async with app.run_test(size=(60, 20)) as pilot:
            await pilot.pause()
            self.assertFalse(self.path.exists(), 'Monitor startup must not open the saved-task database')
            await self.open_list(app, pilot)
            self.assertIn('No saved tasks', app.screen.query_one('#task-status', Static).render().plain)
            app.screen.query_one(DataTable).focus()
            await pilot.press('n')
            await pilot.pause()
            self.assertIsInstance(app.screen, TaskEditor)
            editor = app.screen
            editor.query_one('#task-name', Input).value = 'Refresh expiry · 東京 café 👩‍💻'
            editor.query_one(TextArea).load_text('Intent: make refresh reliable.\nNext: add a replay case.')
            self.assertGreaterEqual(editor.query_one(TextArea).content_region.height, 4)
            for widget in editor.query('Input, TextArea, Button, #task-status'):
                self.assertTrue(editor.region.contains_region(widget.region), (widget.id, widget.region))
            await pilot.press('ctrl+s')
            await self.ready(pilot, lambda: isinstance(app.screen, SavedTasksScreen) and not app.screen.busy)
            self.assertEqual(app.screen.query_one(DataTable).row_count, 1)
            await pilot.press('enter')
            await self.ready(pilot, lambda: isinstance(app.screen, TaskDetailScreen))
            task = app.screen.record
            self.assertEqual(TaskStore(self.path).get(task['id'])['notes'], 'Intent: make refresh reliable.\nNext: add a replay case.')
            await pilot.press('e')
            await pilot.pause()
            app.screen.query_one(TextArea).load_text('Decision: rotate the refresh token.\nNext: verify replay rejection.')
            await pilot.press('ctrl+s')
            await self.ready(pilot, lambda: isinstance(app.screen, TaskDetailScreen))
            await pilot.press('h')
            await self.ready(pilot, lambda: isinstance(app.screen, HandoffScreen) and bool(app.screen.markdown))
            preview = app.screen.markdown
            self.assertIn('Decision: rotate', preview)
            TaskStore(self.path).update(task['id'], notes='Changed after preview')
            with patch.object(app, 'copy_to_clipboard') as copy:
                await pilot.click('#handoff-copy')
                copy.assert_called_once_with(preview)
            destination = self.root / 'handoff.v1.md'
            app.screen.query_one(Input).value = str(destination)
            await pilot.click('#handoff-write')
            await self.ready(pilot, lambda: not app.screen.busy)
            self.assertEqual(destination.read_text(), preview)
            self.assertNotIn('Changed after preview', destination.with_suffix('.html').read_text())
            await pilot.click('#handoff-write')
            await self.ready(pilot, lambda: not app.screen.busy)
            self.assertIn('Could not finish', app.screen.query_one('#task-status', Static).render().plain)
            await pilot.press('escape')
            self.assertIsInstance(app.screen, TaskDetailScreen)
            await pilot.press('escape')
            await self.ready(pilot, lambda: isinstance(app.screen, SavedTasksScreen) and not app.screen.busy)
            await pilot.press('escape')
            self.assertNotIsInstance(app.screen, SavedTasksScreen)

    async def test_layouts_narrow_resize_and_search_bindings_do_not_open_other_screens(self):
        store = TaskStore(self.path)
        task = store.create('Long title ' + '東京 é 👩‍💻 ' * 18, notes='Next: keep this readable.')
        app = self.app()
        async with app.run_test(size=(120, 40)) as pilot:
            await self.open_list(app, pilot)
            search = app.screen.query_one(Input)
            search.focus()
            await pilot.press('b', 'n', 'a')
            self.assertEqual(search.value, 'bna')
            self.assertIsInstance(app.screen, SavedTasksScreen)
            search.value = ''
            app.screen.query_one(DataTable).focus()
            for size in ((120, 40), (80, 24), (60, 20)):
                await pilot.resize_terminal(*size)
                await pilot.pause()
                self.assertFalse(app.screen.query_one(DataTable).show_horizontal_scrollbar)
                for button in app.screen.query(Button):
                    self.assertTrue(app.screen.region.contains_region(button.region), (size, button.region))
            await pilot.press('enter')
            await self.ready(pilot, lambda: isinstance(app.screen, TaskDetailScreen))
            for button in app.screen.query(Button):
                self.assertTrue(app.screen.region.contains_region(button.region), button.region)
            await pilot.press('e')
            await pilot.pause()
            app.screen.query_one(TextArea).load_text('Unsaved decision')
            await pilot.resize_terminal(50, 16)
            await pilot.pause()
            self.assertTrue(app.screen.query_one('#task-too-small').display)
            self.assertFalse(app.screen.query_one('#task-editor').display)
            await pilot.resize_terminal(60, 20)
            await pilot.pause()
            self.assertEqual(app.screen.query_one(TextArea).text, 'Unsaved decision')
            await pilot.press('escape')
            self.assertIsInstance(app.screen, TaskEditor)
            await pilot.press('escape')
            self.assertIsInstance(app.screen, TaskDetailScreen)
            self.assertEqual(store.get(task['id'])['notes'], 'Next: keep this readable.')

    async def test_link_refresh_deduplicates_and_conflict_keeps_editor_text(self):
        store = TaskStore(self.path)
        task = store.create('Fix refresh')
        app = self.app()
        context = {'trace_id': TRACE, 'session_id': 's', 'runtime': 'Codex', 'task': 'Fix refresh',
                   'usage': {'input_tokens': 100, 'output_tokens': 20}, 'state': 'Completed'}
        async with app.run_test(size=(80, 24)) as pilot:
            await self.open_list(app, pilot)
            await pilot.press('enter')
            await self.ready(pilot, lambda: isinstance(app.screen, TaskDetailScreen))
            with patch('observatory.ui_saved_tasks.capture_selected_run', return_value=context):
                for _ in range(2):
                    await pilot.press('l')
                    await self.ready(pilot, lambda: not app.screen.busy)
            self.assertEqual(len(store.get(task['id'])['links']), 1)
            self.assertEqual(store.get(task['id'])['usage_summary']['input_tokens'], 100)
            await pilot.click('#task-runs')
            await pilot.pause()
            self.assertIsInstance(app.screen, TaskRunsScreen)
            self.assertEqual(app.screen.query_one(DataTable).row_count, 1)
            await pilot.click('#task-unlink')
            await self.ready(pilot, lambda: not app.screen.busy)
            self.assertEqual(store.get(task['id'])['links'], [])
            await pilot.press('escape')
            await self.ready(pilot, lambda: not app.screen.busy)
            await pilot.press('e')
            await pilot.pause()
            app.screen.query_one(TextArea).load_text('My unsaved decision')
            store.update(task['id'], notes='A concurrent decision')
            await pilot.press('ctrl+s')
            await self.ready(pilot, lambda: not app.screen.busy)
            self.assertIsInstance(app.screen, TaskEditor)
            self.assertEqual(app.screen.query_one(TextArea).text, 'My unsaved decision')
            self.assertIn('changed elsewhere', app.screen.query_one('#task-status', Static).render().plain)
            self.assertEqual(store.get(task['id'])['notes'], 'A concurrent decision')

    async def test_slow_capture_keeps_resize_and_back_navigation_responsive(self):
        task = TaskStore(self.path).create('A workspace task', str(self.root))
        started, release = threading.Event(), threading.Event()
        def capture(*args):
            started.set()
            release.wait(5)
            return {'captured_ns': time.time_ns(), 'files': [], 'complete': True}
        app = self.app()
        async with app.run_test(size=(80, 24)) as pilot:
            await self.open_list(app, pilot)
            await pilot.press('enter')
            await self.ready(pilot, lambda: isinstance(app.screen, TaskDetailScreen))
            with patch('observatory.saved_tasks.capture_workspace', side_effect=capture):
                try:
                    await pilot.press('r')
                    await self.ready(pilot, started.is_set)
                    self.assertTrue(app.screen.busy)
                    self.assertIn('Capturing Git', app.screen.query_one('#task-status', Static).render().plain)
                    await pilot.resize_terminal(60, 20)
                    await pilot.pause()
                    self.assertEqual(app.screen.size.width, 60)
                    await pilot.press('escape')
                    self.assertIsInstance(app.screen, SavedTasksScreen)
                finally:
                    release.set()
                await self.ready(pilot, lambda: bool(TaskStore(self.path).get(task['id'])['workspace_snapshot']))
