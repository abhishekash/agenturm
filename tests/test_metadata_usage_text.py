import unittest

from observatory.ui_monitoring import metadata_usage_text


class MetadataUsageTextTest(unittest.TestCase):
    def test_missing_counters_remain_unreported_and_zero_is_visible(self):
        self.assertEqual(metadata_usage_text({"input_tokens": 0, "cache_read_tokens": 1200}),
                         "Input 0 · Cache read 1,200")
        self.assertEqual(metadata_usage_text({}), "")
        self.assertEqual(metadata_usage_text({"input_tokens": True}), "")
        self.assertEqual(metadata_usage_text({"total_tokens": "18446744073709551614"}),
                         "Total 18,446,744,073,709,551,614")
