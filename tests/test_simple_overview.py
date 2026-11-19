"""Less default chrome must retain warnings, filtering and detailed evidence."""
import unittest
from textual.widgets import Button, Input, TabbedContent
from observatory.terminal_ui import ObservatoryApp
from observatory.overview import check_already_listed
from observatory.workflow_evidence import reference
from tests.test_terminal_search import ManualFeed


class SimpleOverviewTests(unittest.IsolatedAsyncioTestCase):
    async def test_summary_is_brief_and_detail_routes_still_work(self):
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(150,40)) as pilot:
            await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all');await pilot.pause()
            self.assertFalse(app.query_one('#agent-errors',Button).display)
            self.assertFalse(app.query_one('#search-status-agents').display)
            self.assertFalse(app.query_one('#results-agents').border_subtitle)
            self.assertTrue(app.query_one('#more-views').display)
            for page in ('host','metrics','findings'):
                self.assertFalse(app.query_one('#--content-tab-'+page).display)
            app.select_agent('source-1');app.action_expand();await app.workers.wait_for_complete();await pilot.pause()
            text=app.query_one('#agent-detail').plain_text
            self.assertEqual(text.count('git status --short'),1)
            for redundant in ('Recent work','Evidence window','Status evidence','Recorded work'):
                self.assertNotIn(redundant,text)
            app.set_session_mode('details');await pilot.pause()
            detail=app.query_one('#agent-detail').plain_text
            self.assertIn('Status evidence',detail)
            self.assertIn('Evidence coverage',detail)
            app.set_session_mode('overview');await pilot.press('s');await pilot.pause()
            self.assertEqual(app.query_one('#pages',TabbedContent).active,'activity')
            self.assertEqual(app.feed.session_id,'session-1')
            # A selected empty filter remains visible and can be cleared.
            app.action_page('agents');app.set_session_scope('all');app.set_agent_filter('errors');await pilot.pause()
            self.assertTrue(app.query_one('#agent-errors',Button).display)
            self.assertTrue(app.query_one('#search-status-agents').display)
            app.set_agent_filter('all');app.query_one('#search-agents',Input).value='No matching task';await pilot.pause(.3)
            self.assertTrue(app.query_one('#search-status-agents').display)
            self.assertIn('No matching sessions',app.query_one('#search-status-agents').render().plain)


class OverviewDeduplicationTests(unittest.TestCase):
    def test_repeated_commands_only_collapse_for_the_exact_source_event(self):
        event=dict(id='one',trace_id='turn-one',service='codex',session_id='session',agent_id='agent',
                   subject='pytest',receiver_id='receiver',turn_id='turn',span_id='span',source_id='source',time_unix_nano=1)
        checks=[dict(command='pytest',source=reference(event))]
        self.assertTrue(check_already_listed(event,checks))
        for field in ('id','source_id','trace_id','span_id','service','session_id','agent_id','receiver_id','turn_id','time_unix_nano'):
            with self.subTest(field=field):
                self.assertFalse(check_already_listed(dict(event,**{field:'another'}),checks))
        self.assertFalse(check_already_listed({'subject':'pytest'},[dict(source={})]))
