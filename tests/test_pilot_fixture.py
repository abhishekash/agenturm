"""The participant kit is synthetic, repeatable and never overwrites a store."""
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from observatory.core import Store
from observatory.storage_investigation import read_page
from scripts.generate_pilot_fixture import create


class PilotFixtureTests(unittest.TestCase):
    def test_explicit_new_fixture_answers_typed_questions_and_turn_membership(self):
        with TemporaryDirectory() as directory:
            path = Path(directory) / 'synthetic.db'
            metadata = create(path)
            self.assertTrue(metadata['synthetic'])
            self.assertEqual(metadata['participant_count'], 0)
            self.assertEqual(metadata['records'], 9)
            with self.assertRaises(FileExistsError):
                create(path)
            store = Store(path)
            try:
                with store._connect() as db:
                    paths = read_page(db, 'path', 'src/retry.py', receiver=store.receiver_id)['matches']
                    self.assertEqual({row['source_reference']['service'] for row in paths}, {'codex', 'opencode'})
                    self.assertEqual({row['result'] for row in paths}, {'success', 'unknown'})
                    renamed = read_page(db, 'path', 'src/古い.py', receiver=store.receiver_id)['matches']
                    self.assertEqual(len(renamed), 1)
                    failed = read_page(db, 'command', 'python migrate.py', failed=True, receiver=store.receiver_id)['matches']
                    self.assertEqual(len(failed), 1)
                    self.assertEqual(failed[0]['source_reference']['service'], 'claude')
                    unknown = read_page(db, 'instruction', 'retry budget', receiver=store.receiver_id)['matches'][0]
                    self.assertIsNone(unknown['source_reference']['agent_id'])
                turn = store.review_turn('c' * 32)
                self.assertEqual(turn['turn_id'], 'b' * 32)
                self.assertEqual(turn['step_trace_ids'], ['c' * 32, 'd' * 32])
            finally:
                store.close()


if __name__ == '__main__':
    unittest.main()
