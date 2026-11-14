"""Exact bounded log lookups retain ownership, replay and legacy invalidation."""
from pathlib import Path
import json
from tempfile import TemporaryDirectory
import unittest

from observatory.core import Store, normalize_native, _agent_id_from_attributes
from observatory.storage_activity import drain, initialize

TRACE = 'a' * 32
SPAN = 'b' * 16
LOOKUP = '''SELECT digest,trace_id,span_id,service,time_ns,attributes,resource_attributes,body
    FROM logs WHERE trace_id=? AND span_id=? AND service=? AND agent_id=?
    AND event_name='agent.activity'
    AND length(CAST(attributes AS BLOB))+length(CAST(resource_attributes AS BLOB))+
        length(CAST(body AS BLOB))<=65536
    ORDER BY time_ns DESC,digest DESC LIMIT 16'''


class ActivityLinkedIndexTests(unittest.TestCase):
    def setUp(self):
        self.temp = TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / 'fixture.db'
        self.store = Store(self.path)
        self.addCleanup(self.store.close)
        attrs = self.attrs()
        self.owner = _agent_id_from_attributes(attrs, {}, 'svc')
        self.store.ingest([normalize_native(dict(trace_id=TRACE, span_id=SPAN,
            name='Bash', start_unix_nano=1, end_unix_nano=100, status='ERROR',
            attributes=attrs, events=[]), 'svc')])

    def attrs(self, owner='parent'):
        return {'agent.id': owner, 'agent.runtime': 'SDK', 'agent.namespace': 'svc',
                'session.id': 'session', 'tool.name': 'Bash', 'process.command_line': 'pytest',
                'process.working_directory': '/fixture', 'process.exit.code': 1,
                'agent.activity.domain': 'shell', 'agent.activity.action': 'execute',
                'agent.activity.subject': 'pytest', 'agent.activity.result': 'error'}

    def log(self, stamp, *, owner='parent', service='svc', trace=TRACE, span=SPAN,
            body='synthetic stderr', byte_size=None):
        attrs = self.attrs(owner)
        if owner is None:
            attrs.pop('agent.id')
        if byte_size is not None:
            overhead = len(json.dumps(attrs).encode()) + len(json.dumps({}).encode()) + 2
            body = 'x' * (byte_size - overhead)
        return dict(trace_id=trace, span_id=span, time_unix_nano=stamp, service=service,
            severity_number=17, severity_text='ERROR', event_name='agent.activity', body=body,
            attributes=attrs, resource_attributes={}, scope_name='fixture')

    def rows(self, db):
        return [dict(row) for row in db.execute('SELECT * FROM activity_index_v1 WHERE trace_id=? ORDER BY event_key', (TRACE,))]

    def replay(self, db):
        db.execute("INSERT OR IGNORE INTO activity_dirty_v1 SELECT 'log',digest FROM logs WHERE trace_id=?", (TRACE,))
        db.execute("INSERT OR IGNORE INTO activity_dirty_v1 VALUES('span',?)", (TRACE + ':' + SPAN,))
        while drain(db, limit=3):
            pass
        return self.rows(db)

    def test_full_activity_outputs_equal_without_and_with_index(self):
        self.store.ingest_logs([self.log(i, body='Output ' + str(i)) for i in range(30)] +
            [self.log(90, owner='foreign'), self.log(91, owner=None), self.log(92, service='other')])
        with self.store._connect() as db:
            db.execute('BEGIN IMMEDIATE')
            db.execute('DROP INDEX logs_activity_linked_v1')
            expected = self.replay(db)
        # initialize() is idempotent; no source or queue path changes.
        with self.store._connect() as db:
            initialize(db)
        with self.store._connect() as db:
            db.execute('BEGIN IMMEDIATE')
            self.assertEqual(self.replay(db), expected)
        self.assertTrue(any(json.loads(row['payload']).get('operation_present') for row in expected))

    def test_exact_four_key_plan_cap_ties_and_byte_boundary_exclude_foreign_unknown(self):
        logs = [self.log(i // 2, body='Occurrence ' + str(i)) for i in range(30)]
        logs += [self.log(100, byte_size=65536), self.log(101, byte_size=65537),
                 self.log(102, owner='foreign'), self.log(103, owner=None),
                 self.log(104, service='other'), self.log(105, trace='c' * 32),
                 self.log(106, span='d' * 16), self.log(107, trace=None, span=None)]
        self.store.ingest_logs(logs)
        with self.store._connect() as db:
            args = (TRACE, SPAN, 'svc', self.owner)
            current = [tuple(row) for row in db.execute(LOOKUP, args)]
            plan = [row[3] for row in db.execute('EXPLAIN QUERY PLAN ' + LOOKUP, args)]
            db.execute('DROP INDEX logs_activity_linked_v1')
            original = [tuple(row) for row in db.execute(LOOKUP, args)]
            sizes = [row[0] for row in db.execute('''SELECT length(CAST(attributes AS BLOB))+
                length(CAST(resource_attributes AS BLOB))+length(CAST(body AS BLOB)) FROM logs WHERE time_ns IN (100,101) ORDER BY time_ns''')]
        self.assertEqual(current, original)
        self.assertEqual(len(current), 16)
        self.assertEqual(current[0][4], 100)
        self.assertEqual(sizes, [65536, 65537])
        self.assertTrue(any('logs_activity_linked_v1 (trace_id=? AND span_id=? AND service=? AND agent_id=?)' in value for value in plan), plan)
        self.assertFalse(any('TEMP B-TREE' in value for value in plan), plan)

    def test_legacy_writer_update_delete_and_prune_keep_index_and_queue_current(self):
        self.store.ingest_logs([self.log(10)])
        with self.store._connect() as db:
            row = db.execute('SELECT digest,body,attributes FROM logs').fetchone()
            digest, original_body = row['digest'], row['body']
            db.execute('UPDATE logs SET body=? WHERE digest=?', (json.dumps('x' * 70000), digest))
            self.assertEqual(list(db.execute(LOOKUP, (TRACE, SPAN, 'svc', self.owner))), [])
            queued = {tuple(row) for row in db.execute('SELECT kind,owner_key FROM activity_dirty_v1')}
            self.assertIn(('log', digest), queued)
            self.assertIn(('span', TRACE + ':' + SPAN), queued)
            db.execute('UPDATE logs SET body=?,time_ns=20 WHERE digest=?', (original_body, digest))
            self.assertEqual(db.execute(LOOKUP, (TRACE, SPAN, 'svc', self.owner)).fetchone()['time_ns'], 20)
            foreign = json.loads(row['attributes'])
            foreign['agent.id'] = 'foreign'
            foreign_owner = _agent_id_from_attributes(foreign, {}, 'svc')
            db.execute('UPDATE logs SET attributes=?,agent_id=? WHERE digest=?',
                       (json.dumps(foreign), foreign_owner, digest))
            self.assertEqual(list(db.execute(LOOKUP, (TRACE, SPAN, 'svc', self.owner))), [])
            db.execute('UPDATE logs SET attributes=?,agent_id=? WHERE digest=?',
                       (row['attributes'], self.owner, digest))
            self.assertIsNotNone(db.execute(LOOKUP, (TRACE, SPAN, 'svc', self.owner)).fetchone())
            db.execute('DELETE FROM logs WHERE digest=?', (digest,))
            self.assertEqual(list(db.execute(LOOKUP, (TRACE, SPAN, 'svc', self.owner))), [])
            while drain(db):
                pass
            self.assertFalse(any(row['kind'] == 'log' for row in self.rows(db)))
        self.store.ingest_logs([self.log(30)])
        self.store.prune(1, now_ns=3 * 86400_000_000_000)
        with self.store._connect() as db:
            self.assertEqual(list(db.execute(LOOKUP, (TRACE, SPAN, 'svc', self.owner))), [])
            while drain(db):
                pass
            self.assertEqual(self.rows(db), [])

    def test_unlinkable_rows_do_not_enter_partial_index(self):
        self.store.ingest_logs([self.log(1), self.log(2, span=None),
                                self.log(3, trace=None), self.log(4, owner=None)])
        with self.store._connect() as db:
            rows = db.execute('''SELECT digest FROM logs INDEXED BY logs_activity_linked_v1
                WHERE trace_id IS NOT NULL AND span_id IS NOT NULL AND agent_id IS NOT NULL
                  AND event_name='agent.activity'
                  AND length(CAST(attributes AS BLOB))+length(CAST(resource_attributes AS BLOB))+
                      length(CAST(body AS BLOB))<=65536''').fetchall()
        self.assertEqual(len(rows), 1)


if __name__ == '__main__':
    unittest.main()
