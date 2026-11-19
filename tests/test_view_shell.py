"""Global navigation stays anchored while views change their local content."""
import unittest
from textual.widgets import Button, TabbedContent
from observatory.terminal_ui import ObservatoryApp
from tests.test_terminal_search import ManualFeed


class ViewShellTests(unittest.IsolatedAsyncioTestCase):
    async def test_header_and_navigation_stay_put_across_inspection_and_view_changes(self):
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(150,40)) as pilot:
            await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all')
            for size in ((150,40),(80,24),(60,18)):
                with self.subTest(size=size):
                    app.action_page('agents');await pilot.resize_terminal(*size);await pilot.pause()
                    header=app.query_one('#masthead');tabs=app.query_one('#pages > Tabs')
                    expected=(header.region,tabs.region)
                    def stable():
                        self.assertTrue(header.display)
                        self.assertTrue(app.query_one('#controls').display)
                        self.assertEqual((header.region,tabs.region),expected)
                        self.assertTrue(header.region.contains_region(app.query_one('#pause').region))
                    stable()
                    app.select_agent('source-1');app.query_one('#agent-table').focus()
                    await pilot.press('enter');await pilot.pause();stable()
                    self.assertGreaterEqual(app.query_one('#agent-detail').content_size.height,3)
                    for paused in (True,False):
                        app.feed.paused=paused;app.refresh_data();await pilot.pause();stable()
                    if size[0]>=80:
                        # Mouse tabs retain per-view inspector state; it must
                        # never control the visibility of the global shell.
                        await pilot.click('#--content-tab-activity');await pilot.pause();stable()
                        await pilot.click('#--content-tab-agents');await pilot.pause();stable()
                    for key,page in [('2','activity'),('3','mcp'),('4','traces'),('5','host'),('6','metrics'),('7','findings')]:
                        await pilot.press(key);await pilot.pause();stable()
                        self.assertEqual(app.query_one('#pages',TabbedContent).active,page)
                        app.action_expand();await pilot.pause();stable()
                        await pilot.press('escape');await pilot.pause();stable()
                    await pilot.press('1');await pilot.pause();stable()

    async def test_session_filter_explains_all_and_preserves_other_filters(self):
        feed=ManualFeed();app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,36)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            button=app.query_one('#scope',Button)
            self.assertIn('Session: All',button.label.plain)
            self.assertIn('Today/History',button.tooltip)
            app.set_agent_filter('errors')
            app.set_session_scope('source-1');await pilot.pause()
            self.assertIn('Session:',button.label.plain)
            self.assertEqual(feed.session_id,'session-1')
            app.set_session_scope('all');await pilot.pause()
            self.assertEqual(app.agent_filter,'errors')
            self.assertEqual(feed.session_id,'')
            self.assertIn('Session: All',button.label.plain)
