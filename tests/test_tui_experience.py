"""User journeys that exercise hidden inspectors, shortcuts and compact controls."""
import unittest
from unittest.mock import patch
from textual.widgets import Button, Select
from observatory.terminal_ui import ObservatoryApp
from observatory.ui_export import ExportScreen
from tests.test_terminal_search import ManualFeed


class ExperienceTests(unittest.IsolatedAsyncioTestCase):
    async def test_attention_action_labels_fit_the_split_inspector(self):
        feed=ManualFeed()
        feed.snapshot.local_status['sessions'][0]['run_state']='error'
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all');app.select_agent('source-1')
            await pilot.pause()
            for name in ('agent-open','agent-trace','agent-reviewed','agent-dismissed'):
                button=app.query_one('#'+name,Button)
                rendered=' '.join(button.render_line(row).text for row in range(button.size.height))
                self.assertIn(str(button.label),rendered,name)

    async def test_copy_uses_current_session_when_inspector_is_hidden(self):
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all')
            app.select_agent('source-1')
            await pilot.resize_terminal(80,24)
            await pilot.pause()
            app.select_agent('source-2')
            with patch.object(app,'copy_to_clipboard') as copy:
                app.action_copy()
                self.assertIn('Write release notes',copy.call_args.args[0])
                self.assertIn('\nAgent:',copy.call_args.args[0])
                self.assertNotIn('Review migration',copy.call_args.args[0])

    async def test_export_and_dismiss_have_distinct_shortcuts(self):
        feed=ManualFeed()
        feed.snapshot.local_status['sessions'][0]['run_state']='error'
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all');app.select_agent('source-1')
            await pilot.press('x');await pilot.pause()
            self.assertIsInstance(app.screen,ExportScreen)
            self.assertFalse(app.dismissed_attention)
            await pilot.press('escape');await pilot.pause()
            await pilot.press('X');await pilot.pause()
            self.assertTrue(app.dismissed_attention)
            dismissed=set(app.dismissed_attention)
            await pilot.press('2','X');await pilot.pause()
            self.assertEqual(app.dismissed_attention,dismissed)

    async def test_trace_copy_works_with_collapsed_inspector(self):
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause()
            await pilot.press('4');await pilot.pause()
            tree=app.query_one('#span-tree')
            child=tree.root.children[0].children[0]
            tree.move_cursor(child)
            await pilot.pause()
            with patch.object(app,'copy_to_clipboard') as copy:
                app.action_copy()
                self.assertIn('shell command',copy.call_args.args[0])
                self.assertIn('span-2',copy.call_args.args[0])

    async def test_compact_session_controls_remain_accessible(self):
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(60,20)) as pilot:
            await pilot.pause()
            for key in ('agent-filter','agent-errors','agent-sort','scope'):
                widget=app.query_one('#'+key)
                self.assertTrue(widget.display,key)
                self.assertTrue(app.screen.region.contains_region(widget.region),key)
            sort=app.query_one('#agent-sort',Button)
            self.assertIn(str(sort.label),sort.render_line(0).text)
