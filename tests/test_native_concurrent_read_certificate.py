"""Generated concurrent HTTP proof of native publication and scoped readiness.

The worker is deliberately held: a successful read must come from a coherent
source transaction, not from winning a race with asynchronous repair.
"""
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
from copy import deepcopy
import json
from pathlib import Path
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.request import Request, ProxyHandler, build_opener

from observatory.core import Store
from observatory.server import BoundedHTTPServer, handler_for
from tests.test_derived_readiness_snapshot import wait_ready
from tests.test_native_noop_publication import span


class NativeConcurrentReadCertificateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.store = Store(Path(self.temp.name) / 'generated.db',
                           async_derived_refresh=True, workflow_patterns=False)
        self.addCleanup(self.store.close)
        wait_ready(self.store)
        self.handler = handler_for(self.store, 'generated-token', quiet=True)
        self.errors = []
        self.receiver_headers = {}
        owner = self
        class RecordingServer(BoundedHTTPServer):
            def handle_error(self, *args):
                owner.errors.append(sys.exc_info()[1])
        self.server = RecordingServer(('127.0.0.1', 0), self.handler)
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()
        self.addCleanup(self.stop_server)

    def stop_server(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(3)
        self.handler.response_cache.close()
        self.assertFalse(self.thread.is_alive())
        self.assertEqual(self.errors, [])

    def get(self, path, token='generated-token'):
        headers = {'Authorization': 'Bearer ' + token}
        request = Request(f'http://127.0.0.1:{self.server.server_port}{path}', headers=headers)
        try:
            with build_opener(ProxyHandler({})).open(request, timeout=3) as response:
                self.receiver_headers[path] = response.headers.get('X-Agenturm-Receiver-ID')
                return response.status, json.load(response)
        except HTTPError as exc:
            with closing(exc):
                return exc.code, json.load(exc)

    @staticmethod
    def record(number):
        row = span(number)
        row['events'] = [dict(name='agent.activity', time_unix_nano=10,
            attributes={'agent.activity.domain': 'message',
                        'agent.activity.action': 'user',
                        'agent.activity.subject': f'Generated task {number}'})]
        return row

    def test_replay_and_one_change_have_coherent_cached_and_fresh_http_reads(self):
        records = [self.record(number) for number in range(1, 8)]
        for record in records:
            self.assertEqual(self.store.ingest([record], skip_unchanged_native=True), 1)
        paths = ['/api/activity', '/api/activity?trace_id=' + records[-1]['trace_id'],
                 '/api/metrics', '/api/sessions']
        before = {path: self.get(path) for path in paths}
        self.assertTrue(all(code == 200 for code, _ in before.values()), before)
        expected_before = {
            paths[0]: (200, self.store.activity_page()),
            paths[1]: (200, self.store.activity_page(trace_id=records[-1]['trace_id'])),
            paths[2]: (200, self.store.metrics()),
            paths[3]: (200, {'sessions': self.store.sessions()}),
        }
        self.assertEqual(before, expected_before)
        old_cache_version = self.handler.response_cache.get(paths[0])[0]
        for path in paths:
            version, cached = self.handler.response_cache.get(path)
            self.assertEqual(version, old_cache_version)
            self.assertEqual(json.loads(cached), before[path][1])
            self.assertEqual(self.get(path, token='wrong')[0], 401)
        receiver = self.store.receiver_namespace()
        self.assertEqual(before[paths[0]][1]['receiver_id'], receiver)
        for path in paths:
            self.assertEqual(self.get(path), before[path])
            self.assertEqual(self.receiver_headers[path], receiver)
        changed = deepcopy(records)
        changed[-1]['events'][0]['attributes']['agent.activity.subject'] = 'Generated changed task'
        changed[-1]['attributes']['gen_ai.usage.input_tokens'] = 99
        entered, release = threading.Event(), threading.Event()
        original = self.store._publish_trace_summary
        def publish(*args, **kwargs):
            original(*args, **kwargs)
            entered.set()
            if not release.wait(3):
                raise AssertionError('Generated transaction barrier expired')
        with self.store._derived_lock, ThreadPoolExecutor(max_workers=1) as pool:
            with patch.object(self.store, '_publish_trace_summary', side_effect=publish):
                write = pool.submit(self.store.ingest, changed, skip_unchanged_native=True)
                try:
                    self.assertTrue(entered.wait(3))
                    # The cache and exact read certificate see the entire old
                    # snapshot while the new source/summary is uncommitted.
                    for path in paths:
                        self.assertEqual(self.get(path), before[path], path)
                finally:
                    release.set()
                self.assertEqual(write.result(timeout=3), 1)
            new_cache_version, stale = self.handler.response_cache.get(paths[0])
            self.assertNotEqual(new_cache_version, old_cache_version)
            self.assertIsNone(stale)
            after = {path: self.get(path) for path in paths}
            expected_after = {
                paths[0]: (200, self.store.activity_page()),
                paths[1]: (200, self.store.activity_page(trace_id=records[-1]['trace_id'])),
                paths[2]: (200, self.store.metrics()),
                paths[3]: (200, {'sessions': self.store.sessions()}),
            }
            self.assertEqual(after, expected_after)
            self.assertEqual(after[paths[0]][1]['receiver_id'], receiver)
            for path in paths:
                self.assertEqual(self.get(path), after[path])
                self.assertEqual(self.receiver_headers[path], receiver)
            self.assertTrue(all(code == 200 for code, _ in after.values()), after)
            self.assertEqual(after['/api/metrics'][1]['input_tokens'], 6 * 5 + 99)
            for path in paths[:2]:
                subjects = [item.get('subject') for item in after[path][1]['activity']]
                self.assertIn('Generated changed task', subjects)
                self.assertNotIn('Generated task 7', subjects)
            self.assertNotEqual(after['/api/sessions'], before['/api/sessions'])
            self.assertEqual(self.store.ingest(changed, skip_unchanged_native=True), 0)
            for path in paths:
                self.assertEqual(self.get(path), after[path], path)

    def test_real_unpublished_summary_is_503_but_current_global_activity_is_200(self):
        # Exceed the existing small-transaction admission, without modifying it.
        # Activity is already drained in the raw-source transaction. Scoped
        # Activity and summaries must reject until their own dependencies catch up.
        records = [self.record(1) for _ in range(65)]
        for number, record in enumerate(records, 1):
            record['span_id'] = f'{number:016x}'
            record['events'][0]['time_unix_nano'] = number
        with self.store._derived_lock:
            self.assertEqual(self.store.ingest(records, skip_unchanged_native=True), 65)
            code, page = self.get('/api/activity')
            self.assertEqual(code, 200)
            self.assertEqual(len(page['activity']), 65)
            for path in ('/api/activity?trace_id=' + records[0]['trace_id'],
                         '/api/metrics', '/api/sessions'):
                code, body = self.get(path)
                self.assertEqual(code, 503, path)
                self.assertFalse(body['indexing']['ready'])
                self.assertNotIn('activity', body)
                self.assertNotIn('sessions', body)
                self.assertNotIn('input_tokens', body)
        wait_ready(self.store)
        for path in ('/api/activity?trace_id=' + records[0]['trace_id'],
                     '/api/metrics', '/api/sessions'):
            self.assertEqual(self.get(path)[0], 200, path)


if __name__ == '__main__':
    unittest.main()
