"""Bounded incoming demands use the normal worker; no stale success fallback."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
import copy
from pathlib import Path
import queue
import sqlite3
import threading
import time
import unittest
from unittest.mock import patch

from observatory.core import Store
from tests import test_derived_readiness_snapshot as base


class IncomingHTTPTests(unittest.TestCase):
    setUp=base.DerivedHTTPReadinessTests.setUp
    stop_server=base.DerivedHTTPReadinessTests.stop_server
    get=base.DerivedHTTPReadinessTests.get
    update=base.DerivedHTTPReadinessTests.update

    def test_small_continuous_writer_keeps_every_response_honest(self):
        accepted=queue.Queue();errors=[];responses=[];start=time.monotonic()
        stamp=self.model['end_unix_nano']
        def writer():
            try:
                for counter in range(1,9):
                    time.sleep(max(0,start+counter*.1-time.monotonic()))
                    model=copy.deepcopy(self.model)
                    model['end_unix_nano']=stamp+counter*10**6
                    model['attributes']['gen_ai.usage.input_tokens']=counter
                    self.store.ingest([model]);accepted.put(counter)
            except Exception as exc:errors.append(repr(exc))
            finally:accepted.put(None)
        worker=threading.Thread(target=writer);worker.start()
        while True:
            counter=accepted.get(timeout=5)
            if counter is None:break
            for path in ('/api/health','/api/sessions?limit=500'):
                began=time.monotonic();code,body=self.get(path)
                record=dict(accepted_counter=counter,path=path,status=code,elapsed_seconds=time.monotonic()-began)
                if path=='/api/health':
                    self.assertEqual(code,200,body);record['ready']=body['indexing']['ready']
                elif code==200:
                    row=next(row for row in body['sessions'] if row['session_id']=='snapshot')
                    observed=(row['last_unix_nano']-stamp)//10**6
                    self.assertGreaterEqual(observed,counter,'A write accepted before the read cannot be omitted from a successful snapshot')
                    self.assertEqual(row['input_tokens'],observed or 5,row)
                    self.assertTrue(row['usage_complete']);record.update(source_counter=observed,input_tokens=row['input_tokens'])
                else:
                    self.assertEqual(code,503,body);self.assertFalse(body['indexing']['ready']);record['reason']=body['indexing'].get('stage')
                responses.append(record)
        worker.join(timeout=3);self.assertFalse(worker.is_alive());self.assertEqual(errors,[])
        base.wait_ready(self.store)
        code,body=self.get('/api/sessions');self.assertEqual(code,200,body)
        self.assertEqual(body['sessions'][0]['input_tokens'],8)
        with self.store._read() as db:self.assertEqual(db.execute('SELECT COUNT(*) FROM spans').fetchone()[0],2)
        self.assertEqual(len(responses),16)
        self.responses=responses  # Allows a separate descriptive fixture smoke to retain every result.

    def test_slow_worker_is_503_and_wait_retains_no_read_lease(self):
        local=threading.local();original=self.store._read;incoming=self.store._incoming_status
        wait=self.store._derived_condition.wait;waits=[]
        @contextmanager
        def read():
            with original() as db:
                local.reads=getattr(local,'reads',0)+1
                try:yield db
                finally:local.reads-=1
        def status(deadline):
            local.probing=True
            try:return incoming(deadline)
            finally:local.probing=False
        def waiting(timeout=None):
            # The normal worker also waits; inspect the HTTP demand only.
            if threading.current_thread() not in (self.store._derived_thread,self.store._migration_thread):
                self.assertEqual(getattr(local,'reads',0),0)
                self.assertFalse(getattr(local,'probing',False));waits.append(timeout)
            return wait(timeout=timeout)
        with self.store._derived_lock:
            self.update(999)
            with patch.object(self.store,'_read',read),patch.object(self.store,'_incoming_status',status),patch.object(self.store._derived_condition,'wait',waiting):
                began=time.monotonic();code,body=self.get('/api/sessions');elapsed=time.monotonic()-began
            self.assertEqual(code,503,body);self.assertFalse(body['indexing']['ready'])
            self.assertLess(elapsed,.6,'Functional deadline regression; not a latency benchmark')
            self.assertTrue(waits);self.assertTrue(all(0<value<=.075 for value in waits))
            self.timeout_response=dict(status=code,indexing=body['indexing'],elapsed_seconds=elapsed)
        base.wait_ready(self.store)
        self.assertEqual(self.get('/api/sessions')[1]['sessions'][0]['input_tokens'],999)

    def test_initial_error_probe_and_missing_marker_do_not_arm_demand(self):
        cases=(dict(ready=False,stage='Preparing history'),dict(ready=True,error='fixture migration failure'))
        for state in cases:
            with self.subTest(state=state),patch.object(self.store,'migration_status',state):
                with patch.object(self.store,'_incoming_status',side_effect=AssertionError('Must not probe incoming eligibility')):
                    self.assertEqual(self.get('/api/sessions')[0],503)
                self.assertEqual(self.store._derived_demand_until,0)
        with patch.object(self.store,'catch_up_incoming',side_effect=AssertionError('Probe must remain cheap')):
            self.assertEqual(self.get('/api/health?probe=1'),(200,{'ok':True}))
        with patch.object(self.store,'last_derived_error','fixture worker failure'):
            result=self.store.catch_up_incoming(dict(ready=False,stage='Incoming source changes'))
            self.assertFalse(result['ready']);self.assertEqual(self.store._derived_demand_until,0)
        with self.store._connect() as db:db.execute("DELETE FROM schema_meta WHERE key='trace_derived_v15'")
        result=self.store.catch_up_incoming(dict(ready=False));self.assertFalse(result['ready'])
        self.assertEqual(self.store._derived_demand_until,0)

    def test_concurrent_demands_share_existing_job_and_all_timeouts_remain_visible(self):
        end=time.monotonic()+3
        while self.store._derived_pending and time.monotonic()<end:time.sleep(.01)
        refresh=self.store._refresh_summaries;batches=[];barrier=threading.Barrier(4)
        def derived(ids):batches.append(tuple(ids));return refresh(ids)
        def request():barrier.wait(timeout=3);return self.get('/api/sessions')
        with patch.object(self.store,'_refresh_summaries',derived),patch.object(self.store,'_drain_external_activity',return_value=0):
            with self.store._derived_lock:
                self.update(999)
                with ThreadPoolExecutor(max_workers=4) as pool:
                    futures=[pool.submit(request) for _ in range(4)]
                    results=[future.result(timeout=5) for future in futures]
                self.assertEqual([code for code,_ in results],[503]*4)
                self.assertTrue(all(not body['indexing']['ready'] for _,body in results))
                self.assertIsInstance(self.store._derived_demand_until,float)
                self.concurrent_responses=[dict(status=code,indexing=body['indexing']) for code,body in results]
            base.wait_ready(self.store)
            self.assertEqual(batches,[(base.TRACE,)])
            self.assertEqual(self.get('/api/sessions')[1]['sessions'][0]['input_tokens'],999)

    def test_external_queue_without_local_pending_does_not_fake_ready(self):
        end=time.monotonic()+3
        while self.store._derived_pending and time.monotonic()<end:time.sleep(.01)
        with self.store._derived_lock:
            with self.store._connect() as db:
                db.execute("UPDATE spans SET attributes=json_set(attributes,'$.\"gen_ai.usage.input_tokens\"',999) WHERE trace_id=? AND span_id=?",(base.TRACE,'2'*16))
            self.assertFalse(self.store._derived_pending)
            code,body=self.get('/api/sessions');self.assertEqual(code,503,body)
            self.assertFalse(body['indexing']['ready']);self.assertNotIn('sessions',body)
            self.external_response=dict(status=code,indexing=body['indexing'])
        base.wait_ready(self.store)
        self.assertEqual(self.get('/api/sessions')[1]['sessions'][0]['input_tokens'],999)

    def test_receiver_replacement_during_full_health_wait_is_not_published(self):
        replacement=Path(self.store.path).with_suffix('.replacement')
        other=Store(replacement);other_id=other.receiver_namespace();other.close()
        with sqlite3.connect(replacement) as db:db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        old_id=self.store.receiver_namespace();self.assertNotEqual(old_id,other_id)
        entered=threading.Event();release=threading.Event();original=self.store.catch_up_incoming
        def incoming(indexing):
            entered.set()
            if not release.wait(3):raise AssertionError('Fixture replacement deadline')
            return original(indexing)
        with self.store._derived_lock:
            self.update(999)
            with patch.object(self.store,'catch_up_incoming',incoming),ThreadPoolExecutor(max_workers=1) as pool:
                future=pool.submit(self.get,'/api/health')
                try:
                    self.assertTrue(entered.wait(3))
                    with sqlite3.connect(self.store.path) as db:db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
                    replacement.replace(self.store.path);release.set()
                    code,body=future.result(timeout=5)
                finally:release.set()
                self.assertEqual(code,409,body);self.assertNotIn('receiver_id',body)


class IncomingWorkerTests(unittest.TestCase):
    def test_priority_is_capped_oldest_first_and_existing_queue_drains_once(self):
        store=Store.__new__(Store);store._derived_condition=threading.Condition()
        now=time.monotonic();store._derived_pending={str(n):(now+n*.0001,now,0) for n in range(40)}
        store._derived_demand_until=now+.075;store._derived_generation=0;store._derived_stopping=False
        store.last_derived_error=None;batches=[]
        def refresh(ids):
            batches.append(list(ids))
            with store._derived_condition:
                store._derived_stopping=True;store._derived_condition.notify_all()
        store._refresh_summaries=refresh
        worker=threading.Thread(target=store._derived_refresh_worker);worker.start();worker.join(timeout=3)
        self.assertFalse(worker.is_alive());self.assertEqual(batches[0],[str(n) for n in range(16)])
        flattened=[value for batch in batches for value in batch]
        self.assertEqual(len(flattened),40);self.assertEqual(set(flattened),set(map(str,range(40))))
        self.assertEqual(store._derived_pending,{});self.assertEqual(store._derived_demand_until,0)

    def test_failed_priority_job_uses_existing_retry_and_completion_notification(self):
        store=Store.__new__(Store);store._derived_condition=threading.Condition()
        now=time.monotonic();store._derived_pending={'fixture':(now,now,0)}
        store._derived_demand_until=now+.075;store._derived_generation=0;store._derived_stopping=False
        store.last_derived_error=None;calls=[];first=threading.Event()
        def refresh(ids):
            calls.append(tuple(ids))
            if len(calls)==1:first.set();raise RuntimeError('Fixture derive failure')
            with store._derived_condition:store._derived_stopping=True;store._derived_condition.notify_all()
        store._refresh_summaries=refresh
        worker=threading.Thread(target=store._derived_refresh_worker);worker.start()
        self.assertTrue(first.wait(3));worker.join(timeout=3)
        self.assertFalse(worker.is_alive());self.assertEqual(calls,[('fixture',),('fixture',)])
        self.assertEqual(store._derived_generation,2);self.assertIsNone(store.last_derived_error)
        self.assertEqual(store._derived_pending,{})


if __name__=='__main__':unittest.main()
