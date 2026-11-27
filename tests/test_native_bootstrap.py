"""Native startup metadata is useful without certifying historical evidence."""
from copy import deepcopy
from dataclasses import replace
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from observatory.terminal import Feed,FeedScope,Snapshot
from observatory.native_bootstrap import native_metadata,ready_native,published_metadata
from observatory.source_projection import sources,native_snapshot
from tests.test_feed_capture_safety import PolicyApi,PRIVATE


class NativeApi(PolicyApi):
    def __init__(self):
        super().__init__();self.ready=False;self.calls=[];self.healths=0;self.mutate_health=None
    def health(self):
        result=super().health();now=time.time_ns()
        result.update(ok=True,observed_time_unix_nano=now)
        if not self.ready:result['indexing']['stage']='Indexing source records · 20 indexed'
        result['local_agents'].update(enabled=True,initial_poll_complete=True)
        for row in result['local_agents']['sessions']:
            row.update(task=PRIVATE,project=PRIVATE,working_directory='/'+PRIVATE,
                source_namespace='/'+PRIVATE,waiting_for=PRIVATE,parent_context={'task':PRIVATE},
                last_activity={'subject':PRIVATE,'detail':PRIVATE},
                diagnostics={'last_success_ns':now,'backlog_bytes':0,'pending_storage_records':0,'source_path':'/'+PRIVATE})
        return result
    def get(self,path):
        self.calls.append(path)
        if path=='/api/health':
            self.healths+=1;result=self.health()
            return self.mutate_health(self.healths,result)if self.mutate_health else result
        if not path.startswith('/api/health')and not self.ready:raise AssertionError('Historical read started during native bootstrap: '+path)
        return super().get(path)


def bootstrap():
    api=NativeApi();feed=Feed(api);feed.detail_enabled=False;feed.include_global_totals=False
    feed.refresh()
    assert feed.snapshot.native_bootstrap,feed.snapshot.error
    return api,feed


