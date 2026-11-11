"""A linked execution scope is more precise than its shared trace container."""
from copy import deepcopy
from pathlib import Path
import tempfile
import unittest
from observatory.saved_tasks import TaskStore, render_handoff, usage_summary
from observatory.workflow_evidence import ExecutionScope, ReviewHistory


class ScopedTaskLinkTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.path=Path(self.temp.name)/'tasks.db';self.store=TaskStore(self.path)

    def evidence(self, agent, *, trace='a'*32, session='shared', usage=None, parent=None):
        scope=ExecutionScope('receiver','codex',agent,session,trace,kind='run')
        history=ReviewHistory(scope)
        history.accept({'activity':[dict(id='event-'+agent,source_id='event-'+agent,service='codex',
            agent_id=agent,session_id=session,trace_id=trace,domain='shell',action='execute',
            subject='pytest '+agent,result='success',time_unix_nano=100)],'next_cursor':None})
        return dict(receiver='receiver',producer='codex',agent_id=agent,session_id=session,trace_id=trace,
                    runtime='Codex',task='Review '+agent,state='Completed',captured_ns=1000,
                    parent_agent_id=parent or '',usage=usage or {},review_snapshot=history.projection({'task':agent,'state':'Completed'}))

    def known(self, amount=100):
        return dict(input_tokens=amount,output_tokens=20,known_cost_usd=.1,
                    usage_complete=True,cost_complete=True)

    def test_shared_trace_scopes_survive_reopen_refresh_and_reverse_link_order(self):
        for child_usage in ({},self.known(30)):
            parent=self.evidence('parent',usage=self.known())
            child=self.evidence('child',usage=child_usage)
            summaries=[]
            for entries in ((parent,child),(child,parent)):
                task=self.store.create('Shared trace review')
                for evidence in entries:task=self.store.link(task['id'],evidence)
                task=TaskStore(self.path).get(task['id'])
                self.assertEqual(len(task['links']),2)
                self.assertEqual(len({link['key'] for link in task['links']}),2)
                by_agent={link['agent_id']:deepcopy(link) for link in task['links']}
                self.assertEqual(by_agent['parent']['usage']['input_tokens'],100)
                self.assertEqual(by_agent['parent']['review_snapshot'],parent['review_snapshot'])
                self.assertEqual(by_agent['child']['review_snapshot'],child['review_snapshot'])
                summary=task['usage_summary'];summaries.append(summary)
                self.assertEqual(summary['linked_runs'],2)
                self.assertTrue(summary['aggregation_ambiguous'])
                self.assertIsNone(summary['input_tokens']);self.assertIsNone(summary['known_cost_usd'])
                self.assertFalse(summary['usage_complete']);self.assertFalse(summary['cost_complete'])
                task=self.store.link(task['id'],{**parent,'captured_ns':2000,'task':'Parent refreshed'})
                self.assertEqual(len(task['links']),2)
                self.assertEqual(next(link for link in task['links'] if link['agent_id']=='child'),by_agent['child'])
                handoff=render_handoff(task)
                self.assertIn('pytest parent',handoff);self.assertIn('pytest child',handoff)
                self.assertIn('accounting may overlap',handoff)
                self.assertIn('Recorded usage for this scope only',handoff)
            self.assertEqual(summaries[0],summaries[1])

    def test_same_agent_different_sessions_keep_separate_scope_links(self):
        task=self.store.create('Two session scopes')
        for session in ('one','two'):
            task=self.store.link(task['id'],self.evidence('worker',session=session,usage=self.known()))
        self.assertEqual(len(task['links']),2)
        self.assertEqual({link['session_id'] for link in task['links']},{'one','two'})
        self.assertIsNone(task['usage_summary']['input_tokens'])

    def test_known_parent_child_separate_traces_do_not_sum_but_independent_runs_do(self):
        parent=self.evidence('parent',usage=self.known())
        child=self.evidence('child',trace='b'*32,usage=self.known(30),parent='parent')
        task=self.store.create('Known delegation')
        for evidence in (parent,child):task=self.store.link(task['id'],evidence)
        self.assertTrue(task['usage_summary']['aggregation_ambiguous'])
        self.assertEqual(task['usage_summary']['related_run_scopes'],1)
        self.assertIsNone(task['usage_summary']['input_tokens'])
        self.assertEqual(task['links'][1]['parent_agent_id'],'parent')
        task=self.store.link(task['id'],{**child,'parent_agent_id':'','captured_ns':2000})
        self.assertTrue(task['usage_summary']['aggregation_ambiguous'])
        self.assertTrue(task['links'][1]['parent_identity_retained'])
        independent=self.evidence('unrelated',trace='c'*32,usage=self.known(30))
        task=self.store.create('Independent runs')
        for evidence in (parent,independent):task=self.store.link(task['id'],evidence)
        self.assertFalse(task['usage_summary']['aggregation_ambiguous'])
        self.assertEqual(task['usage_summary']['input_tokens'],130)
        self.assertIn('missing relationships remain unknown',task['usage_summary']['scope'])

    def test_legacy_unnamespaced_trace_key_is_preserved(self):
        task=self.store.create('Legacy')
        task=self.store.link(task['id'],dict(trace_id='a'*32,session_id='old',runtime='Codex'))
        self.assertEqual(task['links'][0]['key'],'trace:'+'a'*32)
        task=self.store.link(task['id'],dict(trace_id='a'*32,session_id='old',runtime='Codex',captured_ns=10**18))
        self.assertEqual(len(task['links']),1)

    def test_old_namespaced_key_refresh_requires_the_same_recorded_scope(self):
        import hashlib
        import json
        parent=self.evidence('parent',usage=self.known())
        task=self.store.create('Existing namespaced association',link=parent)
        legacy_namespace=hashlib.sha256(b'receiver\0codex').hexdigest()[:24]
        task['links'][0]['key']='trace:'+'a'*32+'@'+legacy_namespace
        with self.store._connect() as db:
            db.execute('UPDATE saved_tasks_v1 SET links_json=? WHERE id=?',(json.dumps(task['links']),task['id']))
        task=self.store.link(task['id'],{**parent,'captured_ns':2000})
        self.assertEqual(len(task['links']),1)
        self.assertNotEqual(task['links'][0]['key'],'trace:'+'a'*32+'@'+legacy_namespace)
        task=self.store.link(task['id'],self.evidence('child',usage={}))
        self.assertEqual(len(task['links']),2)
