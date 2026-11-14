"""Typed retained-history searches never infer ownership or execute commands."""
import json
import sqlite3
import unittest
from urllib.parse import parse_qs, urlsplit

from observatory.investigation import decode_reference, prepare_terms, validate_query
from observatory.recorded_paths import path_facts
from observatory.storage_investigation import initialize, read_page, replace_owner


def event(identity, **changes):
    row = dict(id=identity, source='log', source_id=identity, trace_id='a' * 32,
        span_id=None, turn_id=None, service='codex', agent_id='worker', session_id='session',
        time_unix_nano=10, domain='file', action='edit', subject='src/retry.py',
        result='success', detail='')
    row.update(changes)
    return row


class InvestigationTests(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(':memory:')
        self.db.row_factory = sqlite3.Row
        self.addCleanup(self.db.close)
        self.db.executescript('''CREATE TABLE schema_meta(key TEXT PRIMARY KEY,value TEXT);
            CREATE TABLE activity_index_v1(event_key TEXT PRIMARY KEY,kind TEXT,owner_key TEXT,
                stamp INTEGER,trace_id TEXT,session_id TEXT,agent_id TEXT,service TEXT,domain TEXT,
                action TEXT,result TEXT,turn_id TEXT,payload TEXT);''')
        self.assertTrue(initialize(self.db))

    def put(self, row):
        key = row['id']
        self.db.execute('INSERT OR REPLACE INTO activity_index_v1 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)',
            (key, 'log', key, row['time_unix_nano'], row['trace_id'], row['session_id'], row['agent_id'],
             row['service'], row['domain'], row['action'], row['result'], row['turn_id'], json.dumps(row)))
        replace_owner(self.db, 'log', key, [(key, row)])

    def page(self, kind, query, **kwargs):
        return read_page(self.db, kind, query, receiver='receiver', **kwargs)

    def test_path_exact_case_unicode_and_explicit_rename_pairs(self):
        self.put(event('one'))
        self.put(event('two', action='rename', subject='src/新しい.py',
            detail=json.dumps({'old_path': 'src/古い.py', 'new_path': 'src/新しい.py'})))
        self.assertEqual(len(self.page('path', 'src/retry.py')['matches']), 1)
        self.assertEqual(self.page('path', 'SRC/retry.py')['matches'], [])
        self.assertEqual(len(self.page('path', 'src/古い.py')['matches']), 1)
        self.assertEqual(len(self.page('path', 'src/新しい.py')['matches']), 1)
        facts = path_facts(event('arrow', action='rename', subject='literal -> name.py'))
        self.assertEqual(facts['paths'], [{'path': 'literal -> name.py', 'role': 'recorded'}])
        self.assertFalse(facts['complete'])
        self.put(event('read', action='read'))
        self.assertEqual(len(self.page('path', 'src/retry.py')['matches']), 1)

    def test_qualified_command_only_and_failed_filter(self):
        row = event('command', domain='shell', subject='Shell command', action='execute', result='error',
            operation_evidence=dict(version=1, commands=[('process.command_line', 'pytest tests/retry.py')],
                inputs=[], outputs=[('stderr', 'stderr', 'Assertion failed')], exits=[('process.exit.code', 1)],
                cwd='/work', tool='Bash'))
        self.put(row)
        self.put(event('label', domain='shell', subject='pytest tests/retry.py', result='error'))
        self.put(event('stdout', domain='tool', detail='{"command":"pytest tests"}'))
        rows = self.page('command', 'pytest tests', failed=True)['matches']
        self.assertEqual([item['event_id'] for item in rows], ['command'])
        self.assertEqual(rows[0]['source_reference']['agent_id'], 'worker')
        navigation = parse_qs(urlsplit(rows[0]['source_path']).query)
        self.assertEqual(navigation['receiver_id'], ['receiver'])
        self.assertEqual(decode_reference(navigation['reference'][0]), rows[0]['source_reference'])
        self.put({**row, 'id': 'success', 'source_id': 'success', 'result': 'success'})
        self.assertEqual(len(self.page('command', 'pytest')['matches']), 2)
        self.assertEqual(len(self.page('command', 'pytest', failed=True)['matches']), 1)

    def test_instruction_role_bookkeeping_redaction_and_literal_fts(self):
        self.put(event('request', domain='message', action='user', detail='Keep the public API stable. Use résumé fixtures.'))
        self.put(event('assistant', domain='message', action='assistant', detail='Keep the public API stable.'))
        self.put(event('control', domain='message', action='user', detail='<command-name>/model</command-name>'))
        self.put(event('secret', domain='message', action='user', detail='{"password":"PRIVATE_VALUE"}'))
        self.assertEqual(len(self.page('instruction', 'public API')['matches']), 1)
        self.assertEqual(len(self.page('instruction', 'résumé')['matches']), 1)
        self.assertEqual(self.page('instruction', 'resume')['matches'], [])
        self.assertEqual(self.page('instruction', 'PRIVATE_VALUE')['matches'], [])
        self.assertEqual(self.page('instruction', 'API" OR "fixtures')['matches'], [])
        all_terms = str(self.db.execute('SELECT text FROM investigation_terms_v1').fetchall())
        self.assertNotIn('PRIVATE_VALUE', all_terms)

    def test_keyset_scope_namespace_retention_and_equal_timestamps(self):
        for identity, owner, producer in [('a', 'worker', 'codex'), ('b', 'other', 'claude'), ('c', None, 'sdk')]:
            self.put(event(identity, agent_id=owner, service=producer))
        first = self.page('path', 'src/retry.py', limit=1)
        self.assertEqual(first['matches'][0]['event_id'], 'c')
        self.put(event('new', time_unix_nano=11))
        second = self.page('path', 'src/retry.py', limit=1, cursor=first['next_cursor'])
        self.assertEqual(second['matches'][0]['event_id'], 'b')
        self.assertEqual(len(self.page('path', 'src/retry.py', agent_ids=['worker'])['matches']), 2)
        for kwargs in (dict(service='codex'), dict(agent_ids=['worker']), dict(receiver='other')):
            with self.assertRaisesRegex(ValueError, 'cursor'):
                read_page(self.db, 'path', 'src/retry.py', cursor=first['next_cursor'], **kwargs)
        self.db.execute('DELETE FROM activity_index_v1 WHERE event_key=?', ('b',))
        self.assertFalse(self.db.execute('SELECT 1 FROM investigation_terms_v1 WHERE event_key=?', ('b',)).fetchone())
        self.assertEqual(self.page('path', 'src/retry.py', service='claude')['matches'], [])

    def test_updates_revocation_and_bounds(self):
        self.put(event('a', domain='message', action='user', detail='Follow the synthetic long instruction ' + 'x' * 5000))
        item = self.page('instruction', 'synthetic long instruction')['matches'][0]
        self.assertLessEqual(len(item['text'].encode()), 4096)
        self.assertFalse(item['term_complete'])
        self.put(event('a', domain='message', action='user', detail='Changed direction'))
        self.assertEqual(self.page('instruction', 'synthetic long instruction')['matches'], [])
        self.put(event('a', domain='message', action='user', detail='Changed direction', capture_content=False))
        self.assertEqual(self.page('instruction', 'Changed direction')['matches'], [])
        for kind, value in [('path', ''), ('command', '"'), ('instruction', 'x\nY'), ('path', 'x' * 4097)]:
            with self.assertRaises(ValueError):
                validate_query(kind, value)
        self.db.execute('INSERT INTO schema_meta VALUES(?,?)', ('investigation_terms_v1', '1'))
        self.assertFalse(initialize(self.db))

    def test_queries_use_search_indexes_without_raw_json_hydration(self):
        self.put(event('one'))
        statements = []
        self.db.set_trace_callback(statements.append)
        self.page('path', 'src/retry.py')
        self.assertFalse(any('FROM spans' in sql or 'FROM logs' in sql for sql in statements))
        plan = self.db.execute('EXPLAIN QUERY PLAN SELECT event_key FROM investigation_terms_v1 WHERE kind=? AND text=?',
                               ('path', 'src/retry.py')).fetchall()
        self.assertTrue(any('investigation_exact_v1' in row[3] for row in plan))

    def test_scalar_ownership_overrides_conflicting_legacy_payload(self):
        self.put(event('one'))
        self.db.execute("UPDATE activity_index_v1 SET agent_id=NULL,service='sdk',session_id=NULL,result='unknown'")
        global_row = self.page('path', 'src/retry.py')['matches'][0]
        self.assertIsNone(global_row['source_reference']['agent_id'])
        self.assertIsNone(global_row['source_reference']['session_id'])
        self.assertEqual(global_row['source_reference']['service'], 'sdk')
        self.assertEqual(global_row['result'], 'unknown')
        self.assertEqual(self.page('path', 'src/retry.py', agent_ids=['worker'])['matches'], [])

    def test_explicit_move_and_conflicting_rename_fields_remain_observations(self):
        row = event('move', action='move', subject='new.py', detail='{"from":"old.py","to":"new.py"}')
        self.put(row)
        self.assertEqual(len(self.page('path', 'old.py')['matches']), 1)
        facts = path_facts({**row, 'from_path': 'different.py'})
        self.assertFalse(facts['complete'])
        self.assertIn('Recorded rename fields conflict', facts['reasons'])

    def test_missing_fts_has_no_history_scan_fallback(self):
        class Unavailable:
            def execute(self, sql):
                raise sqlite3.OperationalError('no such module: fts5')
        with self.assertRaisesRegex(ValueError, 'no history-scan fallback'):
            initialize(Unavailable())


if __name__ == '__main__':
    unittest.main()
