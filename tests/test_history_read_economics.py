"""Read indexes and snapshot boundaries remain correct during concurrent writes."""
from contextlib import contextmanager
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from observatory.core import Store, normalize_native


class HistoryReadEconomicsTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.store=Store(Path(self.temp.name)/'history.db');self.addCleanup(self.store.close)

    def test_numeric_connection_source_and_mcp_hydration_use_exact_indexes(self):
        self.store.register_connection(dict(source_id='7'*32,service='fixture',runtime='fixture',integrations=['manual']))
        # Preserve read semantics for legacy numeric source metadata, although
        # the current registration API requires a hexadecimal source ID.
        with self.store._connect() as db:db.execute("UPDATE agent_connections SET source_id='7'")
        self.store.ingest([normalize_native(dict(trace_id='a'*32,span_id='b'*16,name='MCP call',
            start_unix_nano=1,end_unix_nano=2,resource_attributes={'observatory.source_id':7},
            attributes={'mcp.method.name':'tools/call','mcp.server.name':'fixture','mcp.tool.name':'search'}),'fixture')])
        queries=[];original=self.store._connect
        @contextmanager
        def traced():
            with original() as db:
                db.set_trace_callback(queries.append);yield db
        with patch.object(self.store,'_connect',traced):
            self.assertEqual(self.store.connections()[0]['span_count'],1)
            self.assertEqual(len(self.store.mcp_page()['calls']),1)
        selects=[q for q in queries if q.startswith('SELECT c.*') or q.startswith('SELECT *,') and (' AS mcp_key' in q)]
        self.assertEqual(len(selects),3)
        with original() as db:
            plans=[' '.join(row[3] for row in db.execute('EXPLAIN QUERY PLAN '+sql)) for sql in selects]
        self.assertIn('spans_source_text_v1',plans[0])
        self.assertIn('spans_mcp_owner_lookup_v4',plans[1])
        self.assertIn('logs_mcp_owner_lookup_v4',plans[2])
        inventory=next(sql for sql in queries if sql.startswith('WITH evidence') and ' AS failed' in sql)
        with original() as db:
            lookups=[row[3] for row in db.execute('EXPLAIN QUERY PLAN '+inventory)
                     if 'SEARCH execution ' in row[3] or 'SEARCH candidate_execution ' in row[3]]
        self.assertTrue(lookups)
        self.assertTrue(all('trace_id=?' in plan for plan in lookups),lookups)

    def test_original_task_and_latest_summary_share_one_snapshot(self):
        from observatory.storage_sessions import session_projection
        self.store.ingest([normalize_native(dict(trace_id=f'{i:032x}',span_id=f'{i:016x}',
            name='agent.run',start_unix_nano=i,end_unix_nano=i+1,
            attributes={'session.id':'s','agent.id':'worker',**({'agent.session.task':'Old original'} if i==1 else {})}),'fixture') for i in (1,2,3)])
        with self.store._connect() as db:
            self.assertEqual(db.execute('SELECT initial_task FROM live_run_stats_v1 WHERE trace_id=?',('1'.zfill(32),)).fetchone()[0],'Old original')
        def concurrent(db,limit):
            result=session_projection(db,limit)
            with self.store._connect() as writer:
                row=writer.execute('SELECT summary_json FROM trace_summaries WHERE trace_id=?',('1'.zfill(32),)).fetchone()
                changed=json.loads(row[0]);changed['initial_task']='New original'
                writer.execute('UPDATE trace_summaries SET summary_json=? WHERE trace_id=?',(json.dumps(changed),'1'.zfill(32)))
            return result
        with patch('observatory.storage_sessions.session_projection',side_effect=concurrent):
            self.assertEqual(self.store.sessions()[0]['initial_task'],'Old original')
        self.assertEqual(self.store.sessions()[0]['initial_task'],'New original')

    def test_repeated_original_task_metadata_returns_one_title_per_actor(self):
        from observatory.storage_sessions import session_projection
        self.store.ingest([normalize_native(dict(trace_id=f'{i:032x}',span_id=f'{i:016x}',
            name='agent.run',start_unix_nano=i,end_unix_nano=i+1,
            attributes={'session.id':'s','agent.id':'worker','agent.session.task':f'Original {i}'}),'fixture') for i in range(1,81)])
        transferred=[]
        class RecordingDB:
            def __init__(self,db):self.db=db
            def execute(self,sql,args=()):
                cursor=self.db.execute(sql,args)
                if 'WITH original AS' in sql:
                    rows=cursor.fetchall();transferred.extend(rows);return rows
                return cursor
        with self.store._connect() as db:
            _,_,titles=session_projection(RecordingDB(db),100)
        self.assertEqual(len(transferred),1)
        self.assertEqual(list(titles.values()),['Original 80'])

    def test_interrupted_derived_chunk_rolls_back_and_can_resume(self):
        from observatory.storage_derived import derive_trace
        records=[normalize_native(dict(trace_id=f'{i:032x}',span_id=f'{i:016x}',name='agent.run',
            start_unix_nano=i,end_unix_nano=i+1,attributes={'agent.task':'Current task'}),'fixture') for i in range(1,34)]
        self.store.ingest(records)
        with self.store._connect() as db:
            db.execute("UPDATE trace_summaries SET summary_json=json_set(summary_json,'$.task','Stale task')")
        calls=0
        def interrupted(db,trace):
            nonlocal calls
            calls+=1
            if calls==20:raise RuntimeError('Simulated rebuild interruption')
            return derive_trace(db,trace)
        ids=[row['trace_id'] for row in records]
        with patch('observatory.storage_derived.derive_trace',side_effect=interrupted),self.assertRaises(RuntimeError):
            self.store._refresh_summaries(ids)
        with self.store._connect() as db:
            tasks=[row[0] for row in db.execute("SELECT json_extract(summary_json,'$.task') FROM trace_summaries ORDER BY trace_id")]
        self.assertEqual(tasks[:16],['Current task']*16)
        self.assertEqual(tasks[16:],['Stale task']*17)
        self.store._refresh_summaries(ids)
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM trace_summaries WHERE json_extract(summary_json,'$.task')='Current task'").fetchone()[0],33)
