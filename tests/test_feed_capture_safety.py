"""Negative capture policy publishes even when ordinary Feed data cannot."""
from copy import deepcopy
from dataclasses import replace
import threading
import time
import unittest
import urllib.error
from unittest.mock import patch
from types import SimpleNamespace

from observatory.agent_hierarchy import canonical_agent_id
from observatory.terminal import Feed
from observatory.review_facts import review_capture_policy
from observatory.workflow_evidence import scope_for
from tests.test_terminal_search import ExampleApi

PRIVATE='SYNTHETIC_PRIVATE_CAPTURE_A'
PUBLIC='Synthetic unrelated worker B'


class PolicyApi(ExampleApi):
    def __init__(self):
        super().__init__();self.receiver='known-synthetic-receiver';self.ready=True
        self.global_capture=True;self.deny_a=False;self.omit_a=False;self.failure=None;self.native=False
        self.health_hook=None;self.data_hook=None
        a=canonical_agent_id('Codex','policy-fixture','worker-a')
        b=canonical_agent_id('Codex','policy-fixture','worker-b')
        self.sessions=[dict(source_id='source-a',service='codex',session_id='shared-session',agent_id=a,
            runtime='Codex',capture_content=True,task=PRIVATE,trace_id='a'*32,run_state='running',last_record_ns=time.time_ns()),
            dict(source_id='source-b',service='codex',session_id='shared-session',agent_id=b,
            runtime='Codex',capture_content=True,task=PUBLIC,trace_id='b'*32,run_state='running',last_record_ns=time.time_ns())]
        self.activity=[dict(id='activity-'+str(n),source_id='event-'+str(n),service='codex',session_id=row['session_id'],
            agent_id=row['agent_id'],trace_id=row['trace_id'],domain='message',action='user',subject=row['task'],detail=row['task'],
            time_unix_nano=time.time_ns(),result='observed')for n,row in enumerate(self.sessions)]
        self.stored=[dict(session_id=row['session_id'],agent_id=row['agent_id'],services=['codex'],latest_task=row['task'],
            review_owner=dict(producer='codex',agent_id=row['agent_id'],session_id=row['session_id']))for row in self.sessions]

    def health(self):
        rows=deepcopy(self.sessions)
        rows[0]['capture_content']=not self.deny_a
        if self.omit_a:rows[0].pop('capture_content')
        local={'sessions':rows}
        if self.global_capture is not None:local['capture_content']=self.global_capture
        return dict(receiver_id=self.receiver,indexing=dict(ready=self.ready,stage='Synthetic incoming index'),
            native_lifecycle_v1=self.native,review_trace_filter=True,local_agents=local)

    def get(self,path):
        if path.startswith('/api/health'):
            value=self.health()
            return self.health_hook(path,value)if self.health_hook else value
        if self.data_hook:
            value=self.data_hook(path)
            if value is not None:return value
        if self.failure and path.startswith('/api/'+self.failure):
            raise urllib.error.HTTPError(path,503,'Synthetic indexing',{},None)
        if path.startswith('/api/native-lifecycle'):
            return {'receiver_id':self.receiver,'observations':[]}
        if path.startswith('/api/sessions'):return {'receiver_id':self.receiver,'sessions':deepcopy(self.stored)}
        return {**deepcopy(super().get(path)),'receiver_id':self.receiver}


def settled():
    api=PolicyApi();feed=Feed(api);feed.detail_enabled=False;feed.active_page='agents'
    feed.include_global_totals=False;feed.refresh()
    assert feed.snapshot.updated and not feed.snapshot.error
    return api,feed


