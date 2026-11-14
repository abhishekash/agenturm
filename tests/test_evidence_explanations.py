"""Exact loaded-window explanations with unknown denominators and safe equivalence."""
from copy import deepcopy
import math
import unittest

from observatory.evidence_explanations import (mcp_window,progress_observations,configured_estimate,
    usage_explanation,validate_rates)
from observatory.ui_monitoring import task_usage_summary
from observatory.ui_preferences import safe_preferences
from observatory.workflow_patterns import _fingerprint,analyze_patterns
from tests.test_workflow_patterns import span


def call(n,**extra):
    return dict(id=str(n),receiver_id='receiver',service='codex',agent_id='worker',session_id='session',
        server='fixture',tool='lookup',method='tools/call',status='OK',time_unix_nano=(n+1)*10**9,**extra)


def rate(mode='flat_input'):
    return {'exact-model':dict(unit='USD_per_million_tokens',input_accounting=mode,input=2,output=8,
        **({'cache_read':.2,'cache_write':3} if mode=='split_cache' else {}))}


class EvidenceExplanations(unittest.TestCase):
    def test_tool_outcome_and_duration_denominators_are_independent(self):
        rows=[call(0,duration_ms=0),call(1,duration_ms=20),call(2,duration_ms=None)]
        rows[1]['status']='ERROR';rows[2]['status']='UNSET'
        report=mcp_window(rows,complete=True,window={'label':'Selected loaded page'})
        item=report['tools'][0]
        self.assertEqual((item['passed'],item['failed'],item['unknown']),(1,1,1))
        self.assertEqual((item['duration_known'],item['duration_unknown']),(2,1))
        self.assertEqual((item['p50_ms'],item['p95_ms']),(0,20))
        self.assertEqual(item['failure_fraction'],.5);self.assertTrue(item['small_sample'])
        self.assertIn('correctness',item['outcome_meaning']);self.assertEqual(len(item['references']),3)
        self.assertFalse(report['coverage']['partial'])

    def test_protocol_error_conflict_missing_time_and_loaded_scope_are_explicit(self):
        row=call(0,output={'isError':True});rows=[row,deepcopy(row)]
        report=mcp_window(rows,complete=True)
        self.assertEqual(report['tools'][0]['calls'],1)
        self.assertEqual(report['tools'][0]['failed'],1) # explicit MCP isError is a recorded failure
        missing=call(1);missing.pop('time_unix_nano')
        report=mcp_window([missing,call(2)],window={'start_ns':1,'end_ns':2*10**9})
        self.assertEqual(report['tools'],[])
        self.assertEqual(report['coverage']['unplaced_timestamp_records'],1)
        self.assertEqual(report['coverage']['outside_requested_window'],1)
        self.assertTrue(report['coverage']['partial'])
        report=mcp_window([call(n) for n in range(502)],configured=['unobserved'])
        self.assertEqual(report['tools'][0]['calls'],500)
        self.assertTrue(report['coverage']['input_clipped'])
        self.assertIn('absence does not justify',report['configured_unobserved'][0]['note'])

    def test_malformed_identity_and_nan_duration_never_crash_or_merge(self):
        rows=[call(0,duration_ms=math.nan),call(1,duration_ms=-2)]
        for row in rows:row.update(service={},server=['server'],agent_id={})
        report=mcp_window(rows)
        self.assertEqual(len(report['tools']),2)
        self.assertTrue(all(item['duration_known']==0 and not item['identity_complete'] for item in report['tools']))

    def test_rate_estimates_require_exact_complete_allocation_and_explicit_accounting(self):
        usage=dict(input_tokens=1000,output_tokens=100,cache_read_input_tokens=200,cache_write_input_tokens=100,
            usage_complete=True,models_complete=True,models=['exact-model'],known_cost_usd=.9)
        result=configured_estimate(usage,model='exact-model',rates=rate(),allocation_proven=True)
        self.assertTrue(result['available']);self.assertAlmostEqual(result['estimated_usd'],.0028)
        split=configured_estimate(usage,model='exact-model',rates=rate('split_cache'),allocation_proven=True)
        self.assertAlmostEqual(split['estimated_usd'],.00254)
        self.assertEqual(usage['known_cost_usd'],.9)
        for patch in ({'usage_complete':False},{'models_complete':False},{'models':['other']},{'usage_allocation':'shared trace'},
                      {'aggregation_ambiguous':True},{'input_tokens':True}):
            self.assertFalse(configured_estimate({**usage,**patch},model='exact-model',rates=rate(),allocation_proven=True)['available'])
        self.assertFalse(configured_estimate(usage,model='exact-model',rates=rate())['available'])
        self.assertFalse(configured_estimate(usage,model='other',rates=rate(),allocation_proven=True)['available'])
        self.assertFalse(configured_estimate({**usage,'cache_read_input_tokens':950},model='exact-model',rates=rate('split_cache'),allocation_proven=True)['available'])
        self.assertEqual(validate_rates({'x':dict(unit='tokens',input=2,output=8)}),{})
        self.assertEqual(validate_rates({'x':dict(unit='USD_per_million_tokens',input_accounting='flat_input',input=math.inf,output=2)}),{})
        self.assertLessEqual(len(validate_rates({str(i):rate()['exact-model'] for i in range(20)})),16)
        self.assertEqual(safe_preferences({'cost_estimate_rates':rate()})['cost_estimate_rates'],validate_rates(rate()))

    def test_usage_defaults_preserve_protocol_and_optional_estimates_are_separate(self):
        usage=dict(input_tokens=1000,output_tokens=100,usage_complete=True,models_complete=True,models=['exact-model'])
        text,details=task_usage_summary(usage)
        self.assertTrue(text.startswith('Task cost Not reported · Tokens 1,100'))
        self.assertNotIn('Configured estimate',text+details)
        text,details=task_usage_summary(usage,rates=rate(),model='exact-model',allocation_proven=True,explanations=True)
        self.assertIn('Configured estimate $0.0028',text);self.assertIn('separate from reported cost',details)
        self.assertIn('cumulative usage is not occupancy',details)
        _,details=task_usage_summary(usage,rates=rate(),model='exact-model')
        self.assertIn('Configured estimate unavailable',details)

    def test_cache_context_and_retry_unknowns_do_not_become_occupancy_or_zero(self):
        usage=dict(input_tokens=1000,cache_read_input_tokens=250,usage_complete=True)
        result=usage_explanation(usage)
        self.assertEqual(result['cache']['cached_input_fraction'],.25)
        self.assertFalse(result['context']['available']);self.assertFalse(result['retry_cost']['known'])
        source=dict(id='context-source',receiver_id='receiver',service='codex',agent_id='worker',session_id='session')
        context=dict(authoritative=True,used_tokens=10,capacity_tokens=100,source=source)
        self.assertFalse(usage_explanation(usage,context=context)['context']['available'])
        self.assertTrue(usage_explanation(usage,context=context,scope_owner=source)['context']['available'])
        self.assertFalse(usage_explanation(usage,context=context,scope_owner={**source,'turn_id':'another-turn'})['context']['available'])
        context['source']={**source,'agent_id':'sibling'}
        self.assertFalse(usage_explanation(usage,context=context,scope_owner=source)['context']['available'])
        self.assertFalse(usage_explanation({**usage,'usage_complete':False})['cache']['known'])

    def test_raw_hash_bypass_redaction_clipping_and_unknown_roles_are_unavailable(self):
        for value in (None,'','{"password":"SECRET"}',{'api_key':'SECRET'},'x'*20000,'{"password":"[REDACTED]"}'):
            self.assertIsNone(_fingerprint(value,sanitize=False))
        self.assertEqual(_fingerprint({'b':2,'a':1}),_fingerprint({'a':1,'b':2}))
        rows=[span(n,status='OK',args='{"password":"same"}') for n in range(5)]
        self.assertEqual(analyze_patterns(rows,[]),[])
        for row in rows:row['attributes']['tool.arguments']='{"a":1}'
        self.assertTrue(analyze_patterns(rows,[]))
        for row in rows:row.pop('receiver_id')
        self.assertEqual(analyze_patterns(rows,[]),[])
        self.assertTrue(analyze_patterns(rows,[],receiver='known-receiver'))
        for row in rows:row['receiver_id']='foreign'
        self.assertEqual(analyze_patterns(rows,[],receiver='known-receiver'),[])
        from observatory.core import _activity_for
        for row in rows:row['receiver_id']='known-receiver';row['events']=[]
        activity=_activity_for(rows,[])
        expected=analyze_patterns(rows,[],receiver='known-receiver')
        self.assertEqual(analyze_patterns(rows,[],receiver='known-receiver',activity=activity),expected)
        for row in activity:row['trace_id']='f'*32
        self.assertEqual(analyze_patterns(rows,[],receiver='known-receiver',activity=activity),[])

    def test_only_strict_later_exact_owner_progress_breaks_repetition(self):
        def event(n):
            return dict(id=str(n),receiver_id='receiver',service='codex',agent_id='worker',session_id='session',
                trace_id='a'*32,time_unix_nano=n*10**9,domain='tool',action='call',result='success',
                attributes={'tool.name':'build','tool.arguments':{'target':'fixture'}})
        def edit(n,agent='worker',result='success'):
            return {**event(n),'agent_id':agent,'domain':'file','action':'edit','result':result,'attributes':{},'subject':'a.py'}
        kind=lambda events:[row['kind'] for row in progress_observations(events)['observations']]
        base=[event(n) for n in (1,2,4,5,6)]
        self.assertIn('repeated_equivalent_call',kind(base))
        self.assertNotIn('repeated_equivalent_call',kind([*base,edit(3)]))
        self.assertIn('repeated_equivalent_call',kind([*base,edit(3,'sibling')]))
        self.assertIn('repeated_equivalent_call',kind([*base,edit(3,result='failed')]))
        self.assertIn('repeated_equivalent_call',kind([edit(1),*base]))
        unknown=edit(3);unknown.pop('time_unix_nano')
        report=progress_observations([*base,unknown]);self.assertIn('repeated_equivalent_call',[row['kind'] for row in report['observations']])
        self.assertEqual(report['coverage']['unknown_timestamp_records'],1)
        check={**event(3),'domain':'shell','subject':'pytest tests/test_fixture.py',
            'attributes':{'tool.name':'shell','tool.arguments':{'cmd':'pytest tests/test_fixture.py','cwd':'/fixture'},'process.exit.code':0}}
        self.assertNotIn('repeated_equivalent_call',kind([*base,check]))

    def test_equal_timestamps_and_cross_owner_progress_cannot_clear_repetition(self):
        # Workflow projection keeps source roles; only exact source namespace can group.
        rows=[span(n,status='OK') for n in range(5)]
        same_time=deepcopy(rows)
        for row in same_time:row['start_unix_nano']=10**9
        self.assertEqual(analyze_patterns(same_time,[]),[])
        for row in rows:row['attributes']['session.id']='s'+str(int(row['span_id'])%2)
        self.assertEqual(analyze_patterns(rows,[]),[])
