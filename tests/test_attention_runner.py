"""Persistent baseline, offline catch-up and shared notification claims."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from tempfile import TemporaryDirectory
import time
import unittest
from unittest.mock import patch

from observatory.attention_runner import AttentionRunner,inventory_ready
from observatory.workflow_store import WorkflowStore,WorkflowStoreError
from tests.test_attention import item,snapshot


class AttentionRunnerTests(unittest.TestCase):
    def setUp(self):
        self.tmp=TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.path=Path(self.tmp.name)/'workflow.db';self.store=WorkflowStore(self.path)

    def update(self,runner,rows,now,**changes):
        data=snapshot(now,**changes)
        runner.update(rows,data,{},now=now)
        runner.persist(self.store,now=now)
        return runner.delivery_candidates(now=now)

    def native(self,request='one',stamp=50,**changes):
        return item(request_id=request,request_time_ns=int(stamp*1e9),request_clock='receiver',**changes)

    def test_first_ready_inventory_quiet_but_post_baseline_request_survives_restart(self):
        first=AttentionRunner()
        self.assertEqual(self.update(first,[self.native()],100),[])
        restarted=AttentionRunner();restarted.ledger.restore(WorkflowStore(self.path).load(now=200))
        candidates=self.update(restarted,[self.native('arrived-while-offline',150)],200)
        self.assertEqual(len(candidates),1)
        notify=lambda *_:'Desktop notification delivered'
        restarted.deliver(self.store,candidates,notify=notify)
        again=AttentionRunner();again.ledger.restore(self.store.load(now=201))
        candidates=self.update(again,[self.native('arrived-while-offline',150)],201)
        with patch.object(self.store,'finish_delivery',wraps=self.store.finish_delivery) as finished:
            self.assertEqual(again.deliver(self.store,candidates,notify=notify),'')
            finished.assert_not_called()

    def test_delayed_old_requests_and_unknown_clock_remain_quiet(self):
        runner=AttentionRunner();self.update(runner,[],100)
        self.assertEqual(self.update(runner,[self.native('backlog',99,agent_id='late',trace_id='late')],200),[])
        self.assertEqual(self.update(runner,[item(request_id='unknown-clock',request_time_ns=150*10**9,
            agent_id='remote',trace_id='remote')],201),[])
        # A fresh same-scope request-ID transition supplies ordering even without time.
        newer=item(request_id='next',agent_id='remote',trace_id='remote')
        self.assertEqual(len(self.update(runner,[newer],202)),1)

    def test_partial_inventory_does_not_finalize_baseline_or_deliver(self):
        runner=AttentionRunner()
        for changes in ({'receiver_time_ns':0},{'indexing_ready':False},{'error':'auth'},
                {'read_errors':{'sessions':'timeout'}},{'local_status':{'enabled':True,'initial_poll_complete':False}}):
            self.update(runner,[self.native()],100,**changes)
            self.assertEqual(self.store.attention_baselines(),{})
            self.assertFalse(inventory_ready(runner.latest[1]))
        self.assertEqual(self.update(runner,[self.native()],200),[])
        self.assertEqual(self.store.attention_baselines(),{'receiver-one':200*10**9})

    def test_two_watchers_and_cockpit_claim_one_delivery(self):
        initial=AttentionRunner();self.update(initial,[self.native()],100)
        records=self.store.load(now=101)
        runners=[AttentionRunner() for _ in range(3)]
        stores=[WorkflowStore(self.path) for _ in runners]
        for runner in runners:
            runner.ledger.restore(records);self.update(runner,[self.native('new',150)],200)
        calls=[]
        def notify(title,body):calls.append((title,body));return 'Desktop notification delivered'
        with ThreadPoolExecutor(max_workers=3) as pool:
            list(pool.map(lambda pair:pair[0].deliver(pair[1],pair[0].delivery_candidates(now=200),notify=notify),zip(runners,stores)))
        self.assertEqual(len(calls),1)
        self.assertNotIn('secret',str(calls))

    def test_seen_request_stays_pending_and_blocks_offline_notification(self):
        runner=AttentionRunner();self.update(runner,[self.native()],100)
        candidates=self.update(runner,[self.native('new',150)],200)
        incident=candidates[0]
        self.store.acknowledge(incident.namespace,incident.id,incident.scope_key,now=201)
        restarted=AttentionRunner();restarted.ledger.restore(self.store.load(now=202))
        self.update(restarted,[self.native('new',150)],202)
        self.assertIn('seen; still pending',restarted.ledger.rows[0][1][0])
        self.assertEqual(restarted.deliver(self.store,restarted.delivery_candidates(now=202),notify=lambda *_:self.fail('acknowledged delivery')),'')
        self.update(restarted,[self.native('new',150,state='Working',waiting_for='')],203)
        self.assertEqual(restarted.ledger.rows,[])

    def test_storage_error_never_creates_an_optimistic_delivery_or_ack(self):
        runner=AttentionRunner();runner.update([self.native()],snapshot(100),{},now=100)
        with patch.object(self.store,'observe',side_effect=WorkflowStoreError('disk full')):
            with self.assertRaisesRegex(WorkflowStoreError,'disk full'):runner.persist(self.store,now=100)
        self.assertEqual(runner.delivery_candidates(now=100),[])
        self.assertEqual(self.store.load(now=101),())

    def test_first_inventory_is_atomic_between_independent_runners(self):
        runners=[AttentionRunner(),AttentionRunner()]
        def observe(runner):
            runner.update([self.native()],snapshot(100),{},now=100)
            runner.persist(WorkflowStore(self.path),now=100)
            return runner.delivery_candidates(now=100)
        with ThreadPoolExecutor(max_workers=2) as pool:self.assertEqual(list(pool.map(observe,runners)),[[],[]])
        self.assertEqual(len(self.store.attention_baselines()),1)
        self.assertNotIn('task',self.path.read_bytes().decode('latin1'))

    def test_v1_upgrade_preserves_receipts_claims_baseline_and_native_outbox(self):
        from observatory.native_lifecycle import validate_binding,enqueue,normalize_event
        from tests.test_native_lifecycle import binding,event
        from observatory.workflow_store import SCHEMA_VERSION,WorkflowSchemaError
        runner=AttentionRunner();self.update(runner,[self.native()],100)
        candidates=self.update(runner,[self.native('new',150)],200);incident=candidates[0]
        self.store.claim(incident.namespace,incident.id,incident.scope_key,now=201)
        self.store.finish_delivery(incident.namespace,incident.id,status='delivered',now=202)
        self.store.acknowledge(incident.namespace,incident.id,incident.scope_key,now=203)
        self.store.mark_error_seen(['a'*64],now=204)
        configured=validate_binding(binding());enqueue(self.store,configured,normalize_event(event(),configured))
        with self.store._transaction() as db:
            db.execute("UPDATE workflow_meta SET value='1' WHERE key='schema_version'")
            tables=('workflow_incidents_v1','workflow_acknowledgements_v1','workflow_notification_claims_v1',
                'workflow_attention_baselines_v1','workflow_attention_eligibility_v1','workflow_error_seen_v1',
                'workflow_native_senders_v1','workflow_native_events_v1','workflow_native_requests_v1')
            original={table:[tuple(row) for row in db.execute('SELECT * FROM '+table)] for table in tables}
        upgraded=WorkflowStore(self.path)
        with upgraded._transaction(write=False) as db:
            self.assertEqual(db.execute("SELECT value FROM workflow_meta WHERE key='schema_version'").fetchone()[0],str(SCHEMA_VERSION))
            self.assertEqual(original,{table:[tuple(row) for row in db.execute('SELECT * FROM '+table)] for table in tables})
        with patch('observatory.workflow_store.SCHEMA_VERSION',1):
            with self.assertRaisesRegex(WorkflowSchemaError,'Unsupported'):WorkflowStore(self.path)
        with upgraded._transaction() as db:db.execute("UPDATE workflow_meta SET value=? WHERE key='schema_version'",(str(SCHEMA_VERSION+1),))
        with self.assertRaisesRegex(WorkflowSchemaError,'Unsupported'):WorkflowStore(self.path)

    def test_explicit_unknown_bridge_clock_never_inherits_native_clock(self):
        from observatory.attention_runner import comparable_source_clock
        native=self.native(run_state='waiting',diagnostics={'last_success_ns':100*10**9})
        self.assertTrue(comparable_source_clock(native))
        native['request_clock']='unknown'
        self.assertFalse(comparable_source_clock(native))
        self.assertTrue(inventory_ready(snapshot(100,local_status={'enabled':True,'initial_poll_complete':True,'read_errors':99})))


if __name__=='__main__':unittest.main()
