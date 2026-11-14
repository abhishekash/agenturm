"""Reported source contributions qualify existing observations, never infer waste."""
from copy import deepcopy
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from observatory.core import Store,normalize_native,_activity_for,_agent_id_from_attributes,summarize
from observatory.evidence_explanations import progress_observations,reported_usage_context
from observatory.workflow_patterns import analyze_patterns

RX='receiver'
ATTRS={'agent.id':'worker','session.id':'session'}
ACTOR=_agent_id_from_attributes(ATTRS,{},'fixture')


def operation(n,*,stamp=None,success=False,actor=ACTOR):
    return dict(id='operation-'+str(n),source_id=f'{n:016x}',source='span',span_id=f'{n:016x}',
        receiver_id=RX,service='fixture',agent_id=actor,session_id='session',trace_id='a'*32,
        time_unix_nano=n*10**9 if stamp is None else stamp,domain='shell',action='execute',result='success' if success else 'error',
        attributes={'tool.name':'Bash','process.command_line':'python build.py','process.working_directory':'/fixture',
            'process.exit.code':0 if success else 1,'tool.error':'' if success else 'Fixture build rejected'})


def model(n,start,end,*,inputs=10,outputs=2,parent=None,**extra):
    attrs={**ATTRS,'gen_ai.usage.input_tokens':inputs,'gen_ai.usage.output_tokens':outputs}
    attrs.update(extra.pop('attributes',{}))
    return dict(trace_id='a'*32,span_id=f'{n:016x}',parent_span_id=parent,name='model.generate',service='fixture',
        receiver_id=RX,status='OK',start_unix_nano=start*10**9,end_unix_nano=end*10**9,
        attributes=attrs,resource_attributes={},events=[],links=[],**extra)


def observation():return progress_observations([operation(n) for n in (1,5,9)],receiver=RX)['observations'][0]


