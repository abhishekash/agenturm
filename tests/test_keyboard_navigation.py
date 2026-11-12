"""Entry focus, nested Escape and persistent help across the core workflows."""
from pathlib import Path
import tempfile
import unittest

from textual.widgets import Select, TextArea

from observatory.saved_tasks import TaskStore
from observatory.terminal_ui import ObservatoryApp
from observatory.ui_keyboard import KeyboardHelpScreen
from observatory.ui_saved_tasks import TaskBriefEditor, TaskDecisionsScreen, TaskDetailScreen
from tests.test_terminal_search import ManualFeed


class KeyboardNavigationTests(unittest.IsolatedAsyncioTestCase):
    async def tab_to(self,app,pilot,target):
        for _ in range(30):
            if app.focused and app.focused.id==target:return
            await pilot.press('tab')
        self.fail('Keyboard focus never reached '+target)

    async def test_help_scrolls_at_floor_and_returns_to_the_exact_control(self):
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(60,18)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            initial=app.focused
            await pilot.press('f1');await pilot.pause()
            self.assertIsInstance(app.screen,KeyboardHelpScreen)
            self.assertEqual(app.focused.id,'keyboard-reference')
            await pilot.press('pagedown');await pilot.pause()
            self.assertGreater(app.focused.scroll_y,0)
            await pilot.press('f1');await pilot.pause()
            self.assertIs(app.focused,initial)
            await pilot.press('/','?','2','m','[',']');await pilot.pause()
            self.assertEqual(app.focused.value,'?2m[]')
            field=app.focused
            await pilot.press('f1','escape');await pilot.pause()
            self.assertIs(app.focused,field)
            self.assertEqual(field.value,'?2m[]')

    async def test_saved_task_reading_selectors_help_and_draft_escape(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'tasks.db';store=TaskStore(path)
            task=store.create('Keyboard task',notes='Saved intent\n'+'Detailed saved line.\n'*80)
            task=store.add_decision(task['id'],kind='accepted',text='Keep native navigation')
            app=ObservatoryApp(ManualFeed(),persist_preferences=False,task_store_path=path)
            async with app.run_test(size=(60,20)) as pilot:
                await pilot.pause();app.refresh_timer.stop()
                detail=TaskDetailScreen(app,task);app.push_screen(detail);await pilot.pause()
                self.assertEqual(app.focused.id,'task-body')
                await pilot.press('pagedown');await pilot.pause()
                self.assertGreater(app.focused.scroll_y,0)
                await pilot.press('d');await pilot.pause()
                decisions=app.screen
                self.assertIsInstance(decisions,TaskDecisionsScreen)
                self.assertEqual(app.focused.id,'decision-version')
                await pilot.press('enter');await pilot.pause()
                self.assertTrue(decisions.query_one(Select).expanded)
                await pilot.press('escape');await pilot.pause()
                self.assertIs(app.screen,decisions)
                self.assertFalse(decisions.query_one(Select).expanded)
                await pilot.press('f1');await pilot.pause()
                self.assertIsInstance(app.screen,KeyboardHelpScreen)
                await pilot.press('escape');await pilot.pause()
                self.assertIs(app.screen,decisions)
                self.assertEqual(app.focused.id,'decision-version')
                await pilot.press('escape','p');await pilot.pause()
                editor=app.screen;self.assertIsInstance(editor,TaskBriefEditor)
                await pilot.press('?','2','m','[',']');await pilot.pause()
                text=editor.query_one(TextArea).text
                self.assertIn('?2m[]',text)
                await pilot.press('f1','escape');await pilot.pause()
                self.assertIs(app.screen,editor)
                self.assertEqual(editor.query_one(TextArea).text,text)
                await self.tab_to(app,pilot,'brief-field')
                await pilot.press('enter');await pilot.pause()
                self.assertTrue(editor.query_one(Select).expanded)
                await pilot.press('escape');await pilot.pause()
                self.assertIs(app.screen,editor)
                self.assertFalse(editor.query_one(Select).expanded)
                self.assertFalse(editor.discard_armed)
                await pilot.press('escape');await pilot.pause()
                self.assertIs(app.screen,editor)
                self.assertTrue(editor.discard_armed)
                await pilot.press('escape');await pilot.pause()
                self.assertIs(app.screen,detail)
                self.assertEqual(store.get(task['id'])['brief']['intent'],'')

    async def test_empty_workflow_focuses_its_report_and_help_returns(self):
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(60,20)) as pilot:
            await pilot.pause();app.refresh_timer.stop();app.workflow_controller.close()
            app.workflow_ledger.rows=[]
            await pilot.press('w');await pilot.pause()
            workflow=app.screen
            self.assertEqual(app.focused.id,'workflow-log')
            await pilot.press('f1');await pilot.pause()
            self.assertIsInstance(app.screen,KeyboardHelpScreen)
            await pilot.press('escape');await pilot.pause()
            self.assertIs(app.screen,workflow)
            self.assertEqual(app.focused.id,'workflow-log')
