"""Queued repairs coalesce without weakening raw-write or snapshot contracts."""
from contextlib import closing,contextmanager
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from observatory.core import Store,normalize_native,IndexNotReady
import observatory.storage_derived as derived

TRACE='1'*32


def spans():
    return [normalize_native(dict(trace_id=TRACE,span_id='1'*16,name='chat generated',
        start_unix_nano=1,end_unix_nano=2,status='OK',attributes={
            'agent.id':'generated-writer','session.id':'generated-session','gen_ai.usage.input_tokens':5}), 'generated')]


class IngestWriterContentionTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.path=Path(self.tmp.name)/'generated.db';self.store=Store(self.path);self.addCleanup(self.store.close)

    def scalar(self,sql,args=()):
        with self.store._connect()as db:return db.execute(sql,args).fetchone()[0]

    def test_raw_and_agent_identity_share_one_accepted_write_transaction(self):
        original=self.store._connect;connections=[]
        @contextmanager
        def connection():
            with original()as db:
                connections.append(db);yield db
        with patch.object(self.store,'_connect',connection),patch.object(self.store,'_refresh_after_write'):
            self.assertEqual(self.store.ingest(spans()),1)
        self.assertEqual(len(connections),1)
        self.assertEqual(self.scalar('SELECT COUNT(*) FROM spans'),1)
        self.assertEqual(self.scalar('SELECT COUNT(*) FROM agent_nodes'),1)
        self.assertEqual(self.scalar('SELECT COUNT(*) FROM finding_dirty_v1'),1)

    def test_identity_write_failure_rolls_back_raw_activity_and_nodes(self):
        with patch.object(self.store,'_bump_hierarchy_revision',side_effect=sqlite3.OperationalError('generated rollback')):
            with self.assertRaisesRegex(sqlite3.OperationalError,'generated rollback'):self.store.ingest(spans())
        for table in ('spans','agent_nodes','trace_activity','activity_index_v1','finding_dirty_v1','agent_identity'):
            self.assertEqual(self.scalar('SELECT COUNT(*) FROM '+table),0,table)

    def test_rejected_records_cannot_publish_orphan_identity(self):
        with patch.object(self.store,'_write_ingest_rows',return_value=[]):self.assertEqual(self.store.ingest(spans()),0)
        self.assertEqual(self.scalar('SELECT COUNT(*) FROM agent_nodes'),0)
        self.assertEqual(self.scalar('SELECT COUNT(*) FROM spans'),0)

    def test_clean_queued_job_skips_but_force_and_missing_cache_still_derive(self):
        self.store.ingest(spans());original=derived.derive_trace
        with patch.object(derived,'derive_trace',wraps=original)as call:
            self.store._refresh_summaries([TRACE],require_dirty=True);self.assertEqual(call.call_count,0)
            self.store._refresh_summaries([TRACE]);self.assertEqual(call.call_count,1)
            with self.store._connect()as db:db.execute('DELETE FROM trace_summaries WHERE trace_id=?',(TRACE,))
            self.store._refresh_summaries([TRACE],require_dirty=True);self.assertEqual(call.call_count,2)
        self.assertEqual(self.scalar('SELECT COUNT(*) FROM trace_summaries'),1)

    def test_external_change_before_writer_acquisition_is_not_skipped(self):
        self.store.ingest(spans());original=self.store._connect;written=[]
        @contextmanager
        def connection():
            if not written:
                written.append(True)
                with closing(sqlite3.connect(self.path))as db,db:
                    db.execute('UPDATE spans SET attributes=json_set(attributes,\'$."gen_ai.usage.input_tokens"\',999) WHERE trace_id=?',(TRACE,))
            with original()as db:yield db
        with patch.object(self.store,'_connect',connection):self.store._refresh_summaries([TRACE],require_dirty=True)
        self.assertEqual(json.loads(self.scalar('SELECT summary_json FROM trace_summaries'))['input_tokens'],999)

    def test_external_change_after_snapshot_commit_retains_next_dirty_job(self):
        self.store.ingest(spans());started=threading.Event();done=threading.Event();errors=[]
        def external():
            try:
                with closing(sqlite3.connect(self.path,timeout=5))as db,db:
                    started.set();db.execute('UPDATE spans SET attributes=json_set(attributes,\'$."gen_ai.usage.input_tokens"\',999) WHERE trace_id=?',(TRACE,))
            except BaseException as exc:errors.append(exc)
            finally:done.set()
        thread=threading.Thread(target=external);original=derived.derive_trace
        def during(db,trace_id,*args,**kwargs):
            thread.start();self.assertTrue(started.wait(2));self.assertFalse(done.wait(.025))
            return original(db,trace_id,*args,**kwargs)
        with patch.object(derived,'derive_trace',side_effect=during):self.store._refresh_summaries([TRACE])
        thread.join(5);self.assertFalse(thread.is_alive());self.assertEqual(errors,[])
        self.assertEqual(json.loads(self.scalar('SELECT summary_json FROM trace_summaries'))['input_tokens'],5)
        self.assertEqual(self.scalar('SELECT COUNT(*) FROM finding_dirty_v1'),1)
        self.store._refresh_summaries([TRACE],require_dirty=True)
        self.assertEqual(json.loads(self.scalar('SELECT summary_json FROM trace_summaries'))['input_tokens'],999)
        self.assertEqual(self.scalar('SELECT COUNT(*) FROM finding_dirty_v1'),0)

    def test_async_missing_cache_repair_does_not_require_dirty_marker(self):
        self.store.ingest(spans());self.store.close()
        watcher=Store(self.path,async_derived_refresh=True);self.addCleanup(watcher.close)
        deadline=time.monotonic()+5
        while not watcher.index_status()['ready']and time.monotonic()<deadline:time.sleep(.01)
        with watcher._connect()as db:db.execute('DELETE FROM trace_summaries WHERE trace_id=?',(TRACE,))
        self.assertEqual(self.scalar('SELECT COUNT(*) FROM finding_dirty_v1'),0)
        with self.assertRaises(IndexNotReady):watcher.trace_overview(TRACE)
        deadline=time.monotonic()+5
        while not self.scalar('SELECT COUNT(*) FROM trace_summaries')and time.monotonic()<deadline:time.sleep(.01)
        self.assertEqual(watcher.trace_overview(TRACE)['summary']['input_tokens'],5)
        self.assertIsNone(watcher.last_derived_error)

    def test_diagnostic_is_generated_only_and_explicitly_nonpublishable(self):
        output=Path(self.tmp.name)/'diagnostic.json'
        run=subprocess.run([sys.executable,'scripts/profile_ingest_writes.py','--batches','1','--output',str(output)],
            cwd=Path(__file__).resolve().parents[1],capture_output=True,text=True,timeout=20)
        self.assertEqual(run.returncode,0,run.stderr)
        result=json.loads(output.read_text())
        self.assertTrue(result['diagnostic']);self.assertFalse(result['publishable'])
        self.assertEqual(result['counts'],dict(spans=2,trace_activity=1,agent_nodes=1))
        self.assertEqual([row['accepted']for row in result['writes']],[2])
        self.assertIsNone(result['derived_error']);self.assertTrue(result['events'])
        self.assertIn('observatory/core.py',result['source_files'])
