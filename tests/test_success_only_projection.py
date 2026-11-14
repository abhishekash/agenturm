"""Success-only work reduction preserves complete frozen legacy projections."""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from observatory import findings,operation_facts,error_reminders
from observatory.operation_preparation import OperationPreparation
from tests.test_operation_facts import event

FIXTURE=Path(__file__).parent/'fixtures/success_projection_8ab.json'


def canonical(number=0,**changes):
    row=event(id=str(number),source_id=str(number),source='log',result='success',time_unix_nano=number+1)
    evidence=row.pop('operation_evidence')
    evidence.update(commands=[['native.command','pytest case'+str(number)]],
        inputs=[['native.input',{'cmd':'pytest case'+str(number),'cwd':'/work'}]],
        exits=[['native.exit_code',0]],outputs=[['native.output','output','Generated check passed']])
    row.update(detail_role='operation_v1',detail=json.dumps(evidence));row.update(changes)
    return row


def evidence_change(row,**changes):
    detail=json.loads(row['detail']);detail.update(changes);row['detail']=json.dumps(detail);return row


def cases():
    many=[canonical(n)for n in range(36)]
    tied=[canonical(n,time_unix_nano=10)for n in range(25)]
    duplicate=deepcopy(tied[0]);duplicate.update(id='z-last',source_id='z-last')
    tied.insert(2,duplicate)
    missing=[canonical(n)for n in range(10)]
    for row,field in zip(missing,('service','agent_id','session_id','trace_id','turn_id','time_unix_nano','source_id')):row.pop(field,None)
    missing[7]['time_unix_nano']=True;missing[8]['source_id']=17;missing[9]['time_unix_nano']='9'
    foreign=[canonical(0,receiver_id='foreign',trace_id='foreign',turn_id='foreign-turn'),
        canonical(1,receiver_id='receiver',trace_id='other',turn_id='our-turn')]
    clipped=[evidence_change(canonical(n),outputs=[['output','output','x'*3000]],output_clipped=True)for n in range(24)]
    secret=[evidence_change(canonical(n),outputs=[['output','output','{"password":"GENERATED_FIXTURE_VALUE"}']])for n in range(24)]
    secret_input=[evidence_change(canonical(n),inputs=[['input',{'cmd':'pytest tests','password':'GENERATED_FIXTURE_VALUE'}]])for n in range(24)]
    one=[]
    for n,domain in enumerate(('shell','mcp','tool','message')):
        row=evidence_change(canonical(n,domain=domain,result='error'),exits=[['exit',1]])
        if domain=='shell':row=evidence_change(row,commands=[['command','rg missing src']],inputs=[])
        one.append(row)
    compact=canonical(1,operation_reference=True,operation_version=1,operation_result='failed',
        operation_fingerprint='a'*64,error_fingerprint='b'*64)
    conflict=[canonical(0,result='incomplete'),canonical(1,reported_result='error'),
        canonical(2,reported_result=None),canonical(3,reported_result=''),
        evidence_change(canonical(4),exits=[['exit',0],['exit',1]])]
    mutable=json.loads(canonical(0)['detail'])
    aliased=[canonical(0,detail=mutable,detail_clipped=True),canonical(1,detail=mutable)]
    malformed=[canonical(0,detail='not JSON'),canonical(1,detail='{"version":true}'),
        canonical(2,detail='{"version":1,"exits":"bad"}'),
        canonical(3,detail='{"version":1,"exits":[["exit",false]],"outputs":[["out","output",NaN]]}'),
        canonical(4,detail='{"version":1,"commands":[["cmd",false]],"exits":[["exit",0]],"outputs":[["out","output",Infinity]]}')]
    return {'many':many,'tied-duplicates':tied,'missing':missing,'foreign-wrapper':foreign,
        'duplicate-source-different-key':[canonical(0,source_id='same'),canonical(1,source_id='same')],
        'secret-input':secret_input,
        'clipped-output':clipped,'secret-output':secret,'code-one-all-domains':one,
        'compact-contradiction':[canonical(0),compact],'raw-and-reported-conflict':conflict,
        'aliased-mutable-detail':aliased,'malformed-nonfinite':malformed,
        'late-noncanonical':[canonical(0),event(id='late',source_id='late')],
        'explicit-mutable-evidence':[event(id='explicit',source_id='explicit')],
        'empty':[],'no-source':[canonical(0,source_id=None)]}


def project(rows,mode,*,analyzer=None,references=None):
    analyzer=analyzer or findings.analyze;references=references or error_reminders.trace_error_references
    # Preserve shared aliases by copying the complete list once, not each row.
    rows=deepcopy(rows)
    logs=[dict(trace_id='trace',id='raw-sdk',service='codex',attributes={'password':'GENERATED_FIXTURE_VALUE'},
        body='Generated raw log failure',severity_number=17,time_unix_nano=1)]
    spans=[dict(trace_id='trace',span_id='raw-sdk-span',service='codex',name='Generated SDK operation',status='ERROR',
        attributes={'exception.message':'Generated unmapped SDK failure'},events=[],end_unix_nano=2)]
    context=OperationPreparation()if mode!='none'else None
    if context:
        context.bind(rows)
        if mode=='prepopulated':
            for row in rows:context.full(row)
    try:
        return dict(findings=analyzer(spans,logs,activity=rows,receiver='receiver',operation_preparation=context),
            references=references(rows,'wrapper-trace',receiver='wrapper-receiver',operation_preparation=context))
    finally:
        if context:context.clear()


