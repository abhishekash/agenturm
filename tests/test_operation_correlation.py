"""Error grouping and later-success observations require complete exact inputs."""
from copy import deepcopy
from types import SimpleNamespace
import unittest

from observatory.error_reminders import error_reference,event_receipt,pending_failures,trace_error_references
from observatory.operation_facts import operation_facts
from observatory.ui_monitoring import failure_groups
from tests.test_operation_facts import event


def success(**changes):
    row=event(id='success',source_id='success',result='success',time_unix_nano=3,**changes)
    row['operation_evidence']['exits']=[('native.exit_code',0)]
    row['operation_evidence']['outputs']=[('native.output','output','Checks passed')]
    return row


class OperationCorrelationTests(unittest.TestCase):
    def test_compact_retry_context_survives_separate_steps_in_same_turn(self):
        first=event(turn_id='turn');last=success(turn_id='turn')
        refs=trace_error_references([first],'step-one',receiver='receiver')
        later=trace_error_references([last],'step-two',receiver='receiver')
        self.assertEqual(len(refs),1);self.assertEqual(len(later),1)
        self.assertEqual(pending_failures(refs+later,receiver='receiver'),[])
        self.assertEqual(pending_failures(refs+[{**later[0],'turn_id':'other'}],receiver='receiver'),refs)
        self.assertNotIn('pytest tests',str(refs+later))
        self.assertNotIn('Checks passed',str(later))
        self.assertEqual(len(trace_error_references([{**success(turn_id=str(i)),
            'source_id':str(i),'id':str(i)} for i in range(100)],'step',receiver='receiver')),20)

    def test_same_inputs_can_observe_strictly_later_success_without_deleting_occurrences(self):
        first=event();last=success()
        self.assertEqual(pending_failures([first,last]),[])
        groups=failure_groups([first],[last])
        self.assertEqual(groups[0]['_failures'],[first])
        self.assertEqual(groups[0]['_later_success'],last)
        self.assertEqual(first['result'],'error')
        for stamp in (1,0,-1,True,'3',None):
            later={**last,'time_unix_nano':stamp}
            self.assertEqual(pending_failures([first,later]),[first])
            self.assertIsNone(failure_groups([first],[later])[0]['_later_success'])

    def test_receiver_worker_session_execution_and_tool_context_never_cross(self):
        first=event()
        for field in ('receiver_id','service','agent_id','session_id','trace_id','turn_id'):
            last=success();last[field]='other'
            self.assertEqual(pending_failures([first,last]),[first],field)
        for field in ('cwd','tool','server'):
            last=success()
            last['operation_evidence'][field]='other'
            self.assertEqual(pending_failures([first,last]),[first],field)
        last=success();last['operation_evidence']['inputs'][0][1]['flags']=[0,False,None]
        self.assertEqual(pending_failures([first,last]),[first])

    def test_grouping_retains_different_arguments_and_distinct_error_evidence(self):
        first=event(id='first',source_id='first')
        repeated=event(id='repeat',source_id='repeat',time_unix_nano=2)
        self.assertEqual(len(failure_groups([first,repeated])),1)
        changed=deepcopy(repeated);changed['operation_evidence']['inputs'][0][1]['flags']=[None,0,False]
        self.assertEqual(len(failure_groups([first,changed])),2)
        changed=deepcopy(repeated);changed['operation_evidence']['outputs']=[('native.output','output','Connection refused')]
        self.assertEqual(len(failure_groups([first,changed])),2)

    def test_missing_redacted_clipped_or_flag_only_evidence_stays_occurrence_specific(self):
        for shape in ('missing','redacted','clipped','flag'):
            first=event(id='first',source_id='first')
            if shape=='missing':first['operation_evidence']['cwd']=None;first['operation_evidence']['inputs']=[]
            if shape=='redacted':first['operation_evidence']['inputs'][0][1]['password']='PRIVATE_VALUE'
            if shape=='clipped':first['operation_evidence']['outputs']=[('stderr','stderr','x'*1000000)]
            if shape=='flag':first['operation_evidence']['outputs']=[('tool.error','error',True)]
            repeated=deepcopy(first);repeated.update(id='repeat',source_id='repeat',time_unix_nano=2)
            groups=failure_groups([first,repeated])
            self.assertEqual(len(groups),2,shape)
            if shape in {'missing','redacted'}:
                last=deepcopy(first);last.update(id='success',source_id='success',result='success',time_unix_nano=3)
                last['operation_evidence']['exits']=[('exit',0)]
                self.assertEqual(len(pending_failures([first,repeated,last])),2,shape)

    def test_compact_fingerprint_metadata_preserves_receipt_ids_and_contains_no_raw_payload(self):
        first=event()
        first['operation_evidence']['inputs'][0][1]['note']='PRIVATE_ARGUMENT'
        first['operation_evidence']['outputs']=[('stderr','stderr','PRIVATE_STDERR')]
        compact=error_reference(first)
        self.assertNotIn('PRIVATE_ARGUMENT',str(compact))
        self.assertNotIn('PRIVATE_STDERR',str(compact))
        self.assertNotIn('pytest tests',str(compact))
        snapshot=SimpleNamespace(receiver_id='receiver',local_status={})
        self.assertEqual(event_receipt(first,snapshot),event_receipt(compact,snapshot))
        self.assertEqual(error_reference(compact),compact)
        last=success();last['operation_evidence']['inputs'][0][1]['note']='PRIVATE_ARGUMENT'
        self.assertEqual(pending_failures([compact,last]),[])
        self.assertIsNone(event_receipt(compact,SimpleNamespace(receiver_id='other',local_status={})))

    def test_legacy_or_unsupported_metadata_cannot_invent_a_recovery(self):
        first=error_reference(event())
        last=success()
        for version in (None,True,0,2,'1'):
            unsupported={**first,'operation_version':version}
            self.assertEqual(pending_failures([unsupported,last]),[unsupported])
        legacy={key:value for key,value in first.items() if not key.startswith('operation_') and key!='error_fingerprint'}
        self.assertEqual(pending_failures([legacy,last]),[legacy])

    def test_scope_kind_and_missing_namespace_do_not_compare_by_identical_label(self):
        first=event()
        last=success();last['turn_id']=first['trace_id']
        self.assertEqual(pending_failures([first,last]),[first])
        first.pop('receiver_id');last=success();last.pop('receiver_id')
        self.assertEqual(pending_failures([first,last]),[first])

    def test_expected_nonzero_is_informational_only_with_complete_command_evidence(self):
        first=event()
        first['operation_evidence']['commands']=[('command','rg missing src')]
        first['operation_evidence']['inputs']=[]
        self.assertTrue(operation_facts(first)['expected_nonzero'])
        self.assertEqual(pending_failures([first]),[])
        self.assertEqual(failure_groups([first]),[])
        first['operation_evidence']['cwd']=None
        self.assertTrue(operation_facts(first)['expected_nonzero'])
        self.assertIsNone(operation_facts(first)['operation_fingerprint'])
        first['operation_evidence']['command_clipped']=True
        self.assertEqual(pending_failures([first]),[first])


if __name__=='__main__':unittest.main()
