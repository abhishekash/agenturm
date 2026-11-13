"""Daily decisions use task outcomes, while raw failures remain inspectable."""
import json
import hashlib
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch

from textual.widgets import DataTable, Select

from observatory.core import Store, normalize_native, summarize
from observatory.logs import normalize_otlp_logs
from observatory.session_status import local_day_start_ns, session_health, today_bucket
from observatory.terminal_ui import ObservatoryApp, review_key, sources, today_order_key
from observatory.ui_workflow import workflow_reasons
from tests.test_terminal_search import ManualFeed


def run_span(state=None, *, root=True, status='OK', stamp=200):
    attrs={'session.id':'session', 'agent.task':'Repair build'}
    if state is not None:attrs['agent.run.state']=state
    return normalize_native({'trace_id':'a'*32,'span_id':('1' if root else '2')*16,
        'parent_span_id':None if root else '1'*16,'name':'agent.run' if root else 'shell',
        'start_unix_nano':100,'end_unix_nano':stamp,'status':status,'attributes':attrs})


class TaskOutcomeTests(unittest.TestCase):
    def test_task_outcome_is_independent_of_child_errors_and_incomplete_evidence(self):
        child=run_span(root=False,status='ERROR')
        for state,expected in [('running','running'),('completed','completed'),
                               ('error','failed'),('failed','failed'),
                               ('interrupted','interrupted'),('incomplete','incomplete'),(None,'unknown')]:
            with self.subTest(state=state):
                summary=summarize([run_span(state),child])
                self.assertEqual(summary['status'],'ERROR')
                self.assertEqual(summary['task_outcome'],expected)
        model=run_span(root=False,status='ERROR');model['parent_span_id']=None
        self.assertEqual(summarize([model])['task_outcome'],'unknown')
        self.assertEqual(summarize([run_span(status='ERROR')])['task_outcome'],'failed')

    def test_cached_summary_sessions_and_ui_keep_recovered_errors_as_evidence(self):
        now=time.time_ns()
        with tempfile.TemporaryDirectory() as directory:
            store=Store(Path(directory)/'store.db')
            try:
                store.ingest([run_span('running',stamp=now),run_span(root=False,status='ERROR',stamp=now)])
                store.ingest([run_span('completed',stamp=now+1)])
                session,=store.sessions()
                self.assertEqual(session['latest_status'],'ERROR')
                self.assertEqual(session['errors'],1)
                self.assertEqual(session['latest_task_outcome'],'completed')
                self.assertEqual(session['latest_run']['task_outcome'],'completed')
                feed=ManualFeed();feed.snapshot.local_status['sessions']=[]
                feed.snapshot.sessions=[session];feed.snapshot.agents=[]
                item,=sources(feed.snapshot)
                self.assertEqual(item['state'],'Completed')
                self.assertEqual(today_bucket(item),'review')
                self.assertEqual(workflow_reasons(item,{},feed.snapshot,{}),[])
            finally:store.close()

    def test_existing_cache_upgrade_reads_only_root_and_lifecycle_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'store.db'
            store=Store(path)
            store.ingest([run_span('completed'),run_span(root=False,status='ERROR')])
            with store._connect() as db:
                db.execute("UPDATE trace_summaries SET summary_json=json_remove(summary_json,'$.task_outcome','$.task_outcome_ns')")
                db.execute("DELETE FROM schema_meta WHERE key='task_outcome_v1'")
            store.close()
            with patch.object(Store,'_refresh_summaries',side_effect=AssertionError('expensive full replay')):
                store=Store(path)
            try:
                self.assertEqual(store.sessions()[0]['latest_task_outcome'],'completed')
                self.assertEqual(store.sessions()[0]['latest_status'],'ERROR')
            finally:store.close()

    def test_unknown_legacy_health_does_not_invent_task_success_or_failure(self):
        feed=ManualFeed();feed.snapshot.local_status['sessions']=[];feed.snapshot.agents=[]
        for status in ('OK','ERROR'):
            feed.snapshot.sessions=[{'session_id':'old','latest_status':status,'last_unix_nano':time.time_ns()}]
            item,=sources(feed.snapshot)
            self.assertEqual(item['state'],'Unknown')

    def test_log_lifecycle_cached_full_trace_and_session_outcomes_agree(self):
        cases=[('gemini_cli.agent.finish',{},'completed'),
               ('agent.progress',{'agent.progress.stage':'awaiting_approval'},'waiting'),
               ('agent.progress',{'agent.run.state':'interrupted'},'interrupted'),
               ('cursor.grok_bot.routine_run',{'cursor.grok_bot.routine_run.outcome':'error'},'failed')]
        for event,attrs,expected in cases:
            with self.subTest(event=event,expected=expected),tempfile.TemporaryDirectory() as directory:
                store=Store(Path(directory)/'store.db')
                payload={'resourceLogs':[{'scopeLogs':[{'logRecords':[{
                    'traceId':'a'*32,'timeUnixNano':'200','eventName':event,
                    'attributes':[{'key':key,'value':{'stringValue':value}}
                        for key,value in {'session.id':'session',**attrs}.items()]}]}]}]}
                try:
                    store.ingest_logs(normalize_otlp_logs(payload))
                    with store._connect() as db:
                        cached=json.loads(db.execute('SELECT summary_json FROM trace_summaries').fetchone()[0])
                    self.assertEqual(cached['task_outcome'],expected)
                    self.assertEqual(store.trace('a'*32)['summary']['task_outcome'],expected)
                    self.assertEqual(store.sessions()[0]['latest_task_outcome'],expected)
                finally:store.close()

    def test_empty_metadata_and_case_variants_do_not_shadow_real_lifecycle(self):
        for state in ('completed','Completed',' completed '):
            with self.subTest(state=state),tempfile.TemporaryDirectory() as directory:
                store=Store(Path(directory)/'store.db')
                records=[]
                for stamp,attrs in [(100,{'agent.run.state':state}),
                                    (200,{'agent.waiting_for':''}),
                                    (300,{'agent.stopped_reason':' '}),
                                    (400,{'agent.wait.reason':'  '})]:
                    records.append({'traceId':'a'*32,'timeUnixNano':str(stamp),'eventName':'agent.progress',
                        'attributes':[{'key':key,'value':{'stringValue':value}}
                            for key,value in {'session.id':'session',**attrs}.items()]})
                try:
                    store.ingest_logs(normalize_otlp_logs({'resourceLogs':[{'scopeLogs':[{'logRecords':records}]}]}))
                    with store._connect() as db:
                        cached=json.loads(db.execute('SELECT summary_json FROM trace_summaries').fetchone()[0])
                    self.assertEqual(cached['task_outcome'],'completed')
                    self.assertEqual(store.trace('a'*32)['summary']['task_outcome'],'completed')
                    self.assertEqual(store.sessions()[0]['latest_task_outcome'],'completed')
                finally:store.close()

    def test_simultaneous_lifecycle_records_use_the_same_stable_tie_break(self):
        results=[]
        for states in (('running','completed'),('completed','running')):
            with tempfile.TemporaryDirectory() as directory:
                store=Store(Path(directory)/'store.db')
                records=[{'traceId':'a'*32,'timeUnixNano':'100','eventName':'agent.progress',
                    'attributes':[{'key':key,'value':{'stringValue':value}}
                        for key,value in {'session.id':'session','agent.run.state':state}.items()]}
                    for state in states]
                try:
                    store.ingest_logs(normalize_otlp_logs({'resourceLogs':[{'scopeLogs':[{'logRecords':records}]}]}))
                    with store._connect() as db:
                        cached=json.loads(db.execute('SELECT summary_json FROM trace_summaries').fetchone()[0])
                    results.append(cached['task_outcome'])
                    self.assertEqual(store.trace('a'*32)['summary']['task_outcome'],results[-1])
                    self.assertEqual(store.sessions()[0]['latest_task_outcome'],results[-1])
                finally:store.close()
        self.assertEqual(*results)


