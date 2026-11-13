"""Deterministic concurrency budgets and atomic publication for feed reads."""
import io
import threading
import unittest
import urllib.error
from unittest.mock import MagicMock, patch

from observatory.feed_requests import FeedScope, RequestBatch
from observatory.terminal import Api, CockpitError, Feed
from tests.test_terminal_search import ExampleApi


class FeedConcurrencyTests(unittest.TestCase):
    def test_receiver_replacement_drops_all_old_optional_evidence_and_history_budget(self):
        api=ExampleApi();feed=Feed(api);feed.active_page='agents';feed.detail_enabled=False
        original=api.get;receiver=['old'];history_reads=[]
        def get(path):
            if path.startswith('/api/health'):
                return {**original(path),'receiver_id':receiver[0]}
            if path.startswith('/api/sessions'):
                history_reads.append(receiver[0])
                if receiver[0]=='new':raise TimeoutError('new receiver history busy')
                return {'sessions':[{'session_id':'old-private','latest_task':'Old receiver only'}]}
            if receiver[0]=='new' and path.startswith(('/api/activity','/api/connections','/api/traces')):
                raise TimeoutError('new receiver stored reads busy')
            return original(path)
        with patch.object(api,'get',side_effect=get):
            feed.refresh()
            self.assertEqual(feed.snapshot.receiver_id,'old')
            self.assertTrue(feed.snapshot.sessions);self.assertTrue(feed.snapshot.activity)
            old_attempt=feed.sessions_attempted
            receiver[0]='new';api.sessions=[{**api.sessions[0],'task':'New receiver live agent'}]
            feed.refresh()
        snapshot=feed.snapshot
        self.assertEqual(snapshot.receiver_id,'new');self.assertFalse(snapshot.error)
        self.assertEqual(snapshot.local_status['sessions'][0]['task'],'New receiver live agent')
        self.assertEqual(snapshot.sessions,[]);self.assertEqual(snapshot.sessions_updated,0)
        self.assertTrue(snapshot.sessions_error);self.assertIn('new',history_reads)
        self.assertGreaterEqual(feed.sessions_attempted,old_attempt)
        for rows in (snapshot.agents,snapshot.activity,snapshot.activity_results,snapshot.traces):self.assertFalse(rows)
        self.assertTrue(all(stamp==0 for stamp in snapshot.read_updated.values()))

    def test_receiver_replacement_resets_paging_and_hidden_view_caches(self):
        api=ExampleApi();feed=Feed(api);feed.detail_enabled=False;original=api.get;receiver=['old']
        def get(path):
            result=original(path)
            if path.startswith('/api/health'):result={**result,'receiver_id':receiver[0]}
            return result
        with patch.object(api,'get',side_effect=get):
            feed.refresh();self.assertIsNotNone(feed.extra_cache)
            feed.history['activity']=[None,'old-cursor'];feed.findings_cache={'findings':[{'id':'old'}]}
            receiver[0]='new';feed.refresh()
            self.assertEqual(feed.history['activity'],[None]);self.assertIsNone(feed.extra_cache)
            self.assertIsNone(feed.findings_cache);self.assertIsNone(feed.sessions_cache)
            feed.refresh();self.assertEqual(feed.snapshot.receiver_id,'new')
            self.assertNotIn('old-cursor',str(feed.snapshot.pagination))

    def test_busy_database_does_not_hide_native_roster_and_recovers(self):
        api=ExampleApi();feed=Feed(api);feed.active_page='agents';feed.detail_enabled=False
        feed.include_global_totals=False;original=api.get
        def busy(path):
            if path.startswith(('/api/connections','/api/activity','/api/traces','/api/sessions')):
                raise TimeoutError('database busy')
            return original(path)
        with patch.object(api,'get',side_effect=busy):
            feed.refresh()
            self.assertTrue(feed.snapshot.updated);self.assertFalse(feed.snapshot.error)
            self.assertEqual(set(feed.snapshot.read_errors),{'connections','activity','traces'})
            self.assertEqual(feed.snapshot.local_status['sessions'],api.sessions)
            self.assertFalse(feed.snapshot.activity);self.assertFalse(feed.snapshot.traces)
            api.sessions=[{**api.sessions[0],'task':'Live task advanced'}]
            feed.refresh()
            self.assertEqual(feed.snapshot.local_status['sessions'][0]['task'],'Live task advanced')
        feed.retry_now();feed.refresh()
        self.assertFalse(feed.snapshot.error);self.assertFalse(feed.snapshot.read_errors)
        self.assertFalse(feed.snapshot.sessions_error)
        self.assertTrue(feed.snapshot.activity);self.assertTrue(feed.snapshot.traces)

    def test_partial_evidence_retains_same_scope_and_never_leaks_across_scope(self):
        api=ExampleApi();feed=Feed(api);feed.active_page='agents';feed.detail_enabled=False
        feed.refresh();old=feed.snapshot;original=api.get
        def busy(path):
            if path.startswith(('/api/connections','/api/activity','/api/traces')):
                raise TimeoutError('database busy')
            return original(path)
        with patch.object(api,'get',side_effect=busy):
            feed.refresh()
            self.assertEqual(feed.snapshot.activity,old.activity)
            self.assertEqual(feed.snapshot.traces,old.traces)
            self.assertEqual(feed.snapshot.read_updated,old.read_updated)
            feed.errors_only=True;feed.refresh()
            self.assertEqual(feed.snapshot.traces,[])
            self.assertEqual(feed.snapshot.read_updated['traces'],0)
        feed.errors_only=False;feed.refresh()
        self.assertTrue(feed.snapshot.activity);self.assertTrue(feed.snapshot.traces)
        with patch.object(api,'get',side_effect=busy):
            feed.session_id='session-2';feed.refresh()
            self.assertEqual(feed.snapshot.activity,[])
            self.assertEqual(feed.snapshot.activity_results,[])
            self.assertEqual(feed.snapshot.traces,[])
            self.assertEqual(feed.snapshot.agents,old.agents)  # Global inventory.
            self.assertEqual(feed.snapshot.read_updated['activity'],0)
            self.assertEqual(feed.snapshot.read_updated['traces'],0)

    def test_partial_roster_does_not_hide_auth_schema_or_other_view_failures(self):
        for route in ('connections','activity','traces'):
            for failure in ('auth','schema','other-view'):
                with self.subTest(route=route,failure=failure):
                    api=ExampleApi();feed=Feed(api);feed.detail_enabled=False
                    feed.active_page='activity' if failure=='other-view' else 'agents'
                    original=api.get
                    def get(path):
                        if path.startswith('/api/'+route):
                            if failure=='auth':raise urllib.error.HTTPError(path,401,'denied',{},None)
                            if failure=='schema':return {route:'broken'}
                            raise TimeoutError('busy')
                        return original(path)
                    with patch.object(api,'get',side_effect=get):feed.refresh()
                    self.assertFalse(feed.snapshot.updated)
                    self.assertTrue(feed.snapshot.error)

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

    def test_session_timeout_publishes_live_agents_and_retries_without_poll_storm(self):
        api=ExampleApi();feed=Feed(api);feed.active_page='agents';feed.include_global_totals=False
        original=api.get
        attempts=[]
        def get(path):
            if path.startswith('/api/sessions'):
                attempts.append(path)
                raise TimeoutError('timed out')
            return original(path)
        with patch.object(api,'get',side_effect=get):
            feed.refresh()
            self.assertFalse(feed.snapshot.error)
            self.assertTrue(feed.snapshot.updated)
            self.assertEqual(feed.snapshot.local_status['sessions'],api.sessions)
            self.assertTrue(feed.snapshot.activity)
            self.assertIn('/api/sessions',feed.snapshot.sessions_error)
            self.assertEqual(feed.snapshot.sessions_updated,0)
            feed.session_id='session-1';feed.active_page='activity';feed.refresh()
            self.assertEqual(len(attempts),1)  # Global history is independent of navigation.
            feed.retry_now();feed.refresh()
            self.assertEqual(len(attempts),2)
        feed.sessions_attempted-=feed.EXTRA_POLL_SECONDS+1
        feed.refresh()
        self.assertFalse(feed.snapshot.sessions_error)
        self.assertGreater(feed.snapshot.sessions_updated,0)

    def test_session_history_retains_last_success_but_live_data_advances(self):
        api=ExampleApi();feed=Feed(api);feed.active_page='agents'
        original=api.get
        stored=[{'session_id':'stored','services':['codex'],'latest_task':'Earlier work'}]
        with patch.object(api,'get',side_effect=lambda path: {'sessions':stored}
                          if path.startswith('/api/sessions') else original(path)):
            feed.refresh()
        old=feed.snapshot
        api.sessions=[{**api.sessions[0],'task':'Current live work'}]
        def unavailable(path):
            if path.startswith('/api/sessions'):
                raise urllib.error.HTTPError(path,503,'busy',{},None)
            return original(path)
        feed.sessions_attempted-=feed.EXTRA_POLL_SECONDS+1
        with patch.object(api,'get',side_effect=unavailable):feed.refresh()
        self.assertFalse(feed.snapshot.error)
        self.assertIs(feed.snapshot.sessions,old.sessions)
        self.assertEqual(feed.snapshot.sessions_updated,old.sessions_updated)
        self.assertIn('503',feed.snapshot.sessions_error)
        self.assertEqual(feed.snapshot.local_status['sessions'][0]['task'],'Current live work')
        feed.retry_now();feed.refresh()
        self.assertFalse(feed.snapshot.sessions_error)
        self.assertEqual(feed.snapshot.sessions,[])

    def test_session_auth_and_schema_failures_are_not_hidden_as_partial_success(self):
        for failure in (401,403,400,'schema'):
            with self.subTest(failure=failure):
                api=ExampleApi();feed=Feed(api);feed.active_page='agents';original=api.get
                def get(path):
                    if path.startswith('/api/sessions'):
                        if failure=='schema':return {'sessions':'invalid'}
                        raise urllib.error.HTTPError(path,failure,'failed',{},None)
                    return original(path)
                with patch.object(api,'get',side_effect=get):feed.refresh()
                self.assertFalse(feed.snapshot.updated)
                self.assertTrue(feed.snapshot.error)
                if failure in (401,403):self.assertEqual(feed.connection.phase,'auth')

    def test_partial_history_still_obeys_scope_generation_guard(self):
        api=ExampleApi();feed=Feed(api);feed.active_page='agents';original=api.get
        def get(path):
            if path.startswith('/api/sessions'):
                feed.request_refresh()
                raise TimeoutError('timed out')
            return original(path)
        with patch.object(api,'get',side_effect=get):feed.refresh()
        self.assertFalse(feed.snapshot.updated)
        self.assertTrue(feed.wake.is_set())

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
        with patch('observatory.terminal.receiver_urlopen', return_value=response), \
                patch.object(Api, 'MAX_RESPONSE_BYTES', 32):
            with self.assertRaisesRegex(CockpitError, 'response exceeded'):
                Api('http://localhost:14318').get('/api/metrics')

    def test_small_json_response_is_accepted(self):
        response = MagicMock()
        response.__enter__.return_value = io.BytesIO(b'{"ok": true}')
        with patch('observatory.terminal.receiver_urlopen', return_value=response):
            self.assertEqual(Api('http://localhost:14318').get('/api/health'), {'ok': True})
