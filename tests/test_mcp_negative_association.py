"""Absence of execution records is a snapshot fact, never association evidence."""
from contextlib import contextmanager
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from types import MethodType
import unittest
from unittest.mock import patch

from observatory.core import Store, _agent_id_from_attributes
from tests.fixtures.mcp_page_before import mcp_page as before
from tests.test_mcp_source_ownership import record


class MCPNegativeAssociationTests(unittest.TestCase):
    def setUp(self):
        self.temp = TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.store = Store(Path(self.temp.name) / 'fixture.db')
        self.addCleanup(self.store.close)
        self.call = record(1, 'parent', 'one', tool=True)
        self.store.ingest([self.call])
        self.agent = _agent_id_from_attributes(self.call['attributes'], {}, 'svc')

    def old_page(self, **options):
        with patch.object(self.store, '_mcp_page', MethodType(before, self.store)):
            return self.store.mcp_page(**options)

    def execution(self, kind='MCPTool'):
        event = record(2, 'parent', 'one', execution=True, parent=self.call['span_id'])
        event['attributes']['gen_ai.tool.type'] = kind
        return event

    def test_absent_execution_keeps_complete_frozen_output_for_filters_and_pages(self):
        self.store.ingest([record(3, 'parent', 'one', tool=True, output=False),
                           record(4, 'other', 'two', tool=True, output={'isError': True})])
        scopes = ({}, {'service': 'svc'}, {'session_id': 'one'}, {'query': 'fixture'},
                  {'query': 'missing'}, {'agent_id': self.agent},
                  {'agent_id': self.agent, 'subtree': True}, {'include_reviews': False})
        for scope in scopes:
            cursor = None
            while True:
                options = dict(scope, limit=1, cursor=cursor)
                expected = self.old_page(**options)
                with patch('observatory.storage_mcp.execution_match_sql',
                           side_effect=AssertionError('No execution can match')):
                    observed = self.store.mcp_page(**options)
                self.assertEqual(observed, expected, options)
                cursor = observed['next_cursor']
                if not cursor:
                    break

    def test_exact_recorded_type_controls_shortcut_without_changing_result(self):
        for kind in (None, True, 7, [], {'type': 'mcptool'}, 'shell', 'MCPTool ',
                     'MCPTool', 'mcptool', 'McPtOoL'):
            with self.subTest(kind=kind):
                self.store.ingest([self.execution(kind)])
                observed = self.store.mcp_page()
                self.assertEqual(observed, self.old_page())
                associated = isinstance(kind, str) and kind.lower() == 'mcptool'
                self.assertEqual(observed['summary']['errors'], int(associated))
                self.assertEqual('execution_source' in observed['calls'][0], associated)

    def test_foreign_execution_only_disables_shortcut_and_never_associates(self):
        execution = self.execution()
        execution['trace_id'] = 'b' * 32
        self.store.ingest([execution])
        observed = self.store.mcp_page()
        self.assertEqual(observed, self.old_page())
        self.assertEqual(observed['summary']['errors'], 0)
        self.assertNotIn('execution_source', observed['calls'][0])

    def interleaved_page(self, mutate):
        original = self.store._connect
        changes = []
        class Cursor:
            def __init__(self, cursor):
                self.cursor = cursor
            def fetchone(inner):
                value = inner.cursor.fetchone()
                if not changes:
                    with original() as writer:
                        mutate(writer)
                    changes.append(True)
                return value
        class Connection:
            def __init__(self, db):
                self.db = db
            def execute(inner, sql, args=()):
                cursor = inner.db.execute(sql, args)
                if sql.startswith('SELECT 1 FROM spans'):
                    return Cursor(cursor)
                return cursor
        @contextmanager
        def interleaved():
            with original() as db:
                yield Connection(db)
        with patch.object(self.store, '_connect', interleaved):
            observed = self.store.mcp_page()
        self.assertEqual(changes, [True])
        return observed

    def test_execution_insert_after_negative_probe_is_visible_only_next_request(self):
        expected = self.old_page()
        # Insert a complete normalized row through the fixture store first,
        # remove it, then restore it in an external connection during the read.
        self.store.ingest([self.execution()])
        with self.store._connect() as db:
            columns = [row[1] for row in db.execute('PRAGMA table_info(spans)')]
            row = db.execute('SELECT * FROM spans WHERE span_id=?', (f'{2:016x}',)).fetchone()
            values = tuple(row[name] for name in columns)
            db.execute('DELETE FROM spans WHERE span_id=?', (f'{2:016x}',))
        def insert(db):
            db.execute('INSERT INTO spans (' + ','.join(columns) + ') VALUES (' +
                       ','.join('?' for _ in columns) + ')', values)
        self.assertEqual(self.interleaved_page(insert), expected)
        current = self.store.mcp_page()
        self.assertEqual(current, self.old_page())
        self.assertEqual(current['summary']['errors'], 1)
        self.assertIn('execution_source', current['calls'][0])

    def test_execution_delete_after_positive_probe_keeps_current_snapshot_association(self):
        self.store.ingest([self.execution()])
        expected = self.old_page()
        observed = self.interleaved_page(lambda db: db.execute(
            'DELETE FROM spans WHERE span_id=?', (f'{2:016x}',)))
        self.assertEqual(observed, expected)
        current = self.store.mcp_page()
        self.assertEqual(current, self.old_page())
        self.assertEqual(current['summary']['errors'], 0)
        self.assertNotIn('execution_source', current['calls'][0])


if __name__ == '__main__':
    unittest.main()
