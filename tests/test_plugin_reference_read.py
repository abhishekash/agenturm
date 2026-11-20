"""Host scope checks for trusted, provider-owned private Activity reads."""
from contextlib import contextmanager
import sqlite3
import unittest
from urllib.parse import parse_qs, urlparse

from observatory.plugin_reference_read import (
    ReferenceReadProvider, ReferenceReadRegistry, ReferenceText, _binding,
)
from observatory.ui_plugin_content import ContentPreviewProvider, ContentPreviewRegistry

EVENT_A = 'plugin-event:' + 'a' * 64
EVENT_B = 'plugin-event:' + 'b' * 64


class FakeStore:
    def __init__(self):
        self.db = sqlite3.connect(':memory:')
        self.db.executescript("""
            CREATE TABLE schema_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
            INSERT INTO schema_meta VALUES('receiver_id','receiver:a');
            CREATE TABLE activity_index_v1(event_key TEXT PRIMARY KEY,kind TEXT NOT NULL,owner_key TEXT NOT NULL);
            CREATE TABLE plugin_source_cursor_v1(source_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,
                lease_epoch INTEGER NOT NULL,policy_epoch INTEGER NOT NULL,dirty_seq INTEGER NOT NULL,
                capture_enabled INTEGER NOT NULL);
            CREATE TABLE plugin_projection_cursor_v1(source_id TEXT PRIMARY KEY,projected_seq INTEGER NOT NULL,
                policy_epoch INTEGER NOT NULL);
            CREATE TABLE plugin_reference_owner_v1(source_id TEXT NOT NULL,provider_id TEXT NOT NULL,
                PRIMARY KEY(source_id,provider_id));
            INSERT INTO activity_index_v1 VALUES('plugin-event:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','plugin','source:a');
            INSERT INTO activity_index_v1 VALUES('plugin-event:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','plugin','source:b');
            INSERT INTO plugin_source_cursor_v1 VALUES('source:a','receiver:a',5,2,9,1);
            INSERT INTO plugin_source_cursor_v1 VALUES('source:b','receiver:a',7,3,11,1);
            INSERT INTO plugin_projection_cursor_v1 VALUES('source:a',9,2);
            INSERT INTO plugin_projection_cursor_v1 VALUES('source:b',11,3);
            INSERT INTO plugin_reference_owner_v1 VALUES('source:a','trusted-private');
            INSERT INTO plugin_reference_owner_v1 VALUES('source:b','trusted-private');
        """)

    def receiver_namespace(self):
        return 'receiver:a'

    @contextmanager
    def _read(self):
        yield self.db


class ReferenceReadTests(unittest.TestCase):
    def setUp(self):
        self.store = FakeStore()
        self.addCleanup(self.store.db.close)

    def provider(self, *, mutate=None, alter=None):
        def read(store, event_id):
            before = _binding(store, 'trusted-private', event_id)
            if before is None:
                return None
            if mutate:
                mutate(store)
            values = alter(before) if alter else before
            return ReferenceText('trusted-private', values[0], values[1], event_id,
                                 values[2], values[3], values[4], 'Synthetic [REDACTED]')
        return ReferenceReadProvider('trusted-private', read)

    def test_current_exact_provider_owner_returns_only_bounded_safe_text(self):
        registry = ReferenceReadRegistry((self.provider(),))
        result = registry.read_exact(self.store, 'trusted-private', EVENT_A)
        self.assertEqual(result.text, 'Synthetic [REDACTED]')
        self.assertEqual((result.source_id, result.receiver_id, result.lease_epoch,
                          result.policy_epoch, result.dirty_seq),
                         ('source:a', 'receiver:a', 5, 2, 9))

    def test_unknown_provider_cross_source_and_cross_receiver_fail_closed(self):
        registry = ReferenceReadRegistry((self.provider(),))
        self.assertIsNone(registry.read_exact(self.store, 'missing-provider', EVENT_A))
        self.assertIsNone(registry.read_exact(self.store, 'trusted-private', 'plugin-event:' + 'c' * 64))
        wrong_source = ReferenceReadRegistry((self.provider(
            alter=lambda _: ('receiver:a', 'source:b', 5, 2, 9)),))
        self.assertIsNone(wrong_source.read_exact(self.store, 'trusted-private', EVENT_A))
        wrong_receiver = ReferenceReadRegistry((self.provider(
            alter=lambda b: ('receiver:other', *b[1:])),))
        self.assertIsNone(wrong_receiver.read_exact(self.store, 'trusted-private', EVENT_A))

    def test_revocation_or_policy_change_during_read_invalidates_result(self):
        def revoke(store):
            store.db.execute("DELETE FROM plugin_reference_owner_v1 WHERE source_id='source:a'")
            store.db.commit()
        registry = ReferenceReadRegistry((self.provider(mutate=revoke),))
        self.assertIsNone(registry.read_exact(self.store, 'trusted-private', EVENT_A))

        store = FakeStore()
        self.addCleanup(store.db.close)
        def revise(current):
            current.db.execute("UPDATE plugin_source_cursor_v1 SET policy_epoch=policy_epoch+1 "
                               "WHERE source_id='source:a'")
            current.db.execute("UPDATE plugin_projection_cursor_v1 SET policy_epoch=policy_epoch+1 "
                               "WHERE source_id='source:a'")
            current.db.commit()
        registry = ReferenceReadRegistry((self.provider(mutate=revise),))
        self.assertIsNone(registry.read_exact(store, 'trusted-private', EVENT_A))

    def test_inactive_and_unprojected_sources_are_withheld(self):
        self.store.db.execute("UPDATE plugin_source_cursor_v1 SET capture_enabled=0 WHERE source_id='source:a'")
        self.store.db.commit()
        registry = ReferenceReadRegistry((self.provider(),))
        self.assertIsNone(registry.read_exact(self.store, 'trusted-private', EVENT_A))
        self.store.db.execute("UPDATE plugin_source_cursor_v1 SET capture_enabled=1 WHERE source_id='source:a'")
        self.store.db.execute("UPDATE plugin_projection_cursor_v1 SET projected_seq=8 WHERE source_id='source:a'")
        self.store.db.commit()
        self.assertIsNone(registry.read_exact(self.store, 'trusted-private', EVENT_A))

    def test_content_budget_and_ui_registry_bind_generic_endpoint_to_provider(self):
        too_large = ReferenceReadRegistry((ReferenceReadProvider(
            'trusted-private', lambda store, event: ReferenceText(
                'trusted-private', 'receiver:a', 'source:a', event, 5, 2, 9, 'x' * 16385)),))
        self.assertIsNone(too_large.read_exact(self.store, 'trusted-private', EVENT_A))
        declaration = ContentPreviewProvider(
            'synthetic-private-preview', lambda row: True, '/api/plugin-reference-content',
            'Synthetic private text', 'Load exact preview', 'Clipped.',
            read_provider_id='trusted-private')
        provider = ContentPreviewRegistry((declaration,)).for_record(
            {'source': 'plugin', 'id': EVENT_A})
        parsed = urlparse(provider.path({'source': 'plugin', 'id': EVENT_A}))
        self.assertEqual(parsed.path, '/api/plugin-reference-content')
        self.assertEqual(parse_qs(parsed.query),
                         {'event_id': [EVENT_A], 'provider_id': ['trusted-private']})


if __name__ == '__main__':
    unittest.main()