class SuccessOnlyProjectionTests(unittest.TestCase):
    def test_complete_serialized_findings_and_references_match_frozen_8ab(self):
        frozen=json.loads(FIXTURE.read_text())
        for name,rows in cases().items():
            for mode in ('none','empty','prepopulated'):
                with self.subTest(case=name,context=mode):
                    actual=json.loads(json.dumps(project(rows,mode),sort_keys=True))
                    self.assertEqual(actual,frozen['projections'][name][mode])

    def test_full_fact_budget_and_output_clipping_does_not_remove_success(self):
        rows=[canonical(n)for n in range(100)];original=operation_facts.operation_facts
        for mode in ('none','empty'):
            with patch.object(operation_facts,'operation_facts',wraps=original)as full:
                result=project(rows,mode)
            self.assertEqual(len(result['references']),20)
            self.assertEqual(full.call_count,20)
        for name in ('clipped-output','secret-output'):
            result=project(cases()[name],'empty');self.assertEqual(len(result['references']),20)
        self.assertEqual([row['source_id']for row in project(cases()['tied-duplicates'],'none')['references']][:3],['0','1','2'])

    def test_compact_and_mutable_shapes_fall_back_before_any_normalization(self):
        original=error_reminders._trace_error_references
        for name in ('compact-contradiction','aliased-mutable-detail','late-noncanonical','missing','explicit-mutable-evidence'):
            rows=deepcopy(cases()[name])
            with patch('observatory.operation_preparation.canonical_terminal',side_effect=AssertionError('No preflight normalization')),\
                    patch.object(error_reminders,'_trace_error_references',wraps=original)as legacy:
                error_reminders.trace_error_references(rows,'trace',receiver='receiver')
            self.assertEqual(legacy.call_count,1,name)

    def test_real_store_derivation_retains_twenty_successes_with_twenty_full_facts(self):
        from observatory.core import Store,normalize_native
        with tempfile.TemporaryDirectory()as directory:
            store=Store(Path(directory)/'generated.db')
            try:
                records=[normalize_native(dict(trace_id='a'*32,span_id=f'{number+1:016x}',name='exec_command',
                    start_unix_nano=number+1,end_unix_nano=number+2,status='OK',events=[],attributes={
                        'agent.id':'fixture-worker','session.id':'fixture-session','tool.name':'exec_command',
                        'tool.command':'pytest generated'+str(number),'tool.arguments':json.dumps({'cmd':'pytest generated'+str(number),'cwd':'/work'}),
                        'process.working_directory':'/work','process.exit.code':0,'output.value':'Generated check passed'}),'fixture')
                    for number in range(100)]
                with patch.object(operation_facts,'operation_facts',wraps=operation_facts.operation_facts)as full:
                    store.ingest(records)
                self.assertEqual(full.call_count,20)
                with store._read()as db:
                    row=db.execute('SELECT activity_json,error_refs_json FROM trace_summaries WHERE trace_id=?',('a'*32,)).fetchone()
                self.assertEqual(len(json.loads(row[0])),100);self.assertEqual(len(json.loads(row[1])),20)
            finally:store.close()

    def test_input_objects_are_unchanged_on_success_or_uncertain_gate(self):
        for name in ('many','clipped-output','malformed-nonfinite','compact-contradiction'):
            rows=cases()[name];before=deepcopy(rows)
            error_reminders.trace_error_references(rows,'trace',receiver='receiver')
            self.assertEqual(rows,before,name)
        rows=[canonical(0)]
        with patch('observatory.operation_preparation.canonical_terminal',side_effect=ValueError('Synthetic gate failure')):
            self.assertEqual(error_reminders.trace_error_references(rows,'trace',receiver='receiver'),
                error_reminders._trace_error_references(rows,'trace',receiver='receiver'))
        # Genuine retained full-fact/context failures must never be retried.
        for function in (error_reminders.trace_error_references,error_reminders._trace_error_references):
            context=OperationPreparation();context.bind(rows)
            with patch.object(context,'correlation',side_effect=[RuntimeError('Retained fact failure'),{}])as called:
                with self.assertRaisesRegex(RuntimeError,'Retained fact failure')as raised:
                    function(rows,'trace',receiver='receiver',operation_preparation=context)
                self.assertEqual(called.call_count,1)
                self.assertEqual(dict(type=type(raised.exception).__name__,message=str(raised.exception)),
                    json.loads(FIXTURE.read_text())['exceptions']['context-failure'])
        # Preserve the original public exception rather than an empty result.
        for function in (error_reminders.trace_error_references,error_reminders._trace_error_references):
            for name,events,trace,kind in (('missing-trace',[canonical(0)],None,TypeError),
                    ('missing-event',[None],'trace',AttributeError)):
                with self.assertRaises(kind)as raised:function(events,trace,receiver='receiver')
                self.assertEqual(dict(type=type(raised.exception).__name__,message=str(raised.exception)),
                    json.loads(FIXTURE.read_text())['exceptions'][name])


if __name__=='__main__':unittest.main()
