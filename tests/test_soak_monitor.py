"""History-mode preparation preserves the corpus outside the recent inbox."""
from pathlib import Path
import sqlite3
import tempfile
import time
import unittest
from scripts.soak_monitor import seed_history


class SoakFixtureTests(unittest.TestCase):
    def test_aging_preserves_source_counts_and_all_storage_timestamps(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'synthetic.db'
            seed_history(path,traces=3,span_count=12,log_count=20)
            cutoff=time.time_ns()-86400*10**9
            with sqlite3.connect(path) as db:
                self.assertEqual(db.execute('SELECT COUNT(*) FROM spans').fetchone()[0],12)
                self.assertEqual(db.execute('SELECT COUNT(*) FROM logs').fetchone()[0],20)
                self.assertEqual(db.execute('SELECT COUNT(*) FROM trace_activity').fetchone()[0],3)
                for table,column in [('spans','start_ns'),('spans','end_ns'),('logs','time_ns'),('trace_activity','last_ns')]:
                    self.assertLess(db.execute(f'SELECT MAX({column}) FROM {table}').fetchone()[0],cutoff)
