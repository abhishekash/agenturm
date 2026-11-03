"""Deterministic concurrency budgets and atomic publication for feed reads."""
import io
import threading
import unittest
from unittest.mock import MagicMock, patch

from observatory.feed_requests import FeedScope, RequestBatch
from observatory.terminal import Api, CockpitError, Feed
from tests.test_terminal_search import ExampleApi


class FeedConcurrencyTests(unittest.TestCase):
    def test_cockpit_does_not_request_global_totals_or_hidden_views(self):
        api=ExampleApi();feed=Feed(api)
        feed.active_page='agents';feed.include_global_totals=False;feed.detail_enabled=False
        original=api.get
        def get(path):
            if path=='/api/metrics':raise TimeoutError('expensive all-history totals')
            return original(path)
        with patch.object(api,'get',side_effect=get):feed.refresh()
        self.assertFalse(feed.snapshot.error)
        self.assertTrue(feed.snapshot.updated)
        self.assertNotIn('/api/metrics',api.requests)
        self.assertFalse(any(path.startswith(('/api/mcp','/api/metric-','/api/measurements','/api/usage-series','/api/findings')) for path in api.requests))
        api.requests.clear();feed.active_page='mcp';feed.refresh()
        self.assertTrue(any(path.startswith('/api/mcp?') for path in api.requests))
        self.assertTrue(feed.snapshot.mcp)
        api.requests.clear();feed.active_page='metrics';feed.refresh()
        self.assertTrue(any(path.startswith('/api/usage-series?') for path in api.requests))

    def test_data_timeout_names_the_request_without_claiming_disconnection(self):
        api=ExampleApi();feed=Feed(api);feed.active_page='agents';feed.include_global_totals=False
        original=api.get
        def get(path):
            if path.startswith('/api/sessions'):raise TimeoutError('timed out')
            return original(path)
        with patch.object(api,'get',side_effect=get):feed.refresh()
        self.assertIn('Loading delayed',feed.snapshot.error)
        self.assertIn('/api/sessions',feed.snapshot.error)

    def test_hidden_trace_payload_is_not_loaded_and_reopens_on_demand(self):
        api = ExampleApi()
        feed = Feed(api)
        feed.detail_enabled = False
        feed.refresh()
        self.assertFalse(feed.snapshot.error)
        self.assertTrue(feed.snapshot.traces)
        self.assertEqual(feed.snapshot.detail, {})
        self.assertFalse(any(path.startswith('/api/traces/') for path in api.requests))
        feed.detail_enabled = True
        feed.refresh()
        self.assertFalse(feed.snapshot.error)
        self.assertTrue(feed.snapshot.detail)
        self.assertTrue(any(path.startswith('/api/traces/') for path in api.requests))

    def test_hiding_inspector_discards_inflight_detail_snapshot(self):
        api = ExampleApi()
        feed = Feed(api)
        original = api.get
        def get(path):
            if path.startswith('/api/traces/'):
                feed.detail_enabled = False
            return original(path)
        with patch.object(api, 'get', side_effect=get):
            feed.refresh()
        self.assertEqual(feed.snapshot.updated, 0)
        self.assertTrue(feed.wake.is_set())

    def test_prefetch_is_parallel_bounded_and_deduplicated(self):
        started = threading.Event()
        release = threading.Event()
        lock = threading.Lock()
        counts = {'active': 0, 'maximum': 0, 'calls': 0}

        class BlockingApi:
            def get(self, path):
                with lock:
                    counts['active'] += 1
                    counts['calls'] += 1
                    counts['maximum'] = max(counts['maximum'], counts['active'])
                    if counts['active'] == RequestBatch.MAX_WORKERS:
                        started.set()
                try:
                    if not release.wait(5):
                        raise TimeoutError('test did not release workers')
                    return path
                finally:
                    with lock:
                        counts['active'] -= 1

        batch = RequestBatch(BlockingApi(), threading.Event())
        paths = [f'/item/{i}' for i in range(12)]
        try:
            batch.prefetch(paths + paths[:2])
            self.assertTrue(started.wait(5))
            self.assertEqual(counts['active'], 4)
            release.set()
            self.assertEqual([batch.get(path) for path in paths], paths)
        finally:
            release.set()
            batch.close()
        self.assertEqual(counts['maximum'], 4)
        self.assertEqual(counts['calls'], 12)
        self.assertEqual(counts['active'], 0)

    def test_snapshot_stays_atomic_while_independent_requests_finish(self):
        blocked, release, other_finished = (threading.Event() for _ in range(3))

        class BlockingApi(ExampleApi):
            def get(self, path):
                if path == '/api/metrics':
                    blocked.set()
                    if not release.wait(5):
                        raise TimeoutError('test did not release metrics')
                result = super().get(path)
                if path == '/api/connections':
                    other_finished.set()
                return result

        feed = Feed(BlockingApi())
        original = feed.snapshot
        worker = threading.Thread(target=feed.refresh)
        worker.start()
        try:
            self.assertTrue(blocked.wait(5))
            self.assertTrue(other_finished.wait(5))
            self.assertIs(feed.snapshot, original)
        finally:
            release.set()
            worker.join(5)
        self.assertFalse(worker.is_alive())
        self.assertIsNot(feed.snapshot, original)
        self.assertFalse(feed.snapshot.error)
        self.assertEqual(feed.snapshot.totals['input_tokens'], 100)
        self.assertTrue(feed.snapshot.activity)

    def test_selection_scope_change_discards_prefetched_snapshot(self):
        api = ExampleApi()
        feed = Feed(api)
        original = api.get

        def get(path):
            if path == '/api/health':
                feed.pinned_trace = True
            return original(path)

        with patch.object(api, 'get', side_effect=get):
            feed.refresh()
        self.assertEqual(feed.snapshot.updated, 0)
        self.assertTrue(feed.wake.is_set())

    def test_scope_captures_navigation_and_filters(self):
        feed = Feed(ExampleApi())
        scope = FeedScope.capture(feed)
        feed.selected_span_id = 'changed'
        self.assertNotEqual(scope, FeedScope.capture(feed))

    def test_trace_error_filter_resets_pagination_scope(self):
        feed = Feed(ExampleApi())
        feed.refresh()
        feed.history['traces'] = [None, 'previous-unfiltered-page']
        feed.errors_only = True
        feed.refresh()
        self.assertEqual(feed.history['traces'], [None])
        self.assertFalse(feed.snapshot.error)

    def test_stopped_batches_do_not_start_more_requests(self):
        api = MagicMock()
        stop = threading.Event()
        stop.set()
        batch = RequestBatch(api, stop)
        batch.prefetch(['/never'])
        with self.assertRaises(InterruptedError):
            batch.get('/never')
        batch.close()
        api.get.assert_not_called()

    def test_prefetch_queue_has_a_hard_budget(self):
        batch = RequestBatch(MagicMock(), threading.Event())
        try:
            with self.assertRaises(ValueError):
                batch.prefetch([str(i) for i in range(batch.MAX_PREFETCH + 1)])
        finally:
            batch.close()


class ApiResponseBudgetTests(unittest.TestCase):
    def test_url_credentials_are_rejected_even_with_empty_username(self):
        for endpoint in ('http://:private@localhost:14318', 'http://user@localhost:14318'):
            with self.subTest(endpoint=endpoint), self.assertRaises(CockpitError) as error:
                Api(endpoint)
            self.assertNotIn('private', str(error.exception))

    def test_response_read_is_bounded_before_json_decode(self):
        response = MagicMock()
        response.__enter__.return_value = io.BytesIO(b' ' * 33)
        with patch('observatory.terminal.urllib.request.urlopen', return_value=response), \
                patch.object(Api, 'MAX_RESPONSE_BYTES', 32):
            with self.assertRaisesRegex(CockpitError, 'response exceeded'):
                Api('http://localhost:14318').get('/api/metrics')

    def test_small_json_response_is_accepted(self):
        response = MagicMock()
        response.__enter__.return_value = io.BytesIO(b'{"ok": true}')
        with patch('observatory.terminal.urllib.request.urlopen', return_value=response):
            self.assertEqual(Api('http://localhost:14318').get('/api/health'), {'ok': True})
