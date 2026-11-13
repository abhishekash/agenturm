"""Worker integration without a terminal: claims, recovery and receipt scoping."""
from pathlib import Path
from tempfile import TemporaryDirectory
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from observatory.ui_workflow_controller import WorkflowController
from observatory.workflow_store import WorkflowStoreError
from tests.test_attention import item,snapshot


class Owner:
    def __init__(self):
        self.feed=SimpleNamespace(snapshot=snapshot(time.time()),api=SimpleNamespace(endpoint='http://localhost:14318'))
        self.preferences=SimpleNamespace(enabled=True)
        self.workflow_notify=True;self.workflow_snooze=0;self.is_headless=False
        self.workflow_ledger=SimpleNamespace(rows=[]);self.workflow_delivery='';self.screen=None
        self.threads=[];self.messages=[]

    def run_worker(self,callback,**kwargs):
        assert kwargs['thread']
        thread=threading.Thread(target=callback);self.threads.append(thread);thread.start()

    def call_from_thread(self,callback,*args):return callback(*args)
    def notify(self,message,**kwargs):self.messages.append(message)
    def wait(self):
        for thread in self.threads:
            thread.join(5)
            assert not thread.is_alive()


class ControllerTests(unittest.TestCase):
    def setUp(self):
        self.tmp=TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.owner=Owner();self.path=Path(self.tmp.name)/'workflow.sqlite3'
        self.controller=WorkflowController(self.owner,store_path=self.path)
        self.addCleanup(self.controller.close)

    def update(self,items):
        self.owner.feed.snapshot=snapshot(time.time())
        self.controller.update(items,self.owner.feed.snapshot,{})
        self.owner.wait()

    def test_claims_delivery_retry_and_private_notification_body(self):
        with patch('observatory.ui_workflow.desktop_notify',return_value='Desktop notification delivery failed') as delivery:
            self.update([item(task='secret instructions',project='project')]);delivery.assert_not_called()
            self.update([item(request_id='two',task='secret instructions',project='project')])
            self.assertEqual(delivery.call_count,1)
            self.assertNotIn('secret instructions',str(delivery.call_args))
            self.update([item(request_id='two')]);self.assertEqual(delivery.call_count,1)
            selected=self.owner.workflow_ledger.rows[0][0]
            self.assertEqual(selected['_attention_delivery'],'failed')
            self.controller.retry(selected);self.owner.wait();self.assertEqual(delivery.call_count,2)

    def test_request_seen_does_not_review_later_completion_and_review_is_reversible(self):
        selected=item();self.update([selected])
        self.controller.acknowledge(selected);self.owner.wait()
        self.assertTrue(self.controller.seen(selected))
        self.assertFalse(self.controller.reviewed(selected))
        self.assertIn('still pending',self.owner.workflow_ledger.rows[0][1][0])
        done=item(state='Completed');self.update([done])
        self.assertFalse(self.controller.reviewed(done))
        self.controller.acknowledge(done,action='reviewed');self.owner.wait()
        self.assertTrue(self.controller.reviewed(done));self.assertEqual(self.owner.workflow_ledger.rows,[])
        self.controller.acknowledge(done,action='reviewed');self.owner.wait()
        self.assertFalse(self.controller.reviewed(done));self.assertEqual(len(self.owner.workflow_ledger.rows),1)

    def test_threshold_or_failure_receipt_cannot_review_completion(self):
        from observatory.attention import classify
        from observatory.workflow_evidence import scope_for
        selected=item(state='Working')
        scope=scope_for(selected,self.owner.feed.snapshot,self.owner.feed.api.endpoint)
        threshold=classify(selected,scope,{'input_tokens':100},{'tokens':50},now=time.time())[0]
        self.controller.receipts={threshold.id:{**threshold.to_record(),'acknowledgement':'seen'}}
        self.assertFalse(self.controller.reviewed(item(state='Completed')))
        failed=classify(item(state='Failed'),scope,{}, {},now=time.time())[0]
        self.controller.receipts[failed.id]={**failed.to_record(),'acknowledgement':'seen'}
        self.assertFalse(self.controller.reviewed(item(state='Completed')))
        self.assertTrue(self.controller.reviewed(item(state='Failed')))

    def test_headless_never_opens_default_state_and_new_unanchored_request_is_visible(self):
        self.owner.is_headless=True
        with patch('observatory.ui_workflow_controller.WorkflowStore',side_effect=AssertionError('filesystem touched')):
            self.update([item(trace_id='')])
        self.assertFalse(self.path.exists());self.assertEqual(len(self.owner.workflow_ledger.rows),1)
        self.controller.acknowledge(item(trace_id=''));self.owner.wait()
        self.assertIn('No durable run/request identity',self.owner.messages[-1])

    def test_storage_error_keeps_monitoring_and_explicit_cleanup_retries(self):
        with patch('observatory.ui_workflow_controller.WorkflowStore',side_effect=WorkflowStoreError('read-only volume')):
            self.update([item()])
        self.assertEqual(len(self.owner.workflow_ledger.rows),1)
        self.assertIn('read-only volume',self.controller.warning)
        self.controller.cleanup();self.owner.wait()
        self.assertTrue(self.path.exists());self.assertIn('Unresolved incidents were kept',self.controller.warning)

    def test_stale_retained_usage_cannot_create_threshold_from_fresh_health(self):
        native=item(run_state='waiting',diagnostics={'last_success_ns':time.time_ns()})
        self.update([])
        self.owner.feed.snapshot=snapshot(time.time(),read_errors={'sessions':'timeout'},sessions=[{
            'agent_id':'worker','session_id':'session','services':['codex'],
            'latest_run':{'trace_id':'run','input_tokens':100}}])
        with patch('observatory.ui_workflow.desktop_notify') as delivery:
            self.controller.update([native],self.owner.feed.snapshot,{'tokens':50});self.owner.wait()
        self.assertEqual([row[0]['_attention_kind'] for row in self.owner.workflow_ledger.rows],['approval'])

    def test_bulk_completion_review_is_frozen_idempotent_and_excludes_requests(self):
        done=[item(id=f'done-{i}',agent_id=f'worker-{i}',trace_id=f'{i+1:032x}',state='Completed',waiting_for='') for i in range(100)]
        approval=item(id='approval',agent_id='approval-worker',trace_id='e'*32)
        self.update([*done,approval])
        frozen=tuple(identity for identity,incident in self.controller.ledger.incidents.items() if incident.kind=='completion')
        approval_id=next(identity for identity,incident in self.controller.ledger.incidents.items() if incident.kind=='approval')
        late=item(id='late',agent_id='late-worker',trace_id='f'*32,state='Completed',waiting_for='')
        self.update([*done,approval,late])
        self.controller.review_completions([*frozen,approval_id,*frozen]);self.owner.wait()
        records=self.controller.store.load()
        self.assertEqual(sum(record.get('acknowledgement')=='reviewed' for record in records),100)
        self.assertFalse(next(record for record in records if record['id']==approval_id).get('acknowledgement'))
        late_id=next(identity for identity,incident in self.controller.ledger.incidents.items() if incident.scope_key not in {self.controller.ledger.incidents[key].scope_key for key in frozen} and incident.kind=='completion')
        self.assertFalse(next(record for record in records if record['id']==late_id).get('acknowledgement'))
        self.controller.review_completions(frozen);self.owner.wait()
        self.assertEqual(sum(record.get('acknowledgement')=='reviewed' for record in self.controller.store.load()),100)

    def test_authoritative_turn_receipt_survives_restart_and_assistant_step_change(self):
        from observatory.attention import AttentionLedger
        from observatory.ui_workflow_controller import prepared_item
        from observatory.workflow_evidence import scope_for
        first=item(service='opencode',agent_id='worker',trace_id='a'*32,state='Completed',waiting_for='')
        turn=dict(trace_id='a'*32,turn_id='c'*32,scope_owner=dict(producer='opencode',agent_id='worker',session_id='session'),agent_count=1,agent_count_complete=False,step_trace_ids=['a'*32],turn_usage_complete=False)
        data=snapshot(time.time(),sessions=[dict(session_id='session',agent_id='worker',services=['opencode'],latest_run=dict(trace_id='a'*32,turn_usage=turn))])
        prepared=prepared_item(first,data);scope=scope_for(prepared,data)
        self.assertEqual(scope.kind,'turn')
        ledger=AttentionLedger();ledger.update([prepared],data,{})
        from observatory.workflow_store import WorkflowStore
        saved=WorkflowStore(self.path).observe(incident.to_record() for incident in ledger.incidents.values())
        restored=AttentionLedger();restored.restore(saved.records)
        self.assertEqual(set(restored.incidents),set(ledger.incidents))
        self.assertEqual(restored.items[scope.scope_key]['turn_id'],'c'*32)
        turn.update(trace_id='b'*32,step_trace_ids=['a'*32,'b'*32])
        data.sessions[0]['latest_run']['trace_id']='b'*32
        second=prepared_item({**first,'trace_id':'b'*32},data)
        self.assertEqual(scope_for(second,data).scope_key,scope.scope_key)
