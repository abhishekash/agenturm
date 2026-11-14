"""Durable review receipts are private, bounded and safe across cockpits."""
import concurrent.futures
from contextlib import closing
from enum import Enum
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import threading
import unittest
from unittest.mock import patch

from observatory.attention import classify
from observatory.workflow_evidence import scope_for
from observatory.workflow_store import WorkflowStore, WorkflowStoreError, WorkflowSchemaError
from tests.test_attention import item,snapshot


class ClaimResult(Enum):
    DEFERRED='SQLite writer contention; claim outcome is unavailable'


def claim_or_deferred(store,incident,*,now):
    try:return store.claim(incident.namespace,incident.id,incident.scope_key,now=now)
    except WorkflowStoreError as exc:
        cause=exc.__cause__
        if (isinstance(cause,sqlite3.OperationalError) and
                getattr(cause,'sqlite_errorcode',None) in {sqlite3.SQLITE_BUSY,sqlite3.SQLITE_LOCKED}):
            return ClaimResult.DEFERRED
        raise


class WorkflowStoreTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.path=Path(self.tmp.name)/'state'/'workflow.sqlite3'
        self.store=WorkflowStore(self.path)

    def incident(self,request='one',receiver='receiver-one'):
        selected=item(request_id=request)
        return classify(selected,scope_for(selected,snapshot(receiver_id=receiver)),{}, {},now=100)[0]

    def test_file_permissions_restart_and_content_allowlist(self):
        record={**self.incident().to_record(),'prompt':'SECRET','command':'private shell','task':'private task'}
        self.store.observe([record],now=100)
        restored=WorkflowStore(self.path).load(now=101)
        self.assertEqual(len(restored),1)
        self.assertEqual(self.path.stat().st_mode&0o777,0o600)
        self.assertNotIn('SECRET',json.dumps(restored));self.assertNotIn('private',json.dumps(restored))

    def test_acknowledgement_is_exact_reversible_and_expires_at_ninety_days(self):
        incident=self.incident();self.store.observe([incident],now=100)
        self.store.acknowledge(incident.namespace,incident.id,incident.scope_key,now=100)
        self.assertEqual(self.store.load(now=101)[0]['acknowledgement'],'seen')
        with self.assertRaises(WorkflowStoreError):
            self.store.acknowledge(incident.namespace,incident.id,'another-run')
        self.store.acknowledge(incident.namespace,incident.id,incident.scope_key,enabled=False)
        self.assertIsNone(self.store.load(now=101)[0]['acknowledgement'])
        self.store.acknowledge(incident.namespace,incident.id,incident.scope_key,now=100)
        self.assertIsNone(self.store.load(now=101+90*86400)[0]['acknowledgement'])

    def test_two_cockpits_claim_once_then_failed_attempt_needs_explicit_retry(self):
        incident=self.incident();self.store.observe([incident],now=100)
        other=WorkflowStore(self.path)
        def claim(store):return claim_or_deferred(store,incident,now=101)
        stores=[self.store,other]
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            results=list(pool.map(claim,stores))
        self.assertEqual(sum(result is True for result in results),1)
        # A bounded busy timeout is unavailable, never an optimistic "already
        # claimed" result. Settle only after both concurrent attempts stop.
        settled=[store.claim(incident.namespace,incident.id,incident.scope_key,now=101)
                 if result is ClaimResult.DEFERRED else result for store,result in zip(stores,results)]
        self.assertEqual(sorted(settled),[False,True])
        with closing(sqlite3.connect(self.path)) as db:
            self.assertEqual(db.execute('SELECT COUNT(*) FROM workflow_notification_claims_v1').fetchone()[0],1)
        self.store.finish_delivery(incident.namespace,incident.id,status='failed',now=102)
        self.assertFalse(claim(other))
        self.assertTrue(other.claim(incident.namespace,incident.id,incident.scope_key,retry=True,now=103))
        self.assertFalse(self.store.claim(incident.namespace,incident.id,incident.scope_key,retry=True,now=103))
        other.finish_delivery(incident.namespace,incident.id,status='delivered',now=104)
        self.assertEqual(self.store.load(now=105)[0]['delivery_attempts'],2)

    def test_busy_contender_never_records_an_optimistic_claim_before_holder_commits(self):
        incident=self.incident();self.store.observe([incident],now=100)
        other=WorkflowStore(self.path)
        holding=threading.Event();release=threading.Event();connect=sqlite3.connect
        def observed_connect(*args,**kwargs):
            db=connect(*args,**kwargs)
            if threading.current_thread().name.startswith('held-claim'):
                def trace(statement):
                    if statement.startswith('SELECT payload FROM workflow_incidents_v1'):
                        # This real SELECT follows the real BEGIN IMMEDIATE;
                        # only the test holder's scheduling is controlled.
                        holding.set();release.wait()
                db.set_trace_callback(trace)
            return db
        with patch('observatory.workflow_store.sqlite3.connect',side_effect=observed_connect):
            with concurrent.futures.ThreadPoolExecutor(max_workers=1,thread_name_prefix='held-claim') as pool:
                holder=pool.submit(self.store.claim,incident.namespace,incident.id,incident.scope_key,now=101)
                try:
                    self.assertTrue(holding.wait(5),'Real claim holder never acquired its write transaction')
                    deferred=claim_or_deferred(other,incident,now=101)
                    self.assertIs(deferred,ClaimResult.DEFERRED)
                    with closing(connect(self.path)) as db:
                        self.assertEqual(db.execute('SELECT COUNT(*) FROM workflow_notification_claims_v1').fetchone()[0],0)
                finally:release.set()
                self.assertIs(holder.result(timeout=5),True)
        self.assertIs(other.claim(incident.namespace,incident.id,incident.scope_key,now=101),False)
        with closing(connect(self.path)) as db:
            self.assertEqual(db.execute('SELECT COUNT(*) FROM workflow_notification_claims_v1').fetchone()[0],1)

    def test_only_exact_wrapped_sqlite_busy_or_locked_is_a_deferred_claim(self):
        incident=self.incident()
        for code in (sqlite3.SQLITE_BUSY,sqlite3.SQLITE_LOCKED,sqlite3.SQLITE_CORRUPT,sqlite3.SQLITE_BUSY|256):
            cause=sqlite3.OperationalError('synthetic database failure');cause.sqlite_errorcode=code
            error=WorkflowStoreError('synthetic storage unavailable');error.__cause__=cause
            with self.subTest(code=code),patch.object(self.store,'claim',side_effect=error):
                if code in {sqlite3.SQLITE_BUSY,sqlite3.SQLITE_LOCKED}:
                    self.assertIs(claim_or_deferred(self.store,incident,now=101),ClaimResult.DEFERRED)
                else:
                    with self.assertRaises(WorkflowStoreError):claim_or_deferred(self.store,incident,now=101)
        with patch.object(self.store,'claim',side_effect=WorkflowStoreError('database is locked')):
            with self.assertRaises(WorkflowStoreError):claim_or_deferred(self.store,incident,now=101)
        unwrapped=sqlite3.OperationalError('synthetic busy');unwrapped.sqlite_errorcode=sqlite3.SQLITE_BUSY
        with patch.object(self.store,'claim',side_effect=unwrapped):
            with self.assertRaises(sqlite3.OperationalError):claim_or_deferred(self.store,incident,now=101)

    def test_claim_isolated_by_receiver_and_never_delivers_seen_or_completion(self):
        first=self.incident();second=self.incident(receiver='second')
        self.store.observe([first,second],now=100)
        self.store.acknowledge(first.namespace,first.id,first.scope_key,now=100)
        self.assertFalse(self.store.claim(first.namespace,first.id,first.scope_key))
        self.assertTrue(self.store.claim(second.namespace,second.id,second.scope_key))
        selected=item(state='Completed')
        completion=classify(selected,scope_for(selected,snapshot()),{}, {},now=100)[0]
        self.store.observe([completion],now=100)
        self.assertFalse(self.store.claim(completion.namespace,completion.id,completion.scope_key))

    def test_capacity_never_evicts_unresolved_and_recovers_after_retention(self):
        store=WorkflowStore(self.path,max_incidents=2)
        first,second,third=[self.incident(str(i)) for i in range(3)]
        store.observe([first,second],now=100)
        overflow=store.observe([third],now=101)
        self.assertEqual(overflow.unpersisted,(third.id,));self.assertIn('memory only',overflow.warning)
        self.assertEqual(len(store.load(now=100+100*86400)),2)
        with self.assertRaises(WorkflowStoreError):store.acknowledge(third.namespace,third.id,third.scope_key)
        self.assertFalse(store.claim(third.namespace,third.id,third.scope_key))
        store.observe([],resolved=[(first.namespace,first.id)],now=200)
        result=store.observe([third],now=201+30*86400)
        self.assertEqual(result.unpersisted,())
        self.assertEqual({r['id'] for r in result.records},{second.id,third.id})

    def test_missing_observation_does_not_resolve_and_retention_keeps_open_claims(self):
        incident=self.incident();self.store.observe([incident],now=100)
        self.store.claim(incident.namespace,incident.id,incident.scope_key,now=100)
        self.store.observe([],now=100+31*86400)
        record=self.store.load(now=100+31*86400)[0]
        self.assertIsNone(record['resolved_at']);self.assertEqual(record['delivery_status'],'claimed')
        self.store.observe([],resolved=[(incident.namespace,incident.id)],now=100+32*86400)
        self.assertEqual(self.store.load(now=101+63*86400),())

    def test_unknown_future_schema_and_corruption_fail_explicitly(self):
        with sqlite3.connect(self.path) as db:db.execute("UPDATE workflow_meta SET value='999'")
        with self.assertRaises(WorkflowSchemaError):WorkflowStore(self.path)
        bad=Path(self.tmp.name)/'bad.sqlite3';bad.write_text('not a database')
        with self.assertRaises(WorkflowStoreError):WorkflowStore(bad)

    def test_readonly_or_full_disk_failure_is_visible(self):
        with patch('observatory.workflow_store.sqlite3.connect',side_effect=sqlite3.OperationalError('disk is full')):
            with self.assertRaisesRegex(WorkflowStoreError,'disk is full'):self.store.load()
        with patch('observatory.workflow_store.os.chmod',side_effect=PermissionError('read only')):
            with self.assertRaisesRegex(WorkflowStoreError,'read only'):WorkflowStore(self.path)

    def test_claimed_crash_never_automatically_retries(self):
        incident=self.incident();self.store.observe([incident],now=100)
        self.assertTrue(self.store.claim(incident.namespace,incident.id,incident.scope_key,now=101))
        restarted=WorkflowStore(self.path)
        self.assertFalse(restarted.claim(incident.namespace,incident.id,incident.scope_key,now=102))
        self.assertFalse(restarted.claim(incident.namespace,incident.id,incident.scope_key,retry=True,now=102))

    def test_reviewed_completion_enters_retention_and_observation_does_not_reopen_it(self):
        selected=item(state='Completed')
        incident=classify(selected,scope_for(selected,snapshot()),{}, {},now=100)[0]
        self.store.observe([incident],now=100)
        self.store.acknowledge(incident.namespace,incident.id,incident.scope_key,action='reviewed',now=101)
        self.store.observe([incident],now=102)
        self.assertEqual(self.store.load(now=103)[0]['resolved_at'],101)
        self.store.acknowledge(incident.namespace,incident.id,incident.scope_key,enabled=False,now=104)
        self.assertIsNone(self.store.load(now=105)[0]['resolved_at'])
        self.store.acknowledge(incident.namespace,incident.id,incident.scope_key,now=106)
        day31=self.store.load(now=107+30*86400)
        self.assertEqual(day31[0]['acknowledgement'],'seen')
        self.store.acknowledge(incident.namespace,incident.id,incident.scope_key,enabled=False,now=108+30*86400)
        self.assertIsNone(self.store.load(now=109+30*86400)[0]['resolved_at'])
        self.store.acknowledge(incident.namespace,incident.id,incident.scope_key,now=110+30*86400)
        self.assertEqual(self.store.load(now=111+121*86400),())
