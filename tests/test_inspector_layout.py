"""The expanded session reader uses its width and preserves prompt chronology."""
import json
import threading
import time
import unittest
from urllib.parse import parse_qs, urlsplit
from unittest.mock import patch

from textual.widgets import Button, Static

from observatory.session_prompts import PromptHistory, prompt_scope
from observatory.terminal_ui import ObservatoryApp, session_event_preview
from tests.test_terminal_search import ManualFeed


def prompt(index, *, session='session-1', agent=None):
    return dict(id=f'prompt-{session}-{index}', source_id=f'prompt-{session}-{index}',
                service='codex', session_id=session, agent_id=agent,
                domain='message', action='user', subject=f'Request {index}: preserve the recorded instructions.',
                time_unix_nano=time.time_ns()-int((10-index)*60e9))


class PromptHistoryTests(unittest.TestCase):
    def test_scoped_paging_deduplicates_and_orders_without_other_agent_prompts(self):
        history=PromptHistory(scope=('codex','session-1','worker',False))
        later,earlier=prompt(4,agent='worker'),prompt(1,agent='worker')
        history.merge([later,prompt(2,agent='other'),prompt(3,session='elsewhere',agent='worker')])
        history.accept({'activity':[earlier,later],'next_cursor':'older/cursor'})
        self.assertEqual(history.ordered(),[earlier,later])
        query=parse_qs(urlsplit(history.path(history.cursor)).query)
        self.assertEqual(query['domain'],['message'])
        self.assertEqual(query['action'],['user'])
        self.assertEqual(query['agent_id'],['worker'])
        self.assertEqual(query['subtree'],['0'])
        self.assertEqual(query['cursor'],['older/cursor'])
        self.assertEqual(query['limit'],['100'])
        self.assertIsNone(prompt_scope({'service':'codex'}))
        self.assertIsNone(prompt_scope({'service':'codex','agent_id':'invented','agent_id_inferred':True}))
        legacy=PromptHistory(scope=('codex','session-1','worker',True))
        legacy.merge([prompt(1),prompt(2,agent='worker'),prompt(3,agent='other')])
        self.assertEqual(len(legacy.events),2)
        self.assertNotIn('agent_id',parse_qs(urlsplit(legacy.path()).query))

    def test_expanded_evidence_preserves_indentation_and_text_beyond_column_100(self):
        command='python verify.py --description "'+'keep all this text '*12+'END_OF_COMMAND"'
        output='    '+'indented output '*12+'END_OF_OUTPUT'
        event={'domain':'shell','subject':command,'detail':json.dumps({'output':output})}
        expanded=session_event_preview(event,expanded=True)
        self.assertEqual(expanded['Command'],command)
        self.assertEqual(expanded['Output'],output)
        self.assertNotIn('END_OF_OUTPUT',session_event_preview(event)['Output'])