class NativeBootstrapTests(unittest.TestCase):
    def test_bootstrap_task_label_requires_current_per_session_opt_in_and_is_sanitized(self):
        health=NativeApi().health()
        local=health['local_agents'];local['capture_content']=True
        row=local['sessions'][0];row['capture_content']=True
        row['initial_task']='<environment_context>private context</environment_context>\nFix token refresh.'
        snapshot=native_metadata(health)
        self.assertEqual(snapshot['sessions'][0]['initial_task'],'Fix token refresh.')
        self.assertNotIn('private context',repr(snapshot))

        row['capture_content']=False
        self.assertEqual(native_metadata(health)['sessions'][0]['initial_task'],'')
        row['capture_content']=True;local['capture_content']=False
        self.assertEqual(native_metadata(health)['sessions'][0]['initial_task'],'')

        for malformed_policy in (0,1,'enabled'):
            local['capture_content']=malformed_policy
            self.assertEqual(native_metadata(health)['sessions'][0]['initial_task'],'')

        local['capture_content']=True;row.pop('capture_content')
        self.assertEqual(native_metadata(health)['sessions'][0]['initial_task'],'')

    def test_published_bootstrap_drops_label_when_global_policy_denies_capture(self):
        health=NativeApi().health();local=health['local_agents']
        local['capture_content']=True
        row=local['sessions'][0];row['capture_content']=True
        row['initial_task']='Repair retry handling'
        published=published_metadata(native_metadata(health))
        self.assertEqual(published['sessions'][0]['initial_task'],'Repair retry handling')
        local_snapshot=native_metadata(health)
        local_snapshot['capture_content']=False
        self.assertEqual(published_metadata(local_snapshot)['sessions'][0]['initial_task'],'')
        local_snapshot['capture_content']=0
        published=published_metadata(local_snapshot)
        self.assertNotIn('capture_content',published)
        self.assertEqual(published['sessions'][0]['initial_task'],'')

    def test_bootstrap_task_label_rejects_oversized_untrusted_input(self):
        health=NativeApi().health();local=health['local_agents']
        local['capture_content']=True
        row=local['sessions'][0];row['capture_content']=True
        row['initial_task']='private marker '+('x'*5000)
        snapshot=native_metadata(health)
        self.assertEqual(snapshot['sessions'][0]['initial_task'],'')
        self.assertNotIn('private marker',repr(snapshot))

    def test_health_first_independent_final_and_no_history_reads_or_content(self):
        api,feed=bootstrap();snapshot=feed.snapshot
        self.assertEqual(api.calls,['/api/health?probe=1','/api/health','/api/health'])
        self.assertEqual(snapshot.updated,0);self.assertGreater(snapshot.bootstrap_updated,0)
        self.assertFalse(snapshot.indexing_ready);self.assertNotIn(PRIVATE,repr(snapshot))
        self.assertFalse(snapshot.native_lifecycle);self.assertTrue(snapshot.native_lifecycle_prepared)
        self.assertTrue(sources(snapshot)[0]['active'])
        self.assertTrue(all(row['capture_content']is False for row in sources(snapshot)))
        self.assertIsNone(feed.extra_cache);self.assertIsNone(feed.findings_cache)

    def test_native_proof_is_strict_and_history_failures_are_not_ordinary_loading(self):
        health=NativeApi().health()
        for key,value in (('ok',False),('observed_time_unix_nano',0)):
            current=deepcopy(health);current[key]=value;self.assertFalse(ready_native(current))
        for key,value in (('enabled',1),('initial_poll_complete',False),('initial_poll_complete',None)):
            current=deepcopy(health);current['local_agents'][key]=value;self.assertFalse(ready_native(current))
        for extra in ({'error':'fatal history failure'},{'stage':'Receiver is stopping'},{'stage':'History migration failed'},
                {'stage':'Readiness probe unavailable'},{'stage':'synthetic unknown'}):
            current=deepcopy(health);current['indexing'].update(extra);self.assertFalse(ready_native(current))
        self.assertFalse(ready_native(None))

    def test_rows_and_tombstones_are_whitelisted_after_policy_overlay(self):
        api=NativeApi()
        def change(count,result):
            if count==1:result['local_agents']['sessions'][0]['capture_content']=False
            else:result['local_agents']['sessions']=[]
            return result
        api.mutate_health=change;feed=Feed(api);feed.refresh()
        self.assertTrue(feed.snapshot.native_bootstrap);self.assertNotIn(PRIVATE,repr(feed.snapshot))
        self.assertTrue(feed._capture_denials)
        self.assertEqual(feed.snapshot.local_status['sessions'],[],'A missing final native row must not reappear as a policy tombstone')
        self.assertTrue(feed.snapshot.local_status['capture_policy_revisions'])

    def test_partial_positive_never_restores_denial_but_coherent_allow_can(self):
        api=NativeApi()
        api.mutate_health=lambda count,row:({**row,'local_agents':{**row['local_agents'],'capture_content':False}}if count==1 else row)
        feed=Feed(api);feed.include_global_totals=False;feed.refresh()
        self.assertFalse(feed.snapshot.local_status['capture_content']);self.assertTrue(feed._capture_global_denied)
        api.mutate_health=None;feed.refresh()
        self.assertTrue(feed.snapshot.native_bootstrap);self.assertTrue(feed._capture_global_denied)
        api.ready=True;feed.refresh()
        self.assertGreater(feed.snapshot.updated,0);self.assertFalse(feed.snapshot.native_bootstrap)
        self.assertFalse(feed._capture_global_denied)

    def test_final_receiver_change_cannot_publish_or_relabel_native_rows(self):
        api=NativeApi();api.mutate_health=lambda count,row:({**row,'receiver_id':'replacement'}if count==2 else row)
        feed=Feed(api);feed.refresh()
        self.assertFalse(feed.snapshot.native_bootstrap);self.assertFalse(feed.snapshot.local_status)
        self.assertIn('identity changed',feed.snapshot.error)

    def test_late_partial_callback_never_overwrites_first_coherent_snapshot(self):
        api,feed=bootstrap();scope=FeedScope.capture(feed);epoch=feed._capture_epoch;health=api.health()
        winner=replace(feed.snapshot,native_bootstrap=False,bootstrap_updated=0,updated=time.time())
        feed.snapshot=winner
        self.assertFalse(feed._publish_native_bootstrap(health,health,scope,epoch));self.assertIs(feed.snapshot,winner)

    def test_first_coherent_other_receiver_wins_even_without_epoch_change(self):
        api=NativeApi();feed=Feed(api);scope=FeedScope.capture(feed);epoch=feed._capture_epoch;health=api.health()
        winner=Snapshot(receiver_id='other-coherent-receiver',updated=time.time())
        feed.snapshot=winner
        self.assertFalse(feed._publish_native_bootstrap(health,health,scope,epoch));self.assertIs(feed.snapshot,winner)

    def test_existing_coherent_snapshot_is_not_downgraded_during_incoming_indexing(self):
        api=NativeApi();api.ready=True;feed=Feed(api);feed.include_global_totals=False;feed.refresh();old=feed.snapshot
        self.assertGreater(old.updated,0)
        api.ready=False;feed.refresh()
        self.assertFalse(feed.snapshot.native_bootstrap);self.assertEqual(feed.snapshot.updated,old.updated)

    def test_duplicate_identity_overflow_and_unproven_collector_status_are_conservative(self):
        health=NativeApi().health();row=health['local_agents']['sessions'][0]
        other=deepcopy(row);other['agent_id']=health['local_agents']['sessions'][1]['agent_id']
        health['local_agents']['sessions']=[row,other]
        self.assertEqual(native_metadata(health)['sessions'],[])
        health=NativeApi().health();health['local_agents']['sessions'][0]['diagnostics']['pending_storage_records']=1
        self.assertEqual(native_metadata(health)['sessions'][0]['run_state'],'unknown')
        health['local_agents']['sessions'][1]['diagnostics'].pop('backlog_bytes')
        self.assertEqual(native_metadata(health)['sessions'][1]['run_state'],'unknown')
        from observatory.agent_hierarchy import canonical_agent_id
        health=NativeApi().health();template=health['local_agents']['sessions'][0]
        health['local_agents']['sessions']=[{**template,'source_id':'generated-'+str(index),
            'agent_id':canonical_agent_id('Codex','generated',str(index))}for index in range(501)]
        bounded=native_metadata(health)
        self.assertEqual(len(bounded['sessions']),500);self.assertEqual(bounded['bootstrap_omitted_rows'],1)

    def test_native_health_staleness_ages_without_inventing_activity_or_completion(self):
        _,feed=bootstrap();snapshot=replace(feed.snapshot,bootstrap_updated=time.time()-20)
        self.assertTrue(all(not row['active']for row in sources(snapshot)))
        self.assertTrue(all(row['state']=='Monitor delayed'for row in sources(snapshot)))

    def test_unknown_readiness_error_keeps_metadata_stale_and_never_copies_stage_text(self):
        from observatory.ui_monitoring import connection_feedback
        api,feed=bootstrap();stamp=feed.snapshot.bootstrap_updated
        api.mutate_health=lambda count,row:{**row,'indexing':{'ready':False,'stage':'/'+PRIVATE}}
        feed.refresh()
        self.assertTrue(feed.snapshot.native_bootstrap);self.assertEqual(feed.snapshot.bootstrap_updated,stamp)
        self.assertNotIn(PRIVATE,repr(feed.snapshot));self.assertIn('readiness is unavailable',feed.snapshot.error)
        label,detail,_=connection_feedback(feed)
        self.assertEqual(label,'Reconnecting');self.assertIn('current observations',detail)
        self.assertTrue(all(not row['active']for row in sources(feed.snapshot)))

    def test_authentication_error_is_not_masked_by_indexing_banner(self):
        import urllib.error
        from observatory.ui_monitoring import connection_feedback
        api,feed=bootstrap()
        with patch.object(api,'get',side_effect=urllib.error.HTTPError('http://generated.invalid',401,'unauthorized',{},None)):
            feed.refresh()
        self.assertEqual(connection_feedback(feed)[0],'Authentication required')
        self.assertTrue(all(not row['active']for row in sources(feed.snapshot)))

    def test_future_collector_and_inconsistent_outcome_clocks_never_claim_live_or_completed(self):
        for change in ('collector','outcome','start'):
            health=NativeApi().health();row=health['local_agents']['sessions'][0];now=health['observed_time_unix_nano']
            row.update(run_state='completed',state_since_ns=now-2,outcome_time_ns=now-2,run_start_ns=now-3)
            if change=='collector':row['diagnostics']['last_success_ns']=now+1
            elif change=='outcome':row['outcome_time_ns']=now+1
            else:row['run_start_ns']=now-1
            self.assertEqual(native_metadata(health)['sessions'][0]['run_state'],'unknown',change)

    def test_no_native_overlay_or_workflow_ledger_mutation(self):
        from observatory.ui_workflow_controller import WorkflowController
        _,feed=bootstrap();owner=SimpleNamespace(feed=feed)
        controller=WorkflowController(owner)
        with patch.object(controller,'_enqueue',side_effect=AssertionError('Bootstrap must not queue workflow jobs')):
            controller.update(sources(feed.snapshot),feed.snapshot,{})
            controller.acknowledge(sources(feed.snapshot)[0]);controller.review_completions(('old',))
        self.assertIsNone(controller._latest);self.assertFalse(controller.ledger.incidents)
        self.assertIs(native_snapshot(feed.snapshot,[{'body':PRIVATE}]),feed.snapshot)

    def test_opencode_without_per_row_freshness_proof_stays_unknown(self):
        health=NativeApi().health();row=health['local_agents']['sessions'][0]
        row.update(runtime='OpenCode',service='opencode',run_state='running',
            collector_updated_ns=health['observed_time_unix_nano'],status_evidence='OpenCode recorded user.')
        row.pop('diagnostics')
        self.assertEqual(native_metadata(health)['sessions'][0]['run_state'],'unknown')

    def test_once_never_emits_bootstrap_as_a_complete_snapshot(self):
        from contextlib import redirect_stderr
        import io
        from observatory.terminal import run_cockpit
        class FakeApi:
            endpoint='http://example.invalid'
            def __init__(self,*args,**kwargs):pass
            def get(self,path):return {'ok':True,'security':{'authentication_required':True},'indexing':{'ready':True}}
        class PartialFeed:
            def __init__(self,api):self.snapshot=Snapshot(native_bootstrap=True,bootstrap_updated=1)
            def refresh(self):pass
            def close(self):pass
        error=io.StringIO()
        with patch('observatory.terminal.Api',FakeApi),patch('observatory.terminal.Feed',PartialFeed),\
                patch('observatory.terminal.time.monotonic',side_effect=[0,16]),\
                patch('observatory.terminal.print_snapshot')as emitted,redirect_stderr(error):
            self.assertEqual(run_cockpit(db_path='/generated-fixture.db',endpoint='http://example.invalid',once=True),3)
        emitted.assert_not_called();self.assertIn('No complete snapshot emitted',error.getvalue())


