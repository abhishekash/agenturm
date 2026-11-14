"""HTTP measurement validity contracts; no retained corpus or long timed run."""
from contextlib import closing
import http.server
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
from tempfile import TemporaryDirectory
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from scripts import benchmark_http_tails as harness
from scripts.validation_support import (create_owned_directory,clone_seed,raw_manifest,prepare_raw_clone,
    fresh_schema_manifest,verify_named_indexes,published_heartbeat)


class Reply(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        code=503 if self.path=='/busy'else 200
        value={'error':'index unavailable'}if code==503 else {'receiver_id':'a'*32,'sessions':[{'session_id':'synthetic'}]}
        raw=json.dumps(value).encode();self.send_response(code)
        self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(raw)))
        self.send_header('X-Agenturm-Receiver-ID','a'*32)
        self.end_headers();self.wfile.write(raw)
    def log_message(self,*args):pass


class HttpTailContractTests(unittest.TestCase):
    def setUp(self):
        self.server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Reply)
        self.thread=threading.Thread(target=self.server.serve_forever);self.thread.start()
        self.addCleanup(self.server.server_close);self.addCleanup(self.server.shutdown)

    def test_real_503_is_retained_as_failure_and_not_filtered_from_statistics(self):
        good,data=harness.http_attempt(self.server.server_port,'/ok','token')
        good=harness.validate_read(good,data,harness.ENDPOINTS[0],'a'*32);good['path']=harness.ENDPOINTS[0]
        bad,data=harness.http_attempt(self.server.server_port,'/busy','token')
        bad=harness.validate_read(bad,data,harness.ENDPOINTS[0],'a'*32);bad['path']=harness.ENDPOINTS[0]
        self.assertEqual(bad['status'],503);self.assertFalse(bad['valid']);self.assertGreater(bad['bytes'],0)
        stats=harness.summarize([good,bad])[harness.ENDPOINTS[0]]
        self.assertEqual((stats['attempts'],stats['valid_attempts']),(2,1))
        self.assertEqual(stats['p95_ms'],max(good['elapsed_ms'],bad['elapsed_ms']))

    def test_wrong_namespace_empty_body_and_incoherent_inventory_fail(self):
        row,data=harness.http_attempt(self.server.server_port,'/ok','token')
        self.assertFalse(harness.validate_read(dict(row),data,harness.ENDPOINTS[0],'b'*32)['valid'])
        body={'sessions':[{'session_id':'synthetic'}]}
        self.assertTrue(harness.validate_read(dict(row),body,harness.ENDPOINTS[0],'a'*32)['valid'])
        self.assertFalse(harness.validate_read({**row,'receiver_header':None},body,harness.ENDPOINTS[0],'a'*32)['valid'])
        self.assertFalse(harness.validate_read(dict(row),{**body,'receiver_id':'b'*32},harness.ENDPOINTS[0],'a'*32)['valid'])
        for body in ({'receiver_id':'a'*32,'sessions':[]},
                     {'receiver_id':'a'*32,'calls':[{}],'summary':{'calls':0}},
                     {'receiver_id':'a'*32,'calls':[{}],'summary':{'calls':True}}):
            path=harness.ENDPOINTS[0]if 'sessions'in body else harness.ENDPOINTS[1]
            self.assertFalse(harness.validate_read(dict(row),body,path,'a'*32)['valid'])

    def test_nearest_rank_uses_all_twenty_attempts_without_interpolation(self):
        rows=[dict(path=harness.ENDPOINTS[0],elapsed_ms=value,valid=value!=20)for value in range(1,21)]
        stats=harness.summarize(rows)[harness.ENDPOINTS[0]]
        self.assertEqual(stats['p95_ms'],19);self.assertEqual(stats['maximum_ms'],20)
        self.assertEqual(stats['valid_attempts'],19)

    def test_literal_overlaps_preserved_without_dropping_fast_no_overlap_read(self):
        reads=[dict(started_ns=10,finished_ns=11),dict(started_ns=20,finished_ns=40)]
        writes=[dict(sequence=0,started_ns=22,finished_ns=30,valid=True)]
        harness.intersect_writes(reads,writes)
        self.assertEqual(reads[0]['overlapping_write_attempts'],[])
        self.assertEqual(reads[1]['overlapping_write_attempts'],[0]);self.assertEqual(len(reads),2)

    def test_payload_identity_values_and_counts_are_fixed_independent_of_tree(self):
        a=harness.payload(7,1230000000000);b=harness.payload(7,1230000000000)
        self.assertEqual(a,b);self.assertEqual(len(a['spans']),2)
        self.assertEqual(a['spans'][0]['trace_id'],a['spans'][1]['trace_id'])
        normal=harness.protocol();self.assertEqual((normal['warmups'],normal['idle_pairs'],normal['loaded_pairs']),(5,20,40))
        self.assertEqual((normal['write_batches'],normal['write_period_s'],normal['read_period_s']),(420,.1,1.))

    def test_late_read_slot_refuses_burst_catchup_without_fake_http_samples(self):
        missed=[]
        with patch.object(harness,'http_attempt',side_effect=AssertionError('Late request must not dispatch')):
            rows=harness.run_reads(1,'token','a'*32,2,period=.001,start_ns=time.monotonic_ns()-1_000_000_000,missed_slots=missed)
        self.assertEqual(rows,[])
        self.assertEqual(missed[0]['kind'],'read')

    def test_failed_json_publication_keeps_existing_complete_handshake(self):
        with TemporaryDirectory()as directory:
            path=Path(directory)/'endpoint.json';harness.write_json(path,{'port':123})
            with self.assertRaises(ValueError):harness.write_json(path,{'port':float('nan')})
            self.assertEqual(json.loads(path.read_text()),{'port':123})
            self.assertEqual(list(path.parent.iterdir()),[path])


