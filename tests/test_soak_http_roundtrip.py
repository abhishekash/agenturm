"""Real loopback HTTP errors retain safe diagnostics without becoming success."""
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
import json
from types import SimpleNamespace
import threading
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

from observatory.terminal import Api
from scripts.soak_monitor import observed_request


class SoakHTTPRoundTripTests(unittest.TestCase):
    def round_trip(self,indexing):
        requests=[];errors=[];responses=[]
        body=json.dumps(dict(indexing=indexing,error='GENERATED_PRIVATE_ERROR',
            configuration={'token':'GENERATED_PRIVATE_TOKEN'},
            local_agents={'sessions':[{'prompt':'GENERATED_PRIVATE_PROMPT','path':'GENERATED_PRIVATE_PATH'}]})).encode()
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                requests.append(dict(path=self.path,authorized=self.headers.get('Authorization')=='Bearer generated-http-test'))
                self.send_response(503);self.send_header('Content-Type','application/json')
                self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
            def log_message(self,*_):pass
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        server.daemon_threads=False
        thread=threading.Thread(target=server.serve_forever,name='generated-soak-http-test');thread.start()
        try:
            api=Api('http://127.0.0.1:'+str(server.server_port),'generated-http-test')
            def get(path):
                try:return api.get(path)
                except HTTPError as exc:
                    errors.append(exc);responses.append(exc.fp);raise
            latencies={};failures=[]
            # Match the real harness's exclusion of inherited proxy routing.
            with patch('urllib.request.getproxies',return_value={}):
                with self.assertRaises(HTTPError)as raised:
                    observed_request(SimpleNamespace(get=get),'/api/mcp?limit=20',
                        latencies=latencies,failures=failures,phase_index=2)
            self.assertEqual(len(errors),1);self.assertIs(raised.exception,errors[0])
            self.assertEqual(raised.exception.code,503);self.assertTrue(responses[0].closed)
            self.assertEqual(requests,[dict(path='/api/mcp?limit=20',authorized=True)])
            self.assertEqual(len(latencies['/api/mcp?limit=20']),1)
            self.assertEqual(set(failures[0]),{'path','status','phase_index','response_metadata','indexing'})
            self.assertNotIn('GENERATED_PRIVATE',json.dumps(failures))
            return failures[0]
        finally:
            for error in errors:error.close()
            server.shutdown();server.server_close();thread.join(2)
            self.assertFalse(thread.is_alive())

    def test_real_incoming_http_503_body_is_safe_metadata_and_still_fails(self):
        row=self.round_trip(dict(ready=False,error=None,stage='Indexing incoming source changes',processed=200))
        self.assertEqual(row,dict(path='/api/mcp?limit=20',status=503,phase_index=2,
            response_metadata='parsed',indexing=dict(indexing_ready=False,indexing_error=False,
                indexing_error_kind=None,indexing_stage='Indexing incoming source changes',indexed_records=200)))

    def test_real_fatal_http_503_preserves_category_without_retaining_error_body(self):
        row=self.round_trip(dict(ready=False,error='database is locked',
            stage='History migration failed; restart to resume'))
        self.assertEqual(row['response_metadata'],'parsed')
        self.assertEqual(row['indexing'],dict(indexing_ready=False,indexing_error=True,
            indexing_error_kind='sqlite_database_locked',
            indexing_stage='History migration failed; restart to resume',indexed_records=None))


if __name__=='__main__':unittest.main()
