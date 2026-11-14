"""Raw MCP snapshots can avoid unrelated queues without avoiding certificates."""
from contextlib import closing,contextmanager
import io
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

from observatory.core import Store,normalize_native,_agent_id_from_attributes
from observatory.server import handler_for

TRACE='a'*32


class McpInitialCertificateTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.store=Store(Path(self.temp.name)/'generated.db');self.addCleanup(self.store.close)
        self.record=normalize_native(dict(trace_id=TRACE,span_id='1'*16,name='Generated MCP',start_unix_nano=1,end_unix_nano=2,status='OK',events=[],
            attributes={'agent.id':'worker','session.id':'fixture-session','mcp.method.name':'tools/call','mcp.server.name':'fixture',
                'mcp.tool.name':'lookup','input.value':{'query':'generated'},'output.value':{'count':1}}),'fixture')
        self.store.ingest([self.record]);self.handler=handler_for(self.store,'fixture-token',quiet=True);self.addCleanup(self.handler.response_cache.close)

    def get(self,path='/api/mcp?limit=20',token='fixture-token'):
        class Socket:
            def __init__(self,raw):self.input,self.output=io.BytesIO(raw),io.BytesIO()
            def makefile(self,mode,*args,**kwargs):return self.input if 'r'in mode else self.output
            def sendall(self,data):self.output.write(data)
        sock=Socket(f'GET {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\r\n'.encode())
        self.handler(sock,('127.0.0.1',1),type('Server',(),{})())
        head,body=sock.output.getvalue().split(b'\r\n\r\n',1)
        return int(head.split()[1]),json.loads(body)

    def dirty(self):
        with self.store._connect()as db:
            db.execute('INSERT OR IGNORE INTO finding_dirty_v1 VALUES(?)',('b'*32,))
            db.execute("INSERT OR IGNORE INTO investigation_dirty_v1 VALUES('span','generated')")
            db.execute("INSERT OR IGNORE INTO native_lifecycle_dirty_v1 VALUES('span','generated')")

    def raw_update(self,**changes):
        self.record['attributes'].update(changes)
        with closing(sqlite3.connect(self.store.path))as db,db:
            db.execute('UPDATE spans SET attributes=?,agent_id=? WHERE trace_id=?',
                (json.dumps(self.record['attributes']),_agent_id_from_attributes(self.record['attributes'],{},'fixture'),TRACE))

    def test_unscoped_service_session_full_payloads_survive_only_unrelated_dirty_queues(self):
        paths=('/api/mcp?limit=20','/api/mcp?service=fixture','/api/mcp?session_id=fixture-session')
        expected={path:self.get(path)for path in paths}
        self.assertTrue(all(code==200 for code,_ in expected.values()));self.dirty();self.assertFalse(self.store.index_status()['ready'])
        with patch.object(self.handler.response_cache,'get',side_effect=AssertionError('Dirty read used cache')),patch.object(self.handler.response_cache,'put',side_effect=AssertionError('Dirty read populated cache')):
            for path in paths:
                with self.subTest(path=path):self.assertEqual(self.get(path),expected[path])

    def test_agent_subtree_and_revision_selectors_keep_strict_global_gate_and_auth(self):
        owner=_agent_id_from_attributes(self.record['attributes'],{},'fixture');self.dirty()
        for path in ('/api/mcp?agent_id='+owner,'/api/mcp?subtree=0','/api/mcp?scope_revision=generated','/api/mcp?agent_id='):
            self.assertEqual(self.get(path)[0],503,path)
        self.assertEqual(self.get(token='wrong')[0],401)

    def test_cached_response_cannot_hide_missing_initial_marker_or_fatal_error(self):
        self.assertEqual(self.get()[0],200)
        with self.store._connect()as db:db.execute("DELETE FROM schema_meta WHERE key='trace_derived_v15'")
        self.assertEqual(self.get()[0],503)
        with self.store._connect()as db:db.execute("INSERT INTO schema_meta VALUES('trace_derived_v15','1')")
        self.store.migration_status=dict(ready=True,stage='History migration failed; restart to resume',error='Generated fatal error')
        self.assertEqual(self.get()[0],503)

    def test_every_healthy_cache_hit_certifies_and_initial_fatal_stopping_reject(self):
        with patch.object(self.store,'_mcp_page',wraps=self.store._mcp_page)as raw,patch.object(self.store,'certify_initial_mcp',wraps=self.store.certify_initial_mcp)as certify:
            expected=self.get();self.assertEqual(self.get(),expected);self.assertEqual(self.get(),expected)
            self.assertEqual(raw.call_count,1);self.assertEqual(certify.call_count,3)
            for field,value in (('migration_status',dict(ready=False,stage='Preparing indexed history')),
                    ('migration_status',dict(ready=True,stage='Failed',error='Generated fatal error')),
                    ('last_derived_error','Generated derived failure'),('_derived_stopping',True)):
                with patch.object(self.store,field,value):self.assertEqual(self.get()[0],503)
            self.assertEqual(raw.call_count,1)

    def test_marker_commit_between_certificate_and_cache_lookup_is_not_a_hit(self):
        self.assertEqual(self.get()[0],200);original=self.handler.response_cache.get
        def cache(key):
            with self.store._connect()as db:db.execute("DELETE FROM schema_meta WHERE key='trace_derived_v15'")
            return original(key)
        with patch.object(self.handler.response_cache,'get',side_effect=cache):self.assertEqual(self.get()[0],503)

    def test_same_receiver_file_replacement_after_cached_lookup_rejects_body(self):
        self.assertEqual(self.get()[0],200)
        replacement=Path(self.temp.name)/'replacement.db'
        with self.store._connect()as db:db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        with closing(sqlite3.connect(self.store.path))as source,closing(sqlite3.connect(replacement))as target:source.backup(target)
        self.assertEqual(self.get()[0],200)
        original=self.handler.response_cache.get
        def cache(key):
            result=original(key);self.assertIsNotNone(result[1]);os.replace(replacement,self.store.path);return result
        with patch.object(self.handler.response_cache,'get',side_effect=cache):self.assertEqual(self.get()[0],409)

    def test_real_capture_redaction_changes_current_payload_before_raw_read(self):
        self.record['resource_attributes']={'observatory.source_id':'c'*32}
        self.record['attributes'].update({'input.value':{'query':'PRIVATE_MCP_INPUT'},'output.value':'PRIVATE_MCP_OUTPUT'})
        self.store.ingest([self.record])
        paths=('/api/mcp','/api/mcp?service=fixture','/api/mcp?session_id=fixture-session','/api/mcp?include_reviews=0')
        for path in paths:self.assertIn('PRIVATE_MCP',json.dumps(self.get(path)))
        with patch.object(self.store,'_refresh_summaries',return_value=None):self.store.redact_internal_source('c'*32)
        self.assertFalse(self.store.index_status()['ready'])
        for path in paths:
            code,body=self.get(path);self.assertEqual(code,200,body);self.assertNotIn('PRIVATE_MCP',json.dumps(body))
            if 'service=fixture'in path or 'include_reviews=0'in path:self.assertEqual(body['calls'],[])

    def test_agent_scoped_commit_after_server_ready_probe_still_rejects(self):
        owner=_agent_id_from_attributes(self.record['attributes'],{},'fixture');path='/api/mcp?agent_id='+owner
        self.assertEqual(self.get(path)[0],200);original=self.store.index_status;fired=[]
        def status():
            result=original()
            if not fired:fired.append(True);self.raw_update(**{'agent.id':'other-worker'})
            return result
        with patch.object(self.store,'index_status',side_effect=status):self.assertEqual(self.get(path)[0],503)

    def test_selector_presence_warmed_cache_still_checks_marker_fatal_and_stopping(self):
        for selector in ('agent_id=','subtree=0','scope_revision=generated','agent_id=&agent_id=',
                'subtree=false&subtree=0','scope_revision=&scope_revision='):
            path='/api/mcp?'+selector;self.assertEqual(self.get(path)[0],200)
            with self.store._connect()as db:db.execute("DELETE FROM schema_meta WHERE key='trace_derived_v15'")
            self.assertEqual(self.get(path)[0],503,selector)
            with self.store._connect()as db:db.execute("INSERT INTO schema_meta VALUES('trace_derived_v15','1')")
            self.assertEqual(self.get(path)[0],200)
            with patch.object(self.store,'last_derived_error','Generated failure'):self.assertEqual(self.get(path)[0],503)
            with patch.object(self.store,'_derived_stopping',True):self.assertEqual(self.get(path)[0],503)

    def test_unavailable_path_during_certificate_is_typed_503_without_recreating_db(self):
        original=self.store.certify_initial_mcp
        def certify(*args,**kwargs):Path(self.store.path).unlink();return original(*args,**kwargs)
        with patch.object(self.store,'certify_initial_mcp',side_effect=certify):self.assertEqual(self.get()[0],503)
        self.assertFalse(Path(self.store.path).exists())

    def test_stat_to_read_lease_race_does_not_recreate_missing_path(self):
        original_certify=self.store.certify_initial_mcp;original_read=self.store._read;active=[]
        def certify(*args,**kwargs):active.append(True);return original_certify(*args,**kwargs)
        @contextmanager
        def read():
            if active:Path(self.store.path).unlink()
            with original_read()as db:yield db
        with patch.object(self.store,'certify_initial_mcp',side_effect=certify),patch.object(self.store,'_read',side_effect=read):
            self.assertEqual(self.get()[0],503)
        self.assertFalse(Path(self.store.path).exists())

    def test_missing_path_after_certificate_before_inventory_is_typed_503(self):
        original=self.store.mcp_page
        def page(*args,**kwargs):Path(self.store.path).unlink();return original(*args,**kwargs)
        with patch.object(self.store,'mcp_page',side_effect=page):self.assertEqual(self.get()[0],503)
        self.assertFalse(Path(self.store.path).exists())

    def test_actual_source_identity_and_output_commit_is_current_without_waiting_for_findings(self):
        self.assertEqual(self.get()[0],200)
        self.raw_update(**{'session.id':'other-session','agent.id':'other-worker','output.value':{'count':999}})
        self.assertFalse(self.store.index_status()['ready'])
        expected={**self.store.mcp_page(),'scope':'stored MCP operations'}
        self.assertEqual(self.get(),(200,expected))
        self.assertEqual(self.get('/api/mcp?session_id=fixture-session')[1]['calls'],[])

    def test_raw_commit_after_ready_probe_uses_one_current_raw_snapshot(self):
        self.assertEqual(self.get()[0],200);original=self.store.index_status;fired=[]
        def status():
            state=original()
            if not fired:fired.append(True);self.raw_update(**{'output.value':{'count':999}})
            return state
        with patch.object(self.store,'index_status',side_effect=status):result=self.get()
        self.assertEqual(result,(200,{**self.store.mcp_page(),'scope':'stored MCP operations'}))

    def test_commit_after_read_snapshot_keeps_old_complete_payload(self):
        expected=self.get('/api/mcp?q=Generated');self.handler.response_cache.close()
        self.handler=handler_for(self.store,'fixture-token',quiet=True);self.addCleanup(self.handler.response_cache.close)
        original=self.store._mcp_page;fired=[]
        def page(db,*args):
            db.execute('SELECT attributes FROM spans WHERE trace_id=?',(TRACE,)).fetchone()
            if not fired:fired.append(True);self.raw_update(**{'output.value':{'count':999}})
            return original(db,*args)
        with patch.object(self.store,'_mcp_page',side_effect=page):self.assertEqual(self.get('/api/mcp?q=Generated'),expected)
        self.assertEqual(fired,[True])


if __name__=='__main__':unittest.main()
