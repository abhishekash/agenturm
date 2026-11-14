"""Session projections keep usage/source readiness, independent of search/native inventory."""
import sqlite3
import unittest
from unittest.mock import patch
from tests import test_mcp_initial_certificate as fixture
from tests import test_metrics_initial_certificate as metrics_fixture


class SessionsInitialCertificateTests(unittest.TestCase):
    def setUp(self):
        metrics_fixture.MetricsInitialCertificateTests.setUp(self)

    def get(self,path='/api/sessions'):
        return fixture.McpInitialCertificateTests.get(self,path)

    def test_full_session_projection_ignores_only_search_and_native_inventory(self):
        expected={'sessions':self.store.sessions()};self.assertEqual(expected['sessions'][0]['input_tokens'],5)
        for queue in ('investigation','native_lifecycle'):
            with self.subTest(queue=queue):
                with self.store._connect()as db:db.execute('INSERT INTO '+queue+"_dirty_v1 VALUES('log','generated')")
                self.assertFalse(self.store.index_status()['ready'])
                with patch.object(self.store,'catch_up_incoming',wraps=self.store.catch_up_incoming)as repair,patch.object(self.handler.response_cache,'get',wraps=self.handler.response_cache.get)as cached,patch.object(self.handler.response_cache,'put',wraps=self.handler.response_cache.put)as put:
                    self.assertEqual(self.get(),(200,expected));repair.assert_not_called();cached.assert_not_called();put.assert_not_called()
                with self.store._connect()as db:db.execute('DELETE FROM '+queue+'_dirty_v1')

    def test_cached_sessions_require_both_real_queues_and_each_marker(self):
        self.assertEqual(self.get()[0],200)
        for queue,key in (('activity',('span','generated')),('finding',('a'*32,))):
            with self.subTest(queue=queue):
                with self.store._connect()as db:db.execute('INSERT INTO '+queue+'_dirty_v1 VALUES('+','.join('?' for _ in key)+')',key)
                self.assertEqual(self.get()[0],503)
                with self.store._connect()as db:db.execute('DELETE FROM '+queue+'_dirty_v1')
        for marker in ('trace_derived_v15','activity_sources_v2','activity_operation_facts_v2','live_stats_v3'):
            with self.store._connect()as db:db.execute('DELETE FROM schema_meta WHERE key=?',(marker,))
            self.assertEqual(self.get()[0],503)
            with self.store._connect()as db:db.execute('INSERT INTO schema_meta VALUES(?,?)',(marker,'1'))
        with patch.object(self.store,'_sessions',wraps=self.store._sessions)as read,patch.object(self.store,'certify_initial_sessions',wraps=self.store.certify_initial_sessions)as certify:
            self.assertEqual(self.get()[0],200);self.assertEqual(self.get()[0],200);self.assertEqual(read.call_count,1);self.assertEqual(certify.call_count,2)
            for field,value in (('migration_status',dict(ready=False)),('migration_status',dict(ready=True,error='Generated fatal')),('last_derived_error','Generated fatal'),('_derived_stopping',True)):
                with patch.object(self.store,field,value):self.assertEqual(self.get()[0],503)

    def test_source_change_after_probe_cannot_certify_stale_sessions(self):
        self.assertEqual(self.get()[1]['sessions'][0]['input_tokens'],5);original=self.store.index_status;fired=[]
        def status():
            result=original()
            if not fired:fired.append(True);metrics_fixture.MetricsInitialCertificateTests.raw_update(self)
            return result
        with patch.object(self.store,'index_status',side_effect=status),patch.object(self.store,'_refresh_summaries',side_effect=AssertionError('HTTP must not repair')):
            self.assertEqual(self.get()[0],503)
        self.store._drain_external_activity();self.assertEqual(self.get()[1]['sessions'][0]['input_tokens'],999)

    def test_schema_failures_remain_typed_503(self):
        with patch.object(self.store,'_require_initial_sessions',side_effect=sqlite3.OperationalError('generated unavailable schema')):
            self.assertEqual(self.get()[0],503)
        with patch.object(self.store,'_sessions',side_effect=sqlite3.OperationalError('generated unavailable schema')):
            self.assertEqual(self.get()[0],503)
