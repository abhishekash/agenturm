import tempfile
import unittest
from pathlib import Path

from observatory.core import Store, normalize_native


class ConnectionCountTests(unittest.TestCase):
    def test_exact_source_counts_use_an_index(self):
        with tempfile.TemporaryDirectory() as directory:
            store=Store(Path(directory)/'counts.db')
            try:
                source='a'*32
                store.register_connection({'source_id':source,'service':'codex','runtime':'Codex',
                                           'integrations':['local-session-log']})
                for i,resources in enumerate(({'observatory.source_id':source},
                                               {'unrelated':source},
                                               {'observatory.source_id':'b'*32}),1):
                    store.ingest([normalize_native({'trace_id':f'{i:032x}','span_id':f'{i:016x}',
                        'service':'codex','name':'task','start_unix_nano':i,'end_unix_nano':i+1,
                        'resource_attributes':resources,'status':'OK'},service='codex')])
                self.assertEqual(store.connections()[0]['span_count'],1)
                with store._connect() as db:
                    plan=db.execute('EXPLAIN QUERY PLAN SELECT COUNT(*) FROM spans WHERE service=? '
                                    'AND json_extract(resource_attributes,\'$."observatory.source_id"\')=?',
                                    ('codex',source)).fetchall()
                    self.assertTrue(any('USING INDEX spans_source' in row[3] for row in plan))
            finally:store.close()
