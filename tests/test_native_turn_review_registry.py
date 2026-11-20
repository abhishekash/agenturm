"""Bundled native turn reviewers must have exact, unambiguous ownership."""
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from observatory.native_turn_review import NativeTurnReviewSpec, review_turn


class NativeTurnReviewRegistryTests(unittest.TestCase):
    def test_bundled_contribution_runs_without_store_dispatch_edit(self):
        spec = NativeTurnReviewSpec("new-harness", lambda db, trace: {
            "trace_id": trace, "turn_id": "turn-one",
            "scope_owner": {"producer": "new-harness"}, "source": db})
        module = SimpleNamespace(turn_review_spec=lambda: spec)
        with patch("observatory.native_turn_review.bundled_adapter_modules",
                   return_value=(module,)):
            self.assertEqual(review_turn("read-only-db", "a" * 32),
                             {"trace_id": "a" * 32, "turn_id": "turn-one",
                              "scope_owner": {"producer": "new-harness"},
                              "source": "read-only-db"})

    def test_duplicate_registration_and_claims_fail_closed(self):
        first = SimpleNamespace(turn_review_spec=lambda: NativeTurnReviewSpec(
            "one", lambda _db, trace: {"trace_id": trace, "turn_id": "turn-one",
                                        "scope_owner": {"producer": "one"}}))
        duplicate = SimpleNamespace(turn_review_spec=lambda: NativeTurnReviewSpec(
            "one", lambda _db, _trace: None))
        second = SimpleNamespace(turn_review_spec=lambda: NativeTurnReviewSpec(
            "two", lambda _db, trace: {"trace_id": trace, "turn_id": "turn-two",
                                        "scope_owner": {"producer": "two"}}))
        with patch("observatory.native_turn_review.bundled_adapter_modules",
                   return_value=(first, duplicate)):
            with self.assertRaisesRegex(ValueError, "duplicate"):
                review_turn(None, "a" * 32)
        with patch("observatory.native_turn_review.bundled_adapter_modules",
                   return_value=(first, second)):
            with self.assertRaisesRegex(ValueError, "multiple"):
                review_turn(None, "a" * 32)

    def test_wrong_trace_or_source_claim_fails_closed(self):
        for candidate in (
            {"trace_id": "b" * 32, "turn_id": "turn", "scope_owner": {"producer": "one"}},
            {"trace_id": "a" * 32, "turn_id": "turn", "scope_owner": {"producer": "other"}},
        ):
            with self.subTest(candidate=candidate):
                module = SimpleNamespace(turn_review_spec=lambda: NativeTurnReviewSpec(
                    "one", lambda _db, _trace: candidate))
                with patch("observatory.native_turn_review.bundled_adapter_modules",
                           return_value=(module,)):
                    with self.assertRaisesRegex(ValueError, "mismatched"):
                        review_turn(None, "a" * 32)


if __name__ == "__main__":
    unittest.main()
