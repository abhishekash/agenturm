"""Cold migration and live metadata share admission without borrowing locks."""
from contextlib import closing,contextmanager
import hashlib
import json
from pathlib import Path
import sqlite3
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from observatory.core import Store,normalize_native
from tests.test_agent_hierarchy import node

FIXTURE=Path(__file__).parent/'fixtures/startup_writer_outputs_473.json'


class Gate:
    def __init__(self):self.lock=threading.RLock();self.local=threading.local()
    @property
    def owned(self):return bool(getattr(self.local,'depth',0))
    def __enter__(self):self.lock.acquire();self.local.depth=getattr(self.local,'depth',0)+1;return self
    def __exit__(self,*_):self.local.depth-=1;self.lock.release()


def endpoint_seed(store):
    def resource(number,signal):return json.dumps({'device.id':'generated-'+str(number),'fixture.signal':signal})
    with store._connect()as db:
        db.executemany('INSERT INTO spans(trace_id,span_id,name,start_ns,end_ns,status,service,attributes,events,resource_attributes)VALUES(?,?,?,?,?,?,?,?,?,?)',
            [(f'{n+1:032x}','1'*16,'fixture',n+1,n+100,'OK','fixture','{}','[]',resource(n,'span'))for n in range(501)])
        db.executemany('INSERT INTO logs(digest,trace_id,span_id,time_ns,service,severity_number,severity_text,event_name,body,attributes,resource_attributes,scope_name)VALUES(?,?,?,?,?,?,?,?,?,?,?,?)',
            [(f'{n+1+offset:064x}',None if offset==0 else f'{n+1:032x}',None,n+200+offset//501*100,'fixture',9,'INFO','generated','"Generated"','{}',resource(n,'null-log'if offset==0 else 'linked-log'),'fixture')
             for offset in (0,501)for n in range(501)])
        db.executemany('INSERT INTO metric_points(digest,time_ns,start_ns,service,name,description,unit,kind,temporality,attributes,resource_attributes,scope_name,data)VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)',
            [(f'{n+1:064x}',n+400,1,'fixture','generated','','','gauge',0,'{}',resource(n,'metric'),'fixture','{}')for n in range(501)])
        db.execute("DELETE FROM schema_meta WHERE key='endpoint_inventory_v1'")


def outcome_seed(store):
    with store._connect()as db:
        db.executemany('INSERT INTO trace_summaries(trace_id,session_id,summary_json)VALUES(?,?,?)',
            [(f'{n+1:032x}','generated',json.dumps({'task_outcome':'legacy','task_outcome_ns':0,'generated':n}))for n in range(201)])
        db.executemany('INSERT INTO outcomes VALUES(?,?,?,?,?,?,?,?,?)',
            [(f'{n+1:032x}','generated',n%2,'[]','','fixture','fixture','{}',n+500)for n in range(201)])
        db.execute("DELETE FROM schema_meta WHERE key='task_outcome_v1'")


def table_hashes(store,tables):
    result={}
    with store._read()as db:
        for table in tables:
            rows=[list(row)for row in db.execute('SELECT * FROM '+table+' ORDER BY '+','.join(str(n+1)for n in range(len(db.execute('PRAGMA table_info('+table+')').fetchall()))))]
            result[table]=dict(rows=len(rows),sha256=hashlib.sha256(json.dumps(rows,separators=(',',':')).encode()).hexdigest())
    return result


def node_project(store,upsert=None):
    upsert=upsert or store.upsert_agent_nodes
    with patch('observatory.core.time.time_ns',return_value=1800000000000000000):
        upsert([node('parent-a'),node('parent-b'),node('child','parent-a')])
        upsert([node('child','parent-b',stamp=200),node('child','parent-a',stamp=300)])
    return table_hashes(store,('agent_nodes','agent_identity'))


class StartupWriterAdmissionTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.store=Store(Path(self.temp.name)/'generated.db');self.addCleanup(self.store.close)

    def test_initial_migration_and_health_metadata_wait_outside_sqlite_for_local_source(self):
        entered=threading.Event();release=threading.Event();connections=[];errors=[]
        original_policy=self.store._policy_records;original_connect=self.store._connect
        def blocked(*args,**kwargs):
            entered.set()
            if not release.wait(3):raise RuntimeError('Generated source release missing')
            return original_policy(*args,**kwargs)
        @contextmanager
        def observed():
            connections.append(threading.current_thread().name)
            with original_connect()as db:yield db
        def run(action):
            try:action()
            except Exception as exc:errors.append(exc)
        record=normalize_native(dict(trace_id='a'*32,span_id='1'*16,name='agent.run',start_unix_nano=1,end_unix_nano=2,status='OK',attributes={},events=[]),'fixture')
        with patch.object(self.store,'_policy_records',side_effect=blocked),patch.object(self.store,'_connect',side_effect=observed):
            source=threading.Thread(name='generated-source',target=lambda:run(lambda:self.store.ingest([record])))
            migration=threading.Thread(name='generated-migration',target=lambda:run(self.store._finish_migration))
            metadata=threading.Thread(name='generated-health',target=lambda:run(lambda:self.store.upsert_agent_nodes([node('native')])) )
            threads=(source,migration,metadata)
            try:
                source.start();self.assertTrue(entered.wait(1));self.store.migration_status={'ready':False,'stage':'Preparing indexed history','processed':0}
                migration.start();metadata.start();time.sleep(.05)
                self.assertNotIn('generated-migration',connections);self.assertNotIn('generated-health',connections)
                self.assertTrue(migration.is_alive());self.assertTrue(metadata.is_alive())
            finally:
                release.set()
                for thread in threads:
                    if thread.ident is not None:thread.join(5)
        self.assertFalse(any(thread.is_alive()for thread in threads));self.assertFalse(errors)
        self.assertTrue(self.store.migration_status['ready']);self.assertNotIn('error',self.store.migration_status)
        self.assertEqual(len(self.store.agent_nodes()),1)

    def test_borrowed_upsert_never_enters_gate_begins_or_commits(self):
        class Forbidden:
            def __enter__(self):raise AssertionError('Borrowed writer acquired local gate')
            def __exit__(self,*_):pass
        with self.store._connect()as db:
            db.execute('BEGIN IMMEDIATE');statements=[];db.set_trace_callback(statements.append)
            with patch.object(self.store,'_writer_gate',Forbidden()):self.store.upsert_agent_nodes([node('borrowed')],_db=db)
            self.assertTrue(db.in_transaction)
            self.assertFalse(any(sql.upper().startswith(('BEGIN','COMMIT'))for sql in statements))
            db.rollback()
        self.assertFalse(self.store.agent_nodes())

    def test_standalone_node_merge_reads_after_external_writer_commits(self):
        self.store.upsert_agent_nodes([node('child','parent-a')]);finished=threading.Event();errors=[]
        def update():
            try:self.store.upsert_agent_nodes([node('child','parent-c')])
            except Exception as exc:errors.append(exc)
            finally:finished.set()
        with closing(sqlite3.connect(self.store.path,timeout=2))as external,external:
            external.row_factory=sqlite3.Row;external.execute('BEGIN IMMEDIATE')
            self.store.upsert_agent_nodes([node('child','parent-b')],_db=external)
            thread=threading.Thread(target=update);thread.start()
            try:self.assertFalse(finished.wait(.05))
            finally:external.commit();thread.join(3)
        self.assertFalse(thread.is_alive());self.assertFalse(errors)
        child=self.store.agent_nodes()[0]
        self.assertEqual(child['relationship_state'],'conflicted')
        from tests.test_agent_hierarchy import agent_id
        self.assertEqual({claim['parent_agent_id']for claim in child['parent_claims']},
            {agent_id('parent-a'),agent_id('parent-b'),agent_id('parent-c')})

    def test_initial_activity_batches_commit_and_yield_outside_gate(self):
        gate=Gate();counts=[];waits=[]
        with self.store._connect()as db:
            db.executemany('INSERT INTO logs VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)',
                [(f'{n+1:064x}',None,None,n+1,'fixture',9,'INFO','generated','"Generated"','{}','{}','fixture',None)for n in range(401)])
        from observatory.storage_activity import drain
        def batch(db,limit=200):
            self.assertTrue(gate.owned);self.assertTrue(db.in_transaction)
            count=drain(db,limit);counts.append(count);return count
        def wait(seconds):self.assertFalse(gate.owned);waits.append(seconds);return False
        with patch.object(self.store,'_writer_gate',gate),patch('observatory.storage_activity.drain',side_effect=batch),patch.object(self.store._migration_stop,'wait',side_effect=wait):
            self.store._finish_migration()
        self.assertEqual(counts,[200,200,1,0]);self.assertEqual(waits,[.025]*3)
        self.assertTrue(self.store.migration_status['ready'])

    def test_endpoint_keyset_mixed_null_groups_matches_complete_frozen_output(self):
        endpoint_seed(self.store);gate=Gate();pages=[];waits=[];original=self.store._update_endpoints
        def update(db,records):
            self.assertTrue(gate.owned);self.assertTrue(db.in_transaction);pages.append(len(records));return original(db,records)
        def wait(seconds):self.assertFalse(gate.owned);waits.append(seconds);return False
        with patch.object(self.store,'_writer_gate',gate),patch.object(self.store,'_update_endpoints',side_effect=update),patch.object(self.store._migration_stop,'wait',side_effect=wait):
            self.store._backfill_endpoints()
        self.assertEqual(pages,[500,1,500,500,2,500,1]);self.assertEqual(len(waits),len(pages))
        self.assertEqual(table_hashes(self.store,('endpoints','endpoint_services','endpoint_traces')),json.loads(FIXTURE.read_text())['endpoints'])

    def test_endpoint_external_writer_commits_between_pages_without_snapshot_upgrade(self):
        endpoint_seed(self.store);started=threading.Event();done=threading.Event();failures=[];threads=[];calls=[];original=self.store._update_endpoints
        def writer():
            started.set()
            try:
                with closing(sqlite3.connect(self.store.path,timeout=2))as db,db:db.execute("INSERT OR REPLACE INTO schema_meta VALUES('generated_external','1')")
            except Exception as exc:failures.append(exc)
            finally:done.set()
        def update(db,records):
            self.assertTrue(db.in_transaction);calls.append(len(records))
            if len(calls)==1:
                thread=threading.Thread(target=writer);threads.append(thread);thread.start();self.assertTrue(started.wait(1));self.assertFalse(done.wait(.025))
            return original(db,records)
        def wait(seconds):self.assertTrue(done.wait(2));return False
        try:
            with patch.object(self.store,'_update_endpoints',side_effect=update),patch.object(self.store._migration_stop,'wait',side_effect=wait):self.store._backfill_endpoints()
        finally:
            for thread in threads:thread.join(3)
        self.assertFalse(failures);self.assertTrue(done.is_set())

    def test_endpoint_stop_or_error_leaves_marker_absent_and_resume_is_exact(self):
        endpoint_seed(self.store)
        with patch.object(self.store._migration_stop,'wait',return_value=True):self.store._backfill_endpoints()
        with self.store._read()as db:self.assertIsNone(db.execute("SELECT 1 FROM schema_meta WHERE key='endpoint_inventory_v1'").fetchone())
        with patch.object(self.store,'_update_endpoints',side_effect=RuntimeError('Generated endpoint error')):
            with self.assertRaisesRegex(RuntimeError,'Generated endpoint error'):self.store._backfill_endpoints()
        with self.store._read()as db:self.assertIsNone(db.execute("SELECT 1 FROM schema_meta WHERE key='endpoint_inventory_v1'").fetchone())
        self.store._backfill_endpoints()
        self.assertEqual(table_hashes(self.store,('endpoints','endpoint_services','endpoint_traces')),json.loads(FIXTURE.read_text())['endpoints'])

    def test_outcome_pages_stop_resume_and_full_frozen_payload(self):
        outcome_seed(self.store);gate=Gate();waits=[]
        def wait(seconds):self.assertFalse(gate.owned);waits.append(seconds);return len(waits)==1
        with patch.object(self.store,'_writer_gate',gate),patch.object(self.store._migration_stop,'wait',side_effect=wait):self.store._backfill_task_outcomes()
        with self.store._read()as db:
            self.assertIsNone(db.execute("SELECT 1 FROM schema_meta WHERE key='task_outcome_v1'").fetchone())
            self.assertEqual(db.execute("SELECT COUNT(*) FROM trace_summaries WHERE json_extract(summary_json,'$.task_outcome')!='legacy'").fetchone()[0],100)
        with patch('observatory.storage_derived.task_outcome_for_trace',side_effect=RuntimeError('Generated outcome error')):
            with self.assertRaisesRegex(RuntimeError,'Generated outcome error'):self.store._backfill_task_outcomes()
        with self.store._read()as db:self.assertIsNone(db.execute("SELECT 1 FROM schema_meta WHERE key='task_outcome_v1'").fetchone())
        self.store._backfill_task_outcomes()
        self.assertEqual(table_hashes(self.store,('trace_summaries',)),json.loads(FIXTURE.read_text())['outcomes'])

    def test_nodes_and_short_writers_use_owned_admission_and_keep_exact_payload(self):
        gate=Gate();original=self.store._connect;entered=[]
        @contextmanager
        def observed():
            self.assertTrue(gate.owned);entered.append(True)
            with original()as db:yield db
        with patch.object(self.store,'_writer_gate',gate),patch.object(self.store,'_connect',side_effect=observed):
            self.store.upsert_agent_nodes([node('check')]);self.store.register_connection(dict(source_id='1'*32,service='fixture',runtime='SDK',integrations=['otlp'],state='running'))
            self.store.ingest_metrics([])
        self.assertEqual(len(entered),3)
        with self.store._connect()as db:db.execute('DELETE FROM agent_nodes');db.execute('DELETE FROM agent_identity')
        self.assertEqual(node_project(self.store),json.loads(FIXTURE.read_text())['nodes'])

    def test_activity_repair_releases_writer_before_fresh_coherent_read(self):
        gate=Gate();original_connect=self.store._connect;states=[]
        with original_connect()as db:
            db.execute('INSERT INTO logs VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)',('1'*64,None,None,1,'fixture',9,'INFO','agent.activity','"Generated"',json.dumps({'agent.activity.domain':'message'}),'{}','fixture',None))
        @contextmanager
        def observed():
            with original_connect()as db:
                db.set_trace_callback(lambda sql:states.append((sql,gate.owned)))
                yield db
        with patch.object(self.store,'_writer_gate',gate),patch.object(self.store,'_connect',side_effect=observed):
            result=self.store.activity_page()
        self.assertEqual(len(result['activity']),1)
        self.assertIn(('BEGIN IMMEDIATE',True),states)
        self.assertTrue(any(sql=='COMMIT'and owned for sql,owned in states))
        self.assertEqual(sum(sql=='BEGIN'and not owned for sql,owned in states),2)

    def test_fatal_migration_stage_is_not_hidden_by_dirty_incoming_label(self):
        self.store.migration_status=dict(ready=False,stage='History migration failed; restart to resume',error='database is locked',processed=9600)
        with self.store._connect()as db:db.execute("INSERT INTO finding_dirty_v1 VALUES(?)",('a'*32,))
        self.assertEqual(self.store.index_status(),self.store.migration_status)

    def test_genuine_drain_failure_keeps_dirty_work_and_fatal_state(self):
        with self.store._connect()as db:db.execute("INSERT INTO activity_dirty_v1 VALUES('log',?)",('1'*64,))
        with patch('observatory.storage_activity.drain',side_effect=sqlite3.OperationalError('Generated external writer failure')):
            with self.assertRaisesRegex(sqlite3.OperationalError,'Generated external writer failure'):self.store._finish_migration()
        state=self.store.index_status()
        self.assertFalse(state['ready']);self.assertEqual(state['stage'],'History migration failed; restart to resume')
        self.assertEqual(state['error'],'Generated external writer failure')
        with self.store._read()as db:self.assertIsNotNone(db.execute('SELECT 1 FROM activity_dirty_v1').fetchone())
        # A previously ready maintenance loop must not retain ready=True when
        # a genuine migration/compatibility failure is caught asynchronously.
        self.store.migration_status=dict(ready=True,stage='Ready',processed=0)
        with patch.object(self.store,'_async_derived_refresh',True),patch('observatory.storage_activity.drain',side_effect=sqlite3.OperationalError('Generated later failure')):
            self.store._finish_migration()
        self.assertFalse(self.store.index_status()['ready'])
        self.assertEqual(self.store.index_status()['error'],'Generated later failure')

    def test_retention_keeps_one_atomic_transaction_inside_derived_then_writer_gate(self):
        self.store.ingest([normalize_native(dict(trace_id='a'*32,span_id='1'*16,name='generated',start_unix_nano=1,end_unix_nano=2,status='OK',attributes={},events=[]),'fixture')])
        tables=('spans','trace_activity','trace_summaries','finding_cache');before=table_hashes(self.store,tables)
        gate=Gate();original=self.store._connect;entered=[]
        @contextmanager
        def fail_mid_retention():
            self.assertTrue(gate.owned);entered.append(True)
            with original()as db:
                db.set_authorizer(lambda action,table,*_:sqlite3.SQLITE_DENY if action==sqlite3.SQLITE_DELETE and table=='trace_summaries'else sqlite3.SQLITE_OK)
                yield db
        with patch.object(self.store,'_writer_gate',gate),patch.object(self.store,'_connect',side_effect=fail_mid_retention):
            with self.assertRaises(sqlite3.DatabaseError):self.store.prune(1,now_ns=3*86400*10**9)
        self.assertEqual(len(entered),1);self.assertEqual(table_hashes(self.store,tables),before)


if __name__=='__main__':unittest.main()
