"""Idle compatibility polling stays cheap without caching the dirty queue."""
from contextlib import contextmanager
import json
from pathlib import Path
import sqlite3
import tempfile
import time
import unittest
from unittest.mock import patch

from observatory.core import Store, normalize_native


class IdleHistoryWatchTests(unittest.TestCase):
    def setUp(self):
        tmp=tempfile.TemporaryDirectory();self.addCleanup(tmp.cleanup)
        self.path=Path(tmp.name)/'history.db';self.store=Store(self.path);self.addCleanup(self.store.close)
        self.store.ingest([normalize_native(dict(trace_id='a'*32,span_id='1'*16,name='tool',
            start_unix_nano=1,end_unix_nano=2,attributes={'agent.id':'worker','session.id':'session'},
            events=[dict(name='agent.activity',time_unix_nano=2,attributes={
                'agent.activity.domain':'file','agent.activity.action':'update',
                'agent.activity.subject':'original.py','agent.activity.result':'success'})]),'fixture')])

    def external_edit(self,subject='changed.py'):
        with sqlite3.connect(self.path) as db:
            row=db.execute('SELECT events FROM spans').fetchone();events=json.loads(row[0])
            events[0]['attributes']['agent.activity.subject']=subject
            db.execute('UPDATE spans SET events=?',(json.dumps(events),))

    def test_empty_queue_never_acquires_a_writer_or_new_connection_after_warmup(self):
        self.assertEqual(self.store._drain_external_activity(),0)
        with patch.object(self.store,'_connect',side_effect=AssertionError('idle writer acquisition')):
            with patch('observatory.read_connections.sqlite3.connect',side_effect=AssertionError('idle connection creation')):
                for _ in range(20):self.assertEqual(self.store._drain_external_activity(),0)

    def test_external_writer_drains_after_lease_ends_and_updates_exact_source(self):
        self.external_edit()
        original_read=self.store._read;original_write=self.store._connect;active=[False]
        @contextmanager
        def read():
            with original_read() as db:
                active[0]=True
                try:yield db
                finally:active[0]=False
        @contextmanager
        def write():
            self.assertFalse(active[0],'read lease held while acquiring writer')
            with original_write() as db:yield db
        with patch.object(self.store,'_read',read),patch.object(self.store,'_connect',write):
            self.assertEqual(self.store._drain_external_activity(),1)
        rows=self.store.activity_page()['activity']
        self.assertEqual([row['subject'] for row in rows],['changed.py'])
        self.assertEqual(rows[0]['session_id'],'session')
        self.assertTrue(self.store.index_status()['ready'])

    def test_write_after_empty_probe_is_observed_on_next_tick(self):
        original=self.store._read;written=[]
        class Cursor:
            def __init__(cursor,db,sql,args):cursor.db=db;cursor.sql=sql;cursor.args=args
            def fetchone(cursor):
                row=cursor.db.execute(cursor.sql,cursor.args).fetchone()
                if row is None and not written:
                    self.external_edit();written.append(True)
                return row
        class Probe:
            def __init__(proxy,db):proxy.db=db
            def execute(proxy,sql,args=()):
                if sql.startswith('SELECT 1 FROM activity_dirty_v1'):return Cursor(proxy.db,sql,args)
                return proxy.db.execute(sql,args)
        @contextmanager
        def interleaved():
            with original() as db:yield Probe(db)
        with patch.object(self.store,'_read',interleaved):
            self.assertEqual(self.store._drain_external_activity(),0)
        self.assertFalse(self.store.index_status()['ready'])
        self.assertEqual(self.store._drain_external_activity(),1)
        self.assertEqual(self.store.activity_page()['activity'][0]['subject'],'changed.py')

    def test_another_receiver_can_consume_the_observed_queue_before_write(self):
        self.external_edit();other=Store(self.path);self.addCleanup(other.close)
        # Make new work after the other receiver's startup repair.
        self.external_edit('latest.py')
        original=self.store._connect;consumed=[]
        @contextmanager
        def interleaved():
            if not consumed:
                consumed.append(other._drain_external_activity())
            with original() as db:yield db
        with patch.object(self.store,'_connect',interleaved):
            self.assertEqual(self.store._drain_external_activity(),0)
        self.assertEqual(consumed,[1])
        self.assertEqual([row['subject'] for row in self.store.activity_page()['activity']],['latest.py'])

    def test_background_watch_closes_without_retaining_read_leases(self):
        path=self.path.with_name('async.db');store=Store(path,async_derived_refresh=True)
        self.addCleanup(store.close)
        deadline=time.monotonic()+5
        while not store.index_status()['ready'] and time.monotonic()<deadline:time.sleep(.01)
        self.assertTrue(store.index_status()['ready'])
        worker=store._migration_thread
        store.close()
        self.assertFalse(worker.is_alive())
        self.assertTrue(store._read_connections.closed)
        self.assertEqual(store._read_connections.idle,[])

    def test_background_watch_repairs_a_legacy_writer_without_foreground_repair(self):
        watcher=Store(self.path,async_derived_refresh=True);self.addCleanup(watcher.close)
        deadline=time.monotonic()+5
        while not watcher.index_status()['ready'] and time.monotonic()<deadline:time.sleep(.01)
        self.assertTrue(watcher.index_status()['ready'])
        self.external_edit('external.py')
        deadline=time.monotonic()+5
        while not watcher.index_status()['ready'] and time.monotonic()<deadline:time.sleep(.01)
        self.assertTrue(watcher.index_status()['ready'])
        self.assertEqual(watcher.activity_page()['activity'][0]['subject'],'external.py')
        self.assertNotIn('error',watcher.migration_status)

    def test_replaced_file_probe_cannot_acquire_writer_for_old_receiver_work(self):
        self.external_edit()
        replacement=self.path.with_suffix('.replacement')
        other=Store(replacement);other.close()
        original=self.store._read
        @contextmanager
        def replace_after_probe():
            with original() as db:
                yield db
                replacement.replace(self.path)
        with patch.object(self.store,'_read',replace_after_probe),patch.object(self.store,'_connect',
                side_effect=AssertionError('writer acquired from a replaced read')):
            with self.assertRaisesRegex(sqlite3.OperationalError,'replaced'):
                self.store._drain_external_activity()
