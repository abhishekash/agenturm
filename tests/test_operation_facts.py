"""Typed operation provenance and conservative correlation from captured facts."""
from copy import deepcopy
import json
import unittest

from observatory.operation_facts import evidence_detail,operation_facts,operation_key,safe_value


def event(**changes):
    evidence=dict(version=1,inputs=[('native.input',{'cmd':'pytest tests','cwd':'/work','flags':[False,0,None]})],
        commands=[('native.command','pytest tests')],exits=[('native.exit_code',1)],
        outputs=[('native.output','output','assert 1 == 2')],cwd='/work',tool='exec_command',server=None)
    row=dict(id='event',source_id='event',receiver_id='receiver',service='codex',agent_id='worker',session_id='session',
        trace_id='trace',domain='shell',action='execute',subject='pytest tests',result='error',
        time_unix_nano=1,operation_evidence=evidence)
    row.update(changes)
    return row


class OperationFactsTests(unittest.TestCase):
    def test_typed_input_exit_and_combined_output_have_distinct_provenance(self):
        facts=operation_facts(event())
        self.assertEqual(facts['command'],'pytest tests')
        self.assertEqual(facts['exit_code'],1)
        self.assertEqual(facts['result'],'failed')
        self.assertEqual(facts['arguments']['flags'],[False,0,None])
        self.assertEqual(facts['output_kind'],'output')
        self.assertEqual(facts['output_origin'],'native.output')
        self.assertTrue(facts['operation_fingerprint'])
        self.assertTrue(facts['error_fingerprint'])

    def test_unclassified_printed_json_never_supplies_input_exit_or_mcp_error(self):
        row=event(operation_evidence=None,domain='tool',subject='Tool',result='observed',
            detail=json.dumps({'command':'rm -rf /','arguments':{'path':'private'},'exit_code':0,'isError':True}))
        facts=operation_facts(row)
        self.assertIsNone(facts['command'])
        self.assertIsNone(facts['arguments'])
        self.assertIsNone(facts['exit_code'])
        self.assertEqual(facts['result'],'unknown')
        self.assertIsNone(facts['operation_fingerprint'])

    def test_display_labels_do_not_prove_input_or_conflict_with_full_semantic_commands(self):
        row=event(operation_evidence=None,working_directory='/work',attributes={'tool.name':'Bash'})
        self.assertIsNone(operation_key(row))
        row['attributes'].update({'process.command_line':'pytest tests --verbose','process.exit.code':1})
        facts=operation_facts(row)
        self.assertEqual(facts['command'],'pytest tests --verbose')
        self.assertFalse(facts['input_conflict'])
        self.assertTrue(facts['operation_fingerprint'])

    def test_input_and_output_clipping_are_independent(self):
        row=event();row['operation_evidence']['outputs']=[('result','stdout','x'*16000)]
        facts=operation_facts(row)
        self.assertTrue(facts['input_complete'])
        self.assertTrue(facts['operation_fingerprint'])
        self.assertTrue(facts['output_clipped'])
        self.assertIsNone(facts['error_fingerprint'])
        self.assertLessEqual(len(facts['output']),1500)
        row=event();row['operation_evidence']['inputs'][0][1]['extra']='x'*16000
        facts=operation_facts(row)
        self.assertFalse(facts['input_complete'])
        self.assertIsNone(facts['operation_fingerprint'])
        self.assertTrue(facts['error_fingerprint'])

    def test_role_labelled_serialization_bounds_each_side_and_keeps_terminal_zero(self):
        row=event(result='success');evidence=row.pop('operation_evidence')
        evidence['exits']=[('process.exit.code',0)]
        evidence['outputs']=[('stdout','stdout','x'*1000000)]
        row.update(detail_role='operation_v1',detail=evidence_detail(evidence))
        self.assertLessEqual(len(row['detail'].encode()),8000)
        facts=operation_facts(row)
        self.assertTrue(facts['operation_fingerprint'])
        self.assertEqual(facts['exit_code'],0)
        self.assertEqual(facts['result'],'passed')
        self.assertTrue(facts['output_clipped'])
        evidence['inputs'][0][1]['extra']='x'*1000000
        evidence['outputs']=[('stderr','stderr','useful bounded error')]
        row['detail']=evidence_detail(evidence)
        facts=operation_facts(row)
        self.assertIsNone(facts['operation_fingerprint'])
        self.assertEqual(facts['output'],'useful bounded error')

    def test_json_dict_order_canonicalizes_but_types_lists_quotes_and_spaces_do_not(self):
        first=event();other=deepcopy(first)
        other['operation_evidence']['inputs'][0]=(other['operation_evidence']['inputs'][0][0],
            dict(reversed(list(first['operation_evidence']['inputs'][0][1].items()))))
        self.assertEqual(operation_key(first),operation_key(other))
        for value in ([0,False,None],[False,0,''],[False,0],False,0):
            other=deepcopy(first);other['operation_evidence']['inputs'][0][1]['flags']=value
            self.assertNotEqual(operation_key(first),operation_key(other))
        for command in ('pytest  tests',"pytest 'tests'"):
            other=deepcopy(first)
            other['operation_evidence']['commands']=[('native.command',command)]
            other['operation_evidence']['inputs'][0][1]['cmd']=command
            self.assertNotEqual(operation_key(first),operation_key(other))

    def test_missing_or_conflicting_cwd_tool_or_inputs_cannot_prove_equivalence(self):
        row=event();row['operation_evidence']['cwd']='/elsewhere'
        self.assertIsNone(operation_key(row))
        row=event();row['operation_evidence']['tool']=None
        self.assertIsNone(operation_key(row))
        row=event();row['operation_evidence']['inputs']=[];row['operation_evidence']['cwd']=None
        self.assertIsNone(operation_key(row))
        row=event();row['operation_evidence']['commands'].append(('other','pytest other'))
        self.assertIsNone(operation_key(row))
        row=event();row['operation_evidence']['inputs'].append(('other',{'cmd':'pytest other'}))
        self.assertIsNone(operation_key(row))

    def test_exact_owner_execution_kind_and_receiver_are_required(self):
        row=event()
        for key in ('receiver_id','service','agent_id','session_id','trace_id'):
            self.assertIsNone(operation_key({**row,key:''}))
        self.assertNotEqual(operation_key({**row,'receiver_id':'one'}),operation_key({**row,'receiver_id':'two'}))
        self.assertIsNone(operation_key(row,receiver='other'))
        self.assertNotEqual(operation_key(row),operation_key({**row,'turn_id':'trace'}))
        self.assertEqual(operation_key({**row,'turn_id':'turn'}),
                         operation_key({**row,'turn_id':'turn','trace_id':'other-step'}))

    def test_secrets_are_redacted_before_fingerprints_including_nested_json_command(self):
        for field in ('password','api_key','access_token','refresh_token','authorization','pass\u200bword'):
            row=event();row['operation_evidence']['inputs'][0][1][field]='PRIVATE_SECRET_VALUE'
            facts=operation_facts(row)
            self.assertNotIn('PRIVATE_SECRET_VALUE',json.dumps(facts))
            self.assertIsNone(facts['operation_fingerprint'])
        row=event();row['operation_evidence']['commands']=[('command',"curl -d '{\"password\":\"PRIVATE_SECRET_VALUE\"}'")]
        row['operation_evidence']['inputs']=[]
        facts=operation_facts(row)
        self.assertNotIn('PRIVATE_SECRET_VALUE',json.dumps(facts))
        self.assertIsNone(facts['operation_fingerprint'])
        row=event();row['operation_evidence']['outputs']=[('stderr','stderr',{'password':'PRIVATE_SECRET_VALUE'})]
        facts=operation_facts(row)
        self.assertNotIn('PRIVATE_SECRET_VALUE',json.dumps(facts))
        self.assertIsNone(facts['error_fingerprint'])

    def test_escaped_and_control_bearing_quoted_credential_keys_are_omitted(self):
        for key in (r'pass\u0077ord','pass\u200bword',r'api\u005fkey',r'pass\\u0077ord'):
            row=event()
            command="curl -d '{\""+key+"\":\"PRIVATE_SECRET_VALUE\"}'"
            row['operation_evidence']['commands']=[('command',command)]
            row['operation_evidence']['inputs']=[]
            facts=operation_facts(row)
            self.assertNotIn('PRIVATE_SECRET_VALUE',json.dumps(facts))
            self.assertIsNone(facts['operation_fingerprint'])

    def test_serializer_never_upgrades_incomplete_terminal_evidence(self):
        for exits in ([('exit',0)]*8+[('other',1)],['bad'],{'exit':0}):
            row=event(result='success')
            row['operation_evidence']['exits']=exits
            before=operation_facts(row)
            evidence=row.pop('operation_evidence')
            row.update(detail_role='operation_v1',detail=evidence_detail(evidence))
            after=operation_facts(row)
            self.assertEqual(before['result'],'unknown')
            self.assertEqual(after['result'],'unknown')
            self.assertTrue(after['result_conflict'])

    def test_flag_only_and_empty_error_values_have_no_distinguishing_signature(self):
        for value in (True,False,1,0,{},[],None,{'isError':True},{'error':True}):
            row=event()
            row['operation_evidence']['outputs']=[('tool.error','error',value)]
            self.assertIsNone(operation_facts(row)['error_fingerprint'],value)

    def test_expected_exit_one_requires_simple_shell_terminal_evidence(self):
        for command in ('rg missing src','/usr/bin/grep missing file','diff before after','git diff --exit-code'):
            row=event();row['operation_evidence']['commands']=[('command',command)]
            row['operation_evidence']['inputs']=[]
            self.assertTrue(operation_facts(row)['expected_nonzero'],command)
        for command in ('git diff --check','diff --check a b','rg missing || true','rg missing; false','pytest tests'):
            row=event();row['operation_evidence']['commands']=[('command',command)]
            row['operation_evidence']['inputs']=[]
            self.assertFalse(operation_facts(row)['expected_nonzero'],command)
        for change in ({'result':'blocked'},{'domain':'mcp'},{'result':'success'}):
            row=event(**change);row['operation_evidence']['commands']=[('command','rg missing')]
            row['operation_evidence']['inputs']=[]
            self.assertFalse(operation_facts(row)['expected_nonzero'])

    def test_conflicting_terminal_results_are_unknown_and_do_not_match_error_signatures(self):
        row=event();row['operation_evidence']['exits'].append(('other',0))
        facts=operation_facts(row)
        self.assertTrue(facts['result_conflict'])
        self.assertEqual(facts['result'],'unknown')
        self.assertIsNone(facts['exit_code'])
        self.assertIsNone(facts['error_fingerprint'])
        row=event(result='success')
        self.assertEqual(operation_facts(row)['result'],'unknown')

    def test_malformed_and_deep_values_are_bounded_without_type_coercion(self):
        for key,value in (('inputs','bad'),('commands',[None]),('exits',['bad']),('outputs',{'bad':True})):
            row=event();row['operation_evidence'][key]=value
            self.assertIsInstance(operation_facts(row),dict)
        for value in (float('nan'),10**2000):
            _,complete=safe_value(value)
            self.assertFalse(complete)
        for value in ('🦀'*3000,'\ud800','x'*1000000):
            sanitized,complete=safe_value(value)
            self.assertFalse(complete)
            self.assertLess(len(sanitized),100)
        _,complete=safe_value({'x':{'x':{'x':{'x':{'x':{'x':{'x':1}}}}}}})
        self.assertFalse(complete)
        row=event();row['operation_evidence']['exits']=[('exit',True)]
        self.assertIsNone(operation_facts(row)['exit_code'])
        row['result']='success'
        self.assertEqual(operation_facts(row)['result'],'unknown')


if __name__=='__main__':unittest.main()
