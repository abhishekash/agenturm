"""One bounded native request projection for roster, Review and attention."""
from copy import deepcopy
from dataclasses import replace
from pathlib import Path
from tempfile import TemporaryDirectory
import threading
import time
import unittest
from unittest.mock import patch

from observatory.attention_runner import AttentionRunner,inventory_ready
from observatory.native_lifecycle import (event_log,merge_observations,native_metadata,normalize_event,validate_binding)
from observatory.attention_cli import collect_snapshot
from observatory.review_facts import refreshed_review_item
from observatory.source_projection import (sources,attention_items,SourcePreparation,NativeSnapshotProjection,native_snapshot)
from observatory.terminal import Snapshot
from observatory.workflow_evidence import scope_for
from observatory.workflow_store import WorkflowStore
from tests.test_native_lifecycle import binding,event
from tests.test_terminal_search import ManualFeed
from tests.test_workflow_controller import Owner
from observatory.terminal_ui import ObservatoryApp
from observatory.ui_review import ReviewScreen


def activity(sequence=1,kind='approval_requested',request='request',**changes):
    configured=validate_binding(binding())
    value=normalize_event(event(sequence,kind,request,**changes),configured)
    value.update(request_kind='approval_requested',received_time_unix_nano=time.time_ns())
    log=event_log(value)
    return dict(receiver_id=configured['receiver_id'],service=configured['service'],agent_id=configured['agent_id'],
        session_id=configured['session_id'],trace_id=configured['trace_id'],turn_id=configured['turn_id'],
        source_id='event-'+str(sequence),domain='agent',action='native_lifecycle',time_unix_nano=value['time_unix_nano'],
        native_lifecycle=native_metadata(log['attributes'],log['resource_attributes']))


def snapshot(rows=()):
    configured=binding();now=time.time();stamp=time.time_ns()
    native=dict(source_id='source',service='codex',runtime='Codex',agent_id=configured['agent_id'],session_id='session',
        latest_trace_id='a'*32,run_state='running',last_record_ns=stamp,initial_task='PRIVATE_TASK',
        diagnostics=dict(last_success_ns=stamp),capture_content=True,capture_revision=1)
    usage=dict(trace_id='a'*32,agent_count=1,agent_count_complete=True,task='PRIVATE_TASK')
    return Snapshot(receiver_id='receiver-one',receiver_time_ns=stamp,indexing_ready=True,updated=now,
        local_status={'sessions':[native]},sessions=[dict(service='codex',agent_id=native['agent_id'],
            session_id='session',latest_run=usage)],activity=list(rows))


