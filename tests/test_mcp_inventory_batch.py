"""One scoped call grouping feeds full inventory and independently filtered pages."""
from contextlib import contextmanager
from pathlib import Path
from tempfile import TemporaryDirectory
from types import MethodType
import unittest
from unittest.mock import patch

from observatory.core import Store, _agent_id_from_attributes
from tests.fixtures.mcp_page_before import mcp_page as before
from tests.test_mcp_source_ownership import record, TRACE


class MCPInventoryBatchTests(unittest.TestCase):
    def setUp(self):
        self.temp = TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.store = Store(Path(self.temp.name) / 'fixture.db')
        self.addCleanup(self.store.close)
        spans = [record(1, 'parent', 'one'), record(2, 'child', 'two')]
        spans[1]['attributes']['agent.parent_id'] = 'parent'
        for index in range(3, 12):
            value = record(index, 'child' if index % 3 == 0 else 'parent',
                           'two' if index % 3 == 0 else 'one', tool=True, output=index)
            value['attributes']['mcp.server.name'] = 'alpha' if index % 2 else 'beta'
            value['end_unix_nano'] = 200  # Equal stamps exercise the complete key tie breaker.
            spans.append(value)
        spans += [record(20, None, None, tool=True), record(21, 'review', 'review', tool=True, service='codex-review')]
        self.store.ingest(spans)
        self.parent = _agent_id_from_attributes(spans[0]['attributes'], {}, 'svc')
        self.store.ingest_logs([self.log(201, 'parent', 'one', False),
                                self.log(202, 'child', 'two', {'isError': True}),
                                self.log(203, None, None, None)])
        self.store.ingest_metrics([dict(time_unix_nano=300, start_unix_nano=100, service='cursor',
            name='cursor.tool.calls', description='', unit='{call}', kind='sum', temporality=1,
            attributes={'cursor.tool.kind': 'mcp', 'cursor.tool.name': 'search',
                        'cursor.mcp.server.name': 'metric-only', 'cursor.tool.status': 'success'},
            resource_attributes={}, scope_name='fixture', data={'value': 7, 'isMonotonic': True})])

    def log(self, stamp, owner, session, result, *, span=None):
        attrs = {'agent.runtime': 'SDK', 'agent.namespace': 'svc', 'mcp.server.name': 'alpha',
                 'mcp.method.name': 'tools/call', 'mcp.tool.name': 'search',
                 'observatory.mcp.call_id': 'same-provider-id', 'output.value': result}
        if owner is not None:
            attrs['agent.id'] = owner
        if session is not None:
            attrs['session.id'] = session
        return dict(trace_id=TRACE, span_id=span, time_unix_nano=stamp, service='svc',
            severity_number=9, severity_text='INFO', event_name='agent.activity', body='',
            attributes=attrs, resource_attributes={}, scope_name='fixture')

    def old_page(self, **kwargs):
        with patch.object(self.store, '_mcp_page', MethodType(before, self.store)):
            return self.store.mcp_page(**kwargs)

    def test_full_frozen_method_equivalence_across_filters_and_all_cursor_pages(self):
        scopes = ({}, {'service': 'svc'}, {'session_id': 'one'}, {'include_reviews': False},
                  {'query': 'alpha'}, {'query': 'absent'}, {'agent_id': self.parent},
                  {'agent_id': self.parent, 'subtree': True},
                  {'agent_id': self.parent, 'subtree': True, 'session_id': 'one', 'query': 'search'})
        for scope in scopes:
            cursor = None
            while True:
                options = dict(scope, limit=2, cursor=cursor)
                current = self.store.mcp_page(**options)
                self.assertEqual(current, self.old_page(**options), options)
                cursor = current['next_cursor']
                if not cursor:
                    break

    def test_inventory_ignores_search_cursor_and_does_not_fabricate_metric_events(self):
        original = self.store.mcp_page(limit=1)
        self.assertEqual(original['summary']['metric_calls'], 7)
        self.assertNotIn('metric-only', {row['server'] for row in original['calls']})
        second = self.store.mcp_page(limit=1, cursor=original['next_cursor'], query='alpha')
        missing = self.store.mcp_page(query='absent')
        self.assertEqual(original['summary'], second['summary'])
        self.assertEqual(original['summary'], missing['summary'])
        self.assertEqual(missing['calls'], [])
        self.assertEqual(self.store.mcp_page(session_id='one')['summary']['metric_calls'], 0)

    def test_one_group_materialization_and_existing_execution_trace_indexes(self):
        statements = []
        original = self.store._connect
        @contextmanager
        def traced():
            with original() as db:
                db.set_trace_callback(statements.append)
                yield db
        with patch.object(self.store, '_connect', traced):
            self.store.mcp_page(limit=2)
        grouping = [sql for sql in statements if sql.startswith('WITH evidence')]
        self.assertEqual(len(grouping), 1)
        with original() as db:
            plan = [row[3] for row in db.execute('EXPLAIN QUERY PLAN ' + grouping[0])]
        self.assertEqual(sum(row == 'MATERIALIZE calls' for row in plan), 1, plan)
        execution = [row for row in plan if 'SEARCH execution ' in row or 'SEARCH candidate_execution ' in row]
        self.assertTrue(execution, plan)
        self.assertTrue(all('trace_id=?' in row for row in execution), execution)

    def test_scope_inventory_candidates_and_hydration_share_snapshot_with_external_writer(self):
        expected = self.old_page(limit=100)
        original = self.store._connect
        fired = []
        @contextmanager
        def interleaved():
            with original() as db:
                def callback(sql):
                    if not sql.startswith('SELECT *,') or fired:
                        return
                    fired.append(True)
                    with original() as writer:
                        writer.execute("UPDATE spans SET attributes=json_set(attributes,'$.\"mcp.server.name\"','foreign')")
                        writer.execute('DELETE FROM logs')
                db.set_trace_callback(callback)
                yield db
        with patch.object(self.store, '_connect', interleaved):
            observed = self.store.mcp_page(limit=100)
        self.assertTrue(fired)
        self.assertEqual(observed, expected)
        self.assertNotEqual(self.store.mcp_page(limit=100), expected)

    def test_existing_source_replacement_visible_without_cache(self):
        initial = self.store.mcp_page(service='svc', session_id='one')
        updated = record(3, 'parent', 'one', tool=True, output={'api_key': 'PRIVATE_ONLY', 'count': 2})
        self.store.ingest([updated])
        current = self.store.mcp_page(service='svc', session_id='one')
        self.assertEqual(current, self.old_page(service='svc', session_id='one'))
        self.assertNotEqual(initial, current)
        # Store has existing capture/redaction rules; the shared grouping creates
        # no second payload representation or retained cross-request identity.
        self.assertEqual(next(row for row in current['calls'] if row['span_id'] == updated['span_id'])['output']['count'], 2)


if __name__ == '__main__':
    unittest.main()
