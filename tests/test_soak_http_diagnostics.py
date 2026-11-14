"""Failure diagnostics preserve HTTP failure and exclude captured content."""
import io
import json
from types import SimpleNamespace
import unittest
from urllib.error import HTTPError

from scripts.soak_monitor import observed_request


class SoakHTTPDiagnosticsTests(unittest.TestCase):
    def run_failure(self, body, *, path='/api/mcp?limit=20', phase=2, failures=None):
        stream=io.BytesIO(body)
        error=HTTPError('http://generated.invalid/private?token=PRIVATE',503,
            'PRIVATE error reason',{},stream)
        def fail(route):
            self.assertEqual(route,path)
            raise error
        latencies={};failures=[] if failures is None else failures
        with self.assertRaises(HTTPError)as raised:
            observed_request(SimpleNamespace(get=fail),path,latencies=latencies,
                failures=failures,phase_index=phase)
        self.assertIs(raised.exception,error)
        self.assertTrue(stream.closed)
        self.assertEqual(len(latencies[path]),1)
        self.assertGreaterEqual(latencies[path][0],0)
        self.assertNotIn('PRIVATE',json.dumps(failures))
        return failures

    def test_incoming_failure_is_classified_without_body_or_native_content(self):
        body=json.dumps(dict(error='PRIVATE',indexing=dict(ready=False,
            stage='Indexing incoming source changes',processed=200,error=None),
            local_agents=dict(sessions=[dict(prompt='PRIVATE',path='PRIVATE')]))).encode()
        row=self.run_failure(body)[0]
        self.assertEqual(row,dict(path='/api/mcp?limit=20',status=503,phase_index=2,
            response_metadata='parsed',indexing=dict(indexing_ready=False,
                indexing_error=False,indexing_error_kind=None,
                indexing_stage='Indexing incoming source changes',indexed_records=200)))

    def test_fatal_error_records_only_category_and_allowlisted_stage(self):
        body=json.dumps(dict(indexing=dict(ready=False,error='database is locked',
            stage='History migration failed; restart to resume'))).encode()
        row=self.run_failure(body)[0]['indexing']
        self.assertEqual(row['indexing_error_kind'],'sqlite_database_locked')
        self.assertEqual(row['indexing_stage'],'History migration failed; restart to resume')

    def test_untrusted_fields_do_not_become_metadata(self):
        body=json.dumps(dict(indexing=dict(ready='PRIVATE',error='PRIVATE',
            stage='PRIVATE',processed=True))).encode()
        row=self.run_failure(body,path='/PRIVATE?token=PRIVATE',phase=True)[0]
        self.assertIsNone(row['path']);self.assertIsNone(row['phase_index'])
        self.assertIsNone(row['indexing']['indexing_ready'])
        self.assertIsNone(row['indexing']['indexed_records'])
        self.assertIsNone(row['indexing']['indexing_stage'])
        self.assertEqual(row['indexing']['indexing_error_kind'],'reported_indexing_error')

    def test_malformed_and_nonobject_bodies_preserve_original_error(self):
        for body in (b'PRIVATE not json',b'["PRIVATE"]',b'null',b'"PRIVATE"',b'\xff'):
            with self.subTest(body_type=type(body).__name__):
                self.assertEqual(self.run_failure(body)[0]['response_metadata'],'unavailable')

    def test_oversized_body_is_not_parsed(self):
        self.assertEqual(self.run_failure(b'PRIVATE'+b'x'*65536)[0]['response_metadata'],'oversized')

    def test_failure_observations_are_bounded(self):
        failures=[dict(number=n)for n in range(8)]
        self.run_failure(b'{}',failures=failures)
        self.assertEqual(len(failures),8)
        self.assertEqual(failures[0],dict(number=1))

    def test_diagnostic_read_and_close_errors_cannot_replace_http_error(self):
        class BrokenStream:
            def read(self,size):raise RuntimeError('PRIVATE diagnostic read')
            def close(self):raise RuntimeError('PRIVATE diagnostic close')
        error=HTTPError('http://generated.invalid',503,'generated',{},BrokenStream())
        def fail(path):raise error
        failures=[]
        with self.assertRaises(HTTPError)as raised:
            observed_request(SimpleNamespace(get=fail),'/api/sessions',latencies={},
                failures=failures,phase_index=0)
        self.assertIs(raised.exception,error)
        self.assertEqual(failures[0]['response_metadata'],'unavailable')
        self.assertNotIn('PRIVATE',json.dumps(failures))

    def test_success_is_unchanged_and_other_errors_propagate(self):
        value=object();latencies={};failures=[]
        self.assertIs(observed_request(SimpleNamespace(get=lambda path:value),'/api/health',
            latencies=latencies,failures=failures,phase_index=0),value)
        error=OSError('generated transport error')
        def fail(path):raise error
        with self.assertRaises(OSError)as raised:
            observed_request(SimpleNamespace(get=fail),'/api/health',latencies=latencies,
                failures=failures,phase_index=0)
        self.assertIs(raised.exception,error)
        self.assertEqual(len(latencies['/api/health']),2)
        self.assertFalse(failures)


if __name__=='__main__':unittest.main()