class NativeBootstrapUITests(unittest.IsolatedAsyncioTestCase):
    async def test_every_unloaded_page_and_direct_search_callback_stays_unknown_until_coherent(self):
        from observatory.terminal_ui import ObservatoryApp
        from textual.widgets import Static,Button,TabbedContent
        api,feed=bootstrap();feed.thread=SimpleNamespace(is_alive=lambda:True,ident=None)
        app=ObservatoryApp(feed,persist_preferences=False)
        pages=(('activity','activity-table','activity-detail'),('mcp','mcp-table','mcp-detail'),
            ('traces','trace-table','span-info'),('host','host-table','host-detail'),
            ('metrics','metrics-table','metrics-detail'),('findings','finding-table','finding-detail'))
        async with app.run_test(size=(100,32))as pilot:
            await pilot.pause();app.refresh_timer.stop()
            for number,(page,table,detail)in enumerate(pages,2):
                app.query_one('#agent-table'if number==2 else '#'+pages[number-3][1]).focus()
                await pilot.press(str(number));await pilot.pause()
                self.assertEqual(app.query_one('#pages',TabbedContent).active,page)
                app.query_one('#search-'+page).value='generated filter';await pilot.pause(.4)
                getattr(app,'render_'+page)()
                show={'activity':app.show_activity,'mcp':app.show_mcp,'traces':app.show_spans,'findings':app.show_finding}.get(page)
                if show:show()
                self.assertIn('History indexing',app.query_one('#'+detail).plain_text)
                hint='mcp-hint'if page=='mcp'else 'search-status-'+page
                self.assertIn('evidence unavailable',str(app.query_one('#'+hint,Static).content))
                if page in {'activity','mcp','traces','findings'}:
                    app.render_paging(page)
                    self.assertEqual(str(app.query_one('#history-label-'+page,Static).content),'History indexing')
                    self.assertTrue(app.query_one('#older-'+page,Button).disabled)
                    self.assertNotIn('Restart',app.query_one('#older-'+page).tooltip)
            self.assertFalse(any(not path.startswith('/api/health')for path in api.calls))
            api.ready=True;feed.refresh();app.refresh_data();await pilot.pause();await app.workers.wait_for_complete();await pilot.pause()
            self.assertFalse(app.data.native_bootstrap)
            for page,_,__ in pages:
                getattr(app,'render_'+page)()
            self.assertNotIn('History indexing',str(app.query_one('#mcp-hint',Static).content))
            self.assertEqual(str(app.query_one('#history-label-findings',Static).content),'Latest')
            self.assertNotIn('History indexing',str(app.query_one('#results-mcp').border_title))

    async def test_floor_frame_keyboard_and_content_actions_are_explicitly_partial(self):
        from observatory.terminal_ui import ObservatoryApp,ObservatoryScreen
        from textual.widgets import DataTable,Static,Button
        api,feed=bootstrap();feed.thread=SimpleNamespace(is_alive=lambda:True,ident=None)
        app=ObservatoryApp(feed,persist_preferences=False);copies=[];app.copy_to_clipboard=copies.append
        async with app.run_test(size=(60,18))as pilot:
            await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all');await pilot.pause()
            self.assertGreater(app.query_one('#agent-table',DataTable).row_count,0)
            self.assertGreaterEqual(app.query_one('#agent-table',DataTable).scrollable_content_region.height,3)
            self.assertEqual(app.data.updated,0);self.assertTrue(app.data.native_bootstrap)
            self.assertIn('older history is being prepared',str(app.query_one('#health',Static).content))
            self.assertEqual(app.summary_values['mcp-count'][0],'—');self.assertEqual(app.summary_values['usage'][0],'—')
            self.assertEqual(app.summary_values['sessions'][0],'—');self.assertEqual(app.summary_values['attention'][0],'—')
            self.assertEqual(str(app.query_one('#agent-errors',Button).label),'Errors —')
            app.save_screenshot('native-bootstrap-roster-60x18.svg',path='/tmp')
            app.action_diagnostics();await pilot.pause()
            text='\n'.join(line.text for line in app.screen.query_one('#diagnostics-log').lines)
            self.assertIn('totals: unavailable',text);self.assertNotIn('Read errors: 0',text);self.assertNotIn(PRIVATE,text)
            await pilot.press('escape');await pilot.pause()
            app.query_one('#agent-table').focus();await pilot.press('down','enter');await pilot.pause()
            self.assertTrue(app.inspector_open());self.assertIn('history indexing',app.query_one('#agent-detail').plain_text.casefold())
            await pilot.press('ctrl+right');await pilot.pause();self.assertEqual(app.session_mode,'prompts')
            self.assertIn('Native observation',app.query_one('#agent-detail').plain_text)
            self.assertNotIn(PRIVATE,repr(app.inspector_payloads))
            for action in (app.action_copy,app.action_export,app.action_usage,app.action_review_work,app.action_review_attention):action()
            await pilot.pause();self.assertIsInstance(app.screen,ObservatoryScreen);self.assertFalse(copies)
            self.assertFalse(app.workflow_controller.ledger.incidents)
            app.save_screenshot('native-bootstrap-60x18.svg',path='/tmp')
            self.assertFalse(any(not path.startswith('/api/health')for path in api.calls))
            await pilot.press('escape');await pilot.resize_terminal(120,32);await pilot.pause()
            app.save_screenshot('native-bootstrap-wide.svg',path='/tmp')
            api.mutate_health=lambda count,row:{**row,'local_agents':{**row['local_agents'],'sessions':[]}}
            feed.refresh();app.refresh_data();await pilot.pause()
            self.assertEqual(app.query_one('#agent-table',DataTable).row_count,0)
            self.assertIn('No matching native sessions',app.query_one('#agent-detail').plain_text)
            self.assertEqual(app.summary_values['sessions'][0],'—')

    async def test_late_prompt_and_evidence_buffers_cannot_publish_during_bootstrap(self):
        from observatory.terminal_ui import ObservatoryApp
        from observatory.session_prompts import PromptHistory,prompt_scope,SessionEvidence
        api,feed=bootstrap();feed.thread=SimpleNamespace(is_alive=lambda:True,ident=None)
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(100,32))as pilot:
            await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all');app.select_agent('source-a')
            app.action_expand();await pilot.pause()
            item=deepcopy(api.sessions[0]);scope=prompt_scope(item)
            old=PromptHistory(scope=scope);app.session_prompts=old
            old_evidence=SessionEvidence(scope=scope);app.session_evidence=old_evidence
            response={'activity':deepcopy(api.activity)}
            app.finish_session_prompts(old,None,response,'');app.finish_session_evidence(old_evidence,response,'')
            await pilot.pause()
            self.assertFalse(app.session_prompts.events);self.assertFalse(app.session_evidence.events)
            self.assertNotIn(PRIVATE,app.query_one('#agent-detail').plain_text)
            self.assertNotIn(PRIVATE,repr(app.inspector_payloads))
            self.assertFalse(app.summary_capture_allowed(item,__import__('observatory.workflow_evidence',fromlist=['scope_for']).scope_for(item,feed.snapshot)))

    async def test_first_coherent_transition_restores_views_without_bootstrap_acknowledgements(self):
        from observatory.terminal_ui import ObservatoryApp
        from textual.widgets import Static,Button
        api,feed=bootstrap();feed.thread=SimpleNamespace(is_alive=lambda:True,ident=None)
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(100,32))as pilot:
            await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all');app.select_agent('source-a')
            old=deepcopy(app.workflow_controller.receipts)
            app.action_review_attention();self.assertEqual(app.workflow_controller.receipts,old)
            app.action_workflow();await pilot.pause()
            self.assertIn('History indexing','\n'.join(line.text for line in app.screen.query_one('#workflow-log').lines))
            self.assertTrue(app.screen.query_one('#workflow-seen',Button).disabled)
            await pilot.press('escape');await pilot.pause()
            api.ready=True;feed.refresh();app.refresh_data();await pilot.pause();await app.workers.wait_for_complete();await pilot.pause()
            self.assertGreater(app.data.updated,0);self.assertFalse(app.data.native_bootstrap)
            self.assertFalse(app.query_one('#agent-errors',Button).disabled)


