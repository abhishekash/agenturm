"""Source interpretation can be contributed without changing core projection."""
import unittest

from observatory.core import _tool_activity
from observatory.operation_contributors import OperationContributorRegistry
from observatory.operation_facts import operation_facts


class FixtureContributor:
    id = "fixture-operation"

    def matches(self, span, attributes):
        return span.get("service") == "fixture" and attributes.get("tool.name") == "fixture-tool"

    def contribute(self, attributes, domain, evidence):
        evidence["commands"].append(("fixture.command", "run fixture check"))
        evidence["exits"].append(("fixture.exit", 0))
        return True


class OperationContributorTests(unittest.TestCase):
    def test_synthetic_contributor_changes_typed_facts_only_for_its_source(self):
        registry = OperationContributorRegistry((FixtureContributor(),))
        span = {"span_id": "1" * 16, "service": "fixture", "status": "OK",
                "attributes": {"tool.name": "fixture-tool",
                               "agent.activity.detail": "unrelated output"}}
        facts = operation_facts(_tool_activity(span, contributors=registry))
        self.assertEqual(facts["command"], "run fixture check")
        self.assertEqual(facts["exit_code"], 0)
        self.assertNotIn("unrelated output", facts["output"] or "")
        other = {**span, "service": "other"}
        unrelated = operation_facts(_tool_activity(other, contributors=registry))
        self.assertIsNone(unrelated["command"])

    def test_duplicate_contributor_ids_fail_before_projection(self):
        with self.assertRaisesRegex(ValueError, "invalid operation contributor registry"):
            OperationContributorRegistry((FixtureContributor(), FixtureContributor()))


if __name__ == "__main__":
    unittest.main()
