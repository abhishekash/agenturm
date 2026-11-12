"""Structured task editing uses the keyboard and preserves offline drafts."""
from pathlib import Path
import tempfile
import time
import unittest
from textual.widgets import Button, Select, TextArea, Static
from observatory.saved_tasks import TaskStore
from observatory.terminal_ui import ObservatoryApp
from observatory.ui_saved_tasks import TaskBriefEditor, TaskDecisionEditor, TaskDecisionsScreen, TaskDetailScreen
from tests.test_terminal_search import ManualFeed


class ContinuityJourney(unittest.IsolatedAsyncioTestCase):
    def controls_fit(self, app):
        for button in app.screen.query(Button):
            self.assertTrue(app.screen.region.contains_region(button.region), str(button.id))
            self.assertTrue(button.parent.region.contains_region(button.region), str(button.id))
            self.assertTrue(any(button.render_line(row).text.strip() for row in range(button.size.height)), str(button.id))

    async def ready(self, pilot, predicate):
        end = time.monotonic()+8
        while not predicate() and time.monotonic()<end: await pilot.pause(.05)
        self.assertTrue(predicate())

    async def test_brief_decision_history_and_conflict_recovery_at_floor(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/'tasks.sqlite3'
            store = TaskStore(path)
            task = store.create('Preserve retries')
            app = ObservatoryApp(ManualFeed(), persist_preferences=False, task_store_path=path)
            async with app.run_test(size=(60,20)) as pilot:
                await pilot.pause();app.refresh_timer.stop()
                app.push_screen(TaskDetailScreen(app,task));await pilot.pause()
                self.controls_fit(app)
                self.assertIn('Brief',app.screen.query_one('#task-brief').render_line(0).text)
                self.assertIn('Decisions',app.screen.query_one('#task-decisions').render_line(0).text)
                await pilot.press('p');await pilot.pause()
                self.assertIsInstance(app.screen,TaskBriefEditor)
                self.controls_fit(app)
                app.screen.query_one(TextArea).load_text('Keep stable request IDs')
                app.screen.query_one('#brief-field',Select).value='next_action';await pilot.pause()
                app.screen.query_one(TextArea).load_text('Run expiry regression')
                await pilot.resize_terminal(50,15);await pilot.pause()
                self.assertTrue(app.screen.has_class('too-small'))
                await pilot.resize_terminal(60,20);await pilot.pause()
                self.assertEqual(app.screen.query_one(TextArea).text,'Run expiry regression')
                await pilot.press('ctrl+s')
                await self.ready(pilot,lambda:isinstance(app.screen,TaskDetailScreen))
                self.assertEqual(store.get(task['id'])['brief']['intent'],'Keep stable request IDs')
                await pilot.press('d');await pilot.pause()
                self.assertIsInstance(app.screen,TaskDecisionsScreen)
                self.controls_fit(app)
                await pilot.press('n');await pilot.pause()
                self.assertIsInstance(app.screen,TaskDecisionEditor)
                self.controls_fit(app)
                app.screen.query_one(TextArea).load_text('Use a per-request deadline')
                app.screen.query_one('#decision-kind',Select).value='accepted';await pilot.pause()
                await pilot.press('ctrl+s')
                await self.ready(pilot,lambda:isinstance(app.screen,TaskDecisionsScreen))
                self.assertEqual(len(store.get(task['id'])['decisions']),1)
                await pilot.press('s');await pilot.pause()
                app.screen.query_one(TextArea).load_text('Keep native retry policy')
                await pilot.press('ctrl+s')
                await self.ready(pilot,lambda:isinstance(app.screen,TaskDecisionsScreen))
                current=store.get(task['id'])
                self.assertEqual(current['decisions'][1]['supersedes_id'],current['decisions'][0]['id'])
                await pilot.press('escape');await pilot.pause()
                await pilot.press('p');await pilot.pause()
                editor=app.screen
                editor.query_one(TextArea).load_text('My unsaved intent')
                store.update(task['id'],notes='Concurrent editor')
                await pilot.press('ctrl+s')
                await self.ready(pilot,lambda:not editor.busy)
                self.assertIs(app.screen,editor)
                self.assertEqual(editor.query_one(TextArea).text,'My unsaved intent')
                self.assertIn('changed elsewhere',editor.query_one('#task-status',Static).render().plain)
                self.assertFalse(editor.query_one(TextArea).disabled)
                self.assertFalse(editor.query_one('#brief-field',Select).disabled)
                self.assertTrue(editor.query_one('#brief-copy').display)
                await pilot.press('escape');await pilot.pause()
                self.assertIs(app.screen,editor)
                await pilot.press('escape');await pilot.pause()
                self.assertIsInstance(app.screen,TaskDetailScreen)
