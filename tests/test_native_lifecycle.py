"""Versioned exact native metadata, replay/order fences and source precedence."""
from copy import deepcopy
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from observatory.native_lifecycle import (bounded_json,enqueue,event_log,flush,normalize_event,overlay_observations,
    read_binding,record_uncertain,runtime_event,UncertainLifecycle,validate_binding,verify_binding)
from observatory.attention_runner import AttentionRunner
from observatory.workflow_store import WorkflowStore
from tests.test_attention import item,snapshot


def binding():
    return dict(schema='agenturm-native-binding-v1',sender_id='1'*32,endpoint='http://127.0.0.1:4318',adapter='generic-v1',
        native_turn_id='native-turn',follow_recorded_turns=False,source_clock='receiver',receiver_id='receiver-one',service='codex',
        agent_id='ag1_'+'2'*32,session_id='session',trace_id='a'*32,turn_id='',source_id='source',parent_agent_id='')


def event(sequence=1,kind='approval_requested',request='request',**changes):
    value={key:value for key,value in binding().items() if key in ('sender_id','receiver_id','service','agent_id','session_id','trace_id','turn_id','source_id','parent_agent_id')}
    value.update(schema='agenturm-native-event-v1',event_id='event-'+str(sequence),sequence=sequence,kind=kind,
        request_id=request,time_unix_nano=150*10**9);value.update(changes)
    return value