class ReportedUsageProgressTests(unittest.TestCase):
    def context(self,spans,events=None,**kwargs):
        return reported_usage_context(spans,[operation(n) for n in (1,5,9)] if events is None else events,
            observation(),receiver=RX,source_complete=True,**kwargs)

    def test_distinct_completed_contributions_match_existing_leaf_accounting(self):
        spans=[model(101,2,3,inputs=10,outputs=2),model(102,6,7,inputs=20,outputs=3)]
        before=deepcopy(spans);context=self.context(spans)
        self.assertTrue(context['available']);self.assertEqual((context['input_tokens'],context['output_tokens']),(30,5))
        summary=summarize(spans)
        self.assertEqual((context['input_tokens'],context['output_tokens']),(summary['input_tokens'],summary['output_tokens']))
        self.assertEqual(context['source_records'],2);self.assertEqual(spans,before)
        self.assertEqual({row['id'] for row in context['references']},{f'{n:016x}' for n in (101,102)})
        self.assertFalse(context['attention']);self.assertIn('not cumulative-counter deltas',context['note'])

    def test_wrapper_input_and_output_leaves_are_independent(self):
        wrapper=model(101,2,7,inputs=100,outputs=50)
        child=model(102,3,6,inputs=100,outputs=0,parent=wrapper['span_id'])
        child['attributes'].pop('gen_ai.usage.output_tokens')
        context=self.context([wrapper,child]);self.assertTrue(context['available'])
        expected=summarize([wrapper,child])
        self.assertEqual((context['input_tokens'],context['output_tokens']),(100,50))
        self.assertEqual((context['input_tokens'],context['output_tokens']),(expected['input_tokens'],expected['output_tokens']))

    def test_partial_foreign_unknown_and_accounting_uncertainty_are_unavailable(self):
        base=[model(101,2,3),model(102,6,7)]
        self.assertFalse(reported_usage_context(base,[],observation(),receiver=RX)['available'])
        for mutate in (
            lambda rows:rows[0].update(receiver_id='foreign'),
            lambda rows:rows[0]['attributes'].update({'agent.id':{}}),
            lambda rows:rows[0]['attributes'].update({'gen_ai.usage.input_tokens':True}),
            lambda rows:rows[0]['attributes'].update({'gen_ai.usage.output_tokens':float('inf')}),
            lambda rows:rows[0]['attributes'].update({'llm.usage.input_tokens':99}),
            lambda rows:rows[0].update(parent_span_id='f'*16),
            lambda rows:rows[0].update(parent_span_id=rows[0]['span_id']),
            lambda rows:rows[0].update(status='RUNNING'),
            lambda rows:rows[0].update(capture_content=False),
            lambda rows:rows[0].update(start_unix_nano=0),
            lambda rows:rows[0].update(end_unix_nano=9*10**9),
            lambda rows:rows[0].update(start_unix_nano=1*10**9),
        ):
            rows=deepcopy(base);mutate(rows)
            with self.subTest(rows=rows):
                result=self.context(rows);self.assertFalse(result['available']);self.assertTrue(result['reason'])
        cross=deepcopy(base);cross[1]['parent_span_id']=cross[0]['span_id'];cross[1]['attributes']['agent.id']='sibling'
        self.assertFalse(self.context(cross)['available'])

    def test_cross_trace_turn_sources_and_unbounded_inputs_are_unavailable(self):
        spans=[model(101,2,3),model(102,6,7)]
        observed=observation();observed['scope']=(*observed['scope'][:4],('turn','turn'))
        for row in spans:row['attributes']['agent.turn.id']='turn'
        spans[1]['trace_id']='c'*32
        self.assertFalse(reported_usage_context(spans,[],observed,receiver=RX,source_complete=True)['available'])
        self.assertFalse(self.context([model(n,2,3) for n in range(501)])['available'])
        equal=observation()
        for ref in equal['references']:ref['time_unix_nano']=1
        self.assertFalse(reported_usage_context(spans,[],equal,receiver=RX,source_complete=True)['available'])

    def test_progress_or_equal_boundary_timing_cannot_be_claimed_absent(self):
        spans=[model(101,2,3),model(102,6,7)]
        checked=operation(4,success=True);checked['attributes']['process.command_line']='pytest tests'
        for changed in (
            {**operation(4),'domain':'file','action':'edit','result':'success'},
            {**operation(4),'domain':'file','action':'edit','result':'unknown'},
            {**operation(1),'domain':'file','action':'write','result':'success'},
            {**operation(4),'time_unix_nano':None,'domain':'file','action':'edit'},
            checked,
        ):
            self.assertFalse(self.context(spans,[operation(n) for n in (1,5,9)]+[changed])['available'])
        failed_check=operation(4);failed_check['attributes']['process.command_line']='pytest tests'
        self.assertTrue(self.context(spans,[operation(n) for n in (1,5,9)]+[failed_check])['available'])
        foreign={**operation(4),'agent_id':'sibling','domain':'file','action':'edit','result':'success'}
        self.assertTrue(self.context(spans,[operation(n) for n in (1,5,9)]+[foreign])['available'])

    def test_later_success_observation_has_failed_and_actual_success_sources(self):
        failed=operation(1);success=operation(2,success=True)
        result=progress_observations([failed,success],receiver=RX)['observations']
        later=next(row for row in result if row['kind']=='later_matching_success')
        self.assertEqual({row['source_id'] for row in later['references']},{failed['source_id'],success['source_id']})
        self.assertFalse(later['attention'])
        checked=[deepcopy(failed),deepcopy(success)]
        for row in checked:row['attributes']['process.command_line']='pytest tests'
        self.assertTrue(any(row['kind']=='later_matching_success' for row in progress_observations(checked,receiver=RX)['observations']))
        many=[operation(n) for n in range(1,31)]+[operation(31,success=True)]
        bounded=next(row for row in progress_observations(many,receiver=RX)['observations'] if row['kind']=='later_matching_success')
        self.assertEqual(len(bounded['references']),20)
        self.assertEqual(bounded['references'][-1]['source_id'],operation(31)['source_id'])
        for changed in ({**success,'time_unix_nano':failed['time_unix_nano']},{**success,'agent_id':'sibling'},
                        {**success,'session_id':'other'},{**success,'receiver_id':'foreign'}):
            self.assertFalse(any(row['kind']=='later_matching_success' for row in progress_observations([failed,changed],receiver=RX)['observations']))

    def test_optin_patterns_emit_usage_context_without_a_new_always_on_kind(self):
        spans=[]
        for n,stamp in ((1,1),(2,5),(3,9)):
            row=operation(n,stamp=stamp*10**9)
            spans.append(dict(trace_id=row['trace_id'],span_id=row['span_id'],parent_span_id=None,name='build',status='ERROR',
                start_unix_nano=row['time_unix_nano'],end_unix_nano=row['time_unix_nano'],service='fixture',receiver_id=RX,
                attributes={**ATTRS,**row['attributes']},resource_attributes={},events=[],links=[]))
        spans.extend([model(101,2,3),model(102,6,7)])
        findings=analyze_patterns(spans,[],receiver=RX,source_complete=True)
        repeated=next(row for row in findings if row['kind']=='repeated_tool_error')
        self.assertTrue(repeated['reported_usage_context']['available'])
        self.assertIn('reported input',repeated['detail']);self.assertFalse(repeated['attention'])
        self.assertFalse(any(row['kind']=='usage_growth' for row in findings))
        self.assertTrue({f'{n:016x}' for n in (101,102)}<={row['id'] for row in repeated['evidence']})
        incomplete=analyze_patterns(spans,[],receiver=RX)
        self.assertFalse(next(row for row in incomplete if row['kind']=='repeated_tool_error')['reported_usage_context']['available'])

    def test_actual_store_later_success_is_optin_source_backed_and_not_attention(self):
        with TemporaryDirectory() as directory:
            store=Store(Path(directory)/'events.db',workflow_patterns=True);self.addCleanup(store.close)
            rows=[]
            for n,passed in ((1,False),(2,True)):
                event=operation(n,success=passed)
                rows.append(normalize_native(dict(trace_id=event['trace_id'],span_id=event['span_id'],name='build',
                    start_unix_nano=event['time_unix_nano'],end_unix_nano=event['time_unix_nano']+1,status='OK' if passed else 'ERROR',
                    attributes={**ATTRS,**event['attributes']}),'fixture'))
            store.ingest(rows)
            findings=[row for row in store.findings() if row['kind']=='later_matching_success']
            self.assertEqual(len(findings),1);self.assertFalse(findings[0]['attention']);self.assertEqual(findings[0]['severity'],'info')
            self.assertEqual({row['id'] for row in findings[0]['evidence']},{f'{n:016x}' for n in (1,2)})
            self.assertEqual(len(store.finding_source(findings[0]['id'])['sources']),2)
