import tempfile
from pathlib import Path
import unittest
from observatory.core import Store

class WorkflowBackendTests(unittest.TestCase):
    def test_shared_trace_pattern_owners_are_scoped_and_counts_exclude_siblings(self):
        from observatory.core import normalize_native, _agent_id_from_attributes
        with tempfile.TemporaryDirectory() as d:
            store=Store(Path(d)/'shared.db',workflow_patterns=True)
            self.addCleanup(store.close)
            root_attrs={'agent.id':'root-worker','session.id':'shared'}
            rows=[normalize_native(dict(trace_id='a'*32,span_id='1'*16,name='agent.run',
                start_unix_nano=1,end_unix_nano=100,status='OK',attributes=root_attrs),'fixture')]
            owners=[]
            for worker,base in (('worker-two',10),('worker-three',20)):
                attrs={'agent.id':worker,'agent.parent_id':'root-worker','session.id':worker+'-session',
                       'tool.name':'build','tool.error':'repeated failure'}
                owners.append(_agent_id_from_attributes(attrs,{},'fixture'))
                for n in range(3):
                    rows.append(normalize_native(dict(trace_id='a'*32,span_id=f'{base+n:016x}',
                        parent_span_id='1'*16,name='build',start_unix_nano=base+n,
                        end_unix_nano=base+n+1,status='ERROR',attributes=attrs),'fixture'))
            store.ingest(rows)
            parent=_agent_id_from_attributes(root_attrs,{},'fixture')
            exact_parent=store.finding_page(agent_id=parent)['findings']
            self.assertFalse(any(row.get('workflow_pattern') for row in exact_parent))
            for owner in owners:
                patterns=[row for row in store.finding_page(agent_id=owner)['findings'] if row.get('workflow_pattern')]
                self.assertEqual(len(patterns),1)
                self.assertEqual(patterns[0]['agent_id'],owner)
                self.assertIn(patterns[0]['session_id'],('worker-two-session','worker-three-session'))
                self.assertEqual(patterns[0]['occurrence_count'],1)
                self.assertEqual(patterns[0]['session_count'],1)
            subtree=[row for row in store.finding_page(agent_id=parent,subtree=True)['findings'] if row.get('workflow_pattern')]
            self.assertEqual({row['agent_id'] for row in subtree},set(owners))
            self.assertTrue(all(row['occurrence_count']==2 for row in subtree))
            self.assertTrue(all(row['session_count']==2 for row in subtree))

    def test_receiver_identity_is_persisted_and_unique(self):
        with tempfile.TemporaryDirectory() as d:
            a=Store(Path(d)/'a.db'); identity=a.receiver_id;a.close()
            a=Store(Path(d)/'a.db');b=Store(Path(d)/'b.db')
            self.assertEqual(identity,a.receiver_id)
            self.assertNotEqual(identity,b.receiver_id)
            a.close();b.close()

    def test_trace_filter_intersects_session_and_pages_both_sources(self):
        from tests.test_cockpit_read_paths import CockpitReadPathTests
        fixture=CockpitReadPathTests();fixture.setUp()
        try:
            fixture.insert_logs(15)
            store=fixture.store
            with store._connect() as db:
                db.execute('UPDATE logs SET trace_id=? WHERE time_ns%2=0',('a'*32,))
                db.execute('UPDATE logs SET trace_id=? WHERE time_ns%2=1',('b'*32,))
            rows=[];cursor=None
            while True:
                page=store.activity_page(trace_id='a'*32,session_id='history',limit=2,cursor=cursor)
                rows.extend(page['activity']);cursor=page['next_cursor']
                if not cursor:break
            self.assertEqual(page['receiver_id'],store.receiver_id)
            self.assertEqual(len(rows),8)
            self.assertEqual({r['trace_id'] for r in rows},{'a'*32})
            self.assertEqual(store.activity_page(trace_id='a'*32,session_id='other')['activity'],[])
            with self.assertRaises(ValueError):store.activity_page(trace_id='invalid')
        finally: fixture.doCleanups()

    def test_review_http_reader_caps_response_before_json_decode(self):
        import io
        import json
        from unittest.mock import patch
        from observatory.terminal import Api, CockpitError
        payload=json.dumps({'data':'x'*(3*1024*1024)}).encode()
        api=Api('http://localhost:1234')
        with patch('urllib.request.urlopen',side_effect=lambda *a,**k:io.BytesIO(payload)):
            with self.assertRaisesRegex(CockpitError,'2 MiB'):
                api.get_review('/api/activity')
            self.assertEqual(len(api.get('/api/activity')['data']),3*1024*1024)

    def test_patterns_are_opt_in_and_cached_observations_hide_when_disabled(self):
        from observatory.core import normalize_native
        with tempfile.TemporaryDirectory() as d:
            path=Path(d)/'patterns.db'
            store=Store(path,workflow_patterns=True)
            rows=[]
            for n in range(3):
                rows.append(normalize_native(dict(trace_id='a'*32,span_id=f'{n+1:016x}',
                    name='tool',start_unix_nano=(n+1)*1_000_000_000,
                    end_unix_nano=(n+1)*1_000_000_000+1,status='ERROR',
                    attributes={'agent.id':'worker','tool.name':'build','tool.error':'broken'}),'fixture'))
            store.ingest(rows)
            self.assertTrue(any(f.get('workflow_pattern') for f in store.findings()))
            store.close()
            store=Store(path,workflow_patterns=False)
            self.assertFalse(any(f.get('workflow_pattern') for f in store.findings()))
            self.assertFalse(any(f.get('workflow_pattern') for f in store._trace_findings('a'*32)))
            store.close()

    def test_unchanged_background_observation_does_not_force_another_monitor_render(self):
        from types import SimpleNamespace
        from observatory.ui_workflow_controller import WorkflowController
        owner=SimpleNamespace(last_update=42,workflow_ledger=SimpleNamespace(rows=[]),screen=None)
        controller=WorkflowController(owner)
        receipt={'scope_key':'run','last_observed_time':1}
        controller._publish([],{'incident':receipt},'','')
        self.assertEqual(owner.last_update,42)
        controller._publish([],{'incident':{**receipt,'last_observed_time':2}},'','')
        self.assertEqual(owner.last_update,42)
        controller._publish([],{'incident':{**receipt,'acknowledgement':'reviewed'}},'','')
        self.assertEqual(owner.last_update,-1)
        owner.last_update=43
        controller.warning = 'Capacity reached'  # worker published state precedes UI callback
        controller._publish([],{},'Capacity reached','')
        self.assertEqual(owner.last_update,-1)

    def test_atomic_database_replacement_updates_health_and_activity_namespace(self):
        import os
        with tempfile.TemporaryDirectory() as d:
            path=Path(d)/'live.db';replacement=Path(d)/'replacement.db'
            store=Store(path);other=Store(replacement)
            before=store.receiver_namespace();after=other.receiver_id
            other.close();os.replace(replacement,path)
            self.assertNotEqual(before,after)
            self.assertEqual(store.receiver_namespace(),after)
            self.assertEqual(store.activity_page()['receiver_id'],after)
            store.close()
