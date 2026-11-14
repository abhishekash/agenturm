"""Independent rollback, older-job and capture controls for native publication."""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch

from observatory.core import IndexNotReady, Store, normalize_native


class AtomicNativePublicationSafetyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.store = Store(Path(self.temp.name) / 'generated.db', async_derived_refresh=True)
        self.addCleanup(self.store.close)
        self.wait_for(lambda: self.store.migration_status.get('ready') is True)
        self.a = self.record('a', 'PRIVATE_A')
        self.b = self.record('b', 'VISIBLE_B')
        self.assertEqual(self.store.ingest([self.a, self.b]), 2)
        self.wait_ready()
        self.assertEqual(self.store.metrics()['input_tokens'], 10)

    @staticmethod
    def record(letter, text, *, tokens=5, offset=20):
        stamp = time.time_ns() - 10**9
        return normalize_native(dict(
            trace_id=letter * 32, span_id='1' * 16, name='chat fixture',
            start_unix_nano=stamp, end_unix_nano=stamp + offset,
            status='OK', scope_name='observatory.local_agents',
            resource_attributes={'observatory.source_id': letter * 32},
            attributes={
                'agent.id': 'worker-' + letter,
                'agent.runtime': 'Codex', 'agent.namespace': 'generated-native',
                'session.id': 'session-' + letter,
                'agent.task': text + '_TASK',
                'input.value': text + '_INPUT', 'output.value': text + '_OUTPUT',
                'gen_ai.usage.input_tokens': tokens,
                'gen_ai.usage.output_tokens': 2,
                'observatory.native_file_id': 'generated:' + letter,
                'observatory.native_file_offset': offset,
                'observatory.native_mtime_ns': offset},
            events=[dict(name='agent.activity', time_unix_nano=stamp + offset,
                attributes={
                    'agent.activity.domain': 'message',
                    'agent.activity.action': 'user',
                    'agent.activity.subject': text + '_SUBJECT',
                    'agent.activity.detail': text + '_DETAIL',
                    'agent.activity.result': 'success'})]), 'codex')

    def wait_for(self, predicate, message='Generated worker did not finish'):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(.01)
        self.fail(message)

    def wait_ready(self):
        self.wait_for(lambda: self.store.index_status()['ready'] and not self.store.last_derived_error)

    def summaries(self):
        with self.store._read() as db:
            return [tuple(row) for row in db.execute('SELECT * FROM trace_summaries ORDER BY trace_id')]

    def retained(self):
        tables = ('spans', 'logs', 'trace_activity', 'endpoints', 'endpoint_services',
                  'endpoint_traces', 'agent_nodes', 'agent_identity', 'activity_index_v1',
                  'activity_dirty_v1', 'finding_dirty_v1', 'trace_summaries', 'finding_cache',
                  'live_run_stats_v1', 'live_log_usage_v1', 'live_summaries_v1')
        with self.store._read() as db:
            db.execute('BEGIN')
            return {table: sorted((tuple(row) for row in db.execute('SELECT * FROM ' + table)), key=repr)
                    for table in tables}

    def test_second_trace_failure_rolls_back_all_publications_but_keeps_raw_and_repairs(self):
        original_summaries = self.summaries()
        projection_tables = ('trace_summaries', 'live_summaries_v1', 'live_run_stats_v1')
        original_projections = {name: rows for name, rows in self.retained().items()
                                if name in projection_tables}
        updated = [deepcopy(self.a), deepcopy(self.b)]
        for record, tokens in zip(updated, (11, 13)):
            record['attributes'].update({
                'gen_ai.usage.input_tokens': tokens,
                'observatory.native_file_offset': 30,
                'observatory.native_mtime_ns': 30})
        publish = self.store._publish_trace_summary
        attempted = []

        def fail_second(db, trace_id, receiver, **options):
            publish(db, trace_id, receiver, **options)
            attempted.append(trace_id)
            if len(attempted) == 2:
                raise RuntimeError('Generated second-trace publication failure')

        with self.store._derived_lock:
            with patch.object(self.store, '_publish_trace_summary', side_effect=fail_second):
                self.assertEqual(self.store.ingest(updated), 2)
            self.assertEqual(attempted, ['a' * 32, 'b' * 32])
            # Both newly accepted raw rows survive, but neither partial summary
            # can escape the savepoint, including the first successful publish.
            with self.store._read() as db:
                raw = [(row[0], json.loads(row[1])['gen_ai.usage.input_tokens'])
                       for row in db.execute('SELECT trace_id,attributes FROM spans ORDER BY trace_id')]
                dirty = [row[0] for row in db.execute('SELECT trace_id FROM finding_dirty_v1 ORDER BY trace_id')]
            self.assertEqual(raw, [('a' * 32, 11), ('b' * 32, 13)])
            self.assertEqual(dirty, ['a' * 32, 'b' * 32])
            self.assertEqual(self.summaries(), original_summaries)
            self.assertEqual({name: rows for name, rows in self.retained().items()
                              if name in projection_tables}, original_projections)
            self.assertEqual(self.store.last_derived_error, 'Generated second-trace publication failure')
            with self.assertRaises(IndexNotReady):
                self.store.metrics()

        # The normal queued worker, with no test repair call, recovers both jobs.
        self.wait_ready()
        self.assertEqual(self.store.metrics()['input_tokens'], 24)
        self.assertNotEqual(self.summaries(), original_summaries)
        with self.store._read() as db:
            self.assertIsNone(db.execute('SELECT 1 FROM finding_dirty_v1').fetchone())

    def test_older_inflight_job_rechecks_dirty_after_new_atomic_publication(self):
        older = deepcopy(self.a)
        older['attributes'].update({
            'gen_ai.usage.input_tokens': 9,
            'observatory.native_file_offset': 25,
            'observatory.native_mtime_ns': 25})
        newer = deepcopy(older)
        newer['attributes'].update({
            'gen_ai.usage.input_tokens': 31,
            'observatory.native_file_offset': 30,
            'observatory.native_mtime_ns': 30})
        held = True
        self.store._derived_lock.acquire()
        try:
            # An accepted native write during preparation legitimately uses the
            # existing async path. Its actual worker becomes inflight and waits.
            with patch.object(self.store, 'migration_status', {'ready': False, 'stage': 'Preparing indexed history'}):
                self.assertEqual(self.store.ingest([older]), 1)
            self.wait_for(lambda: 'a' * 32 in self.store._derived_inflight,
                          'Older queued job did not become inflight')
            with patch.object(self.store, '_publish_trace_summary', wraps=self.store._publish_trace_summary) as publish:
                self.assertEqual(self.store.ingest([newer]), 1)
                self.assertEqual(publish.call_count, 1)
                expected = self.summaries()
                self.assertEqual(self.store.metrics()['input_tokens'], 36)
                publish.reset_mock()
                self.store._derived_lock.release()
                held = False
                self.wait_for(lambda: not self.store._derived_pending and not self.store._derived_inflight)
                publish.assert_not_called()
                self.assertEqual(self.summaries(), expected)
                self.assertEqual(self.store.metrics()['input_tokens'], 36)
        finally:
            if held:
                self.store._derived_lock.release()

    def test_actual_redaction_stale_retry_and_new_capture_keep_source_policy(self):
        before = self.store.activity_page(limit=50)
        self.assertIn('PRIVATE_A', json.dumps(before))
        sibling = [row for row in before['activity'] if row['trace_id'] == 'b' * 32]
        self.assertTrue(sibling)
        self.store.redact_internal_source('a' * 32)
        self.wait_ready()
        safe = self.store.activity_page(limit=50)
        self.assertNotIn('PRIVATE_A', json.dumps(safe))
        self.assertEqual([row for row in safe['activity'] if row['trace_id'] == 'b' * 32], sibling)

        older = deepcopy(self.a)
        older['attributes'].update({
            'observatory.native_file_offset': 10,
            'observatory.native_mtime_ns': 10})
        retained = self.retained()
        with patch.object(self.store, '_publish_trace_summary', side_effect=AssertionError('Ignored retry published summary')):
            self.assertEqual(self.store.ingest([older], native_retry=True), 0)
        self.assertEqual(self.retained(), retained)
        self.assertEqual(self.store.activity_page(limit=50), safe)

        # A genuinely newer accepted collector batch is still scrubbed before
        # the atomic summary helper sees it; disabling capture remains sticky.
        newer = deepcopy(self.a)
        newer['attributes'].update({
            'observatory.native_file_offset': 30,
            'observatory.native_mtime_ns': 30})
        self.assertEqual(self.store.ingest([newer]), 1)
        self.wait_ready()
        self.assertNotIn('PRIVATE_A', json.dumps(self.store.activity_page(limit=50)))
        self.assertNotIn('PRIVATE_A', json.dumps(self.store.sessions()))
        with self.store._read() as db:
            row = db.execute('SELECT attributes,events,service FROM spans WHERE trace_id=?', ('a' * 32,)).fetchone()
            self.assertNotIn('PRIVATE_A', str(tuple(row)))
            self.assertEqual(row['service'], 'codex-review')
        self.assertEqual([row for row in self.store.activity_page(limit=50)['activity']
                          if row['trace_id'] == 'b' * 32], sibling)


if __name__ == '__main__':
    unittest.main()
