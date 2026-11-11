"""Bind a complete read set to one receiver across database/process replacement."""
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from observatory.core import Store
from observatory.server import handler_for
from observatory.terminal import Api, CockpitError, Feed
from tests.test_terminal_search import ExampleApi


class Socket:
    def __init__(self, raw): self.input,self.output=io.BytesIO(raw),io.BytesIO()
    def makefile(self, mode, *args, **kwargs): return self.input if 'r' in mode else self.output
    def sendall(self, data): self.output.write(data)


class ReceiverResponseTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.path=Path(self.temp.name)/'current.db'
        self.store=Store(self.path);self.addCleanup(self.store.close)
        self.handler=handler_for(self.store,quiet=True);self.addCleanup(self.handler.response_cache.close)

    def request(self,path,token=None):
        auth = f'Authorization: Bearer {token}\r\n' if token is not None else ''
        sock=Socket(f'GET {path} HTTP/1.1\r\nHost: localhost\r\n{auth}\r\n'.encode())
        self.handler(sock,('127.0.0.1',1),type('Server',(),{})())
        head,body=sock.output.getvalue().split(b'\r\n\r\n',1)
        headers={line.split(b':',1)[0].decode().lower():line.split(b':',1)[1].strip().decode()
                 for line in head.split(b'\r\n')[1:]}
        return int(head.split()[1]),headers,json.loads(body)

    def replacement(self):
        path=self.path.with_name('replacement.db')
        replacement=Store(path);identity=replacement.receiver_id;replacement.close()
        return path,identity

    def test_every_data_response_and_cached_hit_carries_identity_without_probe_sql(self):
        for path in ('/api/health','/api/sessions','/api/connections','/api/traces','/api/activity','/api/metrics'):
            for _ in range(2):
                code,headers,body=self.request(path)
                self.assertEqual(code,200,path)
                self.assertEqual(headers['x-agenturm-receiver-id'],self.store.receiver_id,path)
        with patch.object(self.store,'receiver_namespace',side_effect=AssertionError('probe must avoid SQLite')):
            code,headers,body=self.request('/api/health?probe=1')
        self.assertEqual(code,200);self.assertNotIn('x-agenturm-receiver-id',headers)

    def test_unauthorized_requests_do_not_read_or_expose_receiver_identity(self):
        self.handler=handler_for(self.store,token='private-token',quiet=True)
        self.addCleanup(self.handler.response_cache.close)
        self.assertEqual(self.request('/api/metrics',token='private-token')[0],200)
        with patch.object(self.store,'receiver_namespace',side_effect=AssertionError('authenticate first')):
            code,headers,body=self.request('/api/metrics',token='wrong-token')
        self.assertEqual(code,401)
        self.assertNotIn('x-agenturm-receiver-id',headers)
        self.assertEqual(body,{'error':'unauthorized'})

    def test_atomic_replacement_inside_sessions_query_rejects_the_response(self):
        replacement,identity=self.replacement()
        original=self.store.sessions
        def replace_during_query(*args,**kwargs):
            rows=original(*args,**kwargs)
            replacement.replace(self.path)
            return rows
        with patch.object(self.store,'sessions',side_effect=replace_during_query):
            code,headers,body=self.request('/api/sessions')
        self.assertEqual(code,409)
        self.assertIn('identity changed',body['error'])
        code,headers,body=self.request('/api/sessions')
        self.assertEqual(code,200);self.assertEqual(headers['x-agenturm-receiver-id'],identity)

    def test_atomic_replacement_after_cache_hit_is_rejected_too(self):
        self.assertEqual(self.request('/api/metrics')[0],200)
        replacement,identity=self.replacement()
        original=self.handler.response_cache.get
        def replace_after_hit(key):
            version,data=original(key)
            self.assertIsNotNone(data)
            replacement.replace(self.path)
            return version,data
        with patch.object(self.handler.response_cache,'get',side_effect=replace_after_hit), patch.object(self.store,'metrics',side_effect=AssertionError('must be cache hit')):
            code,headers,body=self.request('/api/metrics')
        self.assertEqual(code,409)
        code,headers,body=self.request('/api/metrics')
        self.assertEqual(code,200);self.assertEqual(headers['x-agenturm-receiver-id'],identity)

    def test_api_consumes_header_and_refuses_a_conflicting_body(self):
        class Response(io.BytesIO):
            headers={'X-Agenturm-Receiver-ID':'receiver-A'}
        api=Api('http://localhost:1234')
        with patch('urllib.request.urlopen',return_value=Response(b'{"sessions":[]}')):
            self.assertEqual(api.get('/api/sessions')['receiver_id'],'receiver-A')
        with patch('urllib.request.urlopen',return_value=Response(b'{"receiver_id":"receiver-B"}')):
            with self.assertRaisesRegex(CockpitError,'identity'):api.get('/api/activity')


class MixedRefreshTests(unittest.TestCase):
    def test_prefetched_replacement_payloads_cannot_enter_an_old_namespace_cache(self):
        for changed_path in ('/api/sessions','/api/connections','/api/traces','/api/metrics'):
            with self.subTest(changed_path=changed_path):
                api=ExampleApi();original=api.get;changed=[False]
                def get(path):
                    value=original(path)
                    if isinstance(value,dict):
                        value={**value,'receiver_id':'receiver-B' if changed[0] and path.split('?')[0]==changed_path else 'receiver-A'}
                        if changed[0] and path.split('?')[0]==changed_path and 'sessions' in value:
                            value['sessions']=[{'session_id':'private-B','latest_task':'Receiver B only'}]
                    return value
                api.get=get
                feed=Feed(api);feed.active_page='agents';feed.detail_enabled=False
                feed.refresh();before=feed.snapshot;cache=list(feed.sessions_cache)
                self.assertFalse(before.error)
                changed[0]=True;feed.retry_now();feed.refresh()
                self.assertEqual(feed.snapshot.updated,before.updated)
                self.assertEqual(feed.snapshot.receiver_id,'receiver-A')
                self.assertIn('identity changed',feed.snapshot.error)
                self.assertEqual(feed.sessions_cache,cache)
                self.assertNotIn('private-B',str(feed.snapshot.sessions))
