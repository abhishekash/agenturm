"""The expanded session reader uses its width and preserves prompt chronology."""
import json
import threading
import time
import unittest
from urllib.parse import parse_qs, urlsplit
from unittest.mock import patch

from textual.widgets import Button, Static

from observatory.session_prompts import MAX_PROMPT_CHARS, PromptHistory, prompt_scope
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
    async def open_inspector(self,app,pilot,mode='overview'):
        await pilot.pause()
        app.refresh_timer.stop()
        app.set_agent_filter('all')
        app.select_agent('source-1')
        app.action_expand()
        app.set_session_mode(mode)
        await app.workers.wait_for_complete()
        await pilot.pause()

    async def test_overview_leads_with_reported_work_and_modes_separate_prompt_history(self):
        feed=ManualFeed()
        feed.api.activity.extend([prompt(1),prompt(2),dict(prompt(3),action='assistant',subject='Retry implementation is ready; tests are still running.')])
        feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(150,40)) as pilot:
            await self.open_inspector(app,pilot)
            detail=app.query_one('#agent-detail')
            text=detail.plain_text
            self.assertIn('Agent Reported',text)
            self.assertIn('tests are still running',text)
            self.assertNotIn('Request 1:',text)
            self.assertNotIn('Task Usage',text)
            self.assertFalse(any('domain=message' in path for path in feed.api.requests))
            await pilot.press('m')
            await app.workers.wait_for_complete()
            await pilot.pause()
            self.assertEqual(app.session_mode,'prompts')
            self.assertIn('Request 2:',detail.plain_text)
            self.assertNotIn('Request 1:',detail.plain_text)
            self.assertNotIn('tests are still running',detail.plain_text)
            await pilot.press('m')
            await pilot.pause()
            self.assertIn('Session ID',detail.plain_text)
            self.assertIn('Task Usage',detail.plain_text)
            self.assertNotIn('Request 2:',detail.plain_text)

    async def test_selected_prompt_context_original_copy_export_and_responsive_reader(self):
        feed=ManualFeed()
        first=prompt(1)
        first['detail']='<environment_context>local workspace</environment_context>\n\nFix retries.\n    indented continuation\n'+'Readable detail '*15+'RIGHT_EDGE'
        feed.api.activity.extend([first,prompt(2)])
        feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(150,40)) as pilot:
            await self.open_inspector(app,pilot,'prompts')
            await pilot.press('[')
            await pilot.pause()
            self.assertEqual(app.query_one('#agent-prompts').cursor_row,0)
            detail=app.query_one('#agent-detail')
            self.assertIn('Fix retries.',detail.plain_text)
            self.assertIn('    indented continuation',detail.plain_text)
            self.assertNotIn('<environment_context>',detail.plain_text)
            self.assertIn('runtime block',detail.plain_text)
            with patch.object(app,'copy_to_clipboard') as copy:app.action_copy()
            self.assertIn('Fix retries.',copy.call_args.args[0])
            self.assertNotIn('<environment_context>',copy.call_args.args[0])
            await pilot.press('z')
            await pilot.pause()
            self.assertIn('<environment_context>',detail.plain_text)
            with patch.object(app,'copy_to_clipboard') as copy:app.action_copy()
            self.assertIn('<environment_context>',copy.call_args.args[0])
            with patch.object(app,'push_screen') as push:app.action_export()
            exported=json.loads(push.call_args.args[0].content)
            self.assertEqual(exported['evidence']['presentation'],'original captured text')
            self.assertIn('<environment_context>',exported['evidence']['detail'])
            for size in ((120,40),(80,24),(60,20),(60,18)):
                await pilot.resize_terminal(*size)
                await pilot.pause()
                self.assertFalse(detail.show_horizontal_scrollbar)
                self.assertGreaterEqual(detail.content_region.height,7)
                self.assertTrue(app.screen.region.contains_region(detail.region))
                self.assertEqual(app.query_one('#agent-prompts').display,size[0]>=120)
                self.assertTrue(app.query_one('#agent-mode-overview').region.height)
                self.assertTrue(app.query_one('#agent-original').region.height)
                self.assertTrue(app.query_one('#agent-prompt-picker').region.contains_region(
                    app.query_one('#agent-original').region))
                self.assertIn('Readable z',app.query_one('#agent-original').render_line(0).text)
                self.assertIn('RIGHT_EDGE',detail.plain_text.replace('\n',''))
            await pilot.resize_terminal(45,15)
            await pilot.pause()
            self.assertTrue(app.query_one('#terminal-floor').display)
            self.assertIn('60 columns',app.query_one('#terminal-floor').render().plain)

    async def test_minimum_reader_keeps_space_when_review_and_ai_actions_are_available(self):
        feed=ManualFeed();feed.api.activity.append(prompt(2))
        feed.api.sessions[0].update(run_state='waiting',waiting_for='User input',last_record_ns=time.time_ns())
        feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        app.ai_summary_available=True
        async with app.run_test(size=(60,18)) as pilot:
            await self.open_inspector(app,pilot,'prompts')
            for control in ('agent-summary','agent-reviewed','agent-dismissed'):
                self.assertFalse(app.query_one('#'+control).display)
            self.assertGreaterEqual(app.query_one('#agent-detail').content_region.height,7)

    async def test_clipped_older_prompt_handoff_fetches_retained_tail(self):
        feed=ManualFeed();get=feed.api.get
        older=prompt(1);older['detail']='Keep this instruction.\n'*(MAX_PROMPT_CHARS//10)+'RETAINED_TAIL'
        feed.api.activity=[older]+[dict(prompt(i),action='assistant') for i in range(20,230)]
        feed.refresh()
        def api(path):
            if parse_qs(urlsplit(path).query).get('domain')==['message']:
                return {'activity':[older],'next_cursor':None}
            return get(path)
        feed.api.get=api
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,32)) as pilot:
            await self.open_inspector(app,pilot,'prompts')
            self.assertNotIn('RETAINED_TAIL',app.query_one('#agent-detail').plain_text)
            await pilot.press('s');await pilot.pause()
            self.assertEqual(feed.activity_query,older['source_id'])
            feed.refresh();app.last_update=-1;app.refresh_data();await pilot.pause()
            self.assertEqual(app.selected_activity,older['id'])
            self.assertIn('RETAINED_TAIL',app.query_one('#activity-detail').plain_text)

    async def test_older_prompts_preserve_selection_and_scroll_without_repolling(self):
        feed=ManualFeed()
        get=feed.api.get
        calls=[]
        long=prompt(5)
        long['detail']='Selected request\n'+'Keep this line.\n'*50
        def history_api(path):
            query=parse_qs(urlsplit(path).query)
            if query.get('domain')==['message']:
                calls.append(query)
                return ({'activity':[prompt(2),prompt(1)],'next_cursor':None} if query.get('cursor') else
                        {'activity':[long,prompt(4),prompt(3)],'next_cursor':'older'})
            return get(path)
        feed.api.get=history_api
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(150,32)) as pilot:
            await self.open_inspector(app,pilot,'prompts')
            self.assertEqual(len(calls),1)
            selected=app.session_prompts.selected
            detail=app.query_one('#agent-detail')
            detail.scroll_to(y=12,animate=False)
            await pilot.pause()
            position=detail.scroll_y
            self.assertGreater(position,0)
            app.earlier_session_prompts()
            await app.workers.wait_for_complete()
            await pilot.pause()
            self.assertEqual(app.session_prompts.selected,selected)
            self.assertEqual(detail.scroll_y,position)
            self.assertIn('Selected request',detail.plain_text)
            self.assertNotIn('Request 1:',detail.plain_text)
            self.assertEqual(len(app.session_prompts.events),5)
            for _ in range(5):app.show_agent()
            self.assertEqual(len(calls),2)
            app.select_agent('source-2')
            await app.workers.wait_for_complete()
            await pilot.pause()
            self.assertNotIn('Selected request',detail.plain_text)

    async def test_slow_request_does_not_block_navigation_or_publish_stale_prompts(self):
        feed=ManualFeed()
        get=feed.api.get
        started,release=threading.Event(),threading.Event()
        self.addCleanup(release.set)
        def slow_api(path):
            query=parse_qs(urlsplit(path).query)
            if query.get('domain')==['message']:
                if query.get('session_id')==['session-1']:
                    started.set();release.wait(5)
                    return {'activity':[prompt(1)],'next_cursor':None}
                return {'activity':[],'next_cursor':None}
            return get(path)
        feed.api.get=slow_api
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(150,40)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            app.set_agent_filter('all');app.select_agent('source-1')
            app.set_session_mode('prompts')
            await pilot.pause()
            self.assertTrue(started.is_set())
            self.assertIn('Loading',app.query_one('#agent-prompt-status',Static).render().plain)
            app.select_agent('source-2')
            await pilot.pause()
            release.set()
            await app.workers.wait_for_complete()
            await pilot.pause()
            self.assertNotIn('Request 1:',app.query_one('#agent-detail').plain_text)

    async def test_history_failure_has_retry_without_hiding_selected_request(self):
        feed=ManualFeed();feed.api.activity.append(prompt(2));feed.refresh()
        get=feed.api.get
        def broken(path):
            if parse_qs(urlsplit(path).query).get('domain')==['message']:
                raise TimeoutError('Prompt history timed out')
            return get(path)
        feed.api.get=broken
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await self.open_inspector(app,pilot,'prompts')
            self.assertIn('Request 2:',app.query_one('#agent-detail').plain_text)
            self.assertEqual(str(app.query_one('#agent-earlier-prompts',Button).label),'Retry prompts')
            feed.api.get=get
            app.earlier_session_prompts()
            await app.workers.wait_for_complete();await pilot.pause()
            self.assertFalse(app.session_prompts.error)

    async def test_live_updates_preserve_selected_prompt_and_reader_position(self):
        feed=ManualFeed()
        first=prompt(1);first['detail']='First request\n'+'Do not jump while reading.\n'*50
        feed.api.activity.extend([first,prompt(2)]);feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(150,32)) as pilot:
            await self.open_inspector(app,pilot,'prompts')
            app.navigate_prompt('older');await pilot.pause()
            detail=app.query_one('#agent-detail');detail.scroll_to(y=8,animate=False)
            await pilot.pause();position=detail.scroll_y
            selected=app.session_prompts.selected
            feed.snapshot.activity.append(prompt(3));app.show_agent();await pilot.pause()
            self.assertEqual(app.session_prompts.selected,selected)
            self.assertEqual(detail.scroll_y,position)
            self.assertNotIn('Request 3:',detail.plain_text)
            app.navigate_prompt('latest');await pilot.pause()
            self.assertIn('Request 3:',detail.plain_text)
            app.selected_source=None;app.show_agent()
            self.assertFalse(app.query_one('#agent-prompt-picker').display)
            self.assertFalse(app.query_one('#agent-prompt-bar').display)
            self.assertFalse(app.query_one('#agent-prompts').display)

    async def test_revision_refresh_finds_offscreen_prompt_and_preserves_older_cursor(self):
        feed=ManualFeed();get=feed.api.get;calls=[]
        first=prompt(1);second=prompt(2)
        def api(path):
            query=parse_qs(urlsplit(path).query)
            if query.get('domain')==['message']:
                calls.append(query)
                if query.get('cursor'):return {'activity':[first],'next_cursor':'oldest'}
                return {'activity':[second] if len(calls)<3 else [prompt(3),second],'next_cursor':'older'}
            return get(path)
        feed.api.get=api
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,32)) as pilot:
            await self.open_inspector(app,pilot,'prompts')
            app.earlier_session_prompts();await app.workers.wait_for_complete();await pilot.pause()
            app.navigate_prompt('older');await pilot.pause()
            selected=app.session_prompts.selected
            source=feed.snapshot.local_status['sessions'][0]
            source['last_record_ns']=time.time_ns()+1
            app.last_update=-1;app.refresh_data()
            await app.workers.wait_for_complete();await pilot.pause()
            self.assertEqual(len(calls),3)
            self.assertEqual(app.session_prompts.cursor,'oldest')
            self.assertEqual(app.session_prompts.selected,selected)
            app.navigate_prompt('latest');await pilot.pause()
            self.assertIn('Request 3:',app.query_one('#agent-detail').plain_text)

    async def test_selected_older_prompt_opens_exact_activity_even_with_session_errors(self):
        feed=ManualFeed();get=feed.api.get
        older=prompt(1);older.pop('time_unix_nano')
        def api(path):
            if parse_qs(urlsplit(path).query).get('domain')==['message']:
                return {'activity':[older,prompt(2)],'next_cursor':None}
            return get(path)
        feed.api.get=api
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,32)) as pilot:
            await self.open_inspector(app,pilot,'prompts')
            await pilot.press('[');await pilot.pause()
            self.assertIn('Request 1:',app.query_one('#agent-detail').plain_text)
            self.assertIn('Time not reported',app.query_one('#agent-detail').plain_text)
            app.agent_has_failures=True
            await pilot.press('s');await pilot.pause()
            self.assertEqual(app.query_one('#pages').active,'activity')
            self.assertEqual(app.selected_activity,older['id'])
            self.assertNotIn('activity',app.errors_only)
            self.assertIn('Request 1:',app.query_one('#activity-detail').plain_text)


if __name__=='__main__':unittest.main()