def tiny_raw_seed(path):
    """Generate only raw canonical tables; each child must build its own schema."""
    from observatory.core import Store,normalize_native
    full=path.with_name('source-only.db');store=Store(full)
    try:
        records=harness.payload(0,1794650400000000000)
        store.ingest([normalize_native(row,records['service'])for row in records['spans']])
    finally:store.close()
    with closing(sqlite3.connect(full))as source,closing(sqlite3.connect(path))as dest:
        for table in ('spans','logs','trace_activity'):
            dest.execute(source.execute("SELECT sql FROM sqlite_master WHERE type='table' AND name=?",(table,)).fetchone()[0])
            rows=source.execute('SELECT * FROM '+table).fetchall()
            if rows:dest.executemany('INSERT INTO '+table+' VALUES('+','.join('?'for _ in rows[0])+')',rows)
        # Keep the measured writer trace absent from the seed by renaming only
        # this fully generated source's canonical trace in its raw records.
        for table in ('spans','trace_activity'):dest.execute('UPDATE '+table+' SET trace_id=?',('e'*32,))
        dest.commit()


class HttpTailSmokeTests(unittest.TestCase):
    def test_tiny_complete_subprocess_smoke_records_sources_counts_and_cleanup(self):
        with TemporaryDirectory()as directory:
            base=Path(directory);seed=base/'raw-seed.db';tiny_raw_seed(seed)
            output=base/'run'
            result=subprocess.run([sys.executable,str(harness.ROOT/'scripts/benchmark_http_tails.py'),'run',
                '--seed',str(seed),'--verified-synthetic-source','--baseline-root',str(harness.ROOT),
                '--candidate-root',str(harness.ROOT),'--output',str(output),'--smoke'],
                capture_output=True,text=True,timeout=60)
            report=json.loads((output/'results.json').read_text())
            self.assertIn(result.returncode,(0,1),(result.stderr,report['errors']))
            self.assertFalse(report['publishable']);self.assertTrue(report['writer_inputs_equal'])
            self.assertNotEqual(report['versions']['B']['result']['pid'],report['versions']['C']['result']['pid'])
            expected_errors=[]
            for role in ('B','C'):
                row=report['versions'][role]['result']
                self.assertFalse(row['native_monitor_enabled']);self.assertEqual(row['unauthenticated_status'],401)
                self.assertNotEqual(row['pid'],row['client_pid'])
                self.assertTrue(row['production_indexes']['equal'])
                self.assertEqual(row['imports']['root'],str(harness.ROOT))
                self.assertEqual(len(row['idle']),4);self.assertEqual(len(row['loaded']['reads']),4)
                self.assertEqual(len(row['loaded']['writes']),10);self.assertEqual(row['loaded']['accepted_records'],20)
                self.assertTrue(row['drained']);self.assertEqual(row['seed_before'],row['seed_after'])
                self.assertFalse((output/role/'working.db').exists())
                self.assertEqual(row['source_before'],row['source_after'])
                failed=[item for item in row['loaded']['reads']if not item['valid']]
                # Real incoming-history 503s remain a protocol failure, not
                # something this short functional smoke retries or hides.
                self.assertTrue(all(item['status']==503 for item in failed),row['errors'])
                self.assertEqual(row['errors'],['Loaded HTTP response failed']if failed else [])
                if failed:
                    self.assertFalse(row['publishable']);expected_errors.append(role+' invalid: Loaded HTTP response failed')
                self.assertTrue(all(value is None for value in row['loaded_p95_under100ms'].values()))
            self.assertEqual(report['errors'],expected_errors)
            self.assertEqual(result.returncode,1 if expected_errors else 0)


