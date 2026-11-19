"""Native readers use a source-scoped host query for persisted usage."""
import tempfile
import unittest
from pathlib import Path

from observatory.core import Store, normalize_native


class NativePortTests(unittest.TestCase):
    def test_model_usage_is_exact_source_scoped_and_owned(self):
        with tempfile.TemporaryDirectory() as root:
            store = Store(Path(root) / 'port.sqlite')
            try:
                span = normalize_native({
                    'trace_id': 'a' * 32, 'span_id': 'b' * 16,
                    'name': 'model', 'start_unix_nano': 1,
                    'end_unix_nano': 2,
                    'attributes': {'observatory.native_usage': {'input_tokens': 9}},
                    'resource_attributes': {'observatory.source_id': 'owned-source'},
                }, 'claude-code')
                store.ingest([span])
                self.assertEqual(store.native_model_usage('owned-source', 'a' * 32, 'b' * 16),
                                 {'input_tokens': 9})
                self.assertEqual(store.native_model_usage('other-source', 'a' * 32, 'b' * 16), {})
                self.assertEqual(store.native_model_usage('owned-source', 'c' * 32, 'b' * 16), {})
            finally:
                store.close()
