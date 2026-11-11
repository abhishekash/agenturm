"""Attention is keyed by recorded execution/request evidence, not display text."""
import time
import unittest
from types import SimpleNamespace

from observatory.attention import AttentionLedger, classify, fresh_observation
from observatory.workflow_evidence import scope_for


def snapshot(now=100, **values):
    return SimpleNamespace(**dict(dict(updated=now,error='',receiver_id='receiver-one',activity=[],
        sessions=[],traces=[],local_status={},read_errors={}), **values))


def item(**values):
    return dict(dict(id='source',service='codex',agent_id='worker',session_id='session',trace_id='run',
                     state='Waiting',waiting_for='Approval',request_id='request-one'), **values)


class AttentionTests(unittest.TestCase):
    def test_initial_inventory_missing_intervals_and_new_request(self):
        ledger=AttentionLedger();first=item()
        self.assertEqual(ledger.update([first],snapshot(),{},100),[])
        identity=ledger.rows[0][0]['_attention_id']
        ledger.update([],snapshot(110),{},110)
        self.assertEqual(ledger.rows[0][0]['_attention_id'],identity)
        self.assertTrue(ledger.rows[0][0]['_attention_stale'])
        self.assertEqual(ledger.update([first],snapshot(120),{},120),[])
        self.assertEqual(ledger.rows[0][2],100)
        self.assertEqual(len(ledger.update([item(request_id='request-two')],snapshot(130),{},130)),1)
        self.assertIn(('receiver-one',identity),ledger.resolved)

    def test_seen_request_stays_until_fresh_positive_resolution(self):
        ledger=AttentionLedger();ledger.update([item()],snapshot(),{},100)
        incident=next(iter(ledger.incidents.values()))
        ledger.restore([{**incident.to_record(),'acknowledgement':'seen','resolved_at':None}])
        ledger.update([item()],snapshot(101),{},101)
        self.assertIn('seen; still pending',ledger.rows[0][1][0])
        ledger.update([item(state='Working')],snapshot(102,error='timeout'),{},102)
        self.assertEqual(len(ledger.rows),1)
        ledger.update([item(state='Working')],snapshot(103),{},103)
        self.assertEqual(ledger.rows,[])

    def test_source_timestamp_is_separate_from_observed_wait(self):
        ledger=AttentionLedger();ledger.update([item(request_time_ns=50_000_000_000)],snapshot(),{},100)
        self.assertEqual(ledger.rows[0][2],100)
        self.assertEqual(ledger.rows[0][0]['_attention_source_time_ns'],50_000_000_000)
        other=AttentionLedger();other.update([item()],snapshot(),{},100)
        self.assertEqual(other.rows[0][0]['_attention_source_time_ns'],0)

    def test_reason_threshold_changes_never_change_request_identity(self):
        ledger=AttentionLedger();request=item(_workflow_usage={'input_tokens':100})
        ledger.update([request],snapshot(),{},100)
        identity=ledger.rows[0][0]['_attention_id']
        alerts=ledger.update([request],snapshot(101),{'tokens':50},101)
        self.assertEqual(len(alerts),1)
        self.assertEqual(alerts[0][1],['Task token threshold reached'])
        self.assertEqual(ledger.rows[0][0]['_attention_id'],identity)
        self.assertEqual(ledger.update([request],snapshot(102),{'tokens':50},102),[])

    def test_new_receiver_inventory_is_silent_and_scope_isolated(self):
        ledger=AttentionLedger();ledger.update([item()],snapshot(),{},100)
        first=ledger.rows[0][0]['_attention_id']
        self.assertEqual(ledger.update([item()],snapshot(101,receiver_id='receiver-two'),{},101),[])
        self.assertEqual(len(ledger.rows),1)
        self.assertNotEqual(ledger.rows[0][0]['_attention_id'],first)

    def test_restart_restores_wait_and_never_alerts_backlog(self):
        previous=AttentionLedger();previous.update([item()],snapshot(),{},100)
        records=[{**incident.to_record(),'resolved_at':None,'acknowledgement':'seen'} for incident in previous.incidents.values()]
        restarted=AttentionLedger();restarted.restore(records)
        self.assertEqual(restarted.update([item()],snapshot(200),{},200),[])
        self.assertEqual(restarted.rows[0][2],100)
        self.assertTrue(restarted.rows[0][0]['_attention_seen'])

    def test_restart_keeps_missing_request_and_fresh_new_run_resolves_exact_agent_only(self):
        original=AttentionLedger();original.update([item()],snapshot(),{},100)
        records=[{**incident.to_record(),'resolved_at':None} for incident in original.incidents.values()]
        restarted=AttentionLedger();restarted.restore(records)
        restarted.update([],snapshot(200),{},200)
        self.assertEqual(len(restarted.rows),1)
        self.assertIn('Retained approval',restarted.rows[0][0]['task'])
        self.assertTrue(restarted.rows[0][0]['_attention_stale'])
        restarted.update([item(agent_id='other',state='Working',trace_id='next')],snapshot(201),{},201)
        self.assertEqual(len(restarted.rows),1)
        restarted.update([item(state='Working',trace_id='next')],snapshot(202),{},202)
        self.assertEqual(restarted.rows,[])

    def test_absent_run_or_request_is_visible_without_durable_actions(self):
        for missing in ({'trace_id':''},{'request_id':''}):
            with self.subTest(missing=missing):
                ledger=AttentionLedger();ledger.update([],snapshot(),{},100)
                self.assertEqual(ledger.update([item(**missing)],snapshot(101),{},101),[])
                self.assertFalse(ledger.rows[0][0]['_attention_persistent'])

    def test_explicit_worker_never_adopts_untagged_or_other_service_request(self):
        selected=item(request_id='')
        events=[dict(id='request',service='codex',session_id='session',trace_id='run',
                     action='approval_request',time_unix_nano=90_000_000_000),
                dict(id='wrong',service='claude',agent_id='worker',session_id='session',trace_id='run',action='approval_request')]
        ledger=AttentionLedger();ledger.update([selected],snapshot(activity=events),{},100)
        self.assertFalse(ledger.rows[0][0]['_attention_persistent'])
        events[0]['agent_id']='worker';ledger.update([selected],snapshot(101,activity=events),{},101)
        self.assertTrue(ledger.rows[0][0]['_attention_persistent'])

    def test_partial_database_reads_never_alert_or_resolve_stored_incidents(self):
        ledger=AttentionLedger();ledger.update([],snapshot(),{},100)
        partial=snapshot(101,read_errors={'traces':'timeout'})
        self.assertEqual(ledger.update([item()],partial,{},101),[])
        self.assertTrue(ledger.rows[0][0]['_attention_stale'])
        ledger.update([item(state='Working')],snapshot(102,read_errors={'sessions':'timeout'}),{},102)
        self.assertEqual(len(ledger.rows),1)
        native=item(run_state='waiting',diagnostics={'last_success_ns':103_000_000_000})
        self.assertTrue(fresh_observation(native,snapshot(103,read_errors={'traces':'timeout'}),103))
        native['diagnostics']['backlog_bytes']=100
        self.assertFalse(fresh_observation(native,snapshot(103),103))

    def test_stall_episode_survives_delayed_capture_and_resets_on_progress(self):
        ledger=AttentionLedger();ledger.update([],snapshot(),{},100)
        stalled=item(state='Possibly stalled',waiting_for='',request_id='')
        self.assertEqual(len(ledger.update([stalled],snapshot(101),{},101)),1)
        identity=ledger.rows[0][0]['_attention_id']
        ledger.update([item(state='Monitor delayed')],snapshot(102),{},102)
        self.assertEqual(ledger.rows[0][0]['_attention_id'],identity)
        self.assertEqual(ledger.update([stalled],snapshot(103),{},103),[])
        ledger.update([item(state='Working')],snapshot(104),{},104)
        self.assertEqual(ledger.rows,[])
        self.assertEqual(len(ledger.update([stalled],snapshot(105),{},105)),1)
        self.assertNotEqual(ledger.rows[0][0]['_attention_id'],identity)

    def test_completion_is_queue_only_and_priority_order_is_stable(self):
        ledger=AttentionLedger();ledger.update([],snapshot(),{},100)
        done=item(id='done',agent_id='done',state='Completed')
        failed=item(id='failed',agent_id='failed',state='Failed')
        alerts=ledger.update([done,failed,item()],snapshot(101),{},101)
        self.assertEqual(len(alerts),2)
        self.assertEqual([r[0]['_attention_kind'] for r in ledger.rows],['approval','failure','completion'])

    def test_resolved_churn_does_not_retain_source_payloads_or_ids(self):
        ledger=AttentionLedger()
        for index in range(200):
            current=item(trace_id=str(index),request_id=str(index))
            ledger.update([current],snapshot(100+index*2),{},100+index*2)
            ledger.update([dict(current,state='Working')],snapshot(101+index*2),{},101+index*2)
        self.assertEqual(len(ledger.incidents),0)
        self.assertLessEqual(len(ledger.items),1)
        self.assertLessEqual(len(ledger.scopes),1)
        self.assertLessEqual(len(ledger.observed_ids),1)
