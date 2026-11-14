"""Frozen public references and one qualification pass over each raw source."""
from copy import deepcopy
import json
from pathlib import Path
import unittest
from unittest.mock import patch

from observatory.error_reminders import trace_error_references
from observatory.operation_facts import operation_facts

FIXTURE=Path(__file__).parent/'fixtures'/'error_reference_projection'/'baseline.json'


class ErrorReferenceProjection(unittest.TestCase):
    def cases(self):return json.loads(FIXTURE.read_text())['cases']

    def test_references_are_byte_identical_to_frozen_449f64f_output(self):
        for case in self.cases():
            with self.subTest(case=case['name']):
                original=deepcopy(case['activity'])
                actual=trace_error_references(case['activity'],case['trace_id'],receiver=case['receiver'])
                self.assertEqual(json.dumps(actual,ensure_ascii=False,separators=(',',':')),case['expected_json'])
                self.assertEqual(case['activity'],original,'source activity must stay immutable')
                self.assertTrue(all(not ({'detail','operation_evidence','attributes','output','command'}&reference.keys()) for reference in actual))
                self.assertNotIn('SYNTHETIC_PRIVATE_PASSWORD',json.dumps(actual))

    def test_each_raw_source_is_qualified_once_and_compact_sources_are_not_replayed(self):
        before=0;after=0
        for case in self.cases():
            with self.subTest(case=case['name']):
                with patch('observatory.operation_facts.operation_facts',wraps=operation_facts) as qualified:
                    trace_error_references(deepcopy(case['activity']),case['trace_id'],receiver=case['receiver'])
                self.assertEqual(qualified.call_count,case['raw_source_events'])
                before+=case['baseline_raw_fact_calls'];after+=qualified.call_count
        self.assertLess(after,before)
        self.assertEqual((before,after),(237,134))
