import io
import json
import threading
import time
import tempfile
import unittest
import urllib.error
from dataclasses import replace
from unittest.mock import MagicMock, patch

from textual.widgets import Button, Static
from observatory.feed_requests import ConnectionProgress
from observatory.terminal import Api, Feed, LocalReceiver, Snapshot, run_cockpit
from observatory.terminal_ui import ObservatoryApp, readable_output, session_event_preview, session_excerpt
from observatory.ui_monitoring import connection_feedback, SortScreen
from tests.test_terminal_search import ExampleApi, ManualFeed


class RecoveryTests(unittest.TestCase):
    def test_session_preview_formats_usage_and_bounds_large_payloads(self):
        preview=session_event_preview({'domain':'model','detail':json.dumps({
            'input_tokens':'89289','cached_input_tokens':'88960','output_tokens':'116','total_tokens':'89405'})})
        usage=preview['Latest token report']
        self.assertIn('89,289',usage)
        self.assertIn('88,960',usage)
        self.assertNotIn('{',usage)
        self.assertNotIn('input_tokens',usage)
        preview=session_event_preview({'domain':'shell','subject':'bash -lc "python check.py"',
            'detail':json.dumps({'command':'wrapper','output':'\n'.join(f'    line {i}' for i in range(200))})})
        self.assertEqual(preview['Command'],'python check.py')
        self.assertIn('    line 0',preview['Output'])
        self.assertNotIn('line 8',preview['Output'])
        self.assertIn('Open Activity',preview['Output'])
        self.assertLessEqual(len(preview['Output'].splitlines()),9)
        self.assertLess(len(session_excerpt('x'*100000)),200)

    def test_empty_shell_output_does_not_render_transport_envelope(self):
        preview=session_event_preview({'domain':'shell','subject':'true',
            'detail':json.dumps({'command':'/bin/sh -c true','output':'','cwd':'/tmp','exit_code':'0'})})
        self.assertEqual(preview,{'Command':'true'})

    def test_offline_explicit_endpoint_opens_ui_with_startup_feedback(self):
        stderr=io.StringIO()
        with patch('sys.stdin.isatty',return_value=True),patch('sys.stdout.isatty',return_value=True), \
             patch('sys.stderr',stderr),patch.object(Api,'get',side_effect=urllib.error.URLError('refused')) as get, \
             patch('observatory.terminal_ui.ObservatoryApp') as app,patch('observatory.terminal.LocalReceiver') as receiver:
            run_cockpit(endpoint='http://localhost:14319',local_agents=False)
        get.assert_called_once_with('/api/health?probe=1')
        app.return_value.run.assert_called_once()
        receiver.assert_not_called()
        self.assertIn('Connecting to receiver',stderr.getvalue())

    def test_real_health_probe_does_not_wait_for_collection_status(self):
        with tempfile.TemporaryDirectory() as directory:
            receiver=LocalReceiver(directory+'/test.db',0,None,local_agents=False)
            monitor=MagicMock()
            monitor.status.side_effect=AssertionError('Probe must not collect runtime/session status')
            receiver.store.local_agent_monitor=monitor
            try:
                api=Api(f'http://127.0.0.1:{receiver.server.server_port}')
                self.assertEqual(api.get('/api/health?probe=1'),{'ok':True})
                monitor.status.assert_not_called()
            finally:receiver.close()

    def test_outage_probes_only_health_then_recovers_atomically(self):
        api=ExampleApi();feed=Feed(api)
        feed.refresh();old=feed.snapshot
        api.requests.clear()
        def unavailable(path):
            api.requests.append(path)
            raise ConnectionRefusedError('Connection refused')
        with patch.object(api,'get',side_effect=unavailable):
            feed.refresh()
            api.requests.clear()
            feed.refresh()
        self.assertEqual(api.requests,['/api/health?probe=1'])
        self.assertIs(feed.snapshot.activity,old.activity)
        self.assertEqual(feed.snapshot.updated,old.updated)
        self.assertEqual(feed.connection.phase,'waiting')
        self.assertLessEqual(feed.retry_delay(),2)
        self.assertIsNone(feed.extra_cache)
        label,text,visible=connection_feedback(feed)
        self.assertEqual(label,'Reconnecting');self.assertTrue(visible)
        self.assertIn('retry in',text);self.assertIn('saved data',text)
        feed.refresh()
        self.assertFalse(feed.snapshot.error)
        self.assertEqual(feed.connection.phase,'live')
        self.assertFalse(connection_feedback(feed)[2])

    def test_retry_during_request_does_not_wait_another_poll(self):
        feed=Feed(ExampleApi());feed.POLL_SECONDS=60
        calls=[]
        def refresh():
            calls.append(1)
            if len(calls)==1:feed.retry_now()
            else:feed.stop.set();feed.wake.set()
        feed.refresh=refresh
        worker=threading.Thread(target=feed._loop,daemon=True)
        worker.start();worker.join(2)
        try:self.assertFalse(worker.is_alive())
        finally:feed.stop.set();feed.wake.set();worker.join(2)
        self.assertEqual(len(calls),2)

    def test_health_has_shorter_timeout_than_data_requests(self):
        response=MagicMock()
        response.__enter__.side_effect=lambda:io.BytesIO(b'{}')
        with patch('observatory.terminal.urllib.request.urlopen',return_value=response) as open_url:
            api=Api('http://localhost:14318')
            api.get('/api/health?probe=1');self.assertEqual(open_url.call_args.kwargs['timeout'],1)
            api.get('/api/health');self.assertEqual(open_url.call_args.kwargs['timeout'],3)
            api.get('/api/metrics');self.assertEqual(open_url.call_args.kwargs['timeout'],3)

    def test_initial_and_slow_loading_are_not_reported_live(self):
        feed=Feed(ExampleApi())
        self.assertEqual(connection_feedback(feed)[0],'Connecting')
        feed.refresh()
        feed.connection=ConnectionProgress(phase='loading',stage='Loading sessions',started=time.monotonic()-4)
        label,text,visible=connection_feedback(feed)
        self.assertTrue(visible);self.assertEqual(label,'Updating')
        self.assertIn('Loading sessions',text);self.assertIn('4s',text)

    def test_output_keeps_newlines_indentation_and_removes_escape_codes(self):
        text=readable_output('def check():\r\n\treturn {\n        "ok": True\n    }\x1b[31m')
        self.assertEqual(text,'def check():\n    return {\n        "ok": True\n    }')
        self.assertIn('\n  "nested": {\n    "value": 1',readable_output({'nested':{'value':1}}))
        self.assertIn('Preview clipped',readable_output('x'*20,10))


