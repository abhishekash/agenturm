"""Compatibility repair defers owned jobs without starving external sources."""
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from observatory.core import Store,IndexNotReady
import observatory.storage_derived as derived
from tests.test_ingest_writer_contention import spans,TRACE


class AvailableWorker:
    alive=True
    def is_alive(self):return self.alive
    def join(self,timeout=None):self.alive=False


class CompatOwnedJobsTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.store=Store(Path(self.temp.name)/'generated.db');self.addCleanup(self.store.close)
        self.store.ingest(spans())

    def local_worker(self):
        self.store._async_derived_refresh=True;self.store._derived_thread=AvailableWorker()

    def change(self,value=9,trace=TRACE):
        rows=spans();rows[0]['trace_id']=trace;rows[0]['attributes']['gen_ai.usage.input_tokens']=value
        return rows

    def scalar(self,sql,params=()):
        with self.store._read() as db:return db.execute(sql,params).fetchone()[0]

    def test_pending_and_popped_inflight_defer_but_force_and_unavailable_worker_do_not(self):
        self.local_worker();self.store.ingest(self.change())
        first=self.store._derived_pending[TRACE][0]
        with patch.object(derived,'derive_trace',wraps=derived.derive_trace) as calls:
            self.store._drain_external_activity();self.assertEqual(calls.call_count,0)
            self.assertEqual(self.store._derived_pending[TRACE][0],first)
            with self.assertRaises(IndexNotReady):self.store.sessions()
            with self.store._derived_condition:
                self.store._derived_pending.pop(TRACE);self.store._derived_inflight.add(TRACE)
            self.store._refresh_summaries([TRACE],require_dirty=True,defer_local=True)
            self.assertEqual(calls.call_count,0)
            self.store._refresh_summaries([TRACE])
            self.assertEqual(calls.call_count,1)
            self.assertEqual(self.store.sessions()[0]['input_tokens'],9)
            with self.store._connect() as db:
                db.execute("UPDATE spans SET attributes=json_set(attributes,'$.\"gen_ai.usage.input_tokens\"',19)")
            self.store._derived_thread.alive=False
            self.store._drain_external_activity();self.assertEqual(calls.call_count,2)
            self.assertEqual(self.store.sessions()[0]['input_tokens'],19)

    def test_external_trace_after_more_than64_owned_ids_and_cursor_wrap_is_repaired(self):
        records=[self.change(trace=f'{n:032x}') for n in range(1,82)]
        self.store.ingest([row for group in records for row in group]);self.local_worker()
        self.store._queue_derived(f'{n:032x}' for n in range(1,81))
        self.store._queue_derived([TRACE])  # The independent setUp trace is owned too.
        with self.store._connect() as db:
            db.execute("UPDATE spans SET attributes=json_set(attributes,'$.\"gen_ai.usage.input_tokens\"',29)")
        with patch.object(derived,'derive_trace',wraps=derived.derive_trace) as calls:
            self.store._drain_external_activity();self.assertEqual(calls.call_count,0)
            self.store._drain_external_activity();self.assertEqual(calls.call_count,1)
            self.assertEqual(calls.call_args.args[1],f'{81:032x}')
            self.assertEqual(self.scalar('SELECT COUNT(*) FROM finding_dirty_v1'),81)
            with self.store._derived_condition:self.store._derived_pending.pop(f'{1:032x}')
            self.store._drain_external_activity();self.assertEqual(calls.call_count,2)
            self.assertEqual(calls.call_args.args[1],f'{1:032x}')

    def test_raw_commit_to_queue_gap_keeps_compatibility_out_until_ownership_is_visible(self):
        self.local_worker();committed=threading.Event();release=threading.Event();selected=threading.Event()
        queue=self.store._queue_derived;select=self.store._compat_traces
        def admission(traces):
            self.assertEqual(self.scalar("SELECT json_extract(attributes,'$.\"gen_ai.usage.input_tokens\"') FROM spans"),9)
            committed.set()
            if not release.wait(3):raise AssertionError('Fixture release deadline')
            return queue(traces)
        def selecting(db):selected.set();return select(db)
        with patch.object(self.store,'_queue_derived',admission),patch.object(self.store,'_compat_traces',selecting),patch.object(derived,'derive_trace',wraps=derived.derive_trace) as calls:
            with ThreadPoolExecutor(max_workers=2) as pool:
                raw=pool.submit(self.store.ingest,self.change())
                try:
                    self.assertTrue(committed.wait(3));compat=pool.submit(self.store._drain_external_activity)
                    self.assertFalse(selected.wait(.05));release.set()
                    self.assertEqual(raw.result(3),1);compat.result(3)
                finally:release.set()
            self.assertTrue(selected.is_set());self.assertEqual(calls.call_count,0)
            self.assertIn(TRACE,self.store._derived_pending)
            with self.assertRaises(IndexNotReady):self.store.sessions()

    def test_selected_external_job_is_rechecked_after_a_new_local_commit(self):
        self.local_worker()
        with self.store._connect() as db:db.execute("UPDATE spans SET attributes=json_set(attributes,'$.\"gen_ai.usage.input_tokens\"',19)")
        selected=threading.Event();release=threading.Event();refresh=self.store._refresh_summaries
        def delayed(ids,**kwargs):
            if kwargs.get('defer_local'):
                selected.set()
                if not release.wait(3):raise AssertionError('Fixture release deadline')
            return refresh(ids,**kwargs)
        with patch.object(self.store,'_refresh_summaries',delayed),patch.object(derived,'derive_trace',wraps=derived.derive_trace) as calls:
            with ThreadPoolExecutor(max_workers=1) as pool:
                future=pool.submit(self.store._drain_external_activity)
                try:
                    self.assertTrue(selected.wait(3));self.store.ingest(self.change(39));release.set();future.result(3)
                finally:release.set()
            self.assertEqual(calls.call_count,0)
        self.store._refresh_summaries([TRACE],require_dirty=True)
        self.assertEqual(self.store.sessions()[0]['input_tokens'],39)

    def test_synchronous_derivation_runs_after_writer_gate_release(self):
        refresh=self.store._refresh_summaries;observed=[]
        def check(ids,**kwargs):
            def probe():
                acquired=self.store._writer_gate.acquire(timeout=.5)
                observed.append(acquired)
                if acquired:self.store._writer_gate.release()
            thread=threading.Thread(target=probe);thread.start();thread.join(1)
            self.assertFalse(thread.is_alive());self.assertEqual(observed,[True])
            return refresh(ids,**kwargs)
        with patch.object(self.store,'_refresh_summaries',check):self.store.ingest(self.change())
        self.assertEqual(self.store.sessions()[0]['input_tokens'],9)

    def test_all_source_producers_publish_after_commit_and_preserve_dirty_source_on_error(self):
        self.local_worker();queue=self.store._queue_derived;published=[]
        def admit(ids):published.append(set(ids));return queue(ids)
        row=self.change()[0];row['resource_attributes']['observatory.source_id']='f'*32
        with patch.object(self.store,'_queue_derived',admit):
            self.store.ingest([row])
            self.store.ingest_logs([dict(trace_id=TRACE,span_id=None,time_unix_nano=200,service='generated',severity_number=9,
                severity_text='INFO',event_name='info',body='Generated',attributes={},resource_attributes={},scope_name='fixture')])
            self.store.add_progress(TRACE,'test','Generated progress')
            self.store.ingest_outcomes([dict(trace_id=TRACE,passed=True)])
            self.store.redact_internal_source('f'*32)
        self.assertEqual(published,[{TRACE}]*5)
        self.assertEqual(self.scalar('SELECT COUNT(*) FROM finding_dirty_v1'),1)
        with patch.object(self.store,'_write_ingest_rows',side_effect=RuntimeError('Synthetic raw rollback')):
            with self.assertRaisesRegex(RuntimeError,'Synthetic raw'):self.store.ingest(self.change(999))
        self.assertNotEqual(self.scalar("SELECT json_extract(attributes,'$.\"gen_ai.usage.input_tokens\"') FROM spans"),999)


