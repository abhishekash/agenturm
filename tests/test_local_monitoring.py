"""Local monitoring journeys: freeze, investigate, return, and honest spend."""
import copy
import tempfile
import time
import unittest
from pathlib import Path

from textual.widgets import Button, DataTable, Input, Static, TabbedContent
from observatory.core import Store, normalize_native
from observatory.terminal import Api, Feed, LocalReceiver, Snapshot
from observatory.logs import normalize_otlp_logs
from observatory.terminal_ui import ObservatoryApp
from observatory.ui_monitoring import UsageScreen, failure_groups, usage_summary
from tests.test_terminal_search import ManualFeed, ExampleApi


class MonitoringModelsTests(unittest.TestCase):
    def test_paused_history_uses_the_displayed_page_boundary(self):
        feed=Feed(ExampleApi())
        feed.snapshot=Snapshot(pagination={'activity':{'next_cursor':'new-boundary'}})
        frozen=Snapshot(pagination={'activity':{'next_cursor':'frozen-boundary'}})
        self.assertTrue(feed.navigate_history('activity','older',snapshot=frozen))
        self.assertEqual(feed.history['activity'],[None,'frozen-boundary'])

    def test_real_receiver_finds_failures_older_than_recent_activity(self):
        with tempfile.TemporaryDirectory() as directory:
            receiver=LocalReceiver(Path(directory)/'test.db',0,None,local_agents=False)
            feed=Feed(Api(f'http://127.0.0.1:{receiver.server.server_port}'))
            try:
                events=[]
                for i in range(205):
                    attrs={'event.name':'agent.activity','session.id':'local-session',
                           'agent.activity.domain':'shell','agent.activity.action':'run',
                           'agent.activity.subject':'pytest' if i==0 else 'git status',
                           'agent.activity.result':'error' if i==0 else 'success'}
                    events.append({'timeUnixNano':time.time_ns()+i,'severityNumber':17 if i==0 else 9,
                        'body':{'stringValue':'old failure evidence' if i==0 else 'ok'},
                        'attributes':[{'key':key,'value':{'stringValue':value}} for key,value in attrs.items()]})
                receiver.store.ingest_logs(normalize_otlp_logs({'resourceLogs':[{'scopeLogs':[{'logRecords':events}]}]}))
                feed.session_id='local-session';feed.detail_enabled=False
                feed.refresh()
                self.assertFalse(feed.snapshot.error)
                self.assertEqual(len(feed.snapshot.activity),200)
                self.assertFalse(any(event['result']=='error' for event in feed.snapshot.activity))
                feed.activity_errors=True;feed.refresh()
                self.assertFalse(feed.snapshot.error)
                self.assertEqual(len(feed.snapshot.activity_results),1)
                self.assertEqual(feed.snapshot.activity_results[0]['detail'],'old failure evidence')
                self.assertTrue(feed.snapshot.activity_errors)
            finally:
                feed.close();receiver.close()

    def test_matching_failures_do_not_claim_recovery_from_other_work(self):
        base={'session_id':'one','service':'codex','domain':'shell','action':'run',
              'subject':'pytest','result':'error','time_unix_nano':1,'id':'first'}
        events=[base,{**base,'id':'second','time_unix_nano':2},
                {**base,'id':'other-session','session_id':'two'}]
        unrelated={**base,'id':'ok','result':'success','subject':'git status','time_unix_nano':3}
        groups=failure_groups(events,[unrelated])
        self.assertEqual(len(groups),2)
        self.assertEqual(len(groups[0]['_failures']),2)
        self.assertIsNone(groups[0]['_later_success'])
        success={**unrelated,'subject':'pytest'}
        self.assertEqual(failure_groups(events,[success])[0]['_later_success'],success)
        self.assertIsNone(failure_groups(events,[success])[1]['_later_success'])

    def test_usage_distinguishes_unknown_zero_and_partial(self):
        summary,details=usage_summary({'input_tokens':1200,'output_tokens':300,
            'runs':3,'unknown_cost_runs':1,'known_cost_usd':0,
            'average_task_cost_usd':0,'costed_tasks':2,'completed_tasks':3})
        self.assertIn('Tokens 1,500',summary)
        self.assertIn('Cost $0.0000+',summary)
        self.assertIn('Avg/task $0.0000',summary)
        self.assertIn('2 fully priced completed runs of 3',details)
        summary,_=usage_summary({'input_tokens':0,'input_tokens_reported':False,
                                'output_tokens':None,'runs':1,'unknown_cost_runs':1,'known_cost_usd':0})
        self.assertIn('Tokens Not reported',summary)
        self.assertIn('Cost Not reported',summary)
        self.assertIn('Avg/task Not reported',summary)

    def test_completed_run_average_excludes_running_and_missing_cost(self):
        with tempfile.TemporaryDirectory() as directory:
            store=Store(Path(directory)/'test.db')
            try:
                spans=[]
                for i,(cost,state) in enumerate(((.2,'OK'),(.4,'ERROR'),(None,'OK'),(1.5,'RUNNING'),(0,'OK')),1):
                    attrs={'gen_ai.usage.input_tokens':100,'gen_ai.usage.output_tokens':10}
                    if cost is not None:attrs['llm.cost.total']=cost
                    spans.append(normalize_native({'trace_id':f'{i:032x}','span_id':f'{i:016x}',
                        'name':'model.call','start_unix_nano':100,'end_unix_nano':200,
                        'attributes':attrs,'status':state}))
                    if state=='RUNNING':
                        spans.append(normalize_native({'trace_id':f'{i:032x}','span_id':'f'*16,
                            'name':'agent.run','start_unix_nano':100,'end_unix_nano':200,
                            'attributes':{'agent.run.state':'running'},'status':'UNSET'}))
                store.ingest(spans)
                totals=store.metrics()
                self.assertEqual(totals['completed_tasks'],4)
                self.assertEqual(totals['costed_tasks'],3)
                self.assertEqual(totals['average_task_cost_usd'],.2)
                self.assertEqual(totals['known_cost_usd'],2.1)
                self.assertEqual(totals['input_tokens'],500)
            finally:store.close()

    def test_paused_feed_still_polls_and_activity_errors_are_server_filtered(self):
        feed=Feed(ExampleApi())
        feed.paused=True
        def refresh():
            feed.stop.set()
            feed.wake.set()
        feed.refresh=refresh
        feed._loop()
        self.assertTrue(feed.stop.is_set())
        feed=ManualFeed()
        feed.activity_errors=True
        feed.refresh()
        self.assertTrue(feed.snapshot.activity_errors)
        self.assertTrue(any('errors_only=1' in path for path in feed.api.requests))


