"""A certified Activity page must not reuse capture revoked by a source write."""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
import urllib.error
import urllib.request

from observatory.core import Store, normalize_native
from observatory.server import BoundedHTTPServer, handler_for


class ActivityRedactionCertificateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.store = Store(Path(self.temp.name) / 'generated.db')
        self.addCleanup(self.store.close)
        self.a = self.record('a', 'actor-a', 'PRIVATE_A', 20)
        self.b = self.record('b', 'actor-b', 'VISIBLE_B', 20)
        self.store.ingest([self.a, self.b])
        self.handler = handler_for(self.store, 'generated-token', quiet=True)
        self.server = BoundedHTTPServer(('127.0.0.1', 0), self.handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.stop_server)

    @staticmethod
    def record(letter, actor, text, offset):
        return normalize_native(dict(
            trace_id=letter * 32, span_id=letter * 16, name='agent.run',
            start_unix_nano=1, end_unix_nano=20, status='OK',
            scope_name='observatory.local_agents',
            resource_attributes={'observatory.source_id': letter * 32},
            attributes={
                'agent.id': actor, 'agent.runtime': 'Codex',
                'agent.namespace': 'generated', 'session.id': 'session-' + actor,
                'agent.task': text + '_TASK',
                'input.value': text + '_INPUT', 'output.value': text + '_OUTPUT',
                'observatory.native_file_id': 'generated:' + letter,
                'observatory.native_file_offset': offset,
                'observatory.native_mtime_ns': offset},
            events=[dict(name='agent.activity', time_unix_nano=10, attributes={
                'agent.activity.domain': 'shell', 'agent.activity.action': 'execute',
                'agent.activity.subject': text + '_SUBJECT',
                'agent.activity.detail': text + '_DETAIL',
                'agent.activity.result': 'success'})]), 'codex')

    def stop_server(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)
        self.handler.response_cache.close()
        self.assertFalse(self.thread.is_alive())

    def get(self, path='/api/activity?limit=50', token='generated-token'):
        request = urllib.request.Request(
            'http://127.0.0.1:' + str(self.server.server_port) + path,
            headers={'Authorization': 'Bearer ' + token})
        try:
            with urllib.request.urlopen(request, timeout=5) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as response:
            with response:
                return response.code, json.load(response)

    def retained_side_effects(self):
        # Read scalar retained state only; no public read-repair or derivation.
        with self.store._read() as db:
            db.execute('BEGIN')
            tables = ('spans', 'logs', 'trace_activity', 'endpoint_traces',
                      'endpoints', 'endpoint_services', 'agent_nodes',
                      'agent_identity', 'activity_index_v1', 'activity_dirty_v1',
                      'finding_dirty_v1')
            return {table: sorted((tuple(row) for row in db.execute('SELECT * FROM ' + table)), key=repr)
                    for table in tables}

    def test_actual_redaction_dirty_gate_safe_full_page_and_sticky_old_batch(self):
        path = '/api/activity?limit=50'
        code, warm = self.get(path)
        self.assertEqual(code, 200)
        self.assertIn('PRIVATE_A_SUBJECT', json.dumps(warm))
        self.assertIn('PRIVATE_A_DETAIL', json.dumps(warm))
        b_rows = [row for row in warm['activity'] if row['trace_id'] == 'b' * 32]
        self.assertTrue(b_rows)
        # The second identical request really is a complete cached response.
        with patch.object(self.store, 'activity_page', side_effect=AssertionError('Cache miss')):
            self.assertEqual(self.get(path), (200, warm))
        self.assertEqual(self.get(path, token='wrong')[0], 401)

        # Actual source policy transaction invalidates capture before summary work.
        with patch.object(self.store, '_refresh_after_write') as scheduled:
            self.store.redact_internal_source('a' * 32)
        scheduled.assert_called_once()
        with self.store._read() as db:
            self.assertTrue(db.execute('SELECT 1 FROM activity_dirty_v1').fetchone())
            self.assertTrue(db.execute('SELECT 1 FROM finding_dirty_v1').fetchone())
            raw_a = db.execute('SELECT attributes,events FROM spans WHERE trace_id=?', ('a' * 32,)).fetchone()
            self.assertNotIn('PRIVATE_A', str(tuple(raw_a)))
        # Do not let the public 1000-row legacy repair mask the dirty certificate.
        with patch.object(self.store, 'activity_page', side_effect=AssertionError('Dirty public repair')):
            code, unavailable = self.get(path)
        self.assertEqual(code, 503, unavailable)
        self.assertNotIn('activity', unavailable)
        self.assertNotIn('PRIVATE_A', json.dumps(unavailable))

        # Rebuild Activity through its normal bounded worker path only. Findings
        # deliberately remains pending; no queue rows are manually cleared.
        with patch.object(self.store, '_refresh_summaries', return_value=None):
            self.store._drain_external_activity()
        with self.store._read() as db:
            self.assertIsNone(db.execute('SELECT 1 FROM activity_dirty_v1').fetchone())
            self.assertTrue(db.execute('SELECT 1 FROM finding_dirty_v1').fetchone())
        self.assertFalse(self.store.index_status()['ready'])
        expected = self.store.activity_page(limit=50)
        self.assertNotIn('PRIVATE_A', json.dumps(expected))
        self.assertEqual([row for row in expected['activity'] if row['trace_id'] == 'b' * 32], b_rows)
        code, safe = self.get(path)
        self.assertEqual(code, 200, safe)  # Desired200 fails on the old global gate.
        self.assertEqual(safe, expected)

        # A pending older collector retains unsanitized capture, but neither its
        # content nor accepted-only retained side effects may reappear on retry.
        old_a = deepcopy(self.a)
        old_a['attributes']['observatory.native_file_offset'] = 10
        old_a['attributes']['observatory.native_mtime_ns'] = 10
        old_a['end_unix_nano'] = 999
        before = self.retained_side_effects()
        with patch.object(self.store, '_refresh_after_write') as refreshed:
            self.assertEqual(self.store.ingest([old_a], native_retry=True), 0)
        refreshed.assert_not_called()
        self.assertEqual(self.retained_side_effects(), before)
        self.assertEqual(self.get(path), (200, expected))
        self.assertIn('a' * 32, self.store.internal_source_ids(['a' * 32]))
        for selector in ('agent_id=' + b_rows[0]['agent_id'],
                         'agent_id=', 'subtree=0', 'scope_revision=',
                         'trace_id=' + 'a' * 32, 'turn_id=' + 'c' * 32, 'review=0'):
            with self.subTest(selector=selector):
                self.assertEqual(self.get(path + '&' + selector)[0], 503)


if __name__ == '__main__':
    unittest.main()