class TodayRulesTests(unittest.TestCase):
    def setUp(self):
        self.now=time.time_ns()
        self.yesterday=local_day_start_ns(self.now)-1

    def item(self,state,stamp=None,**kwargs):
        return {'id':state,'state':state,'stamp':self.now if stamp is None else stamp,**kwargs}

    def test_current_work_and_explicit_waits_survive_midnight_but_old_outcomes_do_not(self):
        cases=[(self.item('Working',self.yesterday,active=True),'running'),
               (self.item('Waiting',self.yesterday,waiting_for='User input'),'attention'),
               (self.item('Possibly stalled',self.yesterday),'attention'),
               (self.item('Completed'),'review'),(self.item('Interrupted'),'attention'),
               (self.item('Unknown'),'unknown'),(self.item('Completed',self.yesterday),''),
               (self.item('Unconfirmed',task_outcome_ns=self.yesterday),'unknown'),
               (self.item('Error',self.yesterday),''),
               (self.item('Completed',task_outcome_ns=self.yesterday),''),
               (self.item('Completed',_workflow_reviewed=True),'')]
        for item,bucket in cases:
            self.assertEqual(today_bucket(item,self.now),bucket,item)
        stale=session_health({'run_state':'running','last_record_ns':self.yesterday-10**12},self.now)
        self.assertFalse(stale['active'])
        self.assertEqual(today_bucket(stale,self.now),'')

    def test_daily_order_and_alerts_do_not_promote_historical_errors(self):
        items=[self.item('Completed'),self.item('Unknown'),self.item('Interrupted'),
               self.item('Working',self.yesterday,active=True)]
        self.assertEqual([item['state'] for item in sorted(items,key=today_order_key)],
                         ['Working','Interrupted','Completed','Unknown'])
        feed=ManualFeed()
        self.assertEqual(workflow_reasons(self.item('Error',self.yesterday),{},feed.snapshot,{},self.now/1e9),[])
        self.assertEqual(workflow_reasons(self.item('Interrupted'),{},feed.snapshot,{},self.now/1e9),['Task interrupted'])