class InspectorLayoutTests(unittest.IsolatedAsyncioTestCase):
    async def open_inspector(self,app,pilot):
        await pilot.pause()
        app.refresh_timer.stop()
        app.set_agent_filter('all')
        app.select_agent('source-1')
        await pilot.press('e')
        await app.workers.wait_for_complete()
        await pilot.pause()

    async def test_wide_sidebar_narrow_stack_chronology_copy_and_full_width(self):
        feed=ManualFeed()
        events=[prompt(index) for index in (5,2,4,1,3)]
        events[3]['subject']='Request 1: first line\n    indented continuation\n'+'Readable detail '*12+'RIGHT_EDGE'
        feed.api.activity.extend(events)
        feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(190,50)) as pilot:
            await self.open_inspector(app,pilot)
            detail=app.query_one('#agent-detail')
            stats=app.query_one('#agent-stats')
            self.assertTrue(stats.display)
            self.assertEqual(detail.region.y,stats.region.y)
            self.assertGreater(stats.region.x,detail.region.right)
            self.assertGreater(detail.content_region.width,120)
            self.assertEqual(stats.region.right,app.query_one('#agent-info').content_region.right)
            text=detail.plain_text
            positions=[text.index(f'Request {index}:') for index in range(1,6)]
            self.assertEqual(positions,sorted(positions))
            self.assertIn('    indented continuation',text)
            self.assertIn('RIGHT_EDGE',text)
            self.assertNotIn('Earlier Requests',text)
            self.assertNotIn('Task Usage',text)
            self.assertIn('session-1',stats.plain_text)
            self.assertTrue(any(len(line.text.rstrip())>110 for line in detail.lines))
            with patch.object(app,'copy_to_clipboard') as copy:
                app.action_copy()
            copied=copy.call_args.args[0]
            self.assertIn('Request 1:',copied)
            self.assertIn('Task cost: Not reported',copied)
            self.assertIn('Session ID: session-1',copied)
            detail.focus()
            await pilot.press('tab')
            self.assertIs(app.focused,stats)
            self.assertEqual(app.query_one('#masthead').region.height,0)
            self.assertEqual(app.query_one('#controls').region.height,0)
            for size in ((80,24),(60,20),(120,40),(190,50)):
                await pilot.resize_terminal(*size)
                await pilot.pause()
                self.assertEqual(stats.display,size[0]>=120)
                self.assertFalse(detail.show_horizontal_scrollbar)
                self.assertGreaterEqual(detail.content_region.height,10)
                self.assertTrue(app.screen.region.contains_region(detail.region))
                self.assertIn('RIGHT_EDGE',detail.plain_text.replace('\n',''))
                if size[0]<120:
                    self.assertIn('Task Usage',detail.plain_text)
                    self.assertIn('session-1',detail.plain_text)
                    visible='\n'.join(detail.render_line(row).text for row in range(detail.content_region.height))
                    self.assertIn('Request 1:',visible)
                else:
                    self.assertEqual(detail.region.y,stats.region.y)

    async def test_older_prompts_load_on_demand_without_repolling_or_cross_session_leaks(self):
        feed=ManualFeed()
        get=feed.api.get
        calls=[]
        def history_api(path):
            query=parse_qs(urlsplit(path).query)
            if query.get('domain')==['message']:
                calls.append(query)
                if query.get('session_id')==['session-2']:
                    return {'activity':[],'next_cursor':None}
                return ({'activity':[prompt(2),prompt(1)],'next_cursor':None} if query.get('cursor') else
                        {'activity':[prompt(5),prompt(4),prompt(3)],'next_cursor':'older'})
            return get(path)
        feed.api.get=history_api
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(150,40)) as pilot:
            await pilot.pause()
            self.assertEqual(calls,[],'Collapsed previews must not fetch prompt history')
            await self.open_inspector(app,pilot)
            self.assertEqual(len(calls),1)
            self.assertNotIn('Request 1:',app.query_one('#agent-detail').plain_text)
            app.query_one('#agent-earlier-prompts',Button).focus()
            await pilot.press('enter')
            await app.workers.wait_for_complete()
            await pilot.pause()
            text=app.query_one('#agent-detail').plain_text
            self.assertLess(text.index('Request 1:'),text.index('Request 5:'))
            self.assertEqual(text.count('Request 4:'),1)
            self.assertFalse(app.query_one('#agent-earlier-prompts').display)
            for _ in range(5):app.show_agent()
            self.assertEqual(len(calls),2)
            app.select_agent('source-2')
            await app.workers.wait_for_complete()
            await pilot.pause()
            self.assertNotIn('Request 1:',app.query_one('#agent-detail').plain_text)
            self.assertNotIn('session-1',app.query_one('#agent-stats').plain_text)

    async def test_slow_request_does_not_block_navigation_or_publish_stale_prompts(self):
        feed=ManualFeed()
        get=feed.api.get
        started,release=threading.Event(),threading.Event()
        self.addCleanup(release.set)
        def slow_api(path):
            query=parse_qs(urlsplit(path).query)
            if query.get('domain')==['message']:
                if query.get('session_id')==['session-1']:
                    started.set()
                    release.wait(5)
                    return {'activity':[prompt(1)],'next_cursor':None}
                return {'activity':[],'next_cursor':None}
            return get(path)
        feed.api.get=slow_api
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(150,40)) as pilot:
            await pilot.pause()
            app.refresh_timer.stop()
            app.set_agent_filter('all')
            app.select_agent('source-1')
            await pilot.press('e')
            await pilot.pause()
            self.assertTrue(started.is_set())
            self.assertIn('Loading',app.query_one('#agent-prompt-status',Static).render().plain)
            app.select_agent('source-2')
            await pilot.pause()
            self.assertIn('session-2',app.query_one('#agent-stats').plain_text)
            release.set()
            await app.workers.wait_for_complete()
            await pilot.pause()
            self.assertNotIn('Request 1:',app.query_one('#agent-detail').plain_text)

    async def test_live_updates_preserve_reading_position_and_empty_selection_hides_stats(self):
        feed=ManualFeed()
        feed.api.activity.extend(prompt(index) for index in range(1,8))
        feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(150,32)) as pilot:
            await self.open_inspector(app,pilot)
            detail=app.query_one('#agent-detail')
            detail.scroll_to(y=8,animate=False)
            await pilot.pause()
            position=detail.scroll_y
            self.assertGreater(position,0)
            feed.snapshot.activity.append(prompt(8))
            app.show_agent()
            await pilot.pause()
            self.assertEqual(detail.scroll_y,position)
            self.assertIn('Request 8:',detail.plain_text)
            app.selected_source=None
            app.show_agent()
            self.assertFalse(app.query_one('#agent-stats').display)
            self.assertFalse(app.query_one('#agent-prompt-bar').display)
            self.assertNotIn('agent-stats',app.inspector_payloads)

    async def test_history_failure_has_retry_without_hiding_captured_prompts(self):
        feed=ManualFeed()
        feed.api.activity.append(prompt(2))
        feed.refresh()
        get=feed.api.get
        def broken(path):
            if parse_qs(urlsplit(path).query).get('domain')==['message']:
                raise TimeoutError('Prompt history timed out')
            return get(path)
        feed.api.get=broken
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await self.open_inspector(app,pilot)
            self.assertIn('Request 2:',app.query_one('#agent-detail').plain_text)
            self.assertEqual(str(app.query_one('#agent-earlier-prompts',Button).label),'Retry prompts')
            feed.api.get=get
            await pilot.click('#agent-earlier-prompts')
            await app.workers.wait_for_complete()
            await pilot.pause()
            self.assertFalse(app.session_prompts.error)


if __name__=='__main__':
    unittest.main()