class NativeLifecycleTests(unittest.TestCase):
    def setUp(self):
        self.tmp=TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.store=WorkflowStore(Path(self.tmp.name)/'workflow.db');self.binding=validate_binding(binding())

    def accept(self,value):return enqueue(self.store,self.binding,normalize_event(value,self.binding))
    def publish(self):return flush(self.store,self.binding,lambda _:True)

    def test_bound_identity_rejects_foreign_actor_turn_parent_and_payload_fields(self):
        for field in ('sender_id','receiver_id','service','agent_id','session_id','trace_id','turn_id','source_id','parent_agent_id'):
            with self.subTest(field=field),self.assertRaisesRegex(UncertainLifecycle,'contradicts'):
                normalize_event(event(**{field:'foreign'}),self.binding)
        for changes in ({'sequence':True},{'sequence':0},{'time_unix_nano':True},{'request_id':''},
                {'prompt':'PRIVATE_CONTENT'},{'schema':'unknown'}):
            with self.assertRaises(UncertainLifecycle):normalize_event(event(**changes),self.binding)
        self.assertEqual(self.store.native_observations('receiver-one'),[])

    def test_stable_replay_is_idempotent_but_conflict_and_reordering_are_uncertain(self):
        self.assertTrue(self.accept(event()));self.assertFalse(self.accept(event()))
        self.publish()
        with self.assertRaisesRegex(UncertainLifecycle,'conflicts'):self.accept(event(kind='reply_requested'))
        with self.assertRaisesRegex(UncertainLifecycle,'reordered'):self.accept(event(sequence=1,event_id='other'))
        with self.assertRaisesRegex(UncertainLifecycle,'reused'):self.accept(event(sequence=2))
        self.assertEqual(len(self.store.native_observations('receiver-one')),1)
        record_uncertain(self.store,self.binding,'Contradictory request identity')
        self.assertEqual(self.store.native_status()[0]['rejected'],1)

    def test_request_resolution_is_exact_and_pending_outbox_survives_failure(self):
        self.accept(event())
        with self.assertRaises(OSError):flush(self.store,self.binding,lambda _:False)
        self.assertEqual(self.store.native_observations('receiver-one'),[])
        with self.assertRaisesRegex(UncertainLifecycle,'matching'):self.accept(event(2,'request_resolved','other'))
        self.accept(event(2,'request_resolved'))
        self.assertEqual(self.publish(),2)
        observations=self.store.native_observations('receiver-one')
        self.assertTrue(observations[0]['resolved']);self.assertEqual(observations[0]['request_kind'],'approval_requested')
        self.assertEqual(self.publish(),0)

    def test_unknown_source_time_stays_unknown_and_transport_receipt_is_separate(self):
        self.accept(event(time_unix_nano=0));logs=[]
        flush(self.store,self.binding,lambda log:logs.append(log) or True)
        self.assertGreater(logs[0]['time_unix_nano'],0)
        self.assertEqual(logs[0]['attributes']['observatory.native.source_time_ns'],0)
        self.assertEqual(logs[0]['attributes']['observatory.native.time_basis'],'bridge-receipt')
        self.assertEqual(logs[0]['body'],'')
        self.assertNotIn('prompt',json.dumps(logs))

    def test_overlay_is_exact_and_multiple_pending_requests_are_not_arbitrarily_chosen(self):
        self.accept(event());self.publish()
        base=item(agent_id=self.binding['agent_id'],trace_id=self.binding['trace_id'],state='Working',waiting_for='',request_id='')
        observations=self.store.native_observations('receiver-one')
        rows,_=overlay_observations([base],observations,'receiver-one')
        self.assertEqual(rows[0]['state'],'Waiting');self.assertEqual(rows[0]['request_id'],'request')
        for field in ('service','agent_id','session_id','trace_id','turn_id'):
            rows,_=overlay_observations([{**base,field:'foreign'}],observations,'receiver-one')
            self.assertEqual(rows[0]['state'],'Working')
        self.accept(event(2,request='second'));self.publish()
        rows,_=overlay_observations([base],self.store.native_observations('receiver-one'),'receiver-one')
        self.assertEqual(rows[0]['state'],'Working')

    def test_native_request_requires_matching_resolution_not_an_unrelated_new_turn(self):
        base=item(agent_id=self.binding['agent_id'],trace_id=self.binding['trace_id'],state='Working',waiting_for='',request_id='')
        runner=AttentionRunner();runner.update([base],snapshot(100),{},now=100,store=self.store);runner.persist(self.store,now=100)
        self.accept(event());self.publish()
        runner.update([base],snapshot(200),{},now=200,store=self.store);runner.persist(self.store,now=200)
        self.assertEqual(len(runner.delivery_candidates(now=200)),1)
        runner.update([{**base,'trace_id':'b'*32}],snapshot(201),{},now=201,store=self.store);runner.persist(self.store,now=201)
        self.assertEqual(len(runner.ledger.rows),1);self.assertTrue(runner.ledger.rows[0][0]['_attention_stale'])
        self.accept(event(2,'request_resolved'));self.publish()
        runner.update([base],snapshot(202),{},now=202,store=self.store);runner.persist(self.store,now=202)
        self.assertEqual(runner.ledger.rows,[])

    def test_bounded_private_binding_and_documented_codex_completion_only(self):
        path=Path(self.tmp.name)/'binding.json';path.write_text(json.dumps(self.binding));path.chmod(0o600)
        self.assertEqual(read_binding(path),self.binding)
        path.chmod(0o644)
        with self.assertRaisesRegex(UncertainLifecycle,'owner-only'):read_binding(path)
        for value in ('x'*8193,'['+'['*10000):
            with self.assertRaises(UncertainLifecycle):bounded_json(value)
        codex={**self.binding,'adapter':'codex-notify-v1'}
        result=runtime_event({'type':'agent-turn-complete','thread-id':'session','turn-id':'native-turn','last-assistant-message':'PRIVATE'},codex)
        self.assertEqual(result['kind'],'turn_completed');self.assertEqual(result['time_unix_nano'],0)
        self.assertNotIn('PRIVATE',str(result))
        for adapter in ('claude-hook-v1','pi-extension-v1','opencode-event-v2'):
            with self.assertRaisesRegex(UncertainLifecycle,'verified exact'):
                runtime_event({'hook_event_name':'PermissionRequest'},dict(self.binding,adapter=adapter))

    def test_follow_recorded_codex_turns_uses_proved_mapping_and_transport_order_only(self):
        from observatory.local_agents import identity
        from urllib.parse import parse_qs,urlsplit
        configured=validate_binding(dict(self.binding,adapter='codex-notify-v1',follow_recorded_turns=True,native_turn_id=''))
        class Api:
            def get(_,path):
                if path=='/api/health':return {'receiver_id':'receiver-one','local_agents':{'sessions':[dict(service='codex',agent_id=configured['agent_id'],session_id='session',source_id='source')]}}
                if path.startswith('/api/hierarchy'):return {'receiver_id':'receiver-one','nodes':[dict(agent_id=configured['agent_id'],source_id='source')]}
                trace=parse_qs(urlsplit(path).query)['trace_id'][0]
                return {'receiver_id':'receiver-one','activity':[dict(service='codex',agent_id=configured['agent_id'],session_id='session',trace_id=trace)]}
        for native_turn in ('turn-one','turn-two'):
            row=runtime_event({'type':'agent-turn-complete','thread-id':'session','turn-id':native_turn},configured)
            self.assertEqual(row['trace_id'],identity('source',native_turn));verify_binding(Api(),configured,row)
            self.assertEqual(row['sequence'],0);self.assertEqual(row['source_order'],'transport-only')
            self.assertTrue(enqueue(self.store,configured,row))
            self.assertFalse(enqueue(self.store,configured,row))
        sent=[];flush(self.store,configured,lambda log:sent.append(log) or True)
        self.assertEqual([row['attributes']['observatory.native.sequence'] for row in sent],[1,2])
        self.assertTrue(all(row['attributes']['observatory.native.source_order']=='transport-only' for row in sent))
        self.assertEqual(self.store.native_observations('receiver-one'),[])
        class Foreign(Api):
            def get(self,path):
                value=super().get(path)
                if path=='/api/health':value['local_agents']['sessions'][0]['source_id']='foreign'
                return value
        with self.assertRaisesRegex(UncertainLifecycle,'exact recorded native provider'):verify_binding(Foreign(),configured,row)

    def test_activity_metadata_uses_actual_row_owner_and_rejects_sender_collisions(self):
        from observatory.native_lifecycle import native_metadata,activity_observations
        self.accept(event());logs=[];flush(self.store,self.binding,lambda log:logs.append(log) or True)
        metadata=native_metadata(logs[0]['attributes'],logs[0]['resource_attributes'])
        row=dict(receiver_id='receiver-one',service='codex',agent_id=self.binding['agent_id'],session_id='session',
            trace_id='a'*32,turn_id='',source_id='source-event',native_lifecycle=metadata)
        observed=activity_observations([row],'receiver-one');self.assertEqual(len(observed),1)
        self.assertEqual(observed[0]['agent_id'],self.binding['agent_id'])
        self.assertEqual(activity_observations([row,{**row,'agent_id':'ag1_'+'3'*32}],'receiver-one'),[])
        self.assertEqual(activity_observations([{**row,'receiver_id':'foreign'}],'receiver-one'),[])
        malformed={**metadata,'sequence':True}
        self.assertEqual(activity_observations([{**row,'native_lifecycle':malformed}],'receiver-one'),[])
        for raw in ('{"sender_id":"one","sender_id":"two"}','{"sequence":NaN}'):
            with self.assertRaises(UncertainLifecycle):bounded_json(raw)

    def test_bridge_does_not_override_stronger_captured_request_or_delayed_health(self):
        self.accept(event());self.publish();observations=self.store.native_observations('receiver-one')
        base=item(agent_id=self.binding['agent_id'],trace_id=self.binding['trace_id'],request_id='captured-request')
        rows,_=overlay_observations([base],observations,'receiver-one')
        self.assertEqual(rows[0]['request_id'],'captured-request')
        delayed={**base,'state':'Unconfirmed','request_id':''}
        rows,_=overlay_observations([delayed],observations,'receiver-one')
        self.assertEqual(rows[0]['state'],'Unconfirmed')
        completed={**base,'state':'Completed','run_state':'completed','request_id':'','outcome_time_ns':200*10**9,
            'diagnostics':{'last_success_ns':201*10**9}}
        rows,_=overlay_observations([completed],observations,'receiver-one')
        self.assertEqual(rows[0]['state'],'Completed')


if __name__=='__main__':unittest.main()