class ReadableJourneys(unittest.IsolatedAsyncioTestCase):
    async def test_startup_never_says_live_before_a_successful_snapshot(self):
        feed=ManualFeed();feed.snapshot=Snapshot()
        feed.connection=ConnectionProgress(started=time.monotonic())
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause()
            label=lambda:app.query_one('#connection',Static).render_line(0).text
            self.assertIn('Connecting',label());self.assertNotIn('Live',label())
            self.assertEqual(app.inspector_payloads['agent-detail'][0],'Waiting for receiver data')
            self.assertNotIn('Start an agent',app.query_one('#search-status-agents',Static).render_line(0).text)
            feed.snapshot=replace(feed.snapshot,error='Connection refused')
            feed.connection=replace(feed.connection,phase='waiting',retry_at=time.monotonic()+1)
            app.refresh_data();await pilot.pause()
            self.assertIn('Reconnecting',label());self.assertNotIn('Live',label())
            feed.refresh();app.refresh_data();await pilot.pause()
            self.assertIn('Live',label())

    async def test_busy_session_is_a_bounded_summary_with_full_evidence_in_activity(self):
        feed=ManualFeed();now=time.time_ns();base=feed.api.activity[0]
        output='\n'.join(f'    output line {i}' for i in range(200))
        feed.api.activity=[{**base,'source_id':f'event-{i}','time_unix_nano':now+i,
                            'subject':f'python task_{i}.py','detail':json.dumps({'output':output})}
                           for i in range(20)]
        feed.api.activity.append({**base,'source_id':'prompt','time_unix_nano':now+30,
                                  'domain':'message','action':'user','subject':'Make the inspector readable','detail':''})
        feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await pilot.pause();app.set_agent_filter('all');app.select_agent('source-1')
            await pilot.press('e');await pilot.pause()
            text=app.query_one('#agent-detail').plain_text
            self.assertLess(text.index('Make the inspector readable'),text.index('Command'))
            self.assertIn('latest 5 of 20',text)
            self.assertNotIn('task_0.py',text)
            self.assertNotIn('output line 199',text)
            self.assertNotIn('Progress Summary',text)
            await pilot.press('s');await pilot.pause()
            # Summarizing the session must never truncate the stored event.
            self.assertTrue(any('output line 199' in str(e.get('detail'))
                                for e in app.table_data['activity-table'].values()))

    async def test_reconnect_notice_and_retry_are_visible_on_narrow_terminal(self):
        feed=ManualFeed()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(60,20)) as pilot:
            await pilot.pause()
            original=app.selected_source
            feed.snapshot=replace(feed.snapshot,error='Connection refused')
            feed.connection=ConnectionProgress(phase='waiting',attempt=2,started=time.monotonic()-3,retry_at=time.monotonic()+2)
            app.refresh_data();await pilot.pause()
            self.assertTrue(app.query_one('#connection-notice').display)
            retry=app.query_one('#retry-connection',Button)
            self.assertTrue(app.screen.region.contains_region(retry.region))
            self.assertIn('Retry now',retry.render_line(0).text)
            progress=app.query_one('#connection-progress',Static)
            self.assertIn('Reconnecting',progress.render_line(0).text)
            self.assertIn('Showing saved data',progress.render_line(1).text)
            with patch.object(feed,'request_refresh',side_effect=lambda:Feed.request_refresh(feed)):
                await pilot.click('#retry-connection');await pilot.pause()
                self.assertTrue(feed.retry_requested)
                self.assertIn('Retry requested',progress.render_line(0).text)
            self.assertEqual(app.selected_source,original)
            feed.refresh();app.refresh_data();await pilot.pause()
            self.assertFalse(app.query_one('#connection-notice').display)

    async def test_sort_picker_from_inspector_returns_to_sorted_list(self):
        feed=ManualFeed();now=time.time_ns()
        base=feed.api.sessions[0]
        feed.api.sessions=[{**base,'source_id':key,'session_id':key,'task':state,
                            'run_state':state,'last_record_ns':stamp,'last_activity':{},'active':state=='running'}
                           for key,state,stamp in [('working','running',now),('done','completed',now+2),('error','error',now+1)]]
        feed.api.activity=[];feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(60,20)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all');app.tree_mode=False;app.refresh_agent_order()
            app.select_agent('working')
            await pilot.press('e','o');await pilot.pause()
            self.assertIsInstance(app.screen,SortScreen)
            for button in app.screen.query(Button):
                self.assertTrue(app.screen.region.contains_region(button.region))
                self.assertIn(str(button.label),button.render_line(0).text)
            await pilot.press('3');await pilot.pause()
            self.assertEqual(app.visible_agent_rows,['done','error','working'])
            self.assertFalse(app.query_one('#agents').has_class('expanded'))
            self.assertEqual(app.selected_source,'working')
            await pilot.press('o','5');await pilot.pause()
            self.assertEqual(app.visible_agent_rows,['working','error','done'])
            self.assertEqual(app.agent_sort_mode,'status')

    async def test_expanded_inspector_formats_command_and_output_separately(self):
        feed=ManualFeed()
        feed.api.activity[0].update(subject='python check.py',detail='{"output":"def check():\\n    if ready:\\n        return True\\n"}')
        feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await pilot.pause();app.set_agent_filter('all');app.select_agent('source-1')
            await pilot.press('e');await pilot.pause()
            for width in (120,80,60):
                await pilot.resize_terminal(width,30);await pilot.pause()
                log=app.query_one('#agent-detail')
                content=log.plain_text
                self.assertIn('Command',content);self.assertIn('Output',content)
                self.assertNotIn('LATEST RECORDED EVENT',content)
                self.assertIn('def check():\n    if ready:\n        return True',content)
                self.assertIsNone(log.tooltip)
                self.assertEqual(content.count('def check():'),1)