class WorkerOwnershipTests(unittest.TestCase):
    def test_bounded_oldest_batches_fairness_and_inflight_cleanup(self):
        store=Store.__new__(Store);store._derived_condition=threading.Condition();store._derived_inflight=set()
        now=time.monotonic();store._derived_pending={f'{n:032x}':(now-3+n*.001,now-1,0) for n in range(40)}
        store._derived_demand_until=0;store._derived_generation=0;store._derived_stopping=False;store.last_derived_error=None
        batches=[];stamps=[]
        def refresh(ids,**kwargs):
            with store._derived_condition:
                self.assertEqual(set(ids),store._derived_inflight);self.assertLessEqual(len(ids),16)
                self.assertFalse(set(ids)&set(store._derived_pending))
                batches.append(list(ids));stamps.append(time.monotonic())
                if not store._derived_pending:store._derived_stopping=True
        store._refresh_summaries=refresh
        worker=threading.Thread(target=store._derived_refresh_worker);worker.start();worker.join(3)
        self.assertFalse(worker.is_alive());self.assertEqual([len(ids)for ids in batches],[16,16,8])
        self.assertEqual([identity for ids in batches for identity in ids],[f'{n:032x}'for n in range(40)])
        self.assertTrue(all(b-a>=.02 for a,b in zip(stamps,stamps[1:])))
        self.assertEqual(store._derived_inflight,set());self.assertEqual(store._derived_pending,{})
