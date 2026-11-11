"""History pagination and bounded work on the cockpit's startup read paths."""
from contextlib import contextmanager
import json
from pathlib import Path
import tempfile
import unittest

from observatory.core import Store, normalize_native


class CockpitReadPathTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.store=Store(Path(self.tmp.name)/'reads.db')
        self.addCleanup(self.store.close)

    def insert_logs(self, count):
        with self.store._connect() as db:
            db.executemany('INSERT INTO logs VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)', (
                (f'{i:064x}',None,None,i,'codex',9,'INFO','agent.activity',
                 json.dumps('older request' if i==0 else 'payload '*100),
                 json.dumps({'session.id':'history','agent.activity.domain':'message' if i==0 else 'shell',
                             'agent.activity.action':'USER' if i==0 else 'run',
                             'agent.activity.result':'failed' if i==1 else 'success'}),
                 '{}','fixture',None) for i in range(count)))

    def test_filters_precede_limits_and_merge_ties_without_losing_records(self):
        self.insert_logs(80)
        with self.store._connect() as db:
            for trace,service in (('a'*32,'codex'),('b'*32,'claude')):
                events=[{'source':'span','source_id':'same-span','service':service,
                    'time_unix_nano':79,'domain':'tool','action':'call','result':'success'}]
                db.execute('INSERT INTO trace_summaries VALUES(?,?,?,?,?)',
                           (trace,'summary-session','{}',json.dumps(events),'agent-'+service))
            # Summary identity still wins over an inconsistent raw log attribute.
            db.execute('UPDATE logs SET trace_id=?,agent_id=? WHERE digest=?',
                       ('a'*32,'log-agent',f'{78:064x}'))
        expected=self.store.activity_page(limit=500)['activity']
        self.assertEqual(len(expected),82)
        actual=[];cursor=None
        while True:
            page=self.store.activity_page(limit=3,cursor=cursor)
            actual.extend(page['activity']);cursor=page['next_cursor']
            if not cursor:break
        self.assertEqual(actual,expected)
        self.assertEqual(len({item['id'] for item in actual}),82)
        self.assertEqual({item['service'] for item in actual[:3]}, {'codex','claude'})
        prompts=self.store.activity_page(limit=1,domain='message',action='user')['activity']
        self.assertEqual(prompts[0]['detail'],'older request')
        failures=self.store.activity_page(limit=1,errors_only=True)['activity']
        self.assertEqual(failures[0]['time_unix_nano'],1)
        by_id=self.store.activity_page(limit=1,query=prompts[0]['source_id'])['activity']
        self.assertEqual(by_id,prompts)
        self.assertEqual(len(self.store.activity_page(session_id='summary-session')['activity']),3)
        self.assertNotIn(f'{78:064x}',{item['source_id'] for item in
            self.store.activity_page(session_id='history')['activity']})
        self.assertEqual(len(self.store.activity_page(service='claude')['activity']),1)

    def test_latest_page_does_not_serialize_or_scan_all_old_log_payloads(self):
        self.insert_logs(4000)
        connect=self.store._connect
        ticks=[]
        @contextmanager
        def measured():
            with connect() as db:
                db.set_progress_handler(lambda:ticks.append(1) or 0,1000)
                yield db
        self.store._connect=measured
        page=self.store.activity_page(limit=20,include_reviews=False)
        self.assertEqual(len(page['activity']),20)
        self.assertEqual(page['activity'][0]['time_unix_nano'],3999)
        # A structural VM-work budget, independent of CPU speed. The former
        # JSON-before-LIMIT query used hundreds of thousands of instructions.
        self.assertLess(len(ticks),50)

    def test_connection_counts_keep_text_identity_service_and_zero_counts(self):
        with self.store._connect() as db:
            db.executemany('INSERT INTO agent_connections VALUES(?,?,?,?,?,?,?,?,?)',
                [('7','codex','fixture','[]',0,None,1,3,'running'),
                 ('zero','codex','fixture','[]',0,None,1,2,'running')])
        spans=[normalize_native({'trace_id':'a'*32,'span_id':f'{i+1:016x}',
            'name':'tool','start_unix_nano':1,'end_unix_nano':2,
            'resource_attributes':{'observatory.source_id':source}},service=service)
            for i,(source,service) in enumerate([(7,'codex'),('7','codex'),('7','claude')])]
        self.store.ingest(spans)
        rows=self.store.connections()
        self.assertEqual([(r['source_id'],r['span_count']) for r in rows],[('7',2),('zero',0)])
