"""Useful observations stay primary; monitor limitations stay honest."""
import time
import unittest
from dataclasses import replace
from unittest.mock import patch

from textual.widgets import Button, DataTable, RichLog, Static
from observatory.terminal import Feed, Snapshot
from observatory.terminal_ui import ObservatoryApp
from observatory.ui_monitoring import connection_feedback
from tests.test_terminal_search import ManualFeed


class StatusConfidenceJourney(unittest.IsolatedAsyncioTestCase):
    async def test_available_agents_survive_history_delay_without_alarm(self):
        feed = ManualFeed()
        feed.snapshot = replace(feed.snapshot,
            sessions_error='Loading sessions timed out (/api/sessions)',
            sessions_updated=time.time()-120,
            local_status={**feed.snapshot.local_status,
                'hierarchy': {'coverage':'partial', 'announced':0}})
        label, detail, notice = connection_feedback(feed)
        self.assertEqual(label, 'Live')
        self.assertFalse(notice)
        self.assertIn('older session history', detail.casefold())
        self.assertNotIn('delayed', detail.casefold())
        for size in ((120,36), (80,24), (60,18)):
            with self.subTest(size=size):
                app = ObservatoryApp(feed, persist_preferences=False)
                async with app.run_test(size=size) as pilot:
                    await pilot.pause()
                    app.set_agent_filter('all')
                    self.assertGreater(app.query_one('#agent-table',DataTable).row_count, 0)
                    self.assertIn('Live', app.query_one('#connection',Static).render().plain)
                    footer = app.query_one('#health',Static).render().plain
                    self.assertIn('Updated', footer)
                    self.assertIn('d Details', footer)
                    for warning in ('History delayed','Coverage limited',
                                    '0 spawn announcements','relationship coverage'):
                        self.assertNotIn(warning, footer)
                    self.assertIn('older history incomplete',
                        app.query_one('#results-agents').border_title)
                    self.assertEqual(app.summary_values['sessions'][0],
                        sum(item.get('active',False) for item in app.source_map.values()))
                    app.action_diagnostics()
                    await pilot.pause()
                    log = app.screen.query_one('#diagnostics-log')
                    # Responsive rendering may wrap any word boundary; verify
                    # complete diagnostic meaning, not its former fixed width.
                    text = ' '.join(' '.join(line.text for line in log.lines).split())
                    self.assertFalse(log.show_horizontal_scrollbar)
                    self.assertLessEqual(max(line.cell_length for line in log.lines),
                                         log.scrollable_content_region.width)
                    self.assertIn('Older session history: unavailable', text)
                    self.assertIn('/api/sessions', text)
                    self.assertRegex(text, r'last successful read\s+\d+s old')
                    self.assertIn('Some parent/child relationships are not confirmed', text)
                    self.assertNotIn('0 spawn announcements', text)
                    self.assertTrue(app.screen.query_one('#diagnostics-retry',Button).display)
                    with patch.object(feed,'request_refresh',side_effect=lambda:Feed.request_refresh(feed)):
                        app.screen.query_one('#diagnostics-retry',Button).press()
                        await pilot.pause()
                    self.assertTrue(feed.retry_requested)
                    self.assertTrue(app.screen.query_one('#diagnostics-retry',Button).disabled)
                    await pilot.press('escape')
                    await pilot.pause()
                    feed.retry_requested=False

    async def test_empty_history_failure_is_not_a_success_or_no_agents_claim(self):
        feed = ManualFeed()
        feed.snapshot = replace(feed.snapshot, sessions_error='Stored history unavailable',
            sessions_updated=0, sessions=[], local_status={**feed.snapshot.local_status,'sessions':[]})
        label, detail, _ = connection_feedback(feed)
        self.assertEqual(label, 'History unavailable')
        self.assertIn('not established', detail)
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(60,18)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all')
            await pilot.pause()
            self.assertIn('History unavailable', app.query_one('#connection',Static).render().plain)
            self.assertIn('Older history unavailable', app.query_one('#health',Static).render().plain)
            self.assertEqual(app.summary_values['sessions'][0], '—')
            self.assertEqual(app.summary_values['attention'][0], '—')
            self.assertIn('not known', repr(app.inspector_payloads['agent-detail']).casefold())

    async def test_real_disconnection_and_auth_stay_explicit_and_recover(self):
        feed = ManualFeed()
        stamp = feed.snapshot.updated
        feed.snapshot = replace(feed.snapshot,error='Connection refused')
        label, _, visible = connection_feedback(feed)
        self.assertEqual(label, 'Reconnecting')
        self.assertTrue(visible)
        app = ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(60,18)) as pilot:
            await pilot.pause()
            self.assertIn('Reconnecting',app.query_one('#connection',Static).render().plain)
            self.assertIn('Connection refused',app.query_one('#health',Static).render().plain)
            self.assertFalse(app.query_one('#connection-notice').display)
            with patch.object(feed,'request_refresh',side_effect=lambda:Feed.request_refresh(feed)):
                await pilot.press('R')
                await pilot.pause()
            self.assertIn('Refresh requested',app.query_one('#health',Static).render().plain)
            self.assertTrue(app.query_one('#retry-connection',Button).disabled)
        feed.snapshot = replace(feed.snapshot,error='Authentication required',updated=stamp)
        feed.connection = replace(feed.connection,phase='auth')
        self.assertEqual(connection_feedback(feed)[0], 'Authentication required')

    async def test_initialization_shows_preparing_not_a_delay_warning(self):
        feed = ManualFeed()
        feed.snapshot = Snapshot()
        label, detail, _ = connection_feedback(feed)
        self.assertEqual(label, 'Preparing view')
        self.assertIn('first observations', detail)
        self.assertNotIn('delayed', detail.casefold())
        app = ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(60,18)) as pilot:
            await pilot.pause()
            self.assertIn('Preparing view',app.query_one('#connection',Static).render().plain)
            self.assertIn('Preparing view',app.query_one('#health',Static).render().plain)
            self.assertEqual(app.summary_values['sessions'][0], '—')
            self.assertTrue(app.query_one('#retry-connection',Button).display)
            await pilot.press('d')
            await pilot.pause()
            text = '\n'.join(line.text for line in app.screen.query_one(RichLog).lines)
            self.assertIn('Preparing view',text)
            self.assertIn('No observations have been published yet',text)
            await pilot.press('escape')


if __name__ == '__main__':
    unittest.main()
