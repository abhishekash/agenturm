"""Opted native replays skip exact rows, never changed evidence or repairs."""
from contextlib import closing
from copy import deepcopy
import json
import sqlite3
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
from urllib.request import Request,ProxyHandler,build_opener

from observatory.core import Store,normalize_native
from observatory.server import BoundedHTTPServer,handler_for
from tests.test_derived_readiness_snapshot import wait_ready


def span(number=1,**changes):
    row=normalize_native(dict(trace_id=f'{number:032x}',span_id='1'*16,name='chat generated',
        start_unix_nano=1,end_unix_nano=20,status='OK',scope_name='observatory.opencode',
        attributes={'agent.id':'generated-worker','session.id':'generated-session','gen_ai.usage.input_tokens':5},
        events=[],resource_attributes={'observatory.source_id':'a'*32}),'opencode')
    row.update(changes);return row


def log():
    return dict(trace_id='f'*32,span_id=None,time_unix_nano=30,service='opencode',severity_number=9,
        severity_text='INFO',event_name='agent.activity',body='Generated output',
        attributes={'session.id':'generated-session','agent.id':'generated-worker',
            'agent.activity.domain':'message','agent.activity.action':'user','observatory.record_id':'d'*32},
        resource_attributes={'observatory.source_id':'a'*32},scope_name='observatory.local_agents')


class NativeNoopPublicationTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.store=Store(Path(self.temp.name)/'generated.db',async_derived_refresh=True,workflow_patterns=False)
        self.addCleanup(self.store.close);wait_ready(self.store)

    def test_opt_in_only_preserves_default_counts_and_every_non_native_scope(self):
        for scope in ('observatory.opencode','observatory.local_agents','sdk.claim',''):
            with self.subTest(scope=scope),self.store._derived_lock:
                row=span(scope_name=scope)
                self.assertEqual(self.store.ingest([row]),1)
                # Default accepted-count semantics include repeated SDK rows and
                # SDKs claiming a native scope. Private True opts in explicitly.
                self.assertEqual(self.store.ingest([row]),1)
                result=self.store.ingest([row],skip_unchanged_native=True)
                self.assertEqual(result,0 if scope.startswith('observatory.')else 1)
        row=log()
        self.assertEqual(self.store.ingest_logs([row]),1)
        self.assertEqual(self.store.ingest_logs([row],skip_unchanged_native=True),0)
        self.assertEqual(self.store.ingest_logs([row]),1)

    def test_replayed_history_plus_one_change_publishes_fresh_authenticated_http(self):
        rows=[span(i)for i in range(1,8)]
        for row in rows:self.store.ingest([row])
        changed=deepcopy(rows);changed[-1]['attributes']['gen_ai.usage.input_tokens']=99
        handler=handler_for(self.store,'generated-token',quiet=True)
        server=BoundedHTTPServer(('127.0.0.1',0),handler);thread=threading.Thread(target=server.serve_forever);thread.start()
        try:
            with self.store._derived_lock,patch.object(self.store,'_publish_native_summaries',wraps=self.store._publish_native_summaries)as publish:
                self.assertEqual(self.store.ingest(changed,skip_unchanged_native=True),1)
                self.assertEqual(len(publish.call_args.args[1]),1)
                self.assertTrue(self.store.index_status()['ready'])
                request=Request(f'http://127.0.0.1:{server.server_port}/api/metrics',headers={'Authorization':'Bearer generated-token'})
                with build_opener(ProxyHandler({})).open(request,timeout=3)as response:
                    current=json.load(response);self.assertEqual(response.status,200)
                self.assertEqual(current['input_tokens'],6*5+99)
                with patch.object(self.store,'_repair_native_activity',side_effect=AssertionError('Healthy replay scanned raw maxima')):
                    self.assertEqual(self.store.ingest(changed,skip_unchanged_native=True),0)
        finally:
            server.shutdown();server.server_close();thread.join(3);handler.response_cache.close()
            self.assertFalse(thread.is_alive())

    def test_legacy_nullable_scope_is_not_an_opted_native_noop(self):
        path=Path(self.temp.name)/'legacy-nullable.db'
        with self.store._read()as db:
            ddl=[db.execute("SELECT sql FROM sqlite_master WHERE name=?",(table,)).fetchone()[0]for table in ('spans','logs')]
        with closing(sqlite3.connect(path))as db,db:
            for sql in ddl:db.execute(sql.replace('scope_name TEXT NOT NULL','scope_name TEXT'))
        legacy=Store(path,workflow_patterns=False)
        try:
            row=span(scope_name=None);event=log();event['scope_name']=None
            for method,record in ((legacy.ingest,row),(legacy.ingest_logs,event)):
                self.assertEqual(method([record]),1)
                self.assertEqual(method([record]),1)
                self.assertEqual(method([record],skip_unchanged_native=True),1)
        finally:legacy.close()

    def test_all_mutable_span_columns_and_nulls_are_real_updates(self):
        mutations=[{'parent_span_id':'2'*16},{'name':'Different'}, {'start_unix_nano':0},
            {'end_unix_nano':21},{'status':'ERROR'},{'service':'changed'},
            {'attributes':{'agent.id':'different','session.id':'other','extra':None}},
            {'events':[{'name':'generated','time_unix_nano':10,'attributes':{}}]},
            {'links':[{'trace_id':'2'*32,'span_id':'2'*16}]},
            {'resource_attributes':{'observatory.source_id':'b'*32}}, {'scope_name':'observatory.local_agents'}]
        for index,fields in enumerate(mutations,1):
            with self.subTest(fields=list(fields)),self.store._derived_lock:
                original=span(index);self.store.ingest([original])
                changed=deepcopy(original);changed.update(fields)
                self.assertEqual(self.store.ingest([changed],skip_unchanged_native=True),1)
                self.assertEqual(self.store.ingest([changed],skip_unchanged_native=True),0)
                self.assertEqual(self.store.ingest([original],skip_unchanged_native=True),1)

    def test_log_columns_nulls_and_raw_owner_update_are_not_skipped(self):
        mutations=[{'trace_id':'e'*32},{'span_id':'2'*16},{'time_unix_nano':31},
            {'service':'changed'},{'severity_number':17},{'severity_text':'WARN'},
            {'event_name':'generated.event'},{'body':{'count':0}},
            {'resource_attributes':{'observatory.source_id':'b'*32}},
            {'scope_name':'observatory.opencode'}]
        for index,fields in enumerate(mutations,1):
            with self.subTest(fields=fields),self.store._derived_lock:
                original=log();original['attributes']['observatory.record_id']=f'{index:032x}'
                self.store.ingest_logs([original]);changed=deepcopy(original);changed.update(fields)
                self.assertEqual(self.store.ingest_logs([changed],skip_unchanged_native=True),1)
                self.assertEqual(self.store.ingest_logs([changed],skip_unchanged_native=True),0)
        original=log();self.store.ingest_logs([original])
        with self.store._derived_lock:
            with self.store._connect()as db:
                db.execute("UPDATE logs SET agent_id=NULL WHERE json_extract(attributes,'$.\"observatory.record_id\"')=?",('d'*32,))
            self.assertEqual(self.store.ingest_logs([original],skip_unchanged_native=True),1)
            with self.store._read()as db:
                self.assertIsNotNone(db.execute("SELECT agent_id FROM logs WHERE json_extract(attributes,'$.\"observatory.record_id\"')=?",('d'*32,)).fetchone()[0])

    def test_equal_native_positions_and_replaced_generation_remain_fenced(self):
        path=Path(self.temp.name)/'source.jsonl';path.write_text('generated\n');info=path.stat()
        row=span(scope_name='observatory.local_agents')
        row['attributes'].update({'observatory.native_file_id':f'{info.st_dev}:{info.st_ino}',
            'observatory.native_file_offset':10,'observatory.native_mtime_ns':info.st_mtime_ns})
        self.assertEqual(self.store.ingest([row],native_source_path=path,skip_unchanged_native=True),1)
        self.assertEqual(self.store.ingest([row],native_source_path=path,skip_unchanged_native=True),0)
        stale=deepcopy(row);stale['attributes']['observatory.native_file_offset']=9
        stale['attributes']['gen_ai.usage.input_tokens']=999
        self.assertEqual(self.store.ingest([stale],native_source_path=path,skip_unchanged_native=True),0)
        replacement=path.with_suffix('.replacement');replacement.write_text('replacement\n');replacement.replace(path)
        self.assertEqual(self.store.ingest([stale],native_source_path=path,skip_unchanged_native=True),0)
        self.assertEqual(self.store.metrics()['input_tokens'],5)

    def test_missing_summary_dirty_and_activity_clock_repair_still_publish(self):
        first=span();later=span(span_id='2'*16,end_unix_nano=100)
        self.store.ingest([first,later])
        for operation in ('DELETE FROM trace_summaries WHERE trace_id=?',
                          'DELETE FROM trace_activity WHERE trace_id=?',
                          'UPDATE trace_activity SET last_ns=50 WHERE trace_id=?',
                          'INSERT OR IGNORE INTO finding_dirty_v1 VALUES(?)'):
            with self.subTest(operation=operation),self.store._derived_lock:
                with self.store._connect()as db:db.execute(operation,(first['trace_id'],))
                self.assertEqual(self.store.ingest([first],skip_unchanged_native=True),1)
                self.assertTrue(self.store.index_status()['ready'])
                with self.store._read()as db:
                    self.assertEqual(db.execute('SELECT last_ns FROM trace_activity WHERE trace_id=?',(first['trace_id'],)).fetchone()[0],100)
                    self.assertIsNotNone(db.execute('SELECT 1 FROM trace_summaries WHERE trace_id=?',(first['trace_id'],)).fetchone())
                self.assertEqual(self.store.ingest([first],skip_unchanged_native=True),0)

    def test_later_changed_record_does_not_mark_healthy_older_noop_as_repair(self):
        first=span();later=span(span_id='2'*16,end_unix_nano=100)
        self.store.ingest([first,later]);changed=deepcopy(later);changed['end_unix_nano']=101
        with self.store._derived_lock:
            self.assertEqual(self.store.ingest([first,changed],skip_unchanged_native=True),1)
            self.assertEqual(self.store.ingest([first,changed],skip_unchanged_native=True),0)

    def test_more_than_64_replayed_rows_one_real_change_still_publishes_atomically(self):
        rows=[span(span_id=f'{i:016x}',end_unix_nano=20+i)for i in range(1,72)]
        for offset in range(0,len(rows),16):self.store.ingest(rows[offset:offset+16])
        changed=deepcopy(rows);changed[-1]['end_unix_nano']+=1
        with self.store._derived_lock,patch.object(self.store,'_publish_native_summaries',wraps=self.store._publish_native_summaries)as publish:
            self.assertEqual(self.store.ingest(changed,skip_unchanged_native=True),1)
            self.assertEqual(len(publish.call_args.args[1]),1)
            self.assertTrue(self.store.index_status()['ready'])
            self.assertEqual(self.store.ingest(changed,skip_unchanged_native=True),0)

    def test_policy_scrub_before_comparison_no_plaintext_replay(self):
        row=span();row['attributes']['input.value']='PRIVATE_GENERATED_INPUT';self.store.ingest([row])
        self.store.redact_internal_source('a'*32)
        with self.store._derived_lock:
            self.store.ingest([row],skip_unchanged_native=True)
            self.assertEqual(self.store.ingest([row],skip_unchanged_native=True),0)
        with self.store._read()as db:
            stored=db.execute('SELECT attributes,events,service FROM spans').fetchall()
            self.assertNotIn('PRIVATE_GENERATED_INPUT',str([tuple(value)for value in stored]))

    def test_rollback_keeps_prior_source_and_summary(self):
        row=span();self.store.ingest([row]);before=self.store.metrics()
        changed=deepcopy(row);changed['attributes']['gen_ai.usage.input_tokens']=99
        with patch.object(self.store,'_update_endpoints',side_effect=RuntimeError('generated rollback')):
            with self.assertRaisesRegex(RuntimeError,'rollback'):
                self.store.ingest([changed],skip_unchanged_native=True)
        self.assertEqual(self.store.metrics(),before)
        self.assertEqual(self.store.ingest([row],skip_unchanged_native=True),0)

    def test_legacy_record_cleanup_is_repair_even_when_canonical_row_is_identical(self):
        row=log();row['trace_id']=None;self.store.ingest_logs([row])
        with self.store._connect()as db:
            values=dict(db.execute('SELECT * FROM logs').fetchone());values['digest']='f'*64
            db.execute('INSERT INTO logs('+','.join(values)+')VALUES('+','.join('?'for _ in values)+')',tuple(values.values()))
        with self.store._derived_lock:
            self.assertEqual(self.store.ingest_logs([row],skip_unchanged_native=True),1)
            with self.store._read()as db:self.assertEqual(db.execute('SELECT COUNT(*) FROM logs').fetchone()[0],1)
            self.assertEqual(self.store.ingest_logs([row],skip_unchanged_native=True),0)


if __name__=='__main__':unittest.main()