class NativeProjectionTests(unittest.TestCase):
    def test_remote_activity_without_local_binding_is_identical_for_roster_and_attention(self):
        data=snapshot([activity()])
        roster=sources(data);headless=attention_items(data)
        self.assertEqual(roster,headless)
        self.assertEqual(roster[0]['state'],'Waiting');self.assertTrue(roster[0]['owns_attention'])
        self.assertEqual(roster[0]['request_id'],'request')
        runner=AttentionRunner();runner.update(headless,data,{})
        self.assertEqual(runner.ledger.rows[0][0]['state'],'Waiting')
        self.assertEqual(len(runner.native_observations),1)

    def test_headless_remote_pending_survives_activity_page_without_local_hook_binding(self):
        with TemporaryDirectory() as directory:
            store=WorkflowStore(Path(directory)/'workflow.db');runner=AttentionRunner()
            data=snapshot([activity()]);runner.update(attention_items(data),data,{},store=store);runner.persist(store)
            self.assertEqual(store.native_observations(data.receiver_id),[])
            next_data=replace(data,activity=[{'domain':'tool'}]*200,updated=data.updated+2)
            runner.update(attention_items(next_data),next_data,{},store=store);runner.persist(store)
            self.assertEqual(runner.ledger.rows[0][0]['state'],'Waiting')
            done=replace(next_data,activity=[activity(2,'request_resolved')],updated=data.updated+4)
            runner.update(attention_items(done),done,{},store=store);runner.persist(store)
            self.assertFalse(any(incident.kind=='approval' for incident in runner.ledger.incidents.values()))

    def test_review_refresh_uses_same_exact_pending_and_resolved_metadata(self):
        data=snapshot([activity()]);selected=sources(data)[0];scope=scope_for(selected,data)
        refreshed=refreshed_review_item(selected,data,scope)
        self.assertEqual(refreshed['state'],'Waiting');self.assertEqual(refreshed['request_id'],'request')
        resolved=native_snapshot(replace(data,activity=[activity(2,'request_resolved')]),getattr(native_snapshot(data),'native_lifecycle'))
        current=refreshed_review_item(selected,resolved,scope)
        self.assertNotEqual(current['state'],'Waiting');self.assertNotIn('request_id',current)

    def test_namespace_scope_and_sender_conflicts_never_authorize_a_request(self):
        own=activity();foreign=deepcopy(own);foreign['agent_id']='ag1_'+'3'*32
        data=snapshot([foreign]);self.assertNotEqual(sources(data)[0]['state'],'Waiting')
        self.assertNotEqual(sources(replace(data,receiver_id='foreign'))[0]['state'],'Waiting')
        states,note=merge_observations('receiver-one',activity=[own,foreign])
        self.assertTrue(note);self.assertEqual(len(states),1);self.assertTrue(states[0]['uncertain'])
        again,note=merge_observations('receiver-one',states,activity=[own])
        self.assertTrue(again[0]['uncertain']);self.assertTrue(note)
        malformed=deepcopy(own);malformed['turn_id']='not-a-recorded-turn'
        self.assertEqual(merge_observations('receiver-one',activity=[malformed])[0],[])

    def test_retention_outlives_latest_activity_and_exact_resolution_beats_replay(self):
        first=native_snapshot(snapshot([activity()]))
        later=native_snapshot(replace(first,activity=[{'domain':'tool'}]*200),first.native_lifecycle)
        self.assertEqual(sources(later)[0]['state'],'Waiting')
        done=native_snapshot(replace(later,native_lifecycle_prepared=False,activity=[activity(2,'request_resolved')]))
        replay=native_snapshot(replace(done,native_lifecycle_prepared=False,activity=[activity()]))
        self.assertTrue(replay.native_lifecycle[0]['resolved'])
        self.assertNotEqual(sources(replay)[0]['state'],'Waiting')

    def test_capacity_keeps_pending_states_and_marks_inventory_incomplete(self):
        values=[]
        for index in range(1001):
            row=activity(request='request-'+str(index),sequence=index+1)
            values.append(row)
        states,note=merge_observations('receiver-one',activity=values[:1000])
        self.assertEqual(len(states),1000);self.assertFalse(note)
        retained,note=merge_observations('receiver-one',states,activity=values[1000:])
        self.assertEqual(len(retained),1000);self.assertEqual(retained,states);self.assertIn('limit',note)
        self.assertFalse(inventory_ready(replace(snapshot(),native_lifecycle=retained,native_lifecycle_warning=note)))

    def test_overlay_snapshot_reuse_detaches_input_and_preserves_source_preparation(self):
        data=snapshot();observations,_=merge_observations(data.receiver_id,activity=[activity()])
        wrappers=NativeSnapshotProjection();first=wrappers.get(data,observations)
        self.assertIs(first,wrappers.get(data,deepcopy(observations)))
        preparation=SourcePreparation()
        with patch('observatory.source_projection.prepare_sources',wraps=__import__('observatory.source_projection',fromlist=['prepare_sources']).prepare_sources) as prepare:
            sources(first,preparation=preparation);sources(wrappers.get(data,observations),preparation=preparation)
            self.assertEqual(prepare.call_count,1)
        observations[0]['request_id']='mutated'
        self.assertEqual(first.native_lifecycle[0]['request_id'],'request')
        self.assertIsNot(first,wrappers.get(data,observations))
        revoked=replace(data,local_status={**data.local_status,'capture_content':False})
        visible=sources(first,preparation=preparation,policy_snapshot=revoked)[0]
        self.assertNotIn('PRIVATE_TASK',str(visible))

    def test_feed_retains_pending_across_pages_and_resets_on_receiver_replacement(self):
        feed=ManualFeed();original=feed.api.get;receiver=['receiver-one']
        def get(path):
            value=original(path)
            return {**value,'receiver_id':receiver[0]} if isinstance(value,dict) else value
        feed.api.get=get;feed.api.activity=[activity()];feed.refresh()
        self.assertEqual(len(feed.snapshot.native_lifecycle),1)
        feed.api.activity=[{'source_id':str(i),'service':'codex','session_id':'session-1','domain':'tool','action':'call','time_unix_nano':time.time_ns()} for i in range(200)]
        feed.refresh();self.assertEqual(len(feed.snapshot.native_lifecycle),1)
        receiver[0]='replacement';feed.refresh();self.assertEqual(feed.snapshot.native_lifecycle,[])

    def test_optional_receiver_inventory_restores_request_outside_activity_page(self):
        data=snapshot();states,_=merge_observations(data.receiver_id,activity=[activity()])
        class Api:
            endpoint='http://fixture.invalid'
            inventory=dict(receiver_id=data.receiver_id,observations=states,complete=True,warning='')
            def get(self,path):
                if path=='/api/health':return dict(receiver_id=data.receiver_id,native_lifecycle_v1=True,
                    indexing={'ready':True},observed_time_unix_nano=data.receiver_time_ns,local_agents=data.local_status)
                if path=='/api/native-lifecycle':return self.inventory
                if path=='/api/connections':return dict(receiver_id=data.receiver_id,connections=[])
                if path.startswith('/api/sessions'):return dict(receiver_id=data.receiver_id,sessions=data.sessions)
                if path.startswith('/api/activity'):return dict(receiver_id=data.receiver_id,activity=[])
                raise AssertionError(path)
        api=Api();restored=collect_snapshot(api)
        self.assertTrue(inventory_ready(restored));self.assertEqual(attention_items(restored)[0]['state'],'Waiting')
        # A restarted observer needs no local hook binding or recent Activity row.
        runner=AttentionRunner();runner.update(attention_items(restored),restored,{})
        self.assertEqual(runner.ledger.rows[0][0]['request_id'],'request')
        for changes in (dict(complete=False),dict(receiver_id='foreign'),dict(complete='true'),dict(observations=[{'receiver_id':data.receiver_id}])):
            api.inventory={**Api.inventory,**changes}
            partial=collect_snapshot(api)
            self.assertFalse(inventory_ready(partial));self.assertTrue(partial.native_lifecycle_warning)
        for changes in (dict(schema=True),dict(schema='future'),dict(resolved=True)):
            api.inventory={**Api.inventory,'observations':[{**states[0],**changes}]}
            self.assertFalse(inventory_ready(collect_snapshot(api)))

    def test_feed_optional_inventory_failure_keeps_pending_until_recovery(self):
        feed=ManualFeed();original=feed.api.get;states,_=merge_observations('receiver-one',activity=[activity()]);failed=[False]
        def get(path):
            if path=='/api/native-lifecycle':
                if failed[0]:raise OSError('private read error')
                return dict(receiver_id='receiver-one',observations=states,complete=True,warning='')
            value=original(path)
            if path.startswith('/api/health'):
                return {**value,'receiver_id':'receiver-one','native_lifecycle_v1':True}
            return {**value,'receiver_id':'receiver-one'} if isinstance(value,dict) else value
        feed.api.get=get;feed.api.activity=[];feed.refresh()
        self.assertEqual(len(feed.snapshot.native_lifecycle),1)
        failed[0]=True;feed.refresh()
        self.assertEqual(len(feed.snapshot.native_lifecycle),1);self.assertTrue(feed.snapshot.native_lifecycle_warning)
        self.assertNotIn('private read error',feed.snapshot.native_lifecycle_warning)
        failed[0]=False;feed.refresh();self.assertEqual(feed.snapshot.native_lifecycle_warning,'')

    def test_feed_final_health_is_independent_and_applies_current_capture_policy(self):
        feed=ManualFeed();original=feed.api.get;states,_=merge_observations('receiver-one',activity=[activity()])
        armed=[False];after_inventory=[False];mode=['healthy'];health_calls=[]
        def get(path):
            if path=='/api/native-lifecycle':
                after_inventory[0]=armed[0]
                return dict(receiver_id='receiver-one',observations=states,complete=True,warning='')
            value=original(path)
            if path.startswith('/api/health'):
                health_calls.append(path)
                value={**value,'receiver_id':'receiver-one','native_lifecycle_v1':True}
                if after_inventory[0] and mode[0]=='replace':value['receiver_id']='replacement'
                if after_inventory[0] and mode[0]=='revoke':value['local_agents']={**value['local_agents'],'capture_content':False}
                return value
            return {**value,'receiver_id':'receiver-one'} if isinstance(value,dict) else value
        feed.api.get=get;feed.refresh();previous=feed.snapshot
        self.assertGreaterEqual(health_calls.count('/api/health'),2)
        armed[0]=True;mode[0]='replace';feed.refresh()
        self.assertTrue(feed.snapshot.error);self.assertEqual(feed.snapshot.receiver_id,previous.receiver_id)
        self.assertEqual(feed.snapshot.updated,previous.updated)
        after_inventory[0]=False;mode[0]='revoke';feed.refresh()
        self.assertIs(feed.snapshot.local_status['capture_content'],False)

    def test_controller_reads_workflow_native_state_only_in_worker(self):
        from observatory.ui_workflow_controller import WorkflowController
        with TemporaryDirectory() as directory:
            owner=Owner();owner.feed.snapshot=snapshot([activity()])
            controller=WorkflowController(owner,store_path=Path(directory)/'workflow.db')
            original=WorkflowStore.native_observations
            def read(store,receiver):
                self.assertIsNot(threading.current_thread(),threading.main_thread())
                return original(store,receiver)
            with patch.object(WorkflowStore,'native_observations',read):
                controller.update(sources(owner.feed.snapshot),owner.feed.snapshot,{})
                owner.wait();controller.close()
            self.assertEqual(controller.native_receiver,'receiver-one')
            self.assertEqual(len(controller.native_observations),1)


