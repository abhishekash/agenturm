"""Findings retain exact occurrence ownership, independent of shared trace roots."""
from contextlib import contextmanager
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from observatory.core import Store,normalize_native
from observatory.agent_hierarchy import canonical_agent_id
from tests.test_agent_hierarchy import node


class OwnedFindingTests(unittest.TestCase):
    def setUp(self):
        self.tmp=TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.store=Store(Path(self.tmp.name)/'source.db');self.addCleanup(self.store.close)
        self.trace='a'*32
        self.parent=node('parent');self.child=node('child','parent')
        self.store.upsert_agent_nodes([self.parent,self.child])

    def span(self,index,owner=None,session=None,**changes):
        attrs={'tool.name':'Bash','gen_ai.tool.type':'shell','process.command_line':'python migrate.py',
            'process.working_directory':'/work','process.exit.code':1,'stderr':'Migration failed'}
        if owner:attrs['agent.id']=owner
        if session:attrs['session.id']=session
        attrs.update(changes.pop('attributes',{}))
        row=dict(trace_id=self.trace,span_id=f'{index:016x}',name='tool',status='UNSET',
            start_unix_nano=index*10,end_unix_nano=index*10+1,attributes=attrs,events=[])
        row.update(changes);return normalize_native(row,'fixture')

    def rows(self,**scope):return self.store.finding_page(**scope)['findings']

    def test_shared_trace_rows_and_counts_follow_exact_owner_without_root_context(self):
        self.store.ingest([self.span(1,self.parent['agent_id'],'parent',attributes={'agent.task':'ROOT PRIVATE TASK'}),
            self.span(2,self.child['agent_id'],'child'),self.span(3)])
        all_rows=self.rows();self.assertEqual(len(all_rows),3)
        for item in all_rows:
            self.assertEqual(item['task'],'');self.assertEqual(item['run_status'],'UNKNOWN')
            self.assertNotIn('ROOT PRIVATE TASK',json.dumps(item))
            self.assertEqual(item['occurrence_count'],1)
        exact=self.rows(agent_id=self.parent['agent_id'])
        self.assertEqual(len(exact),1);self.assertEqual(exact[0]['session_id'],'parent')
        subtree=self.rows(agent_id=self.parent['agent_id'],subtree=True)
        self.assertEqual({item['agent_id'] for item in subtree},{self.parent['agent_id'],self.child['agent_id']})
        self.assertTrue(any(item['agent_id'] is None for item in all_rows))

    def test_qualified_repeated_inputs_group_but_incomplete_or_changed_inputs_do_not(self):
        owner=self.parent['agent_id']
        self.store.ingest([self.span(1,owner,'session'),self.span(2,owner,'session'),
            self.span(3,owner,'session',attributes={'process.working_directory':'/elsewhere'}),
            self.span(4,owner,'session',attributes={'process.command_line':'python other.py'}),
            self.span(5,owner,'session',attributes={'input.value':{'password':'PRIVATE_SECRET'}}),
            self.span(6,owner,'session',attributes={'input.value':{'password':'PRIVATE_SECRET'}})])
        rows=[item for item in self.rows() if item['kind']=='span_error']
        matching=[item for item in rows if item['exit_code']==1 and item['occurrence_count']==2]
        self.assertEqual(len(matching),2)
        self.assertEqual({item['occurrence_count'] for item in rows},{1,2})
        self.assertNotIn('PRIVATE_SECRET',json.dumps(rows))
        first=self.store.finding_page(limit=1)
        second=self.store.finding_page(limit=1,cursor=first['next_cursor'])
        self.assertTrue(first['has_more']);self.assertNotEqual(first['findings'][0]['id'],second['findings'][0]['id'])

    def test_expected_nonzero_and_conflicts_retain_shared_classification(self):
        owner=self.parent['agent_id']
        self.store.ingest([self.span(1,owner,'session',attributes={'process.command_line':'rg absent src'}),
            self.span(2,owner,'session',attributes={'process.command_line':'git diff --check'}),
            self.span(3,owner,'session',attributes={'process.command_line':'rg absent src; python broken.py'}),
            self.span(4,owner,'session',status='OK')])
        rows={item['evidence'][0]['id']:item for item in self.rows()}
        self.assertEqual(rows[f'{1:016x}']['severity'],'info')
        self.assertEqual(rows[f'{2:016x}']['severity'],'high')
        self.assertEqual(rows[f'{3:016x}']['severity'],'high')
        self.assertEqual(rows[f'{4:016x}']['result_basis'],'conflicting recorded results')

    def test_raw_errors_and_exact_event_credentials_survive_without_activity_mapping(self):
        parent=self.parent['agent_id'];child=self.child['agent_id']
        raw=self.span(1,parent,'parent',name='sdk.internal',status='ERROR',attributes={'tool.name':'','gen_ai.tool.type':'',
            'process.command_line':'','process.exit.code':None,'api_key':'PARENT_PRIVATE_VALUE'},events=[
                {'name':'custom','time_unix_nano':12,'attributes':{'agent.id':child,'session.id':'child',
                    'api_key':'CHILD_PRIVATE_VALUE'}}])
        self.store.ingest([raw])
        self.store.ingest_logs([dict(trace_id=self.trace,span_id=None,time_unix_nano=30,service='fixture',
            scope_name='fixture',event_name='sdk.error',body='Raw standalone error',severity_number=17,
            severity_text='ERROR',attributes={},resource_attributes={})])
        rows=self.rows();self.assertTrue(any(item['kind']=='error_log' for item in rows))
        self.assertTrue(any(item['kind']=='span_error' for item in rows))
        credentials=[item for item in rows if item['kind']=='credential_exposure']
        self.assertEqual({item['agent_id'] for item in credentials},{parent,child})
        selected=self.rows(agent_id=child)
        self.assertEqual(len(selected),1);self.assertEqual(selected[0]['session_id'],'child')
        self.assertEqual(selected[0]['evidence'][0]['id'],f'{1:016x}:event:0')
        self.assertNotIn('PRIVATE_VALUE',json.dumps(rows))

    def test_rows_counts_scope_and_receiver_share_one_wal_snapshot(self):
        owner=self.parent['agent_id'];self.store.ingest([self.span(1,owner,'session')])
        original=self.store._connect;fired=[]
        @contextmanager
        def connection():
            with original() as db:
                def interleave(sql):
                    if 'SELECT * FROM finding_cache' not in sql or fired:return
                    fired.append(True)
                    with original() as writer:
                        writer.execute('DELETE FROM finding_cache')
                        writer.execute("UPDATE schema_meta SET value='different' WHERE key='receiver_id'")
                db.set_trace_callback(interleave)
                yield db
        with patch.object(self.store,'_connect',connection):
            page=self.store.finding_page(agent_id=owner)
        self.assertTrue(fired);self.assertEqual(len(page['findings']),1)
        self.assertEqual(page['findings'][0]['occurrence_count'],1)
        self.assertNotEqual(page['receiver_id'],'different')
        self.assertEqual(self.rows(),[])

    def test_legacy_four_column_writes_remain_unknown_global_occurrences(self):
        legacy=dict(id='f'*16,trace_id=self.trace,kind='span_error',title='Legacy failure',detail='Old text',evidence=[])
        with self.store._connect() as db:
            db.execute('INSERT INTO finding_cache(finding_id,trace_id,time_ns,finding_json) VALUES(?,?,?,?)',
                (legacy['id'],self.trace,1,json.dumps(legacy)))
        rows=self.rows();self.assertEqual(len(rows),1)
        self.assertEqual(rows[0]['occurrence_count'],1);self.assertIsNone(rows[0]['agent_id'])
        self.assertEqual(self.rows(agent_id=self.parent['agent_id']),[])

    def test_external_source_updates_invalidate_before_bounded_repair(self):
        self.store.ingest([self.span(1,self.parent['agent_id'],'parent')])
        with self.store._connect() as db:
            row=db.execute('SELECT attributes FROM spans').fetchone()
            attrs=json.loads(row[0]);attrs.update({'agent.id':self.child['agent_id'],'session.id':'child'})
            db.execute('UPDATE spans SET attributes=?,agent_id=?',(json.dumps(attrs),self.child['agent_id']))
            self.assertEqual(db.execute('SELECT COUNT(*) FROM finding_cache').fetchone()[0],0)
        with self.assertRaisesRegex(ValueError,'catching up'):self.rows()
        self.assertGreater(self.store._drain_external_activity(),0)
        rows=self.rows();self.assertEqual(len(rows),1);self.assertEqual(rows[0]['agent_id'],self.child['agent_id'])
        self.assertEqual(self.rows(agent_id=self.parent['agent_id']),[])
        with self.store._connect() as db:db.execute('DELETE FROM spans')
        self.store._drain_external_activity()
        self.assertEqual(self.rows(),[])
        with self.store._connect() as db:self.assertFalse(db.execute('SELECT 1 FROM finding_dirty_v1').fetchone())

    def test_superseded_owned_tool_span_does_not_reappear_as_raw_fallback(self):
        owner=self.parent['agent_id'];span=self.span(1,owner,'session',status='ERROR')
        span['attributes']={'tool.name':'Bash','agent.id':owner,'session.id':'session'}
        self.store.ingest([span])
        self.store.ingest_logs([dict(trace_id=self.trace,span_id=f'{1:016x}',time_unix_nano=20,service='fixture',
            scope_name='fixture',event_name='agent.activity',body='Migration failed',severity_number=17,severity_text='ERROR',
            attributes={'agent.id':owner,'session.id':'session','agent.activity.domain':'shell',
                'agent.activity.action':'execute','agent.activity.result':'error','agent.activity.subject':'python migrate.py',
                'process.command_line':'python migrate.py','process.working_directory':'/work',
                'process.exit.code':1,'tool.name':'Bash'},resource_attributes={})])
        self.assertEqual(len(self.rows()),1)
        with self.store._connect() as db:
            row=db.execute('SELECT digest,attributes FROM logs').fetchone();attrs=json.loads(row['attributes'])
            attrs.update({'agent.id':self.child['agent_id'],'session.id':'child'})
            db.execute('UPDATE logs SET attributes=?,agent_id=? WHERE digest=?',(json.dumps(attrs),self.child['agent_id'],row['digest']))
        self.store._drain_external_activity()
        self.assertEqual({row['agent_id'] for row in self.rows()},{owner,self.child['agent_id']})

    def test_exact_sources_do_not_include_foreign_parent_events_and_stay_bounded(self):
        owner=self.parent['agent_id'];child=self.child['agent_id']
        self.store.ingest([self.span(1,owner,'parent',events=[dict(name='custom',time_unix_nano=12,
            attributes={'agent.id':child,'session.id':'child','api_key':'CHILD_PRIVATE_SECRET','note':'Child-only detail'})])])
        rows=self.rows();parent=next(item for item in rows if item['kind']=='span_error')
        source=self.store.finding_source(parent['id'])
        self.assertEqual(source['receiver_id'],self.store.receiver_id)
        self.assertNotIn('Child-only detail',json.dumps(source))
        child_finding=next(item for item in rows if item['kind']=='credential_exposure')
        exact=self.store.finding_source(child_finding['id'])
        self.assertIn('Child-only detail',json.dumps(exact));self.assertNotIn('CHILD_PRIVATE_SECRET',json.dumps(exact))
        self.assertEqual(exact['sources'][0]['scope']['agent_id'],child)
        self.assertEqual(exact['sources'][0]['id'],f'{1:016x}:event:0')
        self.store.ingest([self.span(1,owner,'parent',status='ERROR',attributes={'output.value':'x'*100000})])
        clipped=self.store.finding_source(parent['id'])
        self.assertFalse(clipped['sources'][0]['complete']);self.assertIn('64 KiB',clipped['sources'][0]['reason'])
        self.assertLess(len(json.dumps(clipped).encode()),64000)
        with self.store._connect() as db:db.execute('DELETE FROM finding_cache')
        self.assertTrue(self.store.finding_source(parent['id'])['unavailable'])

    def test_progress_updates_revoke_outcome_owner_and_pruning_does_not_stick(self):
        owner=self.parent['agent_id'];self.store.ingest([self.span(1,owner,'session')])
        self.store.ingest_outcomes([dict(trace_id=self.trace,passed=False,error='Evaluation failed')])
        before=next(item for item in self.rows() if item['kind']=='failed_outcome')
        self.assertEqual(before['agent_id'],owner)
        with self.store._connect() as db:
            db.execute('INSERT INTO progress(trace_id,time_ns,stage,message,percent,session_id,service) VALUES(?,?,?,?,?,?,?)',
                (self.trace,100,'Working','Recorded progress',None,'session','fixture'))
        self.assertFalse(self.store.index_status()['ready'])
        self.store._drain_external_activity()
        after=next(item for item in self.rows() if item['kind']=='failed_outcome')
        self.assertIsNone(after['agent_id'])
        self.assertFalse(any(item['kind']=='failed_outcome' for item in self.rows(agent_id=owner)))
        with self.store._connect() as db:
            db.execute('UPDATE progress SET trace_id=?',('b'*32,))
            self.assertEqual({row[0] for row in db.execute('SELECT trace_id FROM finding_dirty_v1')},{self.trace,'b'*32})
        self.store._drain_external_activity()
        self.store.prune(1,now_ns=9*10**18)
        self.assertEqual(self.rows(),[])

    def test_existing_store_backfill_repairs_unknown_legacy_ownership(self):
        owner=self.parent['agent_id'];self.store.ingest([self.span(1,owner,'session')])
        with self.store._connect() as db:
            db.execute('UPDATE finding_cache SET agent_id=NULL,session_id=NULL,producer=NULL')
            db.execute("DELETE FROM schema_meta WHERE key='trace_derived_v15'")
        path=self.store.path;self.store.close();self.store=Store(path)
        rows=self.rows(agent_id=owner)
        self.assertEqual(len(rows),1);self.assertEqual(rows[0]['session_id'],'session')
        self.assertTrue(self.store.finding_page()['ownership_ready'])

    def test_source_encoded_projection_budget_covers_escaped_text(self):
        self.store.ingest_logs([dict(trace_id=self.trace,span_id=None,time_unix_nano=30,service='fixture',
            scope_name='fixture',event_name='sdk.error',body='\n'*7000,severity_number=17,
            severity_text='ERROR',attributes={},resource_attributes={})])
        finding=next(item for item in self.rows() if item['kind']=='error_log')
        source=self.store.finding_source(finding['id'])['sources'][0]
        self.assertLessEqual(len(json.dumps(source['record'],ensure_ascii=False).encode()),8000)
        self.assertFalse(source['complete']);self.assertIn('8,000-byte',source['reason'])

    def test_completed_backfill_still_drains_dirty_findings_after_sync_restart(self):
        owner=self.parent['agent_id'];self.store.ingest([self.span(1,owner,'session')])
        with self.store._connect() as db:
            db.execute("UPDATE spans SET attributes=json_set(attributes,'$.stderr','Changed failure')")
        path=self.store.path;self.store.close();self.store=Store(path)
        self.assertTrue(self.store.index_status()['ready'])
        self.assertIn('Changed failure',self.rows()[0]['detail'])

    def test_malformed_legacy_references_cannot_escape_response_budget(self):
        self.store.ingest([self.span(1,self.parent['agent_id'],'session')])
        row=self.rows()[0]
        row['evidence']=[dict(type='log',id='x'*100000),dict(type='unknown',id='x'*100000),
            dict(type='span',id='1'*16+':event:'+'9'*10000)]
        with self.store._connect() as db:db.execute('UPDATE finding_cache SET finding_json=?',(json.dumps(row),))
        response=self.store.finding_source(row['id'])
        self.assertLess(len(json.dumps(response).encode()),64000)
        self.assertTrue(all(entry['record'] is None and entry['reason']=='Invalid source reference' for entry in response['sources']))

    def test_fallback_credentials_are_redacted_before_any_stringification(self):
        for body in ({'password':'PRIVATE_CREDENTIAL_VALUE'},'{"password":"PRIVATE_CREDENTIAL_VALUE"}',
                     '{"api\\u005fkey":"PRIVATE_CREDENTIAL_VALUE"}'):
            self.store.ingest_logs([dict(trace_id=self.trace,span_id=None,time_unix_nano=30,service='fixture',
                scope_name='fixture',event_name='sdk.error',body=body,severity_number=17,
                severity_text='ERROR',attributes={},resource_attributes={})])
        rows=self.rows()
        self.assertTrue(any(item['kind']=='error_log' for item in rows))
        self.assertNotIn('PRIVATE_CREDENTIAL_VALUE',json.dumps(rows))

    def test_conflicting_or_malformed_session_aliases_never_prove_outcome_ownership(self):
        owner=self.parent['agent_id']
        for value in (False,'other'):
            self.store.ingest([self.span(1,owner,'session',attributes={'gen_ai.conversation.id':value})])
            rows=self.rows();self.assertTrue(all(item['session_id'] is None for item in rows))
            self.store.ingest_outcomes([dict(trace_id=self.trace,passed=False,error='Evaluation failed')])
            outcome=next(item for item in self.rows() if item['kind']=='failed_outcome')
            self.assertIsNone(outcome['agent_id']);self.assertIsNone(outcome['session_id'])

    def test_exact_source_http_route_keeps_auth_and_receiver_identity(self):
        from http.server import ThreadingHTTPServer
        import threading
        from urllib.request import Request,urlopen
        from urllib.error import HTTPError
        from observatory.server import handler_for
        self.store.ingest([self.span(1,self.parent['agent_id'],'session')])
        finding=self.rows()[0]
        server=ThreadingHTTPServer(('127.0.0.1',0),handler_for(self.store,'secret'))
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            url='http://127.0.0.1:'+str(server.server_port)+'/api/findings/'+finding['id']+'/source'
            with self.assertRaises(HTTPError) as denied:urlopen(url,timeout=3)
            self.assertEqual(denied.exception.code,401)
            with urlopen(Request(url,headers={'Authorization':'Bearer secret'}),timeout=3) as response:
                payload=json.load(response)
            self.assertEqual(payload['receiver_id'],self.store.receiver_id)
            self.assertEqual(payload['finding_id'],finding['id'])
            self.assertEqual(payload['finding_scope']['agent_id'],self.parent['agent_id'])
            self.assertEqual(payload['sources'][0]['id'],f'{1:016x}')
        finally:server.shutdown();server.server_close();thread.join(3)


if __name__=='__main__':unittest.main()
