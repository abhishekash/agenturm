"""Cache hits must preserve freshness, authorization and bounded memory."""
import io
import json
import sqlite3
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store, normalize_native
from observatory.response_cache import ResponseCache
from observatory.server import handler_for


class ResponseCacheTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / 'cache.db'
        with sqlite3.connect(self.path) as db:
            db.execute('CREATE TABLE records(value)')
        self.cache = ResponseCache(self.path)
        self.addCleanup(self.cache.close)

    def test_external_commits_and_racing_writes_invalidate_in_both_journal_modes(self):
        for mode in ('delete', 'wal'):
            with self.subTest(mode=mode), sqlite3.connect(self.path) as db:
                db.execute('PRAGMA journal_mode=' + mode)
                version, _ = self.cache.get('key')
                self.cache.put('key', version, b'before')
                self.assertEqual(self.cache.get('key')[1], b'before')
                db.execute('INSERT INTO records VALUES(1)')
                db.commit()
                self.assertIsNone(self.cache.get('key')[1])
                self.cache.put('key', version, b'stale raced result')
                self.assertIsNone(self.cache.get('key')[1])

    def test_database_replacement_cannot_reuse_old_results_or_versions(self):
        version, _ = self.cache.get('key')
        self.cache.put('key', version, b'old')
        replacement = self.path.with_suffix('.new')
        with sqlite3.connect(replacement) as db:
            db.execute('CREATE TABLE replacement(value)')
        replacement.replace(self.path)
        self.assertIsNone(self.cache.get('key')[1])
        self.cache.put('key', version, b'old in-flight result')
        self.assertIsNone(self.cache.get('key')[1])

    def test_lru_byte_entry_and_individual_response_budgets(self):
        self.cache.MAX_BYTES = 8
        self.cache.MAX_ENTRY_BYTES = 6
        self.cache.MAX_ENTRIES = 2
        version, _ = self.cache.get('a')
        self.cache.put('a', version, b'1111')
        self.cache.put('b', version, b'2222')
        self.cache.get('a')
        self.cache.put('c', version, b'3333')
        self.assertIsNone(self.cache.get('b')[1])
        self.assertEqual(self.cache.get('a')[1], b'1111')
        self.cache.put('oversize', version, b'1234567')
        self.assertIsNone(self.cache.get('oversize')[1])
        self.cache.put('c', version, b'444444')
        self.assertLessEqual(self.cache.size, 8)
        self.assertIsNone(self.cache.get('a')[1])
        self.cache.close()
        self.assertEqual(self.cache.get('c'), (None, None))
        self.assertIsNone(self.cache.db)

    def test_time_and_external_state_dependent_routes_are_never_cached(self):
        for path in ('/api/health', '/api/connections', '/api/usage-series', '/api/hierarchy'):
            self.assertFalse(self.cache.eligible(path))


class CachedHttpTests(unittest.TestCase):
    def test_hits_skip_queries_but_not_auth_and_commits_refresh_results(self):
        with tempfile.TemporaryDirectory() as root:
            store = Store(Path(root) / 'api.db')
            self.addCleanup(store.close)
            handler = handler_for(store, token='test-token', quiet=True)
            self.addCleanup(handler.response_cache.close)

            class Socket:
                def __init__(self, raw):
                    self.input, self.output = io.BytesIO(raw), io.BytesIO()
                def makefile(self, mode, *args, **kwargs):
                    return self.input if 'r' in mode else self.output
                def sendall(self, data):
                    self.output.write(data)

            def get(path='/api/metrics', token='test-token'):
                sock = Socket(f'GET {path} HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n'.encode())
                handler(sock, ('127.0.0.1', 1), type('Server', (), {})())
                head, body = sock.output.getvalue().split(b'\r\n\r\n', 1)
                return int(head.split()[1]), json.loads(body)

            with patch.object(store, 'metrics', wraps=store.metrics) as metrics:
                first = get()
                self.assertEqual(first[0], 200)
                self.assertEqual(get(), first)
                self.assertEqual(metrics.call_count, 1)
                self.assertEqual(get(token='incorrect')[0], 401)
                self.assertEqual(metrics.call_count, 1)
                store.ingest([normalize_native({
                    'trace_id': 'a' * 32, 'span_id': 'b' * 16,
                    'name': 'new work', 'start_unix_nano': 1, 'end_unix_nano': 2,
                    'attributes': {},
                })])
                self.assertEqual(get()[1]['runs'], first[1]['runs'] + 1)
                self.assertEqual(metrics.call_count, 2)
                # Query strings form part of the key; differing scopes must
                # never accidentally receive a cached unfiltered response.
                self.assertEqual(get('/api/traces?service=missing')[1]['traces'], [])
                self.assertEqual(len(get('/api/traces')[1]['traces']), 1)
            handler.response_cache.close()
            store.close()