class NativeProjectionJourney(unittest.IsolatedAsyncioTestCase):
    async def test_review_refresh_includes_worker_published_request_before_next_feed_poll(self):
        feed=ManualFeed();feed.snapshot=replace(snapshot(),review_trace_filter=True)
        feed.request_refresh=lambda:None
        original=feed.api.get
        def get(path):
            if path.startswith('/api/activity'):return dict(receiver_id='receiver-one',activity=[])
            return original(path)
        feed.api.get=get
        app=ObservatoryApp(feed,persist_preferences=False)
        observed,_=merge_observations('receiver-one',activity=[activity()])
        controller=app.workflow_controller
        controller.native_observations=observed;controller.native_receiver='receiver-one'
        controller.runner.native_observations=deepcopy(observed)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            app.set_agent_filter('all');app.select_agent('source');app.refresh_data()
            await app.workers.wait_for_complete();await pilot.pause()
            self.assertEqual(feed.snapshot.native_lifecycle,[])
            self.assertEqual(app.source_map['source']['state'],'Waiting')
            app.action_review_work();await app.workers.wait_for_complete();await pilot.pause()
            screen=app.screen;self.assertIsInstance(screen,ReviewScreen)
            screen.action_refresh();await app.workers.wait_for_complete();await pilot.pause()
            self.assertEqual(screen.item['state'],'Waiting');self.assertEqual(screen.item['request_id'],'request')

    async def test_pause_freezes_request_overlay_but_policy_revocation_remains_live(self):
        feed=ManualFeed();data=snapshot([activity()]);feed.snapshot=data
        feed.request_refresh=lambda:None
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            app.set_agent_filter('all');app.select_agent('source');app.refresh_data()
            await app.workers.wait_for_complete();await pilot.pause()
            self.assertEqual(app.source_map['source']['state'],'Waiting')
            app.action_pause()
            feed.snapshot=replace(data,updated=data.updated+2,activity=[activity(2,'request_resolved')])
            app.refresh_data();await app.workers.wait_for_complete();await pilot.pause()
            self.assertEqual(app.source_map['source']['state'],'Waiting')
            self.assertNotEqual(app.workflow_items()[0]['state'],'Waiting')
            feed.snapshot=replace(feed.snapshot,local_status={**data.local_status,'capture_content':False})
            app.refresh_data();await pilot.pause()
            self.assertNotIn('PRIVATE_TASK',str(app.source_map['source']))
            self.assertEqual(app.native_snapshot_projection.entries,[])
            app.action_pause();app.refresh_data();await pilot.pause()
            self.assertNotEqual(app.source_map['source']['state'],'Waiting')


if __name__=='__main__':unittest.main()