class FeedCaptureSafetyTests(unittest.TestCase):
    def test_binding_uses_original_snapshot_and_never_upgrades_even_explicit_unknown(self):
        from observatory.review_facts import bind_capture_policy
        api,feed=settled();old=feed.snapshot;scope=scope_for(api.sessions[0],old)
        unbound=bind_capture_policy(api.sessions[0],old,scope)
        self.assertIn('capture_safety_revision',unbound)
        api.deny_a=True;api.ready=False;feed.refresh();api.deny_a=False;api.ready=True;feed.refresh()
        self.assertIsNone(bind_capture_policy(unbound,feed.snapshot,scope)['capture_safety_revision'])
        self.assertFalse(review_capture_policy(unbound,feed.snapshot,scope)['capture_content'])
        fresh=bind_capture_policy(api.sessions[0],feed.snapshot,scope)
        self.assertTrue(review_capture_policy(fresh,feed.snapshot,scope)['capture_content'])

    def assert_purged(self,feed):
        self.assertNotIn(PRIVATE,repr(feed.snapshot))
        for name in ('agents','traces','detail','findings','finding_results','activity','sessions','mcp','mcp_results',
                     'metric_catalog','metric_points','usage_series','metric_history','activity_results'):
            self.assertFalse(getattr(feed.snapshot,name),name)
        for name in ('sessions_cache','extra_cache','findings_cache'):self.assertIsNone(getattr(feed,name),name)

    def test_global_deny_precedes_index_unready_and_data503_without_positive_metadata(self):
        for failure in ('index','activity'):
            with self.subTest(failure=failure):
                api,feed=settled();old=feed.snapshot;old_stamp=old.local_status['sessions'][0]['last_record_ns']
                api.global_capture=False;api.deny_a=True
                api.sessions[0].update(task='NEW PRIVATE must not publish',run_state='completed',last_record_ns=old_stamp+100)
                api.ready=failure!='index';api.failure=None if failure=='index'else failure
                feed.refresh();self.assert_purged(feed)
                self.assertEqual(feed.snapshot.receiver_id,old.receiver_id)
                self.assertEqual(feed.snapshot.updated,old.updated);self.assertFalse(feed.snapshot.indexing_ready)
                self.assertEqual(feed.snapshot.local_status['sessions'][0]['last_record_ns'],old_stamp)
                self.assertEqual(feed.snapshot.local_status['sessions'][0]['run_state'],'running')
                self.assertFalse(feed.snapshot.local_status['capture_content'])
                self.assertTrue(feed.snapshot.error)

    def test_scoped_deny_is_exact_to_worker_and_keeps_unrelated_fresh_content_after_coherence(self):
        api,feed=settled();a,b=deepcopy(api.sessions)
        api.deny_a=True;api.ready=False;feed.refresh();self.assert_purged(feed)
        for item,allowed in ((a,False),(b,True)):
            policy=review_capture_policy(item,feed.snapshot,scope_for(item,feed.snapshot))
            self.assertEqual(policy['capture_content'],allowed)
        api.ready=True;feed.refresh()
        self.assertFalse(feed.snapshot.error)
        self.assertNotIn(PRIVATE,repr(feed.snapshot));self.assertIn(PUBLIC,repr(feed.snapshot))
        self.assertEqual([row['agent_id']for row in feed.snapshot.activity],[b['agent_id']])
        self.assertNotIn(PRIVATE,repr(feed.sessions_cache))

    def test_partial_allow_and_missing_allow_do_not_release_previous_deny(self):
        api,feed=settled();api.deny_a=True;api.ready=False;feed.refresh()
        a=deepcopy(api.sessions[0]);a['capture_content']=True
        api.deny_a=False;feed.refresh()
        self.assertFalse(review_capture_policy(a,feed.snapshot,scope_for(a,feed.snapshot))['capture_content'])
        api.ready=True;api.failure='activity';feed.refresh()
        self.assert_purged(feed)
        self.assertFalse(review_capture_policy(a,feed.snapshot,scope_for(a,feed.snapshot))['capture_content'])
        api.failure=None;api.omit_a=True;feed.refresh()
        self.assertNotIn(PRIVATE,repr(feed.snapshot))
        self.assertFalse(review_capture_policy(a,feed.snapshot,scope_for(a,feed.snapshot))['capture_content'])
        api.omit_a=False;feed.retry_now();feed.refresh()
        self.assertFalse(feed.snapshot.error);self.assertIn(PRIVATE,repr(feed.snapshot))
        self.assertFalse(review_capture_policy(a,feed.snapshot,scope_for(a,feed.snapshot))['capture_content'])
        from observatory.terminal_ui import sources
        fresh=next(row for row in sources(feed.snapshot)if row['id']=='source-a')
        self.assertTrue(review_capture_policy(fresh,feed.snapshot,scope_for(fresh,feed.snapshot))['capture_content'])

    def test_partial_global_allow_cannot_release_global_deny(self):
        api,feed=settled();api.global_capture=False;api.ready=False;feed.refresh()
        api.global_capture=True;feed.refresh();self.assertFalse(feed.snapshot.local_status['capture_content'])
        api.ready=True;api.global_capture=None;feed.refresh();self.assertFalse(feed.snapshot.local_status['capture_content'])
        self.assertNotIn(PRIVATE,repr(feed.snapshot))
        api.global_capture=True;feed.refresh();self.assertTrue(feed.snapshot.local_status['capture_content'])
        self.assertIn(PRIVATE,repr(feed.snapshot))

    def test_unknown_or_foreign_receiver_deny_never_relabels_old_namespace(self):
        for receiver in ('foreign-receiver','',None):
            with self.subTest(receiver=receiver):
                api,feed=settled();old=feed.snapshot
                api.receiver=receiver;api.ready=False;api.global_capture=False;api.deny_a=True;feed.refresh()
                self.assertEqual(feed.snapshot.receiver_id,old.receiver_id)
                self.assertTrue(feed.snapshot.local_status['capture_content'])
                self.assertEqual(feed._capture_denials,{})
                self.assertTrue(feed.snapshot.error)

    def test_navigation_generation_cancellation_cannot_drop_a_new_deny(self):
        api,feed=settled();api.deny_a=True;api.ready=False
        def health(path,value):feed.generation+=1;return value
        api.health_hook=health;feed.refresh();self.assert_purged(feed)
        self.assertFalse(feed.snapshot.local_status['sessions'][0]['capture_content'])

    def test_independent_final_health_deny_wins_over_already_fetched_content(self):
        api,feed=settled();api.native=True;count=[]
        def health(path,value):
            if path=='/api/health':count.append(path)
            if len(count)>=2:
                value['local_agents']['capture_content']=False;value['local_agents']['sessions'][0]['capture_content']=False
                value['indexing']['ready']=False
            return value
        api.health_hook=health;feed.refresh()
        self.assertGreaterEqual(len(count),2);self.assert_purged(feed)
        self.assertFalse(feed.snapshot.local_status['capture_content'])

    def test_older_parallel_allow_cannot_repopulate_snapshot_or_content_caches(self):
        api,feed=settled();started=threading.Event();release=threading.Event();self.addCleanup(release.set)
        worker=threading.Thread(name='older-allow',target=feed.refresh)
        def data(path):
            if path.startswith('/api/sessions')and threading.current_thread().name.startswith('observatory-feed'):
                started.set();release.wait(5)
            return None
        # Worker pool threads have a standard prefix; block a specifically
        # prefetched request while its refresh still holds an earlier epoch.
        def slow(path):
            if path.startswith('/api/sessions'):
                started.set();release.wait(5)
                return {'receiver_id':api.receiver,'sessions':deepcopy(api.stored)}
            return None
        api.data_hook=slow;feed.sessions_attempted=None;worker.start()
        self.assertTrue(started.wait(2))
        denied=api.health();denied['local_agents']['capture_content']=False
        denied['local_agents']['sessions'][0]['capture_content']=False
        self.assertTrue(feed._capture_from_health(denied));epoch=feed._capture_epoch
        release.set();worker.join(5);self.assertFalse(worker.is_alive())
        self.assertEqual(feed._capture_epoch,epoch);self.assert_purged(feed)
        self.assertFalse(feed.snapshot.local_status['capture_content'])

    def test_malformed_actor_cannot_turn_into_session_wide_deny(self):
        api,feed=settled();api.ready=False
        def health(path,value):
            value['local_agents']['sessions'][0].update(capture_content=False,agent_id=['malformed'])
            return value
        api.health_hook=health;feed.refresh()
        self.assertFalse(feed._capture_denials)
        self.assertTrue(feed.snapshot.local_status['sessions'][1]['capture_content'])

    def test_actor_wide_missing_session_deny_covers_unlisted_sessions_without_hiding_sibling(self):
        api,feed=settled();api.sessions[0].pop('session_id')
        api.activity[0]['session_id']='new-unlisted-session';api.deny_a=True;api.ready=False;feed.refresh()
        api.ready=True;feed.refresh()
        self.assertNotIn(PRIVATE,repr(feed.snapshot));self.assertIn(PUBLIC,repr(feed.snapshot))
        self.assertTrue(any(not key[2]for key in feed._capture_denials))

    def test_source_preparation_does_not_reuse_old_or_paused_generation_after_full_regrant(self):
        from observatory.terminal_ui import sources,SourcePreparation
        api,feed=settled();old=feed.snapshot;cache=SourcePreparation()
        self.assertIn(PRIVATE,repr(sources(old,preparation=cache)))
        api.deny_a=True;api.ready=False;feed.refresh();api.deny_a=False;api.ready=True;feed.refresh()
        paused=sources(old,preparation=cache,policy_snapshot=feed.snapshot)
        self.assertNotIn(PRIVATE,repr(paused));self.assertIn(PUBLIC,repr(paused));self.assertEqual(cache.entries,[])
        fresh=sources(feed.snapshot,preparation=cache)
        self.assertIn(PRIVATE,repr(fresh))
        item=next(row for row in fresh if row['id']=='source-a')
        self.assertTrue(item['capture_content']);self.assertIsNotNone(item['capture_safety_revision'])
        self.assertTrue(cache.entries)

    def test_restarted_feed_unknown_or_new_first_epoch_cannot_revive_old_bound_projection(self):
        from observatory.terminal_ui import sources
        from observatory.workflow_evidence import ReviewHistory
        from observatory.review_continuity import content_allowed
        from observatory.ui_saved_tasks import task_source_policy
        with patch('observatory.terminal.secrets.randbits',side_effect=[100,200]):
            api,feed=settled();api.deny_a=True;api.ready=False;feed.refresh();api.deny_a=False;api.ready=True;feed.refresh()
            first=next(row for row in sources(feed.snapshot)if row['id']=='source-a')
            history=ReviewHistory(scope_for(first,feed.snapshot),capture_revision=first['capture_revision'])
            history.accept({'receiver_id':api.receiver,'activity':deepcopy(api.activity)})
            old=history.projection(first)
            restarted=Feed(api);restarted.detail_enabled=False;restarted.active_page='agents';restarted.include_global_totals=False
            restarted.refresh();owner=SimpleNamespace(feed=restarted)
            self.assertFalse(content_allowed(old,task_source_policy(owner)))
            api.deny_a=True;api.ready=False;restarted.refresh();api.deny_a=False;api.ready=True;restarted.refresh()
            fresh=next(row for row in sources(restarted.snapshot)if row['id']=='source-a')
            self.assertNotEqual(first['capture_safety_revision'],fresh['capture_safety_revision'])
            self.assertFalse(content_allowed(old,task_source_policy(owner)))
            current=ReviewHistory(scope_for(fresh,restarted.snapshot),capture_revision=fresh['capture_revision'])
            current.accept({'receiver_id':api.receiver,'activity':deepcopy(api.activity)})
            self.assertTrue(content_allowed(current.projection(fresh),task_source_policy(owner)))

    def test_known_session_deny_survives_missing_session_in_later_native_health(self):
        api,feed=settled();original=deepcopy(api.sessions[0]);scope=scope_for(original,feed.snapshot)
        api.sessions[0].pop('session_id');api.deny_a=True;api.ready=False;feed.refresh()
        self.assertFalse(review_capture_policy(original,feed.snapshot,scope)['capture_content'])
        api.ready=True;feed.refresh()
        self.assertFalse(review_capture_policy(original,feed.snapshot,scope)['capture_content'])
        self.assertNotIn(PRIVATE,repr(feed.snapshot))
        api.sessions[0]['session_id']=original['session_id'];api.deny_a=False;feed.refresh()
        self.assertFalse(review_capture_policy(original,feed.snapshot,scope)['capture_content'])
        # Only a matching broad allow releases an originally actor-wide deny.
        api.sessions[0].pop('session_id');feed.refresh()
        self.assertFalse(feed._capture_denials)
        self.assertFalse(review_capture_policy(original,feed.snapshot,scope)['capture_content'])

    def test_runtime_qualified_codex_review_alias_overrides_old_allow_without_duplicate_source(self):
        api,feed=settled();a,b=deepcopy(api.sessions)
        api.sessions[0]['service']='codex-review';api.deny_a=True;api.ready=False;feed.refresh()
        self.assertFalse(review_capture_policy(a,feed.snapshot,scope_for(a,feed.snapshot))['capture_content'])
        self.assertTrue(review_capture_policy(b,feed.snapshot,scope_for(b,feed.snapshot))['capture_content'])
        sources=[row['source_id']for row in feed.snapshot.local_status['sessions']]
        self.assertEqual(len(sources),len(set(sources)))

    def test_late_pre_deny_error_cannot_overwrite_newer_coherent_allow_snapshot(self):
        api,feed=settled();started=threading.Event();release=threading.Event();blocked=[];lock=threading.Lock()
        self.addCleanup(release.set)
        def slow(path):
            if not path.startswith('/api/sessions'):return None
            with lock:
                first=not blocked
                if first:blocked.append(True)
            if first:started.set();release.wait(5)
            return {'receiver_id':api.receiver,'sessions':deepcopy(api.stored)}
        api.data_hook=slow;feed.sessions_attempted=None
        worker=threading.Thread(target=feed.refresh);worker.start();self.assertTrue(started.wait(2))
        denied=api.health();denied['local_agents']['capture_content']=False
        self.assertTrue(feed._capture_from_health(denied))
        feed.refresh();new=feed.snapshot;self.assertFalse(new.error);self.assertIn(PRIVATE,repr(new))
        release.set();worker.join(5);self.assertFalse(worker.is_alive())
        self.assertIs(feed.snapshot,new);self.assertFalse(feed.snapshot.error)

    def test_bootstrap_metadata_only_snapshot_fences_older_publications(self):
        api=PolicyApi();api.global_capture=False;api.deny_a=True
        feed=Feed(api);feed.active_page='agents';feed.detail_enabled=False;feed.include_global_totals=False
        feed.refresh()
        self.assertFalse(feed.snapshot.error);self.assertFalse(feed.snapshot.local_status['capture_content'])
        self.assertNotIn(PRIVATE,repr(feed.snapshot));self.assertGreater(feed._capture_epoch,0)

    def test_receiver_replacement_can_restore_only_its_own_coherent_content(self):
        api,feed=settled();api.global_capture=False;api.ready=False;feed.refresh()
        api.receiver='new-known-receiver';api.global_capture=True;api.ready=True;feed.retry_now();feed.refresh()
        self.assertEqual(feed.snapshot.receiver_id,'new-known-receiver');self.assertFalse(feed.snapshot.error)
        self.assertTrue(feed.snapshot.local_status['capture_content']);self.assertIn(PRIVATE,repr(feed.snapshot))
        self.assertFalse(feed._capture_denials)

    def test_deny_scope_memory_is_bounded_and_overflow_cannot_implicitly_regrant(self):
        api,feed=settled();health=api.health();health['indexing']['ready']=False
        health['local_agents']['sessions']=[dict(source_id='scope-'+str(n),service='codex',agent_id='actor-'+str(n),
            session_id='session-'+str(n),capture_content=False)for n in range(501)]
        self.assertTrue(feed._capture_from_health(health));self.assertLessEqual(len(feed._capture_denials),500)
        self.assertFalse(feed.snapshot.local_status['capture_content'])
        feed.refresh()
        self.assertFalse(feed.snapshot.local_status['capture_content'])
        self.assertNotIn(PRIVATE,repr(feed.snapshot))


