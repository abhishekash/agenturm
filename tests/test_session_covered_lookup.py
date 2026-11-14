"""Removing a redundant covered aggregation preserves exact log-row selection."""
from contextlib import contextmanager
import unittest
from unittest.mock import patch
from tests import test_session_projection_batch as fixture


class SessionCoveredLookupTests(unittest.TestCase):
    setUp=fixture.SessionProjectionBatchTests.setUp
    log=fixture.SessionProjectionBatchTests.log

    def fixture_logs(self):
        from observatory.core import normalize_native
        trace='f'*32
        self.store.ingest([normalize_native(dict(trace_id=trace,span_id='f'*16,name='agent.run',
            start_unix_nano=1,end_unix_nano=1000,attributes={'session.id':'log-session','agent.id':'worker'},events=[]),'fixture')])
        self.store.ingest_logs([self.log(stamp,trace=trace) for stamp in range(100,350)]+
            [self.log(2,owner=None,result='error'),self.log(3,owner='foreign',result='error')]+
            [self.log(400+i//3,session='second',owner='other',activity=i%2==0,body=f'Occurrence {i}') for i in range(75)])

    def capture(self,*,frozen=False):
        original=self.store._connect;queries=[]
        class DB:
            def __init__(self,db):self.db=db
            def execute(self,sql,args=()):
                if 'WITH requested(session_id,session_position)' in sql:
                    if frozen:
                        sql=sql.replace('summary.agent_id,activity.last_ns AS last_ns','summary.agent_id,MAX(activity.last_ns) AS last_ns')
                        sql=sql.replace('WHERE summary.agent_id IS NOT NULL), candidates AS (',
                            'WHERE summary.agent_id IS NOT NULL GROUP BY summary.trace_id,summary.agent_id), candidates AS (')
                    queries.append((sql,args))
                return self.db.execute(sql,args)
            def __getattr__(self,name):return getattr(self.db,name)
        @contextmanager
        def connection():
            with original() as db:yield DB(db)
        with patch.object(self.store,'_connect',connection):result=self.store.sessions(limit=500)
        self.assertEqual(len(queries),1)
        return result,queries[0]

    def test_primary_key_premise_in_actual_schema(self):
        with self.store._connect() as db:
            for table in ('live_summaries_v1','trace_activity'):
                primary=[row['name'] for row in db.execute('PRAGMA table_info('+table+')') if row['pk']]
                self.assertEqual(primary,['trace_id'])

    def test_complete_payload_and_per_session_rank_match_frozen_covered_query(self):
        self.fixture_logs()
        current,new=self.capture();before,old=self.capture(frozen=True)
        self.assertEqual(current,before)
        self.assertIn('MAX(activity.last_ns)',old[0]);self.assertNotIn('MAX(activity.last_ns)',new[0])
        with self.store._connect() as db:
            db.execute('BEGIN')
            actual=[dict(row) for row in db.execute(*new)]
            expected=[dict(row) for row in db.execute(*old)]
        self.assertEqual(actual,expected)
        self.assertTrue(any(row['session_id']=='log-session' for row in actual),'Older unknown/foreign errors survive covered rows')
        self.assertEqual(sum(row['session_id']=='second' for row in actual),50)
        self.assertTrue(any(row['event_name']=='custom.signal' for row in actual),'Non-Activity fallback remains present')
        errors=[row for session in current for row in session['recent_errors']]
        self.assertEqual(len(errors),2);self.assertTrue(any(row['agent_id'] is None for row in errors))

    def test_null_falsy_owners_and_cutoff_equality_keep_same_relation(self):
        self.fixture_logs()
        _,new=self.capture();_,old=self.capture(frozen=True)
        with self.store._connect() as db:
            # Legacy writers can retain falsy TEXT identities. Preserve exact
            # SQL NULL semantics rather than normalizing them into each other.
            for owner in (None,'','0'):
                db.execute("UPDATE live_summaries_v1 SET agent_id=? WHERE trace_id=?",(owner,'f'*32))
                db.execute("UPDATE activity_index_v1 SET agent_id=? WHERE kind='log' AND trace_id=?",(owner,'f'*32))
                stamp=db.execute("SELECT MAX(stamp) FROM activity_index_v1 WHERE trace_id=?",('f'*32,)).fetchone()[0]
                for cutoff in (stamp-1,stamp,stamp+1):
                    db.execute('UPDATE trace_activity SET last_ns=? WHERE trace_id=?',(cutoff,'f'*32))
                    self.assertEqual([dict(row) for row in db.execute(*new)],[dict(row) for row in db.execute(*old)],(owner,cutoff))

    def test_query_plan_removes_covered_grouping_without_extra_index(self):
        self.fixture_logs();_,new=self.capture();_,old=self.capture(frozen=True)
        with self.store._connect() as db:
            def plan(query):return [tuple(row) for row in db.execute('EXPLAIN QUERY PLAN '+query[0],query[1])]
            before=plan(old);after=plan(new)
        self.assertTrue(any('USE TEMP B-TREE FOR GROUP BY' in row[3] for row in before),before)
        self.assertFalse(any('USE TEMP B-TREE FOR GROUP BY' in row[3] for row in after),after)
        self.assertTrue(any('activity_log_preview_v2' in row[3] for row in after),after)
        self.assertTrue(any('sqlite_autoindex_trace_activity_1' in row[3] for row in after),after)


if __name__=='__main__':unittest.main()
