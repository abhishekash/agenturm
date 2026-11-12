import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class TaskCLITests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.database = self.root / 'tasks.db'
        self.cwd = Path(__file__).resolve().parents[1]

    def cli(self, *arguments):
        return subprocess.run([sys.executable, '-m', 'observatory', 'task',
            '--tasks-db', str(self.database), *arguments], cwd=self.cwd,
            env={**os.environ, 'NO_COLOR': '1'}, text=True, capture_output=True, timeout=10)

    def test_create_update_link_reopen_and_export_without_receiver(self):
        created = self.cli('create', '--title', 'Refresh expiry', '--notes', 'Next: check rotating tokens', '--json')
        self.assertEqual((created.returncode, created.stderr), (0, ''))
        task = json.loads(created.stdout)
        updated = self.cli('update', task['id'], '--notes', 'Decision: rotate on refresh', '--expected-revision', str(task['revision']), '--json')
        self.assertEqual(updated.returncode, 0, updated.stderr)
        linked = self.cli('link', task['id'], '--runtime', 'Codex', '--trace-id', 'a' * 32, '--session-id', 's1', '--json')
        self.assertEqual(linked.returncode, 0, linked.stderr)
        self.assertEqual(len(json.loads(linked.stdout)['links']), 1)
        listed = self.cli('list', '--json')
        self.assertEqual(json.loads(listed.stdout)['tasks'][0]['id'], task['id'])
        shown = self.cli('show', task['id'])
        self.assertIn('Decision: rotate on refresh', shown.stdout)
        self.assertNotIn('\x1b', shown.stdout)
        exported = self.cli('handoff', task['id'], '--output', str(self.root / 'brief.v1.json'), '--json')
        self.assertEqual((exported.returncode, exported.stderr), (0, ''))
        self.assertEqual({Path(path).suffix for path in json.loads(exported.stdout)['files']}, {'.md', '.html', '.json'})
        second = self.cli('handoff', task['id'], '--output', str(self.root / 'brief.v1.json'), '--json')
        self.assertEqual((second.returncode, second.stdout), (2, ''))
        self.assertEqual(json.loads(second.stderr)['error']['code'], 'already_exists')

    def test_json_errors_use_stderr_and_conflicts_have_nonzero_exit(self):
        task = json.loads(self.cli('create', '--title', 'Saved', '--json').stdout)
        self.cli('update', task['id'], '--notes', 'Newer')
        conflict = self.cli('update', task['id'], '--notes', 'Stale', '--expected-revision', '1', '--json')
        self.assertEqual((conflict.returncode, conflict.stdout), (3, ''))
        self.assertEqual(json.loads(conflict.stderr)['error']['code'], 'conflict')
        missing = self.cli('show', 'missing', '--json')
        self.assertEqual((missing.returncode, missing.stdout), (2, ''))
        self.assertNotIn('Traceback', missing.stderr)
        invalid = self.cli('link', task['id'], '--trace-id', 'bad', '--json')
        self.assertEqual((invalid.returncode, invalid.stdout), (2, ''))
        self.assertIn('32 hexadecimal', json.loads(invalid.stderr)['error']['message'])
        huge = self.root / 'huge-number.json'
        huge.write_text(json.dumps({'trace_id': 'a' * 32, 'usage': {'input_tokens': 10**400}}))
        overflow = self.cli('link', task['id'], '--evidence-file', str(huge), '--json')
        self.assertEqual((overflow.returncode, overflow.stdout), (2, ''))
        self.assertEqual(json.loads(overflow.stderr)['error']['code'], 'invalid_input')
        self.assertNotIn('Traceback', overflow.stderr)

    def test_help_is_noninteractive_and_archive_can_be_restored(self):
        help = self.cli()
        self.assertEqual((help.returncode, help.stderr), (0, ''))
        self.assertIn('handoff', help.stdout)
        task_id = self.cli('create', '--title', 'Archive me').stdout.strip()
        self.assertEqual(self.cli('archive', task_id).returncode, 0)
        self.assertEqual(json.loads(self.cli('list', '--json').stdout)['tasks'], [])
        self.assertEqual(self.cli('archive', task_id, '--restore').returncode, 0)
        self.assertEqual(len(json.loads(self.cli('list', '--json').stdout)['tasks']), 1)

