"""A fatal successful health response is not a recoverable startup wait."""
from collections import deque
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading
import unittest
from unittest.mock import patch

from observatory.terminal import Api
from scripts.soak_monitor import ReadinessIndexError,wait_ready


def health(*,ready=False,initial=True,**indexing):
    return dict(indexing=dict(ready=ready,**indexing),local_agents=dict(initial_poll_complete=initial))


class Clock:
    def __init__(self):self.now=0.;self.sleeps=[]
    def monotonic(self):return self.now
    def sleep(self,seconds):self.sleeps.append(seconds);self.now+=seconds


class Responses:
    def __init__(self,*responses):self.responses=deque(responses);self.calls=0
    def get(self,path):
        assert path=='/api/health'
        self.calls+=1
        value=self.responses.popleft()if len(self.responses)>1 else self.responses[0]
        if isinstance(value,Exception):raise value
        return value


@contextmanager
def clocked():
    clock=Clock()
    with patch('scripts.soak_monitor.time.monotonic',clock.monotonic),\
            patch('scripts.soak_monitor.time.sleep',clock.sleep):yield clock


class SoakReadinessTests(unittest.TestCase):
    def test_first_fatal_poll_stops_without_sleep_and_preserves_failure_category(self):
        api=Responses(health(error='database is locked',processed=9600,stage='Indexing incoming source changes'))
        observations=[]
        with clocked()as clock:
            with self.assertRaises(ReadinessIndexError)as raised:
                wait_ready(api,timeout=600,observations=observations)
        self.assertEqual(api.calls,1);self.assertEqual(clock.sleeps,[])
        self.assertEqual(observations,[raised.exception.observation])
        self.assertEqual(observations[0]['status'],'indexing_error')
        self.assertEqual(observations[0]['indexing_error_kind'],'sqlite_database_locked')
        self.assertEqual(observations[0]['indexed_records'],9600)
        self.assertNotIn('deadline',str(raised.exception))

    def test_error_is_checked_before_ready_even_if_native_poll_is_incomplete(self):
        for ready,initial in [(False,False),(True,True)]:
            with self.subTest(ready=ready,initial=initial),clocked()as clock:
                with self.assertRaises(ReadinessIndexError):
                    wait_ready(Responses(health(ready=ready,initial=initial,error='worker failed')),600)
                self.assertEqual(clock.sleeps,[])

    def test_absent_or_empty_error_does_not_create_a_fatal_failure(self):
        for error in [None,'',False,0]:
            observations=[]
            with self.subTest(error=error),clocked():
                wait_ready(Responses(health(ready=True,error=error)),observations=observations)
            self.assertEqual(observations[0]['status'],'ready')
            self.assertFalse(observations[0]['indexing_error'])

    def test_ordinary_indexing_and_native_poll_keep_existing_cadence(self):
        done=health(ready=True)
        api=Responses(health(stage='Preparing indexed history'),health(ready=True,initial=False),done)
        observations=[]
        with clocked()as clock:result=wait_ready(api,observations=observations,phase='restart')
        self.assertIs(result,done);self.assertEqual(api.calls,3)
        self.assertEqual(clock.sleeps,[.2,.2])
        self.assertEqual(observations[0]['status'],'ready')
        self.assertEqual(observations[0]['phase'],'restart')
        self.assertFalse(observations[0]['indexing_error'])

    def test_transient_connection_errors_are_retried_without_recording_their_text(self):
        observations=[];api=Responses(OSError('PRIVATE_CONNECTION_PATH TOKEN_VALUE'),health(ready=True))
        with clocked()as clock:wait_ready(api,observations=observations)
        self.assertEqual(clock.sleeps,[.2]);self.assertEqual(api.calls,2)
        self.assertEqual(observations[0]['connection_errors'],1)
        self.assertNotIn('PRIVATE',json.dumps(observations))

    def test_timeout_does_not_reset_deadline_for_ordinary_pending_or_connection_errors(self):
        for response in [health(),health(ready=True,initial=False),OSError('PRIVATE_ERROR')]:
            observations=[]
            with self.subTest(response_type=type(response).__name__),clocked()as clock:
                with self.assertRaisesRegex(RuntimeError,'^Receiver readiness deadline exceeded$'):
                    wait_ready(Responses(response),timeout=.6,observations=observations)
                self.assertGreaterEqual(clock.now,.6);self.assertLess(clock.now,.8)
            self.assertEqual(observations[0]['status'],'timeout')
            self.assertNotIn('PRIVATE',json.dumps(observations))

    def test_malformed_truthy_readiness_flags_cannot_certify_ready(self):
        for invalid in ['true',1,['ready']]:
            for field in ['indexing','native']:
                response=health(ready=invalid)if field=='indexing'else health(ready=True,initial=invalid)
                observations=[]
                with self.subTest(field=field,value_type=type(invalid).__name__),clocked():
                    with self.assertRaisesRegex(RuntimeError,'^Receiver readiness deadline exceeded$'):
                        wait_ready(Responses(response),timeout=.4,observations=observations)
                self.assertEqual(observations[0]['status'],'timeout')
                key='indexing_ready'if field=='indexing'else 'native_initial_poll_complete'
                self.assertIsNone(observations[0][key])

    def test_timeout_metadata_is_only_the_last_successful_health_response(self):
        observations=[]
        api=Responses(health(stage='Preparing indexed history'),OSError('PRIVATE_CONNECTION_ERROR'))
        with clocked():
            with self.assertRaisesRegex(RuntimeError,'^Receiver readiness deadline exceeded$'):
                wait_ready(api,timeout=.6,observations=observations)
        self.assertEqual(observations[0]['health_metadata_basis'],'last_successful_health_response')
        self.assertEqual(observations[0]['indexing_stage'],'Preparing indexed history')
        self.assertGreater(observations[0]['connection_errors'],0)
        self.assertNotIn('PRIVATE',json.dumps(observations))

    def test_minimal_artifact_omits_untrusted_error_stage_and_all_captured_fields(self):
        for error in ['PRIVATE_ERROR /tmp/PRIVATE_PATH TOKEN_VALUE',{'password':'TOKEN_VALUE'}]:
            body=health(error=error,stage='PRIVATE_STAGE',processed=True)
            body.update(receiver_id='PRIVATE_RECEIVER',configuration={'token':'TOKEN_VALUE'})
            body['local_agents'].update(sessions=[dict(task='PRIVATE_PROMPT',path='/tmp/PRIVATE_PATH',token='TOKEN_VALUE')])
            observations=[]
            with self.subTest(error_type=type(error).__name__),clocked():
                with self.assertRaises(ReadinessIndexError)as raised:wait_ready(Responses(body),observations=observations)
            serialized=json.dumps(dict(readiness_observations=observations,error=str(raised.exception)),allow_nan=False)
            self.assertNotIn('PRIVATE',serialized);self.assertNotIn('TOKEN_VALUE',serialized)
            self.assertNotIn('sessions',serialized);self.assertNotIn('configuration',serialized)
            self.assertIsNone(observations[0]['indexed_records']);self.assertIsNone(observations[0]['indexing_stage'])
            self.assertEqual(observations[0]['indexing_error_kind'],'reported_indexing_error')

    def test_observation_history_is_bounded_and_phase_is_allowlisted(self):
        observations=[]
        with clocked():
            for _ in range(12):wait_ready(Responses(health(ready=True)),observations=observations,phase='PRIVATE_PHASE')
        self.assertEqual(len(observations),8)
        self.assertEqual({row['phase']for row in observations},{'readiness'})

    def test_real_http_200_fatal_health_does_not_start_another_poll(self):
        requests=[]
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                requests.append(self.path)
                body=json.dumps(health(error='database is locked')).encode()
                self.send_response(200);self.send_header('Content-Type','application/json')
                self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
            def log_message(self,*_):pass
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            observations=[]
            with self.assertRaises(ReadinessIndexError):
                wait_ready(Api('http://127.0.0.1:'+str(server.server_port),None),timeout=600,observations=observations)
            self.assertEqual(requests,['/api/health'])
            self.assertEqual(observations[0]['status'],'indexing_error')
        finally:
            server.shutdown();server.server_close();thread.join(timeout=2)
            self.assertFalse(thread.is_alive())


if __name__=='__main__':unittest.main()