class RawSchemaPreparationTests(unittest.TestCase):
    def test_registered_clone_discards_only_derived_schema_and_rebuilds_normal_indexes(self):
        from observatory.core import Store
        with TemporaryDirectory()as directory:
            root=Path(directory);seed=root/'seed.db';tiny_raw_seed(seed)
            with sqlite3.connect(seed)as db:
                db.execute('CREATE TABLE activity_dirty_v1(kind TEXT,source_key TEXT)')
                db.execute("INSERT INTO activity_dirty_v1 VALUES('span','synthetic')")
                db.execute('CREATE INDEX inherited_experimental_index ON spans(name)')
            before=raw_manifest(seed);base=root/'owned';create_owned_directory(base)
            clone_seed(seed,base/'working.db',reserve_bytes=0)
            fresh=fresh_schema_manifest(base,Store)
            prepared=prepare_raw_clone(base,'working.db',schema_meta_sql=fresh['schema_meta_sql'])
            self.assertEqual(prepared['before']['raw_sha256'],prepared['after']['raw_sha256'])
            self.assertEqual(raw_manifest(seed),before)
            self.assertIn('inherited_experimental_index',{item['name']for item in prepared['removed_schema']})
            with sqlite3.connect(base/'working.db')as db:
                self.assertEqual(dict(db.execute('SELECT key,value FROM schema_meta')),{'trace_activity_v1':'1'})
                for name,ddl in prepared['raw_table_ddl'].items():
                    self.assertEqual(db.execute("SELECT sql FROM sqlite_master WHERE name=?",(name,)).fetchone()[0],ddl)
            store=Store(base/'working.db')
            try:
                deadline=time.monotonic()+5
                while not store.index_status()['ready']and time.monotonic()<deadline:time.sleep(.01)
                self.assertTrue(store.index_status()['ready'])
                self.assertTrue(verify_named_indexes(base/'working.db',fresh)['equal'])
            finally:store.close()

    def test_nonempty_canonical_or_unknown_inputs_refuse_without_discarding_them(self):
        from observatory.core import Store
        for name in ('progress','metric_points','outcomes','agent_connections','agent_nodes','agent_identity',
                     'native_source_policy_v1','future_input_table'):
            with self.subTest(table=name),TemporaryDirectory()as directory:
                root=Path(directory);seed=root/'seed.db';tiny_raw_seed(seed)
                with sqlite3.connect(seed)as db:
                    db.execute('CREATE TABLE '+name+'(value TEXT)');db.execute('INSERT INTO '+name+" VALUES('preserve')")
                base=root/'owned';create_owned_directory(base);clone_seed(seed,base/'working.db',reserve_bytes=0)
                fresh=fresh_schema_manifest(base,Store);before=raw_manifest(base/'working.db')
                with self.assertRaisesRegex(ValueError,'nonempty|Nonempty'):
                    prepare_raw_clone(base,'working.db',schema_meta_sql=fresh['schema_meta_sql'])
                self.assertEqual(raw_manifest(base/'working.db'),before)
                with sqlite3.connect(base/'working.db')as db:self.assertEqual(db.execute('SELECT value FROM '+name).fetchone()[0],'preserve')

    def test_seed_alias_and_unregistered_target_refuse_before_mutation(self):
        from observatory.core import Store
        with TemporaryDirectory()as directory:
            root=Path(directory);seed=root/'seed.db';tiny_raw_seed(seed);base=root/'owned';create_owned_directory(base)
            fresh=fresh_schema_manifest(base,Store)
            with self.assertRaises(ValueError):prepare_raw_clone(base,'../seed.db',schema_meta_sql=fresh['schema_meta_sql'])
            clone_seed(seed,base/'working.db',reserve_bytes=0)
            (base/'working.db').unlink();os.link(seed,base/'working.db')
            before=raw_manifest(seed)
            with self.assertRaisesRegex(ValueError,'alias'):
                prepare_raw_clone(base,'working.db',schema_meta_sql=fresh['schema_meta_sql'])
            self.assertEqual(raw_manifest(seed),before)


class UsefulFrameTests(unittest.TestCase):
    def test_visible_count_observes_published_table_map_without_querying_widgets(self):
        snapshot=SimpleNamespace(receiver_id='rx',local_status={'sessions':[]},error='',read_errors={},sessions_error='',
            updated=99,sessions=[],mcp_summary={})
        app=SimpleNamespace(feed=SimpleNamespace(snapshot=snapshot),data=snapshot,
            source_map={str(n):dict(session_id=str(n),state='Working')for n in range(4)},
            table_data={'agent-table':{'one':{'task':'PRIVATE CONTENT'}}},
            workflow_ledger=SimpleNamespace(rows=[]),workflow_controller=SimpleNamespace(warning=''))
        def forbidden(*args):raise AssertionError('Heartbeat queried a widget or prepared sources')
        app.query_one=forbidden;app.workflow_items=forbidden
        row=published_heartbeat(app,at=100)
        self.assertEqual(row['visible_agent_rows'],1);self.assertNotIn('PRIVATE CONTENT',json.dumps(row))
        app.table_data['agent-table'].clear()
        self.assertEqual(published_heartbeat(app,at=100)['visible_agent_rows'],0)

    def test_internal_sources_cannot_substitute_for_published_visible_rows(self):
        from scripts.benchmark_monitor_idle import useful_agents_heartbeat
        from tests.test_validation_harness import ValidationContracts
        row=ValidationContracts().healthy();row['states']=[{}]*4
        row['visible_agent_rows']=0;self.assertFalse(useful_agents_heartbeat(row,'rx'))
        row['visible_agent_rows']=1;self.assertTrue(useful_agents_heartbeat(row,'rx'))
        row['visible_agent_rows']=True;self.assertFalse(useful_agents_heartbeat(row,'rx'))
