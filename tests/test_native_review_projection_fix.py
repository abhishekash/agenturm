"""Native metadata must not crash exact Review or duplicate the source pass."""
from copy import deepcopy
from dataclasses import replace
import json
import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from observatory.attention_cli import collect_snapshot
from observatory.core import Store
from observatory.native_lifecycle import event_log,merge_observations,normalize_event,validate_binding
from observatory.review_cli import collect_review
from observatory.review_facts import refreshed_review_item
from observatory.server import BoundedHTTPServer,handler_for
from observatory.source_projection import SourcePreparation,native_snapshot,prepare_sources,sources
from observatory.workflow_evidence import scope_for
from tests.test_native_lifecycle import binding,event
from tests.test_native_projection import activity,snapshot


class InventoryApi:
    endpoint='http://fixture.invalid'
    def __init__(self):
        self.data=snapshot();self.requests=[];self.advertised=True;self.failure=False
        self.after_inventory=lambda:None
        states,_=merge_observations(self.data.receiver_id,activity=[activity()])
        self.inventory=dict(receiver_id=self.data.receiver_id,observations=states,complete=True,warning='')
    def get(self,path):
        self.requests.append(path);data=self.data
        if path=='/api/health':return dict(receiver_id=data.receiver_id,review_trace_filter=True,
            native_lifecycle_v1=self.advertised,indexing={'ready':True},observed_time_unix_nano=data.receiver_time_ns,
            local_agents=deepcopy(data.local_status))
        if path=='/api/native-lifecycle':
            if self.failure:raise OSError('PRIVATE failure detail')
            value=deepcopy(self.inventory);self.after_inventory();return value
        if path=='/api/connections':return dict(receiver_id=data.receiver_id,connections=[])
        if path.startswith('/api/sessions'):return dict(receiver_id=data.receiver_id,sessions=deepcopy(data.sessions))
        if path.startswith('/api/activity'):return self.get_review(path)
        raise AssertionError(path)
    def get_review(self,path):
        self.requests.append(path)
        row=dict(receiver_id=self.data.receiver_id,service='codex',agent_id=binding()['agent_id'],session_id='session',
            trace_id='a'*32,domain='message',action='user',subject='Synthetic initiating request',
            source_id='message',time_unix_nano=time.time_ns())
        return dict(receiver_id=self.data.receiver_id,activity=[row])


def review(api):return collect_review(api,service='codex',agent_id=binding()['agent_id'],session_id='session',trace_id='a'*32)


class SourceProjectionFixTests(unittest.TestCase):
    def test_minimal_fact_snapshot_needs_no_activity_collection(self):
        data=snapshot();minimal=SimpleNamespace(**{key:getattr(data,key) for key in
            ('receiver_id','local_status','sessions','traces','updated','error','read_errors')})
        self.assertIs(native_snapshot(minimal),minimal)
        selected=sources(data)[0];selected['trace_id']='a'*32
        refreshed=refreshed_review_item(selected,minimal,scope_for(selected,minimal))
        self.assertEqual(refreshed['state'],'Working')
        self.assertFalse(hasattr(minimal,'activity'))

    def test_four_hundred_events_visit_once_with_native_overlay_and_metadata_only_retention(self):
        class Counted(list):
            visits=0
            def __iter__(self):
                for row in super().__iter__():self.visits+=1;yield row
        row=activity();row.update(subject='PRIVATE body',detail='PRIVATE detail')
        row['native_lifecycle']['unexpected']='PRIVATE envelope extra'
        rows=Counted([row]+[dict(domain='tool',session_id='session',subject='noise') for _ in range(399)])
        data=replace(snapshot(),activity=rows)
        result=sources(data)
        self.assertEqual(rows.visits,400)
        self.assertEqual(result[0]['state'],'Waiting')
        prepared=prepare_sources(data)
        self.assertNotIn('PRIVATE',str(prepared[2]))

    def test_native_cache_reuses_preparation_but_nested_identity_and_receiver_changes_invalidate(self):
        data=snapshot([activity()]);cache=SourcePreparation()
        with patch('observatory.source_projection.prepare_sources',wraps=prepare_sources) as prepare:
            self.assertEqual(sources(data,preparation=cache)[0]['state'],'Waiting')
            again=sources(data,preparation=cache)
            self.assertEqual(prepare.call_count,1)
            again[0]['request_id']='consumer change'
            self.assertEqual(sources(data,preparation=cache)[0]['request_id'],'request')
            data.activity[0]['native_lifecycle']['request_id']='changed'
            self.assertEqual(sources(data,preparation=cache)[0]['request_id'],'changed')
            data.receiver_id='replacement'
            self.assertNotEqual(sources(data,preparation=cache)[0]['state'],'Waiting')
            self.assertEqual(prepare.call_count,3)

    def test_first_thousand_source_positions_and_old_envelope_default_are_preserved(self):
        row=activity();row['native_lifecycle'].pop('source_order')
        self.assertEqual(sources(snapshot([row]))[0]['state'],'Waiting')
        data=snapshot([dict(domain='tool') for _ in range(1000)]+[row])
        self.assertNotEqual(sources(data)[0]['state'],'Waiting')

    def test_paused_policy_denial_clears_cached_text_without_changing_native_identity(self):
        data=snapshot([activity()]);cache=SourcePreparation();sources(data,preparation=cache)
        live=deepcopy(data);live.local_status['sessions'][0].update(capture_content=False,capture_revision=2)
        visible=sources(data,preparation=cache,policy_snapshot=live)[0]
        self.assertNotIn('PRIVATE_TASK',str(visible));self.assertEqual(cache.entries,[])
        self.assertEqual(visible['request_id'],'request')


