"""Host-declared source groups render without harness-specific UI dispatch."""
import unittest
from rich.cells import cell_len

from observatory.source_coverage import (coverage_detail, coverage_rows,
                                         coverage_summary, coverage_warning)


class SourceCoverageTests(unittest.TestCase):
    def test_new_reader_and_process_group_are_rendered_generically(self):
        status = {"source_coverage": [
            {"label": "Eclipse", "unit": "sessions", "active": 4,
             "omitted": 2, "limit": 48, "state": "partial"},
            {"label": "Process adapters", "unit": "sessions", "active": 3,
             "omitted": 1, "omitted_exact": False,
             "pending": 1, "state": "partial"},
        ]}
        self.assertIn("7 sessions observed", coverage_summary(status))
        self.assertIn("Eclipse: 4 sessions / 48 limit", coverage_detail(status))
        warning = coverage_warning(status)
        self.assertIn("2 Eclipse sessions omitted", warning)
        self.assertIn("at least 1 Process adapters sessions omitted", warning)
        self.assertIn("1 Process adapters sources pending", warning)

    def test_malformed_or_unbounded_remote_groups_are_not_rendered(self):
        status = {"source_coverage": [
            {"label": "\x1b[31msecret", "unit": "files", "active": -1,
             "omitted": 2**63, "state": "ready"},
            {"label": "Bad", "unit": "paths", "active": 1},
            {"label": "Bad shape", "unit": ["sessions"], "state": ["ready"]},
            *({"label": f"Adapter {index}", "unit": "sessions",
               "active": index, "omitted": 0, "state": "ready"}
              for index in range(40)),
        ]}
        rows = coverage_rows(status)
        self.assertEqual(len(rows), 22)
        self.assertEqual(rows[0]["active"], None)
        self.assertEqual(rows[0]["omitted"], None)
        self.assertNotIn("\x1b", rows[0]["label"])
        self.assertIn("additional source groups are not shown", coverage_warning(status))

    def test_unknown_and_partial_remain_explicit(self):
        status = {"source_coverage": [
            {"label": "Reader", "unit": "sessions", "active": None,
             "omitted": None, "state": "unavailable"},
            {"label": "Native", "unit": "files", "active": 1,
             "omitted": 0, "state": "partial"},
        ]}
        self.assertIn("1 file observed, sessions count unavailable", coverage_summary(status))
        self.assertIn("Reader coverage unavailable", coverage_warning(status))
        self.assertIn("Native coverage partial", coverage_warning(status))

    def test_long_wide_labels_and_many_limits_remain_bounded(self):
        status = {"source_coverage": [
            {"label": "エクリプス🌘" * 10, "unit": "sessions",
             "active": 1, "omitted": 1, "state": "partial"}
            for _ in range(12)]}
        self.assertLessEqual(cell_len(coverage_rows(status)[0]["label"]), 32)
        self.assertIn("additional coverage limits are not shown", coverage_warning(status))


if __name__ == "__main__":
    unittest.main()
