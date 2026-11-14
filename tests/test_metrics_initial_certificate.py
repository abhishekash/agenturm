"""Metrics may ignore search/native work, never its derived usage dependencies."""
from contextlib import closing
from copy import deepcopy
import json
import os
from pathlib import Path
import sqlite3
import threading
import unittest
from unittest.mock import patch

from tests import test_mcp_initial_certificate as fixture


class MetricsInitialCertificateTests(unittest.TestCase):
    def setUp(self):
        fixture.McpInitialCertificateTests.setUp(self)
        self.record['attributes']['gen_ai.usage.input_tokens']=5
        self.store.ingest([self.record])

    def get(self,path='/api/metrics'):
        return fixture.McpInitialCertificateTests.get(self,path)

    def test_full_aggregate_ignores_only_search_and_native_inventory(self):
        expected=self.store.metrics();self.assertEqual(expected['input_tokens'],5)
        for queue in ('investigation','native_lifecycle'):
            with self.subTest(queue=queue):
                with self.store._connect()as db:
                    db.execute('INSERT INTO '+queue+"_dirty_v1 VALUES('log','generated')")
                self.assertFalse(self.store.index_status()['ready'])
                with patch.object(self.store,'catch_up_incoming',wraps=self.store.catch_up_incoming)as catch_up,patch.object(self.handler.response_cache,'get',wraps=self.handler.response_cache.get)as cached,patch.object(self.handler.response_cache,'put',wraps=self.handler.response_cache.put)as put:
                    self.assertEqual(self.get(),(200,expected))
                    catch_up.assert_not_called()
                    cached.assert_not_called();put.assert_not_called()
                with self.store._connect()as db:db.execute('DELETE FROM '+queue+'_dirty_v1')

    def test_activity_and_findings_remain_required_even_with_warm_cache(self):
        expected=self.get();self.assertEqual(expected[0],200)
        for queue,key in (('activity',('span','generated')),('finding',('a'*32,))):
            with self.subTest(queue=queue):
                with self.store._connect()as db:
                    db.execute('INSERT INTO '+queue+'_dirty_v1 VALUES('+','.join('?' for _ in key)+')',key)
                self.assertEqual(self.get()[0],503)
                with self.store._connect()as db:db.execute('DELETE FROM '+queue+'_dirty_v1')
                self.assertEqual(self.get(),expected)

    def test_each_cached_read_certifies_markers_initial_fatal_and_stopping(self):
        with patch.object(self.store,'_metrics',wraps=self.store._metrics)as read,patch.object(self.store,'certify_initial_metrics',wraps=self.store.certify_initial_metrics)as certify:
            expected=self.get();self.assertEqual(self.get(),expected);self.assertEqual(read.call_count,1);self.assertEqual(certify.call_count,2)
            for marker in ('trace_derived_v15','activity_sources_v2','activity_operation_facts_v2','live_stats_v3'):
                with self.store._connect()as db:db.execute('DELETE FROM schema_meta WHERE key=?',(marker,))
                self.assertEqual(self.get()[0],503,marker)
                with self.store._connect()as db:db.execute('INSERT INTO schema_meta VALUES(?,?)',(marker,'1'))
            stopped=threading.Event();stopped.set()
            for field,value in (('migration_status',dict(ready=False,stage='Preparing indexed history')),
                    ('migration_status',dict(ready=True,error='Generated fatal')),
                    ('last_derived_error','Generated failure'),('_derived_stopping',True),('_migration_stop',stopped)):
                with patch.object(self.store,field,value):self.assertEqual(self.get()[0],503)

    def raw_update(self):
        changed=deepcopy(self.record['attributes']);changed['gen_ai.usage.input_tokens']=999
        with closing(sqlite3.connect(self.store.path))as db,db:
            db.execute('UPDATE spans SET attributes=? WHERE trace_id=?',(json.dumps(changed),fixture.TRACE))

    def test_source_commit_after_probe_cannot_serve_old_totals_or_repair(self):
        self.assertEqual(self.get()[1]['input_tokens'],5);original=self.store.index_status;fired=[]
        def status():
            result=original()
            if not fired:fired.append(True);self.raw_update()
            return result
        with patch.object(self.store,'index_status',side_effect=status),patch.object(self.store,'_refresh_summaries',side_effect=AssertionError('HTTP must not repair')):
            self.assertEqual(self.get()[0],503)
        self.store._drain_external_activity()
        self.assertEqual(self.get()[1]['input_tokens'],999)

    def test_raw_and_derived_fields_share_the_certified_snapshot(self):
        expected=self.store.metrics();original=self.store._require_initial_metrics;fired=[]
        def require(*args,**kwargs):
            original(*args,**kwargs)
            if not fired:
                fired.append(True)
                self.raw_update()
                with closing(sqlite3.connect(self.store.path))as db,db:
                    db.execute("UPDATE spans SET service='after-snapshot' WHERE trace_id=?",(fixture.TRACE,))
        # Isolate hydration certification: pre-cache proof still uses the real certificate.
        certify=self.store.certify_initial_metrics
        with patch.object(self.store,'certify_initial_metrics',side_effect=certify),patch.object(self.store,'_metrics',wraps=self.store._metrics):
            identity=certify(self.store.receiver_id)
            with patch.object(self.store,'_require_initial_metrics',side_effect=require):
                self.assertEqual(self.store.metrics(_initial_receiver=self.store.receiver_id,_initial_file=identity),expected)
        self.assertEqual(self.get()[0],503)
        self.store._drain_external_activity()
        self.assertEqual(self.get()[1]['input_tokens'],999)
        self.assertIn('after-snapshot',self.get()[1]['services'])

    def test_missing_file_after_certificate_does_not_recreate_database(self):
        original=self.store.metrics
        def metrics(*args,**kwargs):Path(self.store.path).unlink();return original(*args,**kwargs)
        with patch.object(self.store,'metrics',side_effect=metrics):self.assertEqual(self.get()[0],503)
        self.assertFalse(Path(self.store.path).exists())

    def test_schema_errors_in_certificate_or_hydration_are_typed_503(self):
        with patch.object(self.store,'_require_initial_metrics',side_effect=sqlite3.OperationalError('generated unavailable schema')):
            self.assertEqual(self.get()[0],503)
        with patch.object(self.store,'_metrics',side_effect=sqlite3.OperationalError('generated unavailable schema')):
            self.assertEqual(self.get()[0],503)

    def test_cached_response_rejects_same_namespace_file_replacement(self):
        self.assertEqual(self.get()[0],200)
        replacement=Path(self.temp.name)/'replacement.db'
        with self.store._connect()as db:db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        with closing(sqlite3.connect(self.store.path))as source,closing(sqlite3.connect(replacement))as target:source.backup(target)
        self.assertEqual(self.get()[0],200);original=self.handler.response_cache.get
        def cache(key):
            result=original(key);self.assertIsNotNone(result[1]);os.replace(replacement,self.store.path);return result
        with patch.object(self.handler.response_cache,'get',side_effect=cache):self.assertEqual(self.get()[0],409)
