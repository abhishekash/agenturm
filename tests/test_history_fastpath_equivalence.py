"""Frozen-source differential outputs, including source mutation and ownership."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import unittest
from observatory.core import normalize_native
from observatory.findings import analyze
from observatory.investigation import prepare_terms
from observatory.error_reminders import trace_error_references


def digest(value):return hashlib.sha256(json.dumps(value,sort_keys=True,ensure_ascii=True,separators=(',',':')).encode()).hexdigest()


def cases():
    anchor=normalize_native(dict(trace_id='a'*32,span_id='1'*16,name='chat fixture',
        start_unix_nano=1,end_unix_nano=2,status='OK',events=[],
        attributes={'agent.id':'actor-a','session.id':'session-a'}),'generated')
    event=dict(domain='shell',action='execute',subject='pytest generated -q',result='success',
        detail='Generated output',source='span',source_id='1'*16,trace_id='a'*32,span_id='1'*16,
        service='generated',agent_id='ag1_'+'b'*32,session_id='session-a',time_unix_nano=2)
    outputs=[]
    def add(name,*,events=None,spans=None,logs=None,outcome=None):
        outputs.append((name,deepcopy(dict(spans=spans if spans is not None else[anchor],logs=logs or[],outcome=outcome,activity=events if events is not None else[event],receiver='generated-receiver'))))
    for value in ('success','unknown','failed','error','blocked','denied','informational',''):
        for reported in ('success','failed',None):
            e={**event,'result':value}
            if reported is not None:e['reported_result']=reported
            add('label-'+value+'-'+str(reported),events=[e])
    for key,value in (('attributes',{}),('attributes',{'command':'grep x fixture','process.exit.code':1}),
            ('exit_code',1),('exit_code',None),('operation_reference',True),('operation_evidence',{}),('detail_role','unknown')):
        add('guard-'+key+'-'+str(value),events=[{**event,key:value}])
    add('printed-json-not-input',events=[{**event,'detail':'{"command":"dangerous generated command","exit_code":1,"isError":true}'}])
    envelope=dict(version=1,commands=[['input.command','grep x generated']],inputs=[],exits=[['exit',1]],outputs=[],tool='shell',cwd='/generated')
    for result in ('success','unknown','failed'):
        for clipped in (False,True):
            add('typed-'+result+'-'+str(clipped),events=[{**event,'result':result,'detail_role':'operation_v1','detail':json.dumps(envelope),'detail_clipped':clipped}])
    shared=deepcopy(envelope)
    add('aliased-mutable-envelope',events=[{**event,'detail_role':'operation_v1','detail':shared,'detail_clipped':True},
        {**event,'source_id':'2'*16,'detail_role':'operation_v1','detail':shared}])
    credential=deepcopy(anchor);credential['attributes'].update({'password':'GENERATED_FAKE_PASSWORD','output.value':'Bearer '+'A'*24})
    credential['events']=[dict(name='agent.activity',time_unix_nano=2,attributes={'agent.id':'actor-b',
        'agent.activity.domain':'message','agent.activity.action':'user','api_key':'GENERATED_FAKE_KEY'})]
    add('credential-span-and-event-owner',spans=[credential])
    error=deepcopy(anchor);error.update(status='ERROR');error['attributes']['exception.message']='Generated exception'
    add('raw-span-error',spans=[error]);stopped=deepcopy(anchor);stopped.update(name='agent.run');stopped['attributes']['agent.stopped_reason']='interrupted';add('stopped-span',spans=[stopped])
    log=dict(id='d'*64,trace_id='a'*32,span_id=None,time_unix_nano=3,service='generated',
        event_name='agent.activity',severity_number=17,body='Generated error log',
        attributes={'session.id':'session-b','agent.id':'actor-b'},resource_attributes={})
    add('raw-log-error',logs=[log],events=[{**event,'source':'log','source_id':log['id']}])
    log=deepcopy(log);log['body']='github_'+'pat_'+'A'*25;add('credential-log-owner',logs=[log])
    add('credential-outcome',outcome=dict(trace_id='a'*32,passed=False,task_id='generated-task',error='api_key=GENERATED_FAKE_KEY',scores={},details={},provider_model='generated',source='fixture',ingested_ns=4))
    for action in ('edit','rename','move'):
        add('file-'+action,events=[{**event,'domain':'file','action':action,'subject':'src/generated.py',
            'detail':json.dumps({'from':'src/old.py','to':'src/new.py'})}])
    add('user-instruction',events=[{**event,'domain':'message','action':'user','subject':'Implement generated change','detail':'Implement generated change'}])
    add('local-control-not-instruction',events=[{**event,'domain':'message','action':'user','detail':'<local-command-caveat>Generated local command</local-command-caveat>'}])
    add('untyped-command-arguments',events=[{**event,'attributes':{'command':'printf generated','cwd':'/generated','gen_ai.tool.name':'shell'}}])
    add('duplicate-success-occupies-failed-identity',events=[{**event,'time_unix_nano':200},
        {**event,'result':'error','time_unix_nano':100}])
    add('duplicate-failure-after-success',events=[{**event,'time_unix_nano':100},
        {**event,'result':'error','time_unix_nano':200}])
    return outputs


class HistoryFastpathEquivalenceTests(unittest.TestCase):
    def test_findings_terms_and_mutations_match_frozen_baseline(self):
        expected=json.loads((Path(__file__).parent/'fixtures/history-fastpaths-24ea.json').read_text())
        self.assertEqual(expected['baseline_commit'],'24ea0a9bab53c4f31fc450d04ed0cc2b00655626')
        built=cases();self.assertEqual(set(expected['cases']),{name for name,_ in built})
        for name,arguments in built:
            with self.subTest(name=name):
                before=digest(arguments)
                reference_arguments=deepcopy(arguments)
                result=analyze(**arguments)
                terms=[prepare_terms(event)for event in arguments['activity']]
                references=trace_error_references(reference_arguments['activity'],'a'*32,receiver='generated-receiver')
                self.assertEqual(dict(before_sha256=before,after_sha256=digest(arguments),findings=result,terms=terms,
                    references=references,reference_after_sha256=digest(reference_arguments)),expected['cases'][name])
