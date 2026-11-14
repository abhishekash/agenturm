"""Plain stdout, bounded reads and capture fences for scripted investigations."""
import argparse
from contextlib import redirect_stderr, redirect_stdout
from copy import deepcopy
import io
import json
import unittest
from urllib.parse import parse_qs, urlsplit
from unittest.mock import patch

from observatory.investigation_cli import (add_investigation_parser, collect_investigation,
    collect_source, run_investigation_command)


class Api:
    def __init__(self):
        self.health_calls = 0
        self.receiver = 'receiver'
        self.policy = {}
        self.change_after = None
        self.paths = []
        self.pages = []
        self.source = None

    def get(self, path):
        self.paths.append(path)
        if path == '/api/health':
            self.health_calls += 1
            if self.change_after and self.health_calls > 1:
                self.change_after(self)
            return dict(receiver_id=self.receiver, investigation_index_v1=True, local_agents=self.policy)
        if path.startswith('/api/investigations/source?'):
            return deepcopy(self.source)
        query = parse_qs(urlsplit(path).query)
        page = deepcopy(self.pages.pop(0))
        page.update(format='agenturm-investigation-v1', receiver_id='receiver',
                    kind=query['kind'][0], query=query['q'][0], match_mode='literal word phrase',
                    coverage='Retained observations only.')
        return page


def match(identity='event', **scope):
    reference = dict(receiver_id='receiver', event_id=identity, service='sdk', agent_id='worker',
                     session_id='session', trace_id='a' * 32, source='log', source_id='b' * 64)
    reference.update(scope)
    return dict(event_id=identity, text='pytest tests', result='error', source_reference=reference,
                source_path='/api/investigations/source?event_id=' + identity, term_complete=True)


class InvestigationCliTests(unittest.TestCase):
    def test_global_paging_and_exact_references_remain_machine_readable(self):
        api = Api()
        api.pages = [dict(matches=[match('one')], next_cursor='next'),
                     dict(matches=[match('two', service='claude', agent_id=None)], next_cursor=None)]
        value = collect_investigation(api, kind='command', query='pytest', pages=2)
        self.assertEqual([row['event_id'] for row in value['matches']], ['one', 'two'])
        self.assertEqual(value['pages_read'], 2)
        self.assertIsNone(value['matches'][1]['source_reference']['agent_id'])
        self.assertIn('cursor=next', api.paths[2])
        self.assertEqual(api.health_calls, 2)

    def test_receiver_replacement_and_policy_revocation_emit_no_partial_result(self):
        for mutate in (lambda api: setattr(api, 'receiver', 'replacement'),
                       lambda api: setattr(api, 'policy', {'capture_content': False}),
                       lambda api: setattr(api, 'policy', {'sessions': [dict(service='sdk', agent_id='worker',
                                                    session_id='session', capture_content=False)]})):
            api = Api()
            api.pages = [dict(matches=[match()], next_cursor=None)]
            api.change_after = mutate
            with self.assertRaises(ValueError):
                collect_investigation(api, kind='command', query='pytest')

    def test_duplicate_events_cursor_and_unsupported_receiver_fail_closed(self):
        api = Api()
        api.pages = [dict(matches=[match()], next_cursor='cursor'),
                     dict(matches=[match()], next_cursor=None)]
        with self.assertRaisesRegex(ValueError, 'repeated'):
            collect_investigation(api, kind='command', query='pytest', pages=2)
        api = Api()
        api.get = lambda path: dict(receiver_id='receiver')
        with self.assertRaisesRegex(ValueError, 'No broad search'):
            collect_investigation(api, kind='instruction', query='keep API')

    def test_exact_source_rechecks_each_owner_and_original_reference(self):
        api = Api()
        api.source = dict(receiver_id='receiver', event_id='event', source_reference=match()['source_reference'],
                          sources=[dict(type='log', id='b' * 64, scope=dict(service='sdk', agent_id='worker', session_id='session'), record={'output': 'Recorded'})])
        self.assertEqual(collect_source(api, 'event')['sources'][0]['record']['output'], 'Recorded')
        api.change_after = lambda api: setattr(api, 'policy', {'sessions': [dict(service='sdk', agent_id='worker',
                                                               session_id='session', capture_content=False)]})
        with self.assertRaises(ValueError):
            collect_source(api, 'event')

    def test_unknown_returned_source_does_not_replace_original_policy(self):
        api = Api()
        api.policy = {'sessions': [dict(service='sdk', agent_id='worker', session_id='session', capture_content=False)]}
        api.source = dict(receiver_id='receiver', event_id='event', source_reference=match()['source_reference'],
            sources=[dict(type='log', id='b' * 64, scope={}, record={'output': 'PRIVATE'})])
        api.change_after = lambda api: setattr(api, 'policy', {})
        with self.assertRaisesRegex(ValueError, 'forbids'):
            collect_source(api, 'event')
        api.policy = {}
        api.source['sources'][0]['id'] = 'foreign'
        with self.assertRaisesRegex(ValueError, 'differs'):
            collect_source(api, 'event')

    def test_stdout_stderr_and_exit_status_without_tty_or_color(self):
        parser = argparse.ArgumentParser()
        add_investigation_parser(parser.add_subparsers(dest='command'))
        args = parser.parse_args(['investigate', 'command', 'pytest', '--format', 'json'])
        api = Api()
        api.pages = [dict(matches=[match()], next_cursor=None)]
        out, err = io.StringIO(), io.StringIO()
        with patch('observatory.terminal.Api', return_value=api), patch.dict('os.environ', {'NO_COLOR': '1'}), redirect_stdout(out), redirect_stderr(err):
            status = run_investigation_command(args)
        self.assertEqual(status, 0)
        self.assertEqual(err.getvalue(), '')
        self.assertEqual(json.loads(out.getvalue())['matches'][0]['event_id'], 'event')
        self.assertNotIn('\x1b', out.getvalue())
        api = Api()
        api.get = lambda path: dict(receiver_id='receiver')
        out, err = io.StringIO(), io.StringIO()
        with patch('observatory.terminal.Api', return_value=api), redirect_stdout(out), redirect_stderr(err):
            status = run_investigation_command(args)
        self.assertEqual(status, 2)
        self.assertEqual(out.getvalue(), '')
        self.assertEqual(json.loads(err.getvalue())['error']['code'], 'investigation_unavailable')


if __name__ == '__main__':
    unittest.main()
