"""Actual retained index, compatibility writers and pinned source/API parity."""
from contextlib import contextmanager
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import threading
import unittest
from unittest.mock import patch

from observatory.core import Store,normalize_native
from observatory.investigation import encode_reference


class InvestigationStoreTests(unittest.TestCase):
    def setUp(self):
        self.tmp=TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.store=Store(Path(self.tmp.name)/'source.db');self.addCleanup(self.store.close)

    def span(self,index=1,**attrs):
        values={'tool.name':'Bash','gen_ai.tool.type':'shell','process.command_line':'pytest tests -q',
            'process.exit.code':1,'stderr':'Expected retry count 3','session.id':'session'}
        values.update(attrs)
        return normalize_native(dict(trace_id='a'*32,span_id=f'{index:016x}',name='tool',status='UNSET',
            start_unix_nano=index,end_unix_nano=index+1,attributes=values),'fixture')

    def test_actual_command_source_and_six_field_execution_identity(self):
        self.store.ingest([self.span()])
        row=self.store.investigation_page('command','pytest',failed=True)['matches'][0]
        ref=row['source_reference']
        result=self.store.investigation_source(row['event_id'],ref['receiver_id'],encode_reference(ref))
        self.assertEqual(result['sources'][0]['scope'],{k:ref[k] for k in
            ('service','agent_id','session_id','trace_id','span_id','turn_id')})
        for key in ('receiver_id','service','agent_id','session_id','trace_id','span_id','turn_id'):
            changed={**ref,key:'different'}
            with self.subTest(key=key),self.assertRaises(ValueError):
                self.store.investigation_source(row['event_id'],changed['receiver_id'],encode_reference(changed))

    def test_source_identity_checked_before_raw_hydration(self):
        self.store.ingest([self.span()])
        row=self.store.investigation_page('command','pytest')['matches'][0];ref=row['source_reference']
        with patch('observatory.finding_sources.read_sources',side_effect=AssertionError('raw hydration')):
            with self.assertRaises(ValueError):
                self.store.investigation_source(row['event_id'],'foreign',encode_reference({**ref,'receiver_id':'foreign'}))

    def test_exact_log_without_trace_keeps_its_known_absent_execution_fields(self):
        self.store.ingest_logs([dict(trace_id=None,span_id=None,time_unix_nano=10,service='fixture',scope_name='fixture',
            event_name='agent.activity',body='',severity_number=9,severity_text='INFO',resource_attributes={},
            attributes={'agent.activity.domain':'shell','agent.activity.action':'execute','process.command_line':'pytest tests -q',
                'process.exit.code':0,'session.id':'session'})])
        row=self.store.investigation_page('command','pytest')['matches'][0];reference=row['source_reference']
        self.assertIsNone(reference['trace_id']);self.assertIsNone(reference['span_id'])
        result=self.store.investigation_source(row['event_id'],reference['receiver_id'],encode_reference(reference))
        self.assertIsNotNone(result['sources'][0]['record'])
        self.assertIsNone(result['sources'][0]['scope']['trace_id'])

    def test_typed_rename_indexes_both_paths_and_review_retains_pair(self):
        from types import SimpleNamespace
        from observatory.workflow_evidence import ReviewHistory,scope_for,event_key
        self.store.ingest([self.span(**{'tool.name':'rename_file','gen_ai.tool.type':'file',
            'tool.action':'rename','agent.activity.subject':'src/new.py',
            'tool.arguments':{'old_path':'src/old.py','new_path':'src/new.py'},'process.exit.code':0})])
        for path in ('src/old.py','src/new.py'):
            self.assertEqual(len(self.store.investigation_page('path',path)['matches']),1)
        rows=self.store.activity_page(limit=200)['activity'];first=rows[0]
        selected={**first,'capture_content':True,'task':'Rename file'}
        scope=scope_for(selected,SimpleNamespace(receiver_id=self.store.receiver_id))
        history=ReviewHistory(scope,events={event_key(row):row for row in rows},loaded=True,receiver_confirmed=True)
        edits=history.projection(selected)['edits']
        self.assertEqual((edits[0]['from_path'],edits[0]['to_path']),('src/old.py','src/new.py'))
        self.assertTrue(edits[0]['paths_complete'])

    def test_older_activity_writer_invalidates_and_bounded_worker_repairs(self):
        self.store.ingest([self.span()])
        with self.store._connect() as db:
            db.execute("DELETE FROM investigation_terms_v1")
            db.execute("UPDATE activity_index_v1 SET stamp=stamp+1")
        self.assertFalse(self.store.index_status()['ready'])
        with self.assertRaisesRegex(ValueError,'catching up'):self.store.investigation_page('command','pytest')
        self.assertEqual(self.store._drain_external_activity(),1)
        self.assertTrue(self.store.index_status()['ready'])
        self.assertEqual(len(self.store.investigation_page('command','pytest')['matches']),1)

    def test_prune_and_rebuild_do_not_leave_fts_terms(self):
        self.store.ingest([self.span()])
        with self.store._connect() as db:db.execute('DELETE FROM spans')
        self.store._drain_external_activity()
        self.assertFalse(self.store.investigation_page('command','pytest')['matches'])
        with self.store._read() as db:self.assertEqual(db.execute('SELECT count(*) FROM investigation_terms_v1').fetchone()[0],0)

    def test_temporarily_unsupported_fts_keeps_collection_and_rebuilds_on_restart(self):
        self.store.ingest([self.span()]);path=self.store.path;self.store.close()
        with patch('observatory.storage_investigation.initialize',side_effect=ValueError('FTS5 unavailable')):
            unsupported=Store(path)
        try:
            self.assertFalse(unsupported.investigation_index_v1)
            unsupported.ingest([self.span(**{'process.command_line':'python replacement.py'})])
            with self.assertRaisesRegex(ValueError,'FTS5'):unsupported.investigation_page('command','pytest')
        finally:unsupported.close()
        restored=Store(path)
        try:
            self.assertTrue(restored.investigation_index_v1)
            self.assertFalse(restored.investigation_page('command','pytest')['matches'])
            self.assertEqual(len(restored.investigation_page('command','replacement')['matches']),1)
        finally:restored.close()

    def test_cursor_query_and_receiver_are_pinned(self):
        self.store.ingest([self.span(1),self.span(2)])
        first=self.store.investigation_page('command','pytest',limit=1)
        second=self.store.investigation_page('command','pytest',limit=1,cursor=first['next_cursor'])
        self.assertNotEqual(first['matches'][0]['event_id'],second['matches'][0]['event_id'])
        with self.assertRaises(ValueError):self.store.investigation_page('command','different',cursor=first['next_cursor'])
        with self.store._connect() as db:db.execute("UPDATE schema_meta SET value='replacement' WHERE key='receiver_id'")
        with self.assertRaises(ValueError):self.store.investigation_page('command','pytest',cursor=first['next_cursor'])

    def test_query_and_receiver_read_one_wal_snapshot(self):
        self.store.ingest([self.span()]);original=self.store._connect;fired=[]
        @contextmanager
        def connection():
            with original() as db:
                def interleave(sql):
                    if 'MIN(t.text)' not in sql or fired:return
                    fired.append(True)
                    with original() as writer:writer.execute("UPDATE schema_meta SET value='replacement' WHERE key='receiver_id'")
                db.set_trace_callback(interleave);yield db
        receiver=self.store.receiver_id
        with patch.object(self.store,'_connect',connection):page=self.store.investigation_page('command','pytest')
        self.assertEqual(fired,[True]);self.assertEqual(page['receiver_id'],receiver)
        self.assertEqual(page['matches'][0]['source_reference']['receiver_id'],receiver)

    def test_authenticated_real_api_and_shared_cli_collector(self):
        from http.server import ThreadingHTTPServer
        from urllib.request import urlopen
        from urllib.error import HTTPError
        from observatory.server import handler_for
        from observatory.terminal import Api
        from observatory.investigation_cli import collect_investigation,collect_source
        self.store.ingest([self.span()])
        server=ThreadingHTTPServer(('127.0.0.1',0),handler_for(self.store,'secret'))
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            endpoint='http://127.0.0.1:'+str(server.server_port)
            with self.assertRaises(HTTPError) as denied:urlopen(endpoint+'/api/investigations?kind=command&q=pytest',timeout=3)
            self.assertEqual(denied.exception.code,401)
            api=Api(endpoint,'secret');page=collect_investigation(api,kind='command',query='pytest')
            row=page['matches'][0];source=collect_source(api,row['event_id'],row['source_reference'])
            self.assertEqual(source['source_reference'],row['source_reference'])
            self.assertEqual(source['sources'][0]['record']['attributes']['process.command_line'],'pytest tests -q')
        finally:server.shutdown();server.server_close();thread.join(3)


if __name__=='__main__':unittest.main()