class ReviewInventoryFixTests(unittest.TestCase):
    def test_optional_inventory_has_roster_review_and_headless_parity_without_extra_activity(self):
        api=InventoryApi();data=collect_snapshot(api)
        self.assertEqual(sources(data)[0]['state'],'Waiting')
        projection=review(api)
        self.assertEqual(projection['reported_outcome'],'Waiting')
        self.assertEqual(projection['request']['text'],'Synthetic initiating request')
        # Exact Review only uses its explicitly scoped evidence reader.
        exact=InventoryApi();review(exact)
        self.assertEqual(sum(path=='/api/native-lifecycle' for path in exact.requests),1)
        self.assertTrue(all('trace_id='+'a'*32 in path for path in exact.requests if path.startswith('/api/activity')))

    def test_legacy_receiver_never_fetches_unadvertised_inventory(self):
        api=InventoryApi();api.advertised=False
        projection=review(api)
        self.assertNotIn('/api/native-lifecycle',api.requests)
        self.assertNotEqual(projection['reported_outcome'],'Waiting')

    def test_whole_turn_metadata_matches_only_after_authoritative_membership_resolution(self):
        api=InventoryApi();turn='c'*32
        api.data.local_status['sessions'][0]['service']='opencode'
        api.data.sessions=[]
        api.inventory['observations'][0].update(service='opencode',turn_id=turn)
        original=api.get
        def get(path):
            if path.startswith('/api/review-turn?'):return dict(receiver_id=api.data.receiver_id,turn=dict(
                turn_id=turn,trace_id='a'*32,scope_owner=dict(producer='opencode',agent_id=binding()['agent_id'],session_id='session'),
                step_trace_ids=['a'*32],turn_usage_complete=True,agent_count=1,agent_count_complete=True))
            value=original(path)
            if path=='/api/health':value['review_turn_filter']=True
            return value
        api.get=get
        projection=collect_review(api,service='opencode',agent_id=binding()['agent_id'],session_id='session',trace_id='a'*32,whole_turn=True)
        self.assertEqual(projection['scope']['turn_id'],turn)
        self.assertEqual(projection['reported_outcome'],'Waiting')

    def test_optional_failed_partial_and_malformed_inventory_remain_explicitly_qualified(self):
        for change in ('failure','partial','oversized','malformed'):
            api=InventoryApi()
            if change=='failure':api.failure=True
            if change=='partial':api.inventory['complete']=False
            if change=='oversized':api.inventory['observations']*=1001
            if change=='malformed':api.inventory['observations']=[{'receiver_id':api.data.receiver_id}]
            with self.subTest(change=change):
                projection=review(api);reason=' '.join(projection['window']['reasons'])
                self.assertIn('Native lifecycle inventory',reason)
                self.assertEqual(projection['window']['coverage'],'partial')
                self.assertNotIn('PRIVATE failure detail',reason)
                self.assertNotEqual(projection['reported_outcome'],'Completed')

    def test_foreign_receiver_or_final_receiver_replacement_never_publishes_review(self):
        api=InventoryApi();api.inventory['receiver_id']='foreign'
        with self.assertRaisesRegex(ValueError,'identity changed'):review(api)
        api=InventoryApi();api.after_inventory=lambda:setattr(api.data,'receiver_id','replacement')
        with self.assertRaisesRegex(ValueError,'identity changed'):review(api)

    def test_exact_actor_session_trace_and_turn_are_never_borrowed(self):
        for key,value in [('agent_id','ag1_'+'3'*32),('session_id','other'),('trace_id','b'*32),('turn_id','c'*32)]:
            api=InventoryApi();api.inventory['observations'][0][key]=value
            with self.subTest(field=key):self.assertNotEqual(review(api)['reported_outcome'],'Waiting')

    def test_capture_revocation_after_inventory_or_activity_still_blocks_publication(self):
        for stage in ('inventory','activity'):
            api=InventoryApi()
            def revoke():api.data.local_status['sessions'][0].update(capture_content=False,capture_revision=2)
            if stage=='inventory':api.after_inventory=revoke
            else:
                original=api.get_review
                def read(path):value=original(path);revoke();return value
                api.get_review=read
            with self.subTest(stage=stage),self.assertRaisesRegex(ValueError,'Capture policy changed'):review(api)
        api=InventoryApi();api.data.local_status['capture_content']=False
        review(api);self.assertEqual(api.requests,['/api/health'])

    def test_final_capability_loss_is_disclosed_without_resolving_a_request(self):
        api=InventoryApi();api.after_inventory=lambda:setattr(api,'advertised',False)
        projection=review(api)
        self.assertIn('capability changed',' '.join(projection['window']['reasons']))
        self.assertEqual(projection['reported_outcome'],'Waiting')

    def test_authenticated_actual_store_cli_reads_inventory_without_backfilling_general_activity(self):
        with TemporaryDirectory() as directory:
            store=Store(Path(directory)/'source.db');self.addCleanup(store.close)
            configured=validate_binding({**binding(),'receiver_id':store.receiver_id})
            value=normalize_event({**event(),'receiver_id':store.receiver_id,'time_unix_nano':time.time_ns()},configured)
            value.update(request_kind='approval_requested',received_time_unix_nano=time.time_ns())
            store.ingest_logs([event_log(value)])
            data=snapshot();store.local_agent_monitor=SimpleNamespace(status=lambda:deepcopy(data.local_status))
            server=BoundedHTTPServer(('127.0.0.1',0),handler_for(store,'fixture-token',quiet=True))
            threading.Thread(target=server.serve_forever,daemon=True).start()
            self.addCleanup(server.server_close);self.addCleanup(server.shutdown)
            result=subprocess.run([sys.executable,'-m','observatory','review','--endpoint',f'http://127.0.0.1:{server.server_port}',
                '--service','codex','--agent-id',configured['agent_id'],'--session-id','session','--trace-id','a'*32,'--format','json'],
                env={**os.environ,'OBSERVATORY_TOKEN':'fixture-token','XDG_STATE_HOME':directory},capture_output=True,text=True,timeout=15)
            self.assertEqual(result.returncode,0,result.stderr)
            projection=json.loads(result.stdout)
            self.assertEqual(projection['reported_outcome'],'Waiting')
            self.assertEqual(projection['scope']['receiver'],store.receiver_id)
            # Native metadata is independent of the narrower Review content
            # page; the reader must not fabricate/backfill Activity for it.
            self.assertEqual(projection['window']['count'],0)


