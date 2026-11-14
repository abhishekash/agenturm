"""History-mode preparation preserves the corpus outside the recent inbox."""
from pathlib import Path
import sqlite3
import tempfile
import time
import unittest
from scripts.soak_monitor import seed_history,active_index_state,index_proof


class SoakFixtureTests(unittest.TestCase):
    def test_proof_reads_durable_marker_and_receiver_without_creating_a_missing_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'proof.db'
            self.assertEqual(index_proof(path,'receiver','receiver'),{})
            self.assertFalse(path.exists())
            with sqlite3.connect(path) as db:
                db.execute('CREATE TABLE schema_meta(key TEXT PRIMARY KEY,value TEXT)')
                db.executemany('INSERT INTO schema_meta VALUES(?,?)',[('receiver_id','receiver'),('trace_derived_v15','1')])
            self.assertTrue(all(index_proof(path,'receiver','receiver').values()))
            self.assertFalse(index_proof(path,'receiver','other')['health_matches'])
            self.assertFalse(index_proof(path,'other','other')['receiver_matches'])
            with sqlite3.connect(path) as db:db.execute("DELETE FROM schema_meta WHERE key='trace_derived_v15'")
            self.assertFalse(index_proof(path,'receiver','receiver')['derived_marker_complete'])

    def test_active_incoming_state_needs_complete_identity_marker_and_successful_reads(self):
        proof=dict(receiver_matches=True,health_matches=True,derived_marker_complete=True,file_unchanged=True)
        incoming=dict(ready=False,stage='Indexing incoming source changes')
        self.assertEqual(active_index_state(incoming,proof,True,True),'incoming')
        self.assertEqual(active_index_state(dict(ready=True),proof,True,True),'ready')
        for key in proof:
            self.assertEqual(active_index_state(incoming,{**proof,key:False},True,True),'failure')
        for state in ({},dict(ready=False),dict(ready=False,stage='Preparing history'),
                      {**incoming,'error':'worker failed'},{**incoming,'ready':0}):
            self.assertEqual(active_index_state(state,proof,True,True),'failure')
        self.assertEqual(active_index_state(incoming,proof,False,True),'failure')
        self.assertEqual(active_index_state(incoming,proof,True,False),'failure')

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