class NativeBootstrapLockPreflightTests(unittest.TestCase):
    def test_real_local_poll_and_authenticated_health_wait_for_writer_before_admission(self):
        from contextlib import closing
        from concurrent.futures import ThreadPoolExecutor
        from datetime import datetime,timezone
        import json,os,sqlite3,tempfile,threading
        from pathlib import Path
        from observatory.core import Store
        from observatory.local_agents import LocalAgents,Session
        from observatory.server import BoundedHTTPServer,handler_for
        from observatory.terminal import Api
        with tempfile.TemporaryDirectory()as directory:
            root=Path(directory);store=Store(root/'receiver.db');monitor=None;server=None;serverthread=None
            started=threading.Event();original=store.ingest
            source=root/'generated-native.jsonl'
            source.write_text(json.dumps(dict(type='event_msg',timestamp=datetime.now(timezone.utc).isoformat(),
                payload=dict(type='task_started',turn_id='generated-turn')))+'\n')
            def ingest(*args,**kwargs):started.set();return original(*args,**kwargs)
            with patch('observatory.opencode.database_path',return_value=root/'absent-generated-opencode.db'):
                monitor=LocalAgents(store,capture_details=False)
            monitor.sessions[source]=Session(store,source,'Codex',capture_details=False)
            monitor.runtime_updated=time.monotonic()+3600
            monitor.discover=lambda:None;monitor.reader_plugins['opencode'].poll=lambda:0
            monitor.reader_plugins['opencode'].set_mcp_servers=lambda *args:None
            store.local_agent_monitor=monitor
            store.migration_status={'ready':False,'stage':'Preparing indexed history','processed':0}
            patches=(patch.object(store,'ingest',side_effect=ingest),
                patch('observatory.local_runtime.project_directories',return_value=set()),
                patch('observatory.local_agents.discover_mcp_configurations',return_value=[]))
            try:
                with patches[0],patches[1],patches[2],closing(sqlite3.connect(store.path))as writer:
                    writer.execute('BEGIN IMMEDIATE')
                    monitor.start();self.assertTrue(started.wait(2))
                    server=BoundedHTTPServer(('127.0.0.1',0),handler_for(store,'generated-preflight-token',quiet=True))
                    serverthread=threading.Thread(target=server.serve_forever);serverthread.start()
                    api=Api(f'http://127.0.0.1:{server.server_port}',token='generated-preflight-token')
                    with ThreadPoolExecutor(max_workers=1)as executor:
                        began=time.monotonic();health=executor.submit(api.get,'/api/health')
                        try:
                            time.sleep(.15)
                            self.assertFalse(monitor.initial_poll_complete)
                            self.assertFalse(health.done(),'Health hierarchy writes unexpectedly escaped the held writer')
                        finally:writer.rollback()
                        response=health.result(timeout=5)
                    waited=time.monotonic()-began
                    deadline=time.monotonic()+5
                    while not monitor.initial_poll_complete and time.monotonic()<deadline:time.sleep(.01)
                    self.assertTrue(monitor.initial_poll_complete)
                    feed=Feed(api);feed.include_global_totals=False;feed.detail_enabled=False;feed.refresh()
                    self.assertTrue(feed.snapshot.native_bootstrap,feed.snapshot.error)
                    self.assertEqual(feed.snapshot.updated,0);self.assertFalse(feed.snapshot.indexing_ready)
                    self.assertGreaterEqual(waited,.15)
                    report=dict(generated_only=True,diagnostic_only=True,publishable=False,
                        held_writer_seconds=.15,health_elapsed_seconds=waited,
                        initial_poll_complete_while_locked=False,health_complete_while_locked=False,
                        initial_poll_complete_after_release=monitor.initial_poll_complete,
                        native_frame_after_release=feed.snapshot.native_bootstrap,
                        coherent_updated=feed.snapshot.updated,indexing_ready=feed.snapshot.indexing_ready,
                        limitation='A held SQLite writer blocks native ingestion and health hierarchy persistence. This is not a full retained-history cold measurement.')
                    if destination:=os.environ.get('AGENTURM_BOOTSTRAP_PREFLIGHT_REPORT'):
                        Path(destination).write_text(json.dumps(report,indent=2)+'\n')
            finally:
                if monitor:monitor.close()
                if server:server.shutdown();server.server_close()
                if serverthread:serverthread.join(5)
                store.close()