class LocalMonitoringJourneys(unittest.IsolatedAsyncioTestCase):
    async def test_trace_only_failure_keeps_a_scoped_trace_action(self):
        feed=ManualFeed()
        feed.api.activity=[]
        feed.api.sessions[0]['run_state']='error'
        feed.api.sessions[0]['latest_trace_id']='trace-1'
        feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await pilot.pause()
            app.set_agent_filter('errors');app.select_agent('source-1')
            await pilot.press('s');await pilot.pause()
            self.assertIsNone(app.selected_activity)
            self.assertFalse(app.query_one('#activity-trace',Button).disabled)
            self.assertIn('trace-level failure',app.query_one('#activity-detail').plain_text)
            await pilot.press('t');await pilot.pause()
            self.assertEqual(feed.selected_trace,'trace-1')
            await pilot.press('alt+left');await pilot.pause()
            app.action_all();await pilot.pause()
            self.assertIsNone(app.activity_trace_fallback())

    async def test_real_local_feed_investigation_while_paused(self):
        with tempfile.TemporaryDirectory() as directory:
            receiver=LocalReceiver(Path(directory)/'journey.db',0,None,local_agents=False)
            feed=Feed(Api(f'http://127.0.0.1:{receiver.server.server_port}'))
            feed.POLL_SECONDS=.2;feed.EXTRA_POLL_SECONDS=.2
            tid,sid='a'*32,'b'*16
            now=time.time_ns()
            receiver.store.ingest([normalize_native({'trace_id':tid,'span_id':sid,'name':'model.call',
                'start_unix_nano':now-100,'end_unix_nano':now,'status':'ERROR',
                'attributes':{'session.id':'local-session','agent.task':'Fix local tests',
                              'gen_ai.usage.input_tokens':100,'gen_ai.usage.output_tokens':20,'llm.cost.total':.3}})])
            attrs={'event.name':'agent.activity','session.id':'local-session',
                   'agent.activity.domain':'shell','agent.activity.action':'run',
                   'agent.activity.subject':'pytest','agent.activity.result':'error'}
            receiver.store.ingest_logs(normalize_otlp_logs({'resourceLogs':[{'scopeLogs':[{'logRecords':[{
                'traceId':tid,'spanId':sid,'timeUnixNano':now,'severityNumber':17,
                'body':{'stringValue':'local assertion failed'},
                'attributes':[{'key':key,'value':{'stringValue':value}} for key,value in attrs.items()]}]}]}]}))
            receiver.store.metrics()  # Drain deferred summaries before publishing the first snapshot.
            feed.detail_enabled=False;feed.refresh()
            app=ObservatoryApp(feed,persist_preferences=False)
            try:
                async with app.run_test(size=(120,40)) as pilot:
                    async def until(predicate):
                        for _ in range(60):
                            if predicate():return
                            await pilot.pause(.1)
                        self.fail('Timed out waiting for local receiver/UI: '+feed.snapshot.error)
                    await until(lambda:bool(getattr(app,'source_map',{})))
                    source=next(key for key,value in app.source_map.items() if value.get('session_id')=='local-session')
                    app.set_agent_filter('errors');app.select_agent(source)
                    await pilot.press('p','s')
                    await until(lambda:app.selected_activity and
                                app.table_data.get('activity-table',{}).get(app.selected_activity,{}).get('detail')=='local assertion failed')
                    selected=app.selected_activity
                    self.assertTrue(feed.paused)
                    await pilot.press('t')
                    await until(lambda:bool(app.query_one('#span-tree').cursor_node and
                                app.query_one('#span-tree').cursor_node.data))
                    self.assertEqual(app.query_one('#span-tree').cursor_node.data['span_id'],sid)
                    await pilot.press('alt+left');await pilot.pause(.4)
                    self.assertEqual(app.selected_activity,selected)
                    self.assertEqual(app.query_one('#pages',TabbedContent).active,'activity')
                    await pilot.press('alt+left');await pilot.pause(.4)
                    self.assertEqual(app.selected_source,source)
                    self.assertEqual(app.scope_value,'all')
                    self.assertEqual(app.query_one('#pages',TabbedContent).active,'agents')
                    self.assertFalse(feed.snapshot.error)
            finally:
                feed.close();receiver.close()

    async def test_pause_keeps_order_and_selection_then_follow_catches_up(self):
        feed=ManualFeed()
        now=time.time_ns()
        for i,session in enumerate(feed.api.sessions):
            session.update(run_state='running',last_record_ns=now-i,active=True)
        feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all');app.select_agent('source-1')
            original=list(app.visible_agent_rows)
            await pilot.press('p');await pilot.pause()
            # Feed snapshots are immutable publications; avoid aliasing the fixture.
            feed.api.sessions=copy.deepcopy(feed.api.sessions)
            feed.api.sessions[1]['last_record_ns']=now+1_000_000_000
            feed.refresh();app.refresh_data();await pilot.pause()
            self.assertEqual(app.visible_agent_rows,original)
            self.assertEqual(app.selected_source,'source-1')
            self.assertEqual(app.pause_change_count,1)
            feed.refresh();app.refresh_data();await pilot.pause()
            self.assertEqual(app.pause_change_count,1)  # Polls aren't new events.
            for size in ((80,24),(60,20)):
                await pilot.resize_terminal(*size);await pilot.pause()
                button=app.query_one('#pause',Button)
                self.assertIn('Follow live',button.render_line(0).text)
                usage=app.query_one('#usage-summary',Static)
                self.assertTrue(app.screen.region.contains_region(usage.region))
                rendered='\n'.join(usage.render_line(row).text for row in range(2))
                self.assertIn('Tokens Not reported',rendered)
                self.assertIn('Task cost Not reported',rendered)
                self.assertNotIn('Avg/task',rendered)
            await pilot.press('p');await pilot.pause()
            self.assertFalse(feed.paused)
            self.assertEqual(app.visible_agent_rows,list(reversed(original)))
            self.assertEqual(app.selected_source,'source-1')
            await pilot.press('u');await pilot.pause()
            self.assertIsInstance(app.screen,UsageScreen)
            await pilot.press('escape');await pilot.pause()

    async def test_error_to_exact_span_and_back_restores_session_and_event(self):
        feed=ManualFeed()
        now=time.time_ns()
        event=feed.api.activity[0]
        event.update(id='failed-event',result='error',detail='{"input":"pytest","output":"assertion failed"}',
                     trace_id='trace-1',span_id='span-2',time_unix_nano=now-2)
        feed.api.activity=[{**event,'id':'newer-event','source_id':'newer','result':'success',
                            'subject':'git status','time_unix_nano':now},event]
        feed.api.sessions[0]['run_state']='error'
        feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(160,48)) as pilot:
            await pilot.pause()
            app.set_agent_filter('errors');app.select_agent('source-1')
            app.action_pause()
            # A stale Activity search must not hide the failed event.
            app.search_queries['activity']='missing';feed.activity_query='missing'
            app.query_one('#search-activity',Input).value='missing'
            await pilot.pause(.4)
            await pilot.press('s');await pilot.pause()
            self.assertEqual(app.query_one('#pages',TabbedContent).active,'activity')
            self.assertEqual(app.selected_activity,'failed-event')
            self.assertEqual(app.scope_value,'source-1')
            self.assertEqual(app.search_queries['activity'],'')
            self.assertIn('assertion failed',app.query_one('#activity-detail').plain_text)
            self.assertIn('git status',app.query_one('#activity-detail').plain_text)
            self.assertIn('resolution is unknown',app.query_one('#activity-detail').plain_text)
            await pilot.press('t');await pilot.pause()
            self.assertEqual(app.query_one('#pages',TabbedContent).active,'traces')
            self.assertEqual(feed.selected_span_id,'span-2')
            self.assertEqual(app.query_one('#span-tree').cursor_node.data['span_id'],'span-2')
            await pilot.press('alt+left');await pilot.pause()
            self.assertEqual(app.query_one('#pages',TabbedContent).active,'activity')
            self.assertEqual(app.selected_activity,'failed-event')
            self.assertEqual(app.query_one('#activity-table',DataTable).get_row_at(0)[2].plain,'Error')
            await pilot.press('alt+left');await pilot.pause()
            self.assertEqual(app.query_one('#pages',TabbedContent).active,'agents')
            self.assertEqual(app.selected_source,'source-1')
            self.assertEqual(app.agent_filter,'errors')
            self.assertEqual(app.scope_value,'all')
            self.assertEqual(app.search_queries['activity'],'missing')
            self.assertTrue(feed.paused)

