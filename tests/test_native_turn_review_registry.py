"""Bundled native turn reviewers must have exact, unambiguous ownership."""
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from observatory.native_turn_review import NativeTurnReviewSpec, review_turn


class NativeTurnReviewRegistryTests(unittest.TestCase):
    def test_bundled_contribution_runs_without_store_dispatch_edit(self):
        spec = NativeTurnReviewSpec("new-harness", lambda db, trace: {
            "trace_id": trace, "source": db})
        module = SimpleNamespace(turn_review_spec=lambda: spec)
        with patch("observatory.native_turn_review.bundled_adapter_modules",
                   return_value=(module,)):
            self.assertEqual(review_turn("read-only-db", "a" * 32),
                             {"trace_id": "a" * 32, "source": "read-only-db"})

    def test_duplicate_registration_and_claims_fail_closed(self):
        first = SimpleNamespace(turn_review_spec=lambda: NativeTurnReviewSpec(
            "one", lambda _db, _trace: {"source": "one"}))
        duplicate = SimpleNamespace(turn_review_spec=lambda: NativeTurnReviewSpec(
            "one", lambda _db, _trace: None))
        second = SimpleNamespace(turn_review_spec=lambda: NativeTurnReviewSpec(
            "two", lambda _db, _trace: {"source": "two"}))
        with patch("observatory.native_turn_review.bundled_adapter_modules",
                   return_value=(first, duplicate)):
            with self.assertRaisesRegex(ValueError, "duplicate"):
                review_turn(None, "a" * 32)
        with patch("observatory.native_turn_review.bundled_adapter_modules",
                   return_value=(first, second)):
            with self.assertRaisesRegex(ValueError, "multiple"):
                review_turn(None, "a" * 32)


if __name__ == "__main__":
    unittest.main()
