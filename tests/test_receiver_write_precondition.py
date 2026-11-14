"""Native lifecycle writes cannot cross a receiver replacement after preflight."""
from pathlib import Path
from tempfile import TemporaryDirectory
import json
import threading
import unittest
from urllib.request import Request,urlopen
from urllib.error import HTTPError
from http.server import ThreadingHTTPServer
from unittest.mock import patch

from observatory.core import Store
from observatory.server import handler_for


class ReceiverWritePreconditionTests(unittest.TestCase):
    def setUp(self):
        self.tmp=TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.store=Store(Path(self.tmp.name)/'source.db');self.addCleanup(self.store.close)
        self.log=dict(trace_id=None,span_id=None,time_unix_nano=100,service='fixture',severity_number=9,
            severity_text='INFO',scope_name='fixture',event_name='agent.activity',body='',
            attributes={'agent.activity.domain':'agent','agent.activity.result':'observed'},resource_attributes={})

    def count(self):
        with self.store._connect() as db:return db.execute('SELECT COUNT(*) FROM logs').fetchone()[0]

    def test_exact_database_namespace_is_checked_before_policy_or_write(self):
        actual=self.store.receiver_id;self.store.receiver_id='stale-cache'
        for expected in ('stale-cache','foreign',True,'', 'x'*129):
            with patch.object(self.store,'_policy_records',side_effect=AssertionError('Policy mutation before precondition')):
                with self.assertRaises(ValueError):self.store.ingest_logs([self.log],expected_receiver=expected)
            self.assertEqual(self.count(),0)
        self.assertEqual(self.store.ingest_logs([self.log],expected_receiver=actual),1)
        self.assertEqual(self.count(),1)

    def test_database_namespace_change_after_preflight_rejects_entire_batch(self):
        expected=self.store.receiver_id
        with self.store._connect() as db:db.execute("UPDATE schema_meta SET value='replacement' WHERE key='receiver_id'")
        with self.assertRaisesRegex(ValueError,'namespace changed'):
            self.store.ingest_logs([self.log,{**self.log,'time_unix_nano':101}],expected_receiver=expected)
        self.assertEqual(self.count(),0)
        self.assertEqual(self.store.ingest_logs([self.log]),1)  # Existing clients unchanged.

    def test_http_header_precondition_and_health_clock(self):
        server=ThreadingHTTPServer(('127.0.0.1',0),handler_for(self.store,'secret'))
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        root='http://127.0.0.1:'+str(server.server_port)
        headers={'Authorization':'Bearer secret','Content-Type':'application/json'}
        payload={'resourceLogs':[{'resource':{'attributes':[{'key':'service.name','value':{'stringValue':'fixture'}}]},
            'scopeLogs':[{'scope':{'name':'fixture'},'logRecords':[{'timeUnixNano':'100','body':{'stringValue':'Recorded event'}}]}]}]}
        try:
            with urlopen(Request(root+'/api/health',headers=headers),timeout=3) as response:health=json.load(response)
            self.assertIs(type(health['observed_time_unix_nano']),int)
            with self.assertRaises(HTTPError) as denied:
                urlopen(Request(root+'/v1/logs',data=json.dumps(payload).encode(),headers={**headers,
                    'X-Agenturm-Expected-Receiver-ID':'foreign'}),timeout=3)
            self.assertEqual(denied.exception.code,400);self.assertEqual(self.count(),0)
            with urlopen(Request(root+'/v1/logs',data=json.dumps(payload).encode(),headers={**headers,
                    'X-Agenturm-Expected-Receiver-ID':health['receiver_id']}),timeout=3) as response:
                self.assertEqual(json.load(response)['accepted'],1)
            self.assertEqual(self.count(),1)
        finally:server.shutdown();server.server_close();thread.join(3)


if __name__=='__main__':unittest.main()
