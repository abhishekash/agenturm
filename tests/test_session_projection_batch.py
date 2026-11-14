"""Session batching preserves exact owners, scalar facts and bounded text transfer."""
from contextlib import contextmanager
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from observatory.core import Store, normalize_native
from observatory.storage_sessions import session_projection
from tests.fixtures.session_projection_before import session_projection as before


def canonical(result):
    heads, totals, titles = result
    return [dict(row) for row in heads], totals, titles


class SessionProjectionBatchTests(unittest.TestCase):
    def setUp(self):
        self.tmp = TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.store = Store(Path(self.tmp.name) / 'sessions.db')
        self.addCleanup(self.store.close)
        self.store.ingest([normalize_native(dict(trace_id=f'{i:032x}', span_id=f'{i:016x}',
            name='agent.run', start_unix_nano=i * 10, end_unix_nano=i * 10 + 1,
            status='ERROR' if i % 5 == 0 else 'OK', attributes={
                'session.id': f'session-{i % 4}',
                **({'agent.id': f'worker-{i % 3}'} if i % 7 else {}),
                'gen_ai.request.model': f'model-{i % 6}',
                **({'agent.session.task': f'Original {i}'} if i % 3 == 0 else {}),
                **({'gen_ai.usage.input_tokens': i, 'gen_ai.usage.output_tokens': i // 2} if i % 6 else {})},
            events=[]), 'fixture-' + str(i % 2)) for i in range(1, 49)])

    def test_projection_equals_baseline_for_shared_sessions_unknowns_usage_and_titles(self):
        with self.store._connect() as db:
            db.execute('BEGIN')
            for limit in (1, 2, 4, 500):
                self.assertEqual(canonical(session_projection(db, limit)), canonical(before(db, limit)))

    def test_constant_query_count_and_bounded_per_owner_row_transfer(self):
        class Cursor:
            def __init__(self, cursor, sql, transfers):
                self.cursor, self.sql, self.transfers = cursor, sql, transfers
            def fetchall(self):
                rows = self.cursor.fetchall()
                self.transfers.append((self.sql, len(rows)))
                return rows
            def __iter__(self):
                rows = self.fetchall()
                return iter(rows)
        class RecordingDB:
            def __init__(self, db):
                self.db, self.transfers, self.queries = db, [], []
            def execute(self, sql, args=()):
                self.queries.append(sql)
                return Cursor(self.db.execute(sql, args), sql, self.transfers)
        with self.store._connect() as db:
            recording = RecordingDB(db)
            heads, totals, titles = session_projection(recording, 500)
            self.assertEqual(len(recording.queries), 5)
            owners = len(totals)
            self.assertLessEqual(len(heads), owners * 2)
            self.assertLessEqual(len(titles), owners)
            label_rows = next(count for sql, count in recording.transfers if 'distinct_labels AS' in sql)
            self.assertLessEqual(label_rows, owners * 129 * 2)
            self.assertEqual(sum('json_each' in sql for sql in recording.queries), 1)

    def test_labels_preserve_clipping_flags_and_deduplicate_prefix_before_cap(self):
        prefix = 'A' * 128
        values = [prefix, prefix + 'clipped', *[f'B{i:03}' for i in range(128)]]
        with self.store._connect() as db:
            owner = db.execute('SELECT session_id,agent_id FROM live_run_stats_v1 ORDER BY trace_id LIMIT 1').fetchone()
            db.execute('UPDATE live_run_stats_v1 SET models_json=?,models_complete=1 WHERE session_id=? AND agent_id IS ?',
                       (json.dumps(values), *owner))
            _, totals, _ = session_projection(db, 500)
        item = totals[tuple(owner)]
        self.assertEqual(len(item['models']), 128)
        self.assertIn('B126', item['models'])
        self.assertNotIn('B127', item['models'])
        self.assertFalse(item['models_complete'])

    def test_whole_store_log_only_and_partial_usage_are_unchanged(self):
        self.store.ingest_logs([dict(trace_id=None, span_id=None, time_unix_nano=1000,
            service='log-only', event_name='agent.activity', severity_number=9, severity_text='INFO',
            body='Standalone log', attributes={'session.id': 'log-session', 'agent.activity.domain': 'message',
                'agent.activity.action': 'user', 'gen_ai.usage.input_tokens': 17},
            resource_attributes={}, scope_name='fixture')])
        current = self.store.sessions(limit=500)
        with patch('observatory.storage_sessions.session_projection', before):
            original = self.store.sessions(limit=500)
        self.assertEqual(current, original)
        self.assertTrue(any(row['session_id'] == 'log-session' for row in current))

    def test_selection_labels_titles_and_totals_share_one_wal_snapshot(self):
        original = self.store._connect
        fired = []
        @contextmanager
        def interleaved():
            with original() as db:
                def callback(sql):
                    if 'WITH ranked AS' not in sql or fired:
                        return
                    fired.append(True)
                    with original() as writer:
                        writer.execute('DELETE FROM trace_activity')
                        writer.execute('UPDATE live_run_stats_v1 SET initial_task=\'Changed\',models_json=\'["FOREIGN"]\'')
                db.set_trace_callback(callback)
                yield db
        with original() as db:
            expected = canonical(session_projection(db, 500))
        with interleaved() as db:
            db.execute('BEGIN')
            observed = canonical(session_projection(db, 500))
        self.assertTrue(fired)
        self.assertEqual(observed, expected)
        with original() as db:
            self.assertEqual(session_projection(db, 500), ([], {}, {}))

    def test_narrow_label_batch_uses_existing_session_index(self):
        statements = []
        with self.store._connect() as db:
            db.set_trace_callback(statements.append)
            session_projection(db, 1)
            db.set_trace_callback(None)
            label_sql = next(sql for sql in statements if 'distinct_labels AS' in sql)
            plan = [row[3] for row in db.execute('EXPLAIN QUERY PLAN ' + label_sql)]
        self.assertTrue(any('SEARCH live_run_stats_v1 USING INDEX live_run_session_v2' in row for row in plan), plan)
        self.assertFalse(any('FROM spans' in sql or 'FROM logs' in sql for sql in statements))


if __name__ == '__main__':
    unittest.main()