class TodayJourneyTests(unittest.IsolatedAsyncioTestCase):
    async def test_today_history_review_selection_and_narrow_layouts(self):
        feed=ManualFeed();now=time.time_ns();yesterday=local_day_start_ns(now)-1
        base=feed.snapshot.local_status['sessions'][0]
        feed.snapshot.sessions=[];feed.snapshot.activity=[]
        feed.snapshot.local_status['sessions']=[
            {**base,'source_id':sid,'session_id':sid,'task':title,'run_state':state,
             'last_record_ns':stamp,'state_since_ns':stamp,'last_activity':{},'waiting_for':wait,
             'trace_id':hashlib.sha256(sid.encode()).hexdigest()[:32]}
            for sid,title,state,stamp,wait in [
                ('running','Build payments','running',now,''),
                ('waiting','Approve migration','waiting',yesterday,'Approval'),
                ('done','Review generated tests','completed',now,''),
                ('old-error','Old command failure','error',yesterday,''),
                ('yesterday','Yesterday completed work','completed',yesterday,''),
                ('unknown','Outcome not reported','unknown',now,'')]]
        with tempfile.TemporaryDirectory() as directory:
            prefs=Path(directory)/'ui.json'
            app=ObservatoryApp(feed,preferences_path=prefs,persist_preferences=True)
            async with app.run_test(size=(120,36)) as pilot:
                await pilot.pause()
                self.assertEqual(app.agent_filter,'today')
                self.assertEqual(app.visible_agent_rows,['running','waiting','done','unknown'])
                self.assertIn('1 needs attention',app.query_one('#stat-attention').render().plain)
                self.assertIn('Ready for review',str(app.query_one('#agent-table',DataTable).get_row('done')[1]))
                app.select_agent('done');await pilot.pause()
                self.assertIn('checks are not verified',' '.join(app.query_one('#agent-detail').plain_text.split()))
                app.last_update=-1;app.refresh_data();await pilot.pause()
                self.assertEqual(app.selected_source,'done')
                await app.workers.wait_for_complete()
                app.action_review_attention();await app.workers.wait_for_complete();await pilot.pause();app.refresh_data()
                self.assertNotIn('done',app.visible_agent_rows)
                self.assertTrue(app.workflow_controller.reviewed(app.source_map['done']))
                app.save_preferences()
                self.assertEqual(json.loads(prefs.read_text())['reviewed_tasks'],[])
                app.set_agent_filter('all');await pilot.pause()
                self.assertIn('yesterday',app.visible_agent_rows)
                self.assertIn('old-error',app.visible_agent_rows)
                app.select_agent('done');app.action_review_attention();await app.workers.wait_for_complete();await pilot.pause();app.refresh_data()
                app.set_agent_filter('today');await pilot.pause()
                self.assertEqual(app.selected_source,'done')
                for size in ((80,24),(60,20)):
                    await pilot.resize_terminal(*size);await pilot.pause()
                    picker=app.query_one('#agent-filter',Select)
                    self.assertTrue(app.screen.region.contains_region(picker.region))
                    table=app.query_one('#agent-table',DataTable)
                    self.assertGreaterEqual(table.size.height,7)  # Header + three two-line tasks.
                    for footer in ('#shortcuts','#health'):
                        self.assertTrue(app.screen.region.contains_region(app.query_one(footer).region))
                    self.assertIn('q Quit',app.query_one('#shortcuts').render().plain)
                    self.assertFalse(app.query_one('#agent-bottom').display)
                    await pilot.press('e');await pilot.pause()
                    self.assertTrue(app.query_one('#agent-bottom').display)
                    await pilot.press('escape');await pilot.pause()
                # A new completed turn in the same native session needs its own review.
                old_key=review_key(app.source_map['done'])
                feed.snapshot.local_status['sessions'][2]['state_since_ns']=now+1
                feed.snapshot.local_status['sessions'][2]['trace_id']='b'*32
                app.last_update=-1;app.refresh_data();await pilot.pause()
                self.assertNotEqual(review_key(app.source_map['done']),old_key)
