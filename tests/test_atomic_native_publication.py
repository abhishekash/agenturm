"""Accepted small native writes publish complete source/summary snapshots."""
from concurrent.futures import ThreadPoolExecutor
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from observatory.core import Store,normalize_native
from observatory.server import handler_for
from tests import test_mcp_initial_certificate as http
from tests import test_derived_readiness_snapshot as readiness

TRACE='f'*32


def native(value=5,*,trace=TRACE,span='0000000000000001',scope='observatory.local_agents'):
    return normalize_native(dict(trace_id=trace,span_id=span,name='chat generated',
        start_unix_nano=1,end_unix_nano=20,status='OK',scope_name=scope,
        attributes={'session.id':'atomic-native','agent.id':'generated-worker',
            'agent.task':'Generated native task','gen_ai.usage.input_tokens':value},events=[]),'codex')


class AtomicNativePublicationTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.store=Store(Path(self.tmp.name)/'generated.db',async_derived_refresh=True)
        self.addCleanup(self.store.close);readiness.wait_ready(self.store)
        self.handler=handler_for(self.store,'fixture-token',quiet=True)
        self.addCleanup(self.handler.response_cache.close)

    def get(self,path='/api/metrics'):
        return http.McpInitialCertificateTests.get(self,path)

    def test_accepted_spans_and_logs_are_current_while_other_thread_holds_derived_lock(self):
        log=dict(trace_id='e'*32,span_id=None,time_unix_nano=30,service='codex',severity_number=9,
            severity_text='INFO',event_name='agent.activity',body='Generated native log',
            attributes={'session.id':'atomic-logs','agent.activity.domain':'message',
                'agent.activity.action':'user','agent.activity.subject':'Generated log request'},
            resource_attributes={},scope_name='observatory.local_agents')
        # The worker lock is owned by this thread; ingestion runs in another.
        # Acquiring it below SQLite's writer would block this request forever.
        with ThreadPoolExecutor(max_workers=1)as pool,self.store._derived_lock:
            self.assertEqual(pool.submit(self.store.ingest,[native(11)]).result(timeout=3),1)
            self.assertEqual(pool.submit(self.store.ingest_logs,[log]).result(timeout=3),1)
            self.assertTrue(self.store.index_status()['ready'])
            self.assertEqual(self.get(),(200,self.store.metrics()))
            self.assertEqual(self.get()[1]['input_tokens'],11)
            self.assertEqual(self.get()[1]['logs'],1)
            self.assertEqual(self.get('/api/sessions'),(200,{'sessions':self.store.sessions()}))
            with self.store._read()as db:
                self.assertEqual(db.execute('SELECT COUNT(*) FROM finding_dirty_v1').fetchone()[0],0)

    def test_reader_before_commit_gets_entire_old_snapshot_then_entire_new_snapshot(self):
        self.store.ingest([native(5)]);expected=self.store.metrics()
        published=threading.Event();release=threading.Event();original=self.store._publish_trace_summary
        def publish(*args,**kwargs):
            original(*args,**kwargs);published.set()
            if not release.wait(3):raise AssertionError('Generated publication barrier expired')
        with patch.object(self.store,'_publish_trace_summary',side_effect=publish),ThreadPoolExecutor(max_workers=1)as pool:
            future=pool.submit(self.store.ingest,[native(99)])
            try:
                self.assertTrue(published.wait(3));self.assertEqual(self.get(),(200,expected))
            finally:release.set()
            self.assertEqual(future.result(timeout=3),1)
        self.assertEqual(self.get(),(200,self.store.metrics()))
        self.assertEqual(self.get()[1]['input_tokens'],99)
        self.assertEqual(self.get('/api/sessions')[1]['sessions'][0]['input_tokens'],99)

    def test_foreign_pending_trace_cannot_be_certified_by_native_publication(self):
        foreign=native(55,trace='d'*32,scope='generated-sdk')
        with self.store._derived_lock:
            self.store.ingest([foreign]);self.store.ingest([native(22)])
            with self.store._read()as db:
                self.assertEqual([r[0]for r in db.execute('SELECT trace_id FROM finding_dirty_v1')],['d'*32])
            self.assertEqual(self.get()[0],503)
        readiness.wait_ready(self.store)
        self.assertEqual(self.get()[1]['input_tokens'],77)

    def test_non_native_mixed_cold_error_and_pattern_cases_keep_async_path(self):
        record=native(5)
        cases=(('sdk',[native(6,scope='generated-sdk')],None,None),
            ('mixed',[native(7),native(8,trace='d'*32,scope='generated-sdk')],None,None),
            ('cold',[record],'migration_status',dict(ready=False)),
            ('migration error',[record],'migration_status',dict(ready=True,error='Generated error')),
            ('derived error',[record],'last_derived_error','Generated error'),
            ('patterns',[record],'workflow_patterns',True))
        with self.store._derived_lock,patch.object(self.store,'_publish_trace_summary',wraps=self.store._publish_trace_summary)as publish:
            for name,records,field,value in cases:
                with self.subTest(name=name):
                    if field:
                        with patch.object(self.store,field,value):self.assertEqual(self.store.ingest(records),len(records))
                    else:self.assertEqual(self.store.ingest(records),len(records))
                    self.assertEqual(publish.call_count,0)
            with self.store._connect()as db:db.execute("DELETE FROM schema_meta WHERE key='live_stats_v3'")
            try:
                self.assertEqual(self.store.ingest([record]),1);self.assertEqual(publish.call_count,0)
            finally:
                with self.store._connect()as db:db.execute("INSERT INTO schema_meta VALUES('live_stats_v3','1')")
        readiness.wait_ready(self.store)

    def test_large_record_batch_keeps_source_and_queues_background_work(self):
        records=[native(1,span=f'{i+1:016x}')for i in range(65)]
        with self.store._derived_lock,patch.object(self.store,'_publish_trace_summary',wraps=self.store._publish_trace_summary)as publish:
            self.assertEqual(self.store.ingest(records),65);self.assertEqual(publish.call_count,0)
            self.assertEqual(self.get()[0],503)
            with self.store._read()as db:self.assertEqual(db.execute('SELECT COUNT(*) FROM spans').fetchone()[0],65)
        readiness.wait_ready(self.store)

    def test_retained_row_and_payload_limits_keep_async_path(self):
        records=[native(1,span=f'{i+1:016x}')for i in range(300)]
        self.store.ingest(records);readiness.wait_ready(self.store)
        large=native(3,trace='c'*32);large['attributes']['generated.large_payload']='x'*(600*1024)
        with self.store._derived_lock,patch.object(self.store,'_publish_trace_summary',wraps=self.store._publish_trace_summary)as publish:
            self.assertEqual(self.store.ingest([native(2)]),1)
            self.assertEqual(self.store.ingest([large]),1)
            self.assertEqual(publish.call_count,0);self.assertEqual(self.get()[0],503)
            with self.store._read()as db:self.assertEqual(db.execute('SELECT COUNT(*) FROM spans').fetchone()[0],301)
        readiness.wait_ready(self.store)
