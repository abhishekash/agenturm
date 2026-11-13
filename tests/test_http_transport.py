"""Local polling must avoid TLS setup without weakening HTTPS or redirect rules."""
import threading
import ssl
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import unittest
from unittest.mock import patch
import urllib.request
import urllib.error
from observatory.http_transport import urlopen, HTTPSOnDemand


class ReceiverTransportTests(unittest.TestCase):
    def test_repeated_local_http_reads_do_not_initialize_https(self):
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_GET(self):
                self.send_response(200);self.end_headers();self.wfile.write(b'ok')
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        self.addCleanup(server.server_close);self.addCleanup(server.shutdown)
        with patch.object(urllib.request.HTTPSHandler,'__init__',side_effect=AssertionError('HTTP loaded TLS')):
            for _ in range(3):
                with urlopen(f'http://127.0.0.1:{server.server_port}/',timeout=2) as response:
                    self.assertEqual(response.read(),b'ok')

    def test_https_delegates_to_standard_verified_handler(self):
        handler=HTTPSOnDemand();parent=urllib.request.OpenerDirector();handler.add_parent(parent)
        request=urllib.request.Request('https://localhost/');marker=object()
        with patch.object(urllib.request.HTTPSHandler,'https_open',autospec=True,return_value=marker) as opened:
            self.assertIs(handler.https_open(request),marker)
            standard,actual=opened.call_args.args
            self.assertIs(type(standard),urllib.request.HTTPSHandler)
            self.assertIs(actual,request);self.assertIs(standard.parent,parent)
            context=getattr(standard,'_context',None)
            if context is not None:
                self.assertTrue(context.check_hostname)
                self.assertEqual(context.verify_mode,ssl.CERT_REQUIRED)

    def test_redirect_statuses_never_replay_authorization_or_post_body(self):
        received=[]
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_POST(self):
                if self.path=='/destination':received.append(self.headers.get('Authorization'))
                self.send_response(int(self.path.lstrip('/')) if self.path!='/destination' else 200)
                self.send_header('Location','/destination');self.end_headers()
            do_GET=do_POST
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        threading.Thread(target=server.serve_forever,daemon=True).start()
        self.addCleanup(server.server_close);self.addCleanup(server.shutdown)
        for code in (301,302,303,307,308):
            with self.subTest(code=code):
                request=urllib.request.Request(f'http://127.0.0.1:{server.server_port}/{code}',
                    data=b'synthetic payload',headers={'Authorization':'Bearer synthetic'})
                with self.assertRaises(urllib.error.HTTPError) as raised:
                    urlopen(request,timeout=2)
                self.assertEqual(raised.exception.code,code)
                raised.exception.close()
        self.assertEqual(received,[])
