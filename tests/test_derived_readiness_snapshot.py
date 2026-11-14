"""Derived HTTP reads either certify one source snapshot or return retryable 503."""
from contextlib import contextmanager
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
import urllib.error
import urllib.request

from observatory.core import IndexNotReady,Store,normalize_native
from observatory.server import BoundedHTTPServer,handler_for

TRACE='a'*32

def fixture(store):
    stamp=time.time_ns()-10**9
    attrs={'session.id':'snapshot','agent.id':'worker'}
    def span(id,name,usage=None):
        return normalize_native(dict(trace_id=TRACE,span_id=id*16,parent_span_id=None if id=='1' else '1'*16,
            name=name,start_unix_nano=stamp,end_unix_nano=stamp+100,status='OK',
            attributes={**attrs,**(usage or {})},events=[]),'fixture')
    model=span('2','chat fixture',{'gen_ai.usage.input_tokens':5,'gen_ai.usage.output_tokens':2})
    store.ingest([span('1','agent.run'),model]);return model


def wait_ready(store):
    end=time.monotonic()+5
    while time.monotonic()<end:
        if store.index_status()['ready']:return
        time.sleep(.01)
    raise AssertionError('Tiny fixture did not drain')


class DerivedHTTPReadinessTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.store=Store(Path(self.tmp.name)/'fixture.db',async_derived_refresh=True);self.addCleanup(self.store.close)
        self.model=fixture(self.store);wait_ready(self.store)
        self.handler=handler_for(self.store,'fixture-token',quiet=True)
        self.server=BoundedHTTPServer(('127.0.0.1',0),self.handler)
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        self.addCleanup(self.stop_server)

    def stop_server(self):
        self.server.shutdown();self.server.server_close();self.thread.join(timeout=3);self.handler.response_cache.close()

    def get(self,path,token='fixture-token'):
        request=urllib.request.Request(f'http://127.0.0.1:{self.server.server_port}'+path,
            headers={'Authorization':'Bearer '+token})
        try:
            with urllib.request.urlopen(request,timeout=5) as response:return response.status,json.load(response)
        except urllib.error.HTTPError as exc:return exc.code,json.load(exc)

    def update(self,value):
        self.model['attributes']['gen_ai.usage.input_tokens']=value;self.store.ingest([self.model])

    def paths(self):
        return ('/api/sessions?limit=500','/api/review-run?trace_id='+TRACE,
            '/api/traces?limit=1','/api/traces/'+TRACE,'/api/traces/'+TRACE+'?span_limit=2',
            '/api/metrics','/api/usage-series?days=1&step=hour')

    def tokens(self,path,payload):
        if path.startswith('/api/sessions'):return next(row for row in payload['sessions'] if row['session_id']=='snapshot')['input_tokens']
        if path.startswith('/api/review-run'):return payload['run']['input_tokens']
        if path.startswith('/api/traces?'):return payload['traces'][0]['input_tokens']
        if path.startswith('/api/traces/'):return payload['summary']['input_tokens']
        if path.startswith('/api/usage-series'):return sum(row['input_tokens'] for row in payload['points'])
        return payload['input_tokens']

    def test_commit_after_ready_probe_is_503_not_stale_complete_data(self):
        for path in self.paths():
            with self.subTest(path=path):
                self.update(5);wait_ready(self.store)
                captured=threading.Event();release=threading.Event();original=self.store.index_status
                def status(**kwargs):
                    value=original(**kwargs)
                    if kwargs.get('_db') is None:
                        self.assertTrue(value['ready']);captured.set()
                        if not release.wait(3):raise AssertionError('Probe release deadline')
                    return value
                with self.store._derived_lock,patch.object(self.store,'index_status',status):
                    with ThreadPoolExecutor(max_workers=1) as pool:
                        future=pool.submit(self.get,path)
                        try:
                            self.assertTrue(captured.wait(3));self.update(999)
                            self.assertFalse(original()['ready']);release.set()
                            code,body=future.result(timeout=5)
                        finally:release.set()
                    self.assertEqual(code,503,body);self.assertFalse(body['indexing']['ready'])
                    self.assertEqual(body['error'],'History index is not ready')
                    self.assertNotIn('sessions',body)
                wait_ready(self.store);code,body=self.get(path)
                self.assertEqual(code,200,body);self.assertEqual(self.tokens(path,body),999)

    def test_commit_after_guard_pins_old_raw_and_derived_snapshot(self):
        for index,path in enumerate(self.paths()):
            with self.subTest(path=path):
                self.update(5);wait_ready(self.store);old_logs=self.store._log_count()
                captured=threading.Event();release=threading.Event();original=self.store._require_index_ready
                def require(db):
                    original(db)
                    if not captured.is_set():
                        captured.set()
                        if not release.wait(3):raise AssertionError('Snapshot release deadline')
                        raw=db.execute('SELECT attributes FROM spans WHERE trace_id=? AND span_id=?',(TRACE,'2'*16)).fetchone()[0]
                        self.assertEqual(json.loads(raw)['gen_ai.usage.input_tokens'],5)
                with self.store._derived_lock,patch.object(self.store,'_require_index_ready',require):
                    with ThreadPoolExecutor(max_workers=1) as pool:
                        future=pool.submit(self.get,path)
                        try:
                            self.assertTrue(captured.wait(3));self.update(999)
                            self.store.ingest_logs([dict(trace_id=None,span_id=None,time_unix_nano=time.time_ns(),
                                service='after-guard-'+str(index),event_name='info',severity_number=9,severity_text='INFO',
                                body='Generated after guard',attributes={},resource_attributes={},scope_name='fixture')])
                            release.set();code,body=future.result(timeout=5)
                        finally:release.set()
                    self.assertEqual(code,200,body);self.assertEqual(self.tokens(path,body),5)
                    if path=='/api/metrics':
                        self.assertEqual(body['logs'],old_logs);self.assertNotIn('after-guard-'+str(index),body['services'])
                    if path.startswith('/api/traces/'):
                        model=next(row for row in body['spans'] if row['span_id']=='2'*16)
                        self.assertEqual(model['attributes']['gen_ai.usage.input_tokens'],5)
                wait_ready(self.store);code,body=self.get(path)
                self.assertEqual(code,200,body);self.assertEqual(self.tokens(path,body),999)

    def test_async_missing_cache_schedules_repair_and_returns_503(self):
        with self.store._derived_lock:
            with self.store._connect() as db:db.execute('DELETE FROM trace_summaries WHERE trace_id=?',(TRACE,))
            code,body=self.get('/api/traces?limit=1')
            self.assertEqual(code,503,body);self.assertFalse(body['indexing']['ready'])
            self.assertNotIn('traces',body)
        end=time.monotonic()+5
        while time.monotonic()<end:
            with self.store._connect() as db:
                repaired=db.execute('SELECT 1 FROM trace_summaries WHERE trace_id=?',(TRACE,)).fetchone()
            if repaired:break
            time.sleep(.01)
        self.assertTrue(repaired,'Existing async worker did not repair the missing cache')
        code,body=self.get('/api/traces?limit=1');self.assertEqual(code,200,body)
        self.assertEqual(body['traces'][0]['input_tokens'],5)

    def test_initial_migration_error_and_auth_remain_explicit(self):
        with patch.object(self.store,'migration_status',dict(ready=False,stage='Preparing indexed history',processed=0)):
            self.assertEqual(self.get('/api/sessions')[0],503)
            self.assertEqual(self.get('/api/sessions',token='wrong')[0],401)
        with patch.object(self.store,'migration_status',dict(ready=True,stage='History migration failed',error='fixture failure')):
            code,body=self.get('/api/sessions');self.assertEqual(code,503);self.assertEqual(body['indexing']['error'],'fixture failure')
        self.assertEqual(self.get('/api/traces?cursor=invalid')[0],400)


class DerivedStoreReadinessTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.store=Store(Path(self.tmp.name)/'fixture.db');self.addCleanup(self.store.close);fixture(self.store)

    def test_missing_cache_repairs_outside_read_and_restarts(self):
        for read in (lambda:self.store.trace_page(),lambda:self.store.trace_overview(TRACE)):
            with self.store._connect() as db:db.execute('DELETE FROM trace_summaries WHERE trace_id=?',(TRACE,))
            active=[];original=self.store._connect;refresh=self.store._refresh_summaries;calls=[]
            @contextmanager
            def connection():
                with original() as db:
                    active.append(db)
                    try:yield db
                    finally:active.remove(db)
            def repair(traces):
                self.assertEqual(active,[],'Cache repair must not retain the old read transaction');calls.append(tuple(traces));return refresh(traces)
            with patch.object(self.store,'_connect',connection),patch.object(self.store,'_refresh_summaries',repair):
                result=read()
            self.assertEqual(calls,[(TRACE,)])
            self.assertEqual((result.get('summary') or result['traces'][0])['input_tokens'],5)
        self.assertIsNone(self.store.trace_overview('f'*32)['summary'])

    def test_queued_external_source_or_missing_backfill_cannot_certify_reads(self):
        with self.store._connect() as db:
            db.execute("UPDATE spans SET attributes=json_set(attributes,'$.\"gen_ai.usage.input_tokens\"',999) WHERE trace_id=? AND span_id=?",(TRACE,'2'*16))
        for read in (self.store.sessions,lambda:self.store.review_run(TRACE),self.store.metrics,
            self.store.usage_series,self.store.trace_page,lambda:self.store.trace_overview(TRACE),lambda:self.store.trace(TRACE)):
            with self.subTest(read=read),self.assertRaises(IndexNotReady):read()
        self.store._drain_external_activity();self.assertEqual(self.store.sessions()[0]['input_tokens'],999)
        with self.store._connect() as db:db.execute("DELETE FROM schema_meta WHERE key='trace_derived_v15'")
        with self.assertRaises(IndexNotReady):self.store.sessions()

    def test_independent_queues_do_not_certify_initial_or_changed_sources(self):
        with self.store._connect() as db:
            db.execute("INSERT INTO investigation_dirty_v1 VALUES('log','unrelated')")
            db.execute("INSERT INTO native_lifecycle_dirty_v1 VALUES('log','unrelated')")
        self.assertFalse(self.store.index_status()['ready'],'Global HTTP preflight keeps every queue')
        self.assertEqual(self.store.sessions()[0]['input_tokens'],5)
        with patch.object(self.store,'migration_status',dict(ready=False,stage='Preparing indexed history')):
            with self.assertRaises(IndexNotReady):self.store.sessions()
        with patch.object(self.store,'migration_status',dict(ready=True,error='fixture failure')):
            with self.assertRaises(IndexNotReady):self.store.sessions()
        with self.store._connect() as db:
            db.execute("UPDATE spans SET attributes=json_set(attributes,'$.\"gen_ai.usage.input_tokens\"',999) WHERE trace_id=? AND span_id=?",(TRACE,'2'*16))
        with self.assertRaises(IndexNotReady):self.store.sessions()

    def test_nested_native_turn_uses_the_same_session_snapshot(self):
        from tests import test_opencode_turn_proof as native_fixture
        case=native_fixture.TurnProofTests();case.setUp()
        try:
            case.prepare();store=case.store;store._drain_external_activity();original=store.review_turn;written=[]
            def review(trace_id,*,_db=None):
                self.assertIsNotNone(_db);self.assertTrue(_db.in_transaction)
                if not written:
                    with store._connect() as writer:
                        row=writer.execute("SELECT span_id,attributes FROM spans WHERE trace_id=? AND name='chat model-x'",(case.traces[0],)).fetchone()
                        attrs=json.loads(row['attributes']);attrs['gen_ai.usage.input_tokens']=999
                        writer.execute('UPDATE spans SET attributes=? WHERE trace_id=? AND span_id=?',(json.dumps(attrs),case.traces[0],row['span_id']))
                    written.append(True)
                return original(trace_id,_db=_db)
            with patch.object(store,'review_turn',review):
                sessions=store.sessions()
            self.assertTrue(written)
            native=next(row for row in sessions if row['session_id']=='ses_test')
            self.assertEqual(native['input_tokens'],17)
            self.assertEqual(native['latest_run']['turn_usage']['input_tokens'],17)
            store._drain_external_activity()
            current=next(row for row in store.sessions() if row['session_id']=='ses_test')
            self.assertEqual(current['latest_run']['turn_usage']['input_tokens'],1004)
        finally:case.doCleanups()

    def test_raw_mcp_does_not_gain_a_derived_summary_dependency(self):
        with self.store._connect() as db:
            db.execute("UPDATE spans SET attributes=json_set(attributes,'$.\"mcp.server.name\"','fixture','$.\"mcp.tool.name\"','read','$.\"output.value\"',json('false')) WHERE trace_id=? AND span_id=?",(TRACE,'2'*16))
        self.assertFalse(self.store.index_status()['ready'])
        page=self.store.mcp_page();self.assertEqual(page['summary']['calls'],1)
        self.assertIs(page['calls'][0]['output'],False);self.assertTrue(page['calls'][0]['output_reported'])


if __name__=='__main__':unittest.main()