class FeedCaptureSafetyUITests(unittest.IsolatedAsyncioTestCase):
    async def test_excerpt_copy_checks_frozen_permission_and_keeps_authored_text(self):
        from observatory.terminal_ui import ObservatoryApp,sources
        from observatory.ui_saved_tasks import TaskDecisionEditor
        from observatory.review_facts import bind_capture_policy
        from textual.widgets import Button,TextArea
        from observatory.saved_tasks import TaskStore
        from pathlib import Path
        from tempfile import TemporaryDirectory
        api,feed=settled();feed.thread=SimpleNamespace(is_alive=lambda:True,ident=None)
        with TemporaryDirectory()as directory:
            path=Path(directory)/'tasks.db';task=TaskStore(path).create('Authored task')
            app=ObservatoryApp(feed,persist_preferences=False,task_store_path=path);copies=[];app.copy_to_clipboard=copies.append
            async with app.run_test(size=(100,32))as pilot:
                await pilot.pause();app.refresh_timer.stop()
                item=next(row for row in sources(feed.snapshot)if row['id']=='source-a')
                source=dict(receiver=feed.snapshot.receiver_id,producer=item['service'],agent_id=item['agent_id'],session_id=item['session_id'])
                source['_capture_binding']=bind_capture_policy({},feed.snapshot,scope_for(item,feed.snapshot))
                editor=TaskDecisionEditor(app,task,source=source,excerpt=PRIVATE);app.push_screen(editor);await pilot.pause()
                editor.copy_draft(Button.Pressed(editor.query_one('#brief-copy')));self.assertIn(PRIVATE,copies[-1]);copies.clear()
                api.deny_a=True;api.ready=False;feed.refresh();api.deny_a=False;api.ready=True;feed.refresh()
                editor.copy_draft(Button.Pressed(editor.query_one('#brief-copy')));self.assertFalse(copies)
                self.assertNotIn(PRIVATE,editor.query_one(TextArea).text)
                editor.query_one(TextArea).load_text('Human-owned decision')
                editor.check_source_excerpt();self.assertEqual(editor.query_one(TextArea).text,'Human-owned decision')
                editor.copy_draft(Button.Pressed(editor.query_one('#brief-copy')))
                self.assertIn('Human-owned decision',copies[-1]);self.assertNotIn(PRIVATE,copies[-1])

    async def test_fresh_excerpt_decision_and_parallel_cache_preserve_their_own_binding(self):
        from observatory.terminal_ui import ObservatoryApp,sources
        from observatory.ui_review import ReviewScreen,ReviewExcerptScreen
        from observatory.ui_saved_tasks import SavedTasksScreen,TaskDecisionEditor
        from observatory.saved_tasks import TaskStore
        from textual.widgets import DataTable
        from pathlib import Path
        from tempfile import TemporaryDirectory
        api,feed=settled();feed.thread=SimpleNamespace(is_alive=lambda:True,ident=None)
        api.activity.append({**api.activity[0],'id':'synthetic-check','source_id':'synthetic-check',
            'domain':'shell','action':'run','subject':'pytest','detail':'',
            'result':'success','attributes':{'process.command_line':'pytest','process.exit.code':0}})
        api.deny_a=True;api.ready=False;feed.refresh();api.deny_a=False;api.ready=True;feed.refresh()
        with TemporaryDirectory()as directory:
            path=Path(directory)/'tasks.db';task=TaskStore(path).create('Authored task')
            app=ObservatoryApp(feed,persist_preferences=False,task_store_path=path)
            async with app.run_test(size=(100,32))as pilot:
                await pilot.pause();app.refresh_timer.stop()
                fresh=next(row for row in sources(feed.snapshot)if row['id']=='source-a')
                review=ReviewScreen(app,fresh);app.push_screen(review)
                await pilot.pause();await app.workers.wait_for_complete();await pilot.pause()
                review.attach_parallel_observations();keys=list(app.review_git_comparisons)
                self.assertTrue(keys);review.attach_parallel_observations()
                self.assertEqual(keys,list(app.review_git_comparisons))
                app.push_screen(ReviewExcerptScreen(app,review.projection,review.item));await pilot.pause()
                self.assertTrue(app.screen.query_one(DataTable).row_count)
                await pilot.press('enter');await app.workers.wait_for_complete();await pilot.pause()
                self.assertIsInstance(app.screen,SavedTasksScreen)
                await pilot.press('enter');await app.workers.wait_for_complete();await pilot.pause()
                self.assertIsInstance(app.screen,TaskDecisionEditor)
                self.assertIn('_capture_binding',app.screen.source)
                await pilot.press('ctrl+s');await app.workers.wait_for_complete();await pilot.pause()
                saved=TaskStore(path).get(task['id']);self.assertEqual(len(saved['decisions']),1)
                self.assertNotIn('_capture_binding',saved['decisions'][0]['source'])

    async def test_real_feed_denial_reaches_paused_inspector_and_rejects_late_prompt_buffers(self):
        from observatory.terminal_ui import ObservatoryApp
        api,feed=settled();feed.thread=SimpleNamespace(is_alive=lambda:True,ident=None)
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,36))as pilot:
            await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all')
            app.select_agent('source-a');app.set_session_mode('prompts');await pilot.pause()
            self.assertIn(PRIVATE,app.query_one('#agent-detail').plain_text)
            old_history=app.session_prompts;old_evidence=app.session_evidence;event=deepcopy(api.activity[0])
            app.action_pause();await pilot.pause()
            api.global_capture=False;api.deny_a=True;api.ready=False;feed.refresh()
            app.refresh_data();await pilot.pause()
            self.assertTrue(feed.paused);self.assertFalse(app.source_map['source-a']['capture_content'])
            self.assertEqual(app.session_prompts.events,{})
            app.finish_session_prompts(old_history,None,{'activity':[event]},'')
            app.finish_session_evidence(old_evidence,{'activity':[event]},'')
            for mode in ('overview','prompts','details'):
                app.set_session_mode(mode);await pilot.pause()
                self.assertNotIn(PRIVATE,app.query_one('#agent-detail').plain_text)
                self.assertNotIn(PRIVATE,app.query_one('#agent-heading').tooltip or '')

    async def open_review(self,app,pilot):
        from observatory.ui_review import ReviewScreen
        app.set_agent_filter('all');app.select_agent('source-a')
        item=deepcopy(app.source_map['source-a']);screen=ReviewScreen(app,item)
        app.push_screen(screen);await pilot.pause();await app.workers.wait_for_complete();await pilot.pause()
        self.assertIn(PRIVATE,screen.frozen_json())
        return screen

    async def test_rapid_reallow_with_no_ui_tick_blocks_old_json_draft_latepage_and_allows_deliberate_refresh(self):
        from observatory.terminal_ui import ObservatoryApp
        from observatory.ui_saved_tasks import projection_policy_state
        api,feed=settled();feed.thread=SimpleNamespace(is_alive=lambda:True,ident=None)
        app=ObservatoryApp(feed,persist_preferences=False);copies=[];app.copy_to_clipboard=copies.append
        async with app.run_test(size=(100,32))as pilot:
            await pilot.pause();app.refresh_timer.stop();screen=await self.open_review(app,pilot)
            old_projection=deepcopy(screen.projection);old_generation=screen.generation;old_candidate=deepcopy(screen.item)
            # Deliberately no event-loop turn between revocation, explicit
            # coherent regrant and the actual JSON action.
            api.deny_a=True;api.ready=False;feed.refresh()
            api.deny_a=False;api.ready=True;feed.refresh()
            screen.action_copy_json()
            self.assertNotIn(PRIVATE,copies[-1]);self.assertNotIn(PRIVATE,screen.frozen_json())
            self.assertFalse(projection_policy_state(app,old_projection)[0])
            screen.finish_page(old_generation,None,True,{'receiver_id':api.receiver,'activity':deepcopy(api.activity)},'',old_candidate,screen.scope)
            self.assertNotIn(PRIVATE,screen.frozen_json())
            screen.action_draft_task();self.assertIs(app.screen,screen)
            screen.action_refresh();await app.workers.wait_for_complete();await pilot.pause()
            self.assertIn(PRIVATE,screen.frozen_json())
            self.assertTrue(projection_policy_state(app,screen.projection)[0])
            screen.action_copy_json();self.assertIn(PRIVATE,copies[-1])

    async def test_old_export_and_usage_are_invalid_after_missed_deny_while_fresh_review_handoff_works(self):
        from observatory.terminal_ui import ObservatoryApp,sources
        from observatory.ui_export import ReceiptExportScreen
        from observatory.ui_monitoring import UsageScreen
        from observatory.ui_saved_tasks import task_source_policy,capture_selected_run,projection_policy_state,current_capture_context
        from observatory.work_receipts import freeze_receipt
        from observatory.review_continuity import editable_draft,content_allowed
        from observatory.saved_tasks import TaskStore,render_handoff
        from tempfile import TemporaryDirectory
        from pathlib import Path
        api,feed=settled();feed.thread=SimpleNamespace(is_alive=lambda:True,ident=None)
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(100,32))as pilot:
            await pilot.pause();app.refresh_timer.stop();review=await self.open_review(app,pilot)
            old=deepcopy(review.projection);old_item=deepcopy(review.item)
            original_state=projection_policy_state(app,old)
            export=ReceiptExportScreen(freeze_receipt(old),valid=lambda:projection_policy_state(app,old)==original_state)
            app.push_screen(export);await pilot.pause()
            api.deny_a=True;api.ready=False;feed.refresh();api.deny_a=False;api.ready=True;feed.refresh()
            with TemporaryDirectory()as directory:
                destination=Path(directory)/'must-not-exist.md'
                export.query_one('#export-path').value=str(destination);export.confirm()
                await pilot.pause();self.assertFalse(destination.exists());self.assertTrue(export._export_invalidated)
                self.assertNotIn(PRIVATE,export.content)
                usage=UsageScreen(details=PRIVATE,valid=lambda:review_capture_policy(old_item,feed.snapshot,scope_for(old_item,feed.snapshot)).get('capture_content')is not False)
                app.push_screen(usage);await pilot.pause();usage.check_policy()
                self.assertNotIn(PRIVATE,usage.details);app.pop_screen();await pilot.pause()
                if app.screen is review:app.pop_screen();await pilot.pause()
                app.refresh_data();await pilot.pause()
                fresh_item=next(row for row in sources(feed.snapshot)if row['id']=='source-a')
                from observatory.ui_review import ReviewScreen
                fresh=ReviewScreen(app,fresh_item);app.push_screen(fresh)
                await pilot.pause();await app.workers.wait_for_complete();await pilot.pause()
                self.assertIn(PRIVATE,fresh.frozen_json());self.assertTrue(projection_policy_state(app,fresh.projection)[0])
                captured=capture_selected_run(app,item_override=fresh.item,review_snapshot=fresh.projection)
                self.assertTrue(captured['capture_content']);self.assertEqual(captured['review_snapshot'],fresh.projection)
                self.assertTrue(current_capture_context(app,{**fresh.projection['scope'],'review_snapshot':fresh.projection}).get('capture_content')is not False)
                self.assertFalse(current_capture_context(app,{**old['scope'],'review_snapshot':old})['capture_content'])
                policy=task_source_policy(app)
                self.assertFalse(content_allowed(old,policy));self.assertTrue(content_allowed(fresh.projection,policy))
                store=TaskStore(Path(directory)/'tasks.db')
                old_draft=editable_draft(old);old_draft['text']='OLD_GENERATION_DRAFT'
                task=store.create('Authored task',notes='Keep authored notes',continuity_draft=old_draft,source_allowed=True)
                task=store.approve_continuity(task['id'],editable_draft(fresh.projection),text='NEW_GENERATION_DRAFT',source_allowed=True)
                handoff=render_handoff(task,source_policy=policy)
                self.assertNotIn('OLD_GENERATION_DRAFT',handoff);self.assertIn('NEW_GENERATION_DRAFT',handoff)
                self.assertIn('Keep authored notes',handoff)

    async def test_receiver_replacement_clears_actual_review_json_body(self):
        from observatory.terminal_ui import ObservatoryApp
        api,feed=settled();feed.thread=SimpleNamespace(is_alive=lambda:True,ident=None)
        app=ObservatoryApp(feed,persist_preferences=False);copies=[];app.copy_to_clipboard=copies.append
        async with app.run_test(size=(100,32))as pilot:
            await pilot.pause();app.refresh_timer.stop();screen=await self.open_review(app,pilot)
            feed.snapshot=replace(feed.snapshot,receiver_id='replacement-receiver')
            screen.action_copy_json()
            self.assertNotIn(PRIVATE,copies[-1]);self.assertFalse(screen.history.events)

    async def test_paused_buffers_cannot_miss_a_synchronous_deny_reallow_transition(self):
        from observatory.terminal_ui import ObservatoryApp
        api,feed=settled();feed.thread=SimpleNamespace(is_alive=lambda:True,ident=None)
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,36))as pilot:
            await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all');app.select_agent('source-a')
            app.set_session_mode('prompts');await pilot.pause();old=app.session_prompts
            self.assertIn(PRIVATE,app.query_one('#agent-detail').plain_text)
            app.action_pause();await pilot.pause()
            api.deny_a=True;api.ready=False;feed.refresh();api.deny_a=False;api.ready=True;feed.refresh()
            app.refresh_data();await pilot.pause()
            app.finish_session_prompts(old,None,{'activity':deepcopy(api.activity)},'')
            self.assertNotIn(PRIVATE,app.query_one('#agent-detail').plain_text)
            self.assertTrue(feed.paused);self.assertFalse(app.source_map['source-a']['capture_content'])