class ReviewUIParityTests(unittest.IsolatedAsyncioTestCase):
    async def test_log_only_run_anchor_keeps_roster_review_and_headless_request_parity(self):
        from observatory.terminal_ui import ObservatoryApp
        from observatory.ui_review import ReviewScreen
        from tests.test_terminal_search import ManualFeed
        api=InventoryApi();data=api.data
        data.sessions=[];data.traces=[]
        data.native_lifecycle=deepcopy(api.inventory['observations']);data.native_lifecycle_prepared=True
        data.review_trace_filter=True
        feed=ManualFeed();feed.snapshot=data;feed.request_refresh=lambda:None
        feed.api.get_review=api.get_review
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            app.set_agent_filter('all');app.select_agent('source');app.refresh_data()
            await app.workers.wait_for_complete();await pilot.pause()
            self.assertEqual(app.source_map['source']['state'],'Waiting')
            self.assertEqual(sources(data)[0]['state'],review(api)['reported_outcome'])
            app.action_review_work();await app.workers.wait_for_complete();await pilot.pause()
            self.assertIsInstance(app.screen,ReviewScreen)
            app.screen.action_refresh();await app.workers.wait_for_complete();await pilot.pause()
            self.assertEqual(app.screen.item['state'],'Waiting')
            self.assertEqual(app.screen.item['request_id'],'request')


if __name__=='__main__':unittest.main()
