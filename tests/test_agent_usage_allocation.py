"""Whole-trace accounting is not attributed to a worker without recorded allocation."""
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from observatory.core import Store,normalize_native
from observatory.agent_hierarchy import canonical_agent_id
from observatory.ui_monitoring import task_run_usage,task_usage_summary
from observatory.workflow_evidence import ExecutionScope,ReviewHistory


class AgentAllocationTests(unittest.TestCase):
    def test_shared_parent_child_usage_stays_trace_wide(self):
        with TemporaryDirectory() as tmp:
            store=Store(Path(tmp)/'shared.db');self.addCleanup(store.close)
            trace='a'*32
            spans=[]
            for native,identity,parent,tokens in [('parent','1'*16,None,100),('child','2'*16,'1'*16,200)]:
                attrs={'agent.id':native,'agent.runtime':'SDK','agent.namespace':'svc','session.id':'shared',
                    'gen_ai.usage.input_tokens':tokens}
                if parent:attrs['agent.parent_id']='parent'
                spans.append(normalize_native(dict(trace_id=trace,span_id=identity,parent_span_id=parent,
                    name='model',start_unix_nano=1,end_unix_nano=2,attributes=attrs,status='OK'),'svc'))
            store.ingest(spans)
            detail=store.trace(trace)['summary']
            self.assertEqual(detail['agent_count'],2)
            self.assertEqual(detail['input_tokens'],200)  # Existing wrapper/leaf accounting remains trace-wide.
            sessions=store.sessions()
            for native in ('parent','child'):
                agent=canonical_agent_id('SDK','svc',native)
                selected=dict(agent_id=agent,service='svc',session_id='shared',trace_id=trace)
                usage=task_run_usage(sessions,selected,[detail])
                self.assertNotIn('input_tokens',usage)
                if native=='parent':self.assertIn('shared trace',task_usage_summary(usage)[0])
            scope=ExecutionScope(store.receiver_id,'svc',canonical_agent_id('SDK','svc','parent'),'shared',trace,kind='run')
            projection=ReviewHistory(scope).projection({'_review_usage':detail})
            self.assertEqual(projection['usage']['values'],{})
            self.assertEqual(projection['usage']['scope'],'unavailable')

    def test_legacy_allocation_uses_metadata_count_but_does_not_claim_single_agent_proof(self):
        with TemporaryDirectory() as tmp:
            store=Store(Path(tmp)/'legacy.db');self.addCleanup(store.close)
            trace='b'*32;agent='ag1_'+'c'*32
            store.ingest([normalize_native(dict(trace_id=trace,span_id='1'*16,name='model',
                start_unix_nano=1,end_unix_nano=2,attributes={'agent.id':agent,'session.id':'session',
                    'gen_ai.usage.input_tokens':20}),service='svc')])
            current=store.sessions()[0]
            selected=dict(agent_id=agent,service='svc',session_id='session',trace_id=trace)
            self.assertEqual(task_run_usage([current],selected)['input_tokens'],20)
            with store._connect() as db:
                raw=json.loads(db.execute('SELECT summary_json FROM trace_summaries').fetchone()[0])
                raw.pop('agent_count');raw.pop('agent_count_complete')
                db.execute('UPDATE trace_summaries SET summary_json=?',(json.dumps(raw),))
            legacy=store.sessions()[0]['latest_run']
            self.assertEqual(legacy['agent_count'],1)
            self.assertFalse(legacy['agent_count_complete'])
            guarded=task_run_usage(store.sessions(),selected)
            self.assertNotIn('input_tokens',guarded)
            self.assertIn('unverified',guarded['usage_allocation'])

    def test_event_recorded_second_agent_prevents_parent_allocation(self):
        with TemporaryDirectory() as tmp:
            store=Store(Path(tmp)/'events.db');self.addCleanup(store.close)
            store.ingest([normalize_native(dict(trace_id='d'*32,span_id='1'*16,name='model',
                start_unix_nano=1,end_unix_nano=2,attributes={'agent.id':'parent','agent.runtime':'SDK',
                    'agent.namespace':'svc','session.id':'session','gen_ai.usage.input_tokens':20},
                events=[{'name':'agent.activity','time_unix_nano':2,'attributes':{
                    'agent.id':'child','agent.activity.domain':'shell','agent.activity.subject':'pytest'}}]),'svc')])
            self.assertEqual(store.trace('d'*32)['summary']['agent_count'],2)

    def test_sampled_single_owner_does_not_prove_allocation(self):
        with TemporaryDirectory() as tmp:
            store=Store(Path(tmp)/'sampled.db');self.addCleanup(store.close)
            trace='e'*32;agent='ag1_'+'c'*32
            spans=[]
            for index in range(259):
                spans.append(normalize_native(dict(trace_id=trace,span_id=f'{index+1:016x}',
                    parent_span_id=None if index==0 else f'{1:016x}',name='model',
                    start_unix_nano=index+1,end_unix_nano=index+2,
                    attributes={'agent.id':agent,'session.id':'session','gen_ai.usage.input_tokens':1}), 'svc'))
            store.ingest(spans)
            latest=store.sessions()[0]['latest_run']
            self.assertEqual(latest['agent_count'],1)
            self.assertFalse(latest['agent_count_complete'])
            self.assertNotIn('input_tokens',task_run_usage(store.sessions(),dict(
                agent_id=agent,service='svc',session_id='session',trace_id=trace)))

    def test_log_owner_is_counted_in_cached_summary(self):
        with TemporaryDirectory() as tmp:
            store=Store(Path(tmp)/'logs.db');self.addCleanup(store.close)
            trace='f'*32;agent='ag1_'+'c'*32
            store.ingest([normalize_native(dict(trace_id=trace,span_id='1'*16,name='model',
                start_unix_nano=1,end_unix_nano=2,attributes={'agent.id':agent,
                    'session.id':'session','gen_ai.usage.input_tokens':20}), 'svc')])
            store.ingest_logs([dict(trace_id=trace,span_id=None,time_unix_nano=3,service='svc',
                severity_number=9,severity_text='INFO',event_name='agent.activity',body='output',
                resource_attributes={},scope_name='fixture',attributes={
                    'agent.id':'ag1_'+'d'*32,'session.id':'session'})])
            self.assertEqual(store.sessions()[0]['latest_run']['agent_count'],2)
