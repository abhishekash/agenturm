import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from observatory.saved_tasks import (TaskConflict, TaskError, TaskStore, export_handoff,
                                     render_handoff)
from observatory.task_workspace import _git, capture_workspace


class SavedTaskTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.path = self.root / 'tasks.sqlite3'
        self.store = TaskStore(self.path)

    def link(self, trace='a' * 32, **extra):
        return {'trace_id': trace, 'session_id': 'session', 'runtime': 'Codex',
                'state': 'Completed', 'captured_ns': time.time_ns(),
                'usage': {'input_tokens': 100, 'output_tokens': 20,
                          'cache_read_input_tokens': 80, 'known_cost_usd': .04,
                          'cost_complete': True, 'usage_complete': True}, **extra}

    def test_notes_and_links_survive_reopen_and_stale_edit_cannot_overwrite(self):
        task = self.store.create('Fix expired refresh', str(self.root), 'Intent\n\nNext: test expiry', self.link())
        second = TaskStore(self.path)
        saved = second.update(task['id'], notes='Decision: rotate after refresh', expected_revision=task['revision'])
        with self.assertRaises(TaskConflict):
            self.store.update(task['id'], title='Stale editor title', expected_revision=task['revision'])
        current = self.store.update(task['id'], title='Refresh expiry')
        self.assertEqual(current['notes'], saved['notes'])
        self.assertEqual(current['links'][0]['trace_id'], 'a' * 32)
        self.assertEqual(os.stat(self.path).st_mode & 0o777, 0o600)
        archived = self.store.update(task['id'], archived=True)
        self.assertEqual(self.store.list(), [])
        self.assertEqual(self.store.list(True)[0]['id'], task['id'])
        self.assertNotIn('links', self.store.list(True)[0])
        self.assertEqual(second.get(task['id'])['notes'], archived['notes'])

    def test_exact_run_links_deduplicate_and_unknown_usage_stays_unknown(self):
        task = self.store.create('Portable task', link=self.link())
        refreshed = self.store.link(task['id'], self.link(usage={'input_tokens': 200,
            'output_tokens': 40, 'known_cost_usd': .08, 'cost_complete': True, 'usage_complete': True}))
        self.assertEqual(len(refreshed['links']), 1)
        self.assertEqual(refreshed['usage_summary']['input_tokens'], 200)
        self.assertEqual(refreshed['usage_summary']['known_cost_usd'], .08)
        partial = self.store.link(task['id'], self.link('b' * 32, runtime='Claude Code', usage={'output_tokens': 50}))
        self.assertEqual(partial['usage_summary']['output_tokens'], 90)
        self.assertFalse(partial['usage_summary']['input_tokens_complete'])
        self.assertFalse(partial['usage_summary']['cost_complete'])
        unscoped = self.store.link(task['id'], self.link('', session_id='unscoped'))
        self.assertEqual(unscoped['usage_summary']['input_tokens'], 200)
        self.assertEqual(unscoped['usage_summary']['unscoped_links'], 1)
        self.assertEqual(unscoped['links'][-1]['usage'], {})
        unknown = self.store.create('No cost yet', link=self.link(usage={}))
        self.assertIsNone(unknown['usage_summary']['known_cost_usd'])
        self.assertIsNone(unknown['usage_summary']['input_tokens'])
        zero = self.store.create('Known free', link=self.link(usage={'known_cost_usd': 0, 'cost_complete': True}))
        self.assertEqual(zero['usage_summary']['known_cost_usd'], 0)
        self.assertTrue(zero['usage_summary']['cost_complete'])

    def test_current_ns_checks_are_kept_and_other_run_checks_are_excluded(self):
        now = time.time_ns()
        task = self.store.create('Checks', link=self.link(checks=[
            {'command': 'python -m unittest', 'result': 'success', 'time_ns': now, 'trace_id': 'a' * 32},
            {'command': 'different task test', 'result': 'success', 'time_ns': now, 'trace_id': 'b' * 32}]))
        checks = task['links'][0]['checks']
        self.assertEqual([check['command'] for check in checks], ['python -m unittest'])
        text = render_handoff(task)
        self.assertIn('freshness against the current workspace is unverified', text)
        self.assertNotIn('different task test', text)

    def test_validation_rejects_bad_or_unbounded_records_without_changes(self):
        task = self.store.create('Valid')
        for kwargs in ({'title': ''}, {'title': 'x' * 201}, {'notes': 'x' * 32001}, {'workspace': 'a\x1bb'}):
            with self.subTest(kwargs=next(iter(kwargs))), self.assertRaises(TaskError):
                self.store.update(task['id'], **kwargs)
        for evidence in ({}, {'trace_id': 'invalid'}, self.link(usage={'input_tokens': -1}),
                         self.link(usage={'input_tokens': 1.5}), self.link(usage=[]), self.link(checks={}),
                         self.link(usage={'input_tokens': 10**400}), self.link(usage={'known_cost_usd': 10**400}),
                         self.link(usage={'known_cost_usd': float('nan')}), self.link(captured_ns=-1)):
            with self.assertRaises(TaskError):
                self.store.link(task['id'], evidence)
        self.assertEqual(self.store.get(task['id'])['revision'], 1)

    def test_control_characters_are_rejected_and_frozen_exports_keep_them_inert(self):
        for character in ('\x1b', '\x07', '\x7f', '\x85', '\x9b'):
            with self.subTest(character=repr(character)):
                with self.assertRaises(TaskError):
                    self.store.create('Unsafe', notes='before' + character + 'after')
                task = self.store.create('Frozen evidence')
                # A frozen record can originate outside TaskStore. Its code block
                # and JSON renderers must still not emit terminal controls.
                task['notes'] = 'before' + character + 'after'
                for format in ('markdown', 'html', 'json'):
                    output = render_handoff(task, format)
                    self.assertNotIn(character, output)
                self.assertEqual(json.loads(render_handoff(task, 'json'))['task']['notes'], task['notes'])

    def test_incomplete_evidence_never_labels_reported_token_subtotals_as_complete(self):
        task = self.store.create('Partial telemetry', link=self.link(usage={
            'input_tokens': 100, 'output_tokens': 20, 'usage_complete': False}))
        self.assertFalse(task['usage_summary']['input_tokens_complete'])
        self.assertIn('Input tokens: 100 (partial)', render_handoff(task))

    def test_list_paginates_without_loading_notes_or_hiding_older_records(self):
        now = time.time_ns()
        with self.store._connect() as db:
            db.executemany('INSERT INTO saved_tasks_v1 VALUES(?,?,?,?,?,?,?,?,?,?)', [
                (f'task-{index}', f'Task {index}', '', 'A retained note', 0,
                 now, now + index, 1, '[]', '{}') for index in range(1001)])
        newest = self.store.list()
        older = self.store.list(offset=1000)
        self.assertEqual(len(newest), 1000)
        self.assertEqual(self.store.count(), 1001)
        self.assertEqual([entry['id'] for entry in older], ['task-0'])
        self.assertNotIn('notes', newest[0])
        with self.assertRaises(TaskError):
            self.store.list(offset=-1)

    def test_workspace_change_resets_base_and_snapshot_conflict_preserves_notes(self):
        task = self.store.create('Workspace', str(self.root))
        def concurrent_change(*args):
            self.store.update(task['id'], notes='A new decision')
            return {'base_commit': 'a' * 40}
        with patch('observatory.saved_tasks.capture_workspace', side_effect=concurrent_change):
            with self.assertRaises(TaskConflict):
                self.store.snapshot(task['id'])
        current = self.store.get(task['id'])
        self.assertEqual(current['notes'], 'A new decision')
        self.assertEqual(current['workspace_snapshot'], {})
        with patch('observatory.saved_tasks.capture_workspace', return_value={'base_commit': 'a' * 40}):
            self.store.snapshot(task['id'])
        moved = self.store.update(task['id'], workspace=str(self.root / 'different'))
        self.assertEqual(moved['workspace_snapshot'], {})

    def test_frozen_exports_escape_html_preserve_dotted_names_and_never_overwrite(self):
        task = self.store.create('<script>alert(1)</script>', notes='<img src="https://example.invalid/leak">\n```\n# still a note')
        self.store.update(task['id'], notes='Changed after preview')
        paths = export_handoff(task, self.root / 'handoff.v1.md')
        self.assertEqual([Path(path).name for path in paths], ['handoff.v1.md', 'handoff.v1.html'])
        markdown = Path(paths[0]).read_text()
        html = Path(paths[1]).read_text()
        self.assertNotIn('Changed after preview', markdown)
        self.assertIn('still a note', markdown)
        self.assertNotIn('<script>', html)
        self.assertNotIn('<img ', html)
        self.assertIn('&lt;script&gt;', html)
        self.assertIn('prefers-color-scheme:dark', html)
        for path in paths:
            self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)
        with self.assertRaises(FileExistsError):
            export_handoff(task, self.root / 'handoff.v1.md')
        self.assertEqual(Path(paths[0]).read_text(), markdown)
        (self.root / 'blocked.html').symlink_to(self.root / 'missing')
        with self.assertRaises(FileExistsError):
            export_handoff(task, self.root / 'blocked.md')
        self.assertFalse((self.root / 'blocked.md').exists())
        self.assertTrue((self.root / 'blocked.html').is_symlink())
        self.assertEqual(list(self.root.glob('.agenturm-handoff-*')), [])
        upper = export_handoff(task, self.root / 'report.HTML')
        self.assertIn(str(self.root / 'report.HTML'), upper)
        with self.assertRaises(FileExistsError):
            export_handoff(task, self.root / 'report.HTML')


class GitSnapshotTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo '$literal"
        self.repo.mkdir()
        self.git('init', '-q')
        self.git('config', 'user.name', 'Test')
        self.git('config', 'user.email', 'test@example.invalid')
        (self.repo / 'tracked.txt').write_text('original\n')
        self.git('add', 'tracked.txt')
        self.git('commit', '-qm', 'base')

    def git(self, *args):
        return subprocess.run(['git', '-C', str(self.repo), *args], capture_output=True,
                              text=True, check=True).stdout.strip()

    def test_saved_base_keeps_committed_staged_and_unstaged_changes_and_untracked_paths(self):
        initial = capture_workspace(str(self.repo))
        self.assertTrue(initial['available'], initial)
        self.assertTrue(initial['complete'], initial)
        base = self.git('rev-parse', 'HEAD')
        (self.repo / 'committed.txt').write_text('committed later\n')
        self.git('add', 'committed.txt')
        self.git('commit', '-qm', 'later')
        (self.repo / 'staged.txt').write_text('staged\n')
        self.git('add', 'staged.txt')
        (self.repo / 'tracked.txt').write_text('updated\n')
        (self.repo / 'untracked.txt').write_text('UNTRACKED SECRET CONTENT\n')
        before = self.git('status', '--porcelain')
        snapshot = capture_workspace(str(self.repo), initial)
        self.assertEqual(snapshot['base_commit'], base)
        self.assertNotEqual(snapshot['head'], base)
        self.assertEqual({entry['path'] for entry in snapshot['files']},
                         {'committed.txt', 'staged.txt', 'tracked.txt', 'untracked.txt'})
        for expected in ('committed later', 'staged', 'updated'):
            self.assertIn(expected, snapshot['diff'])
        self.assertNotIn('UNTRACKED SECRET CONTENT', json.dumps(snapshot))
        self.assertEqual(self.git('status', '--porcelain'), before)
        self.assertIn('pre-existing or concurrent', snapshot['scope'])

    def test_filters_external_diff_and_fsmonitor_are_not_executed(self):
        marker = self.root / 'executed'
        script = self.root / 'unsafe.py'
        script.write_text('from pathlib import Path\nimport sys\nPath(' + repr(str(marker)) + ').write_text("executed")\nsys.stdout.write(sys.stdin.read())\n')
        unsafe = shlex.join([sys.executable, str(script)])
        self.git('config', 'filter.test.clean', unsafe)
        self.git('config', 'filter.test.process', unsafe)
        self.git('config', 'filter.test.required', 'true')
        self.git('config', 'diff.test.textconv', unsafe)
        self.git('config', 'diff.external', unsafe)
        self.git('config', 'core.fsmonitor', unsafe)
        (self.repo / '.gitattributes').write_text('tracked.txt filter=test diff=test\n')
        (self.repo / 'tracked.txt').write_text('new content\n')
        snapshot = capture_workspace(str(self.repo))
        self.assertTrue(snapshot['available'], snapshot)
        self.assertTrue(snapshot['complete'], snapshot)
        self.assertIn('new content', snapshot['diff'])
        self.assertFalse(marker.exists())

    def test_missing_promisor_objects_do_not_fetch_or_execute_a_transport(self):
        initial = capture_workspace(str(self.repo))
        blob = self.git('hash-object', 'tracked.txt')
        self.git('config', 'core.repositoryformatversion', '1')
        self.git('config', 'extensions.partialClone', 'origin')
        self.git('config', 'remote.origin.promisor', 'true')
        self.git('config', 'remote.origin.url', 'agenturmprobe::unused')
        self.git('config', 'protocol.agenturmprobe.allow', 'always')
        (self.repo / '.git' / 'objects' / blob[:2] / blob[2:]).unlink()
        (self.repo / 'tracked.txt').write_text('changed without the original object\n')
        bin_path = self.root / 'transport-bin'
        bin_path.mkdir()
        marker = self.root / 'transport-executed'
        helper = bin_path / 'git-remote-agenturmprobe'
        helper.write_text('#!' + sys.executable + '\nfrom pathlib import Path\nPath(' + repr(str(marker)) + ').touch()\n')
        helper.chmod(0o700)
        def git_files():
            directory = self.repo / '.git'
            return {str(path.relative_to(directory)): path.read_bytes() for path in directory.rglob('*') if path.is_file()}
        before = git_files()
        with patch.dict(os.environ, {'PATH': str(bin_path) + os.pathsep + os.environ['PATH'],
                                     'GIT_NO_LAZY_FETCH': '0', 'GIT_ALLOW_PROTOCOL': 'agenturmprobe'}):
            snapshot = capture_workspace(str(self.repo), initial)
        self.assertFalse(snapshot['complete'])
        self.assertIn('error', snapshot)
        self.assertFalse(marker.exists())
        self.assertEqual(git_files(), before)

    def test_missing_workspace_and_non_git_directory_keep_readable_partial_state(self):
        missing = capture_workspace(str(self.root / 'absent'))
        self.assertFalse(missing['available'])
        self.assertIn('unavailable', missing['error'])
        plain = capture_workspace(str(self.root))
        self.assertFalse(plain['complete'])
        self.assertIn('not a git repository', plain['error'])

    def test_pinned_base_survives_a_transient_unavailable_repository(self):
        initial = capture_workspace(str(self.repo))
        base = initial['base_commit']
        moved = self.root / 'temporarily-moved'
        self.repo.rename(moved)
        unavailable = capture_workspace(str(self.repo), initial)
        self.assertFalse(unavailable['available'])
        self.assertEqual(unavailable['base_commit'], base)
        moved.rename(self.repo)
        (self.repo / 'tracked.txt').write_text('changed after base\n')
        self.git('add', 'tracked.txt')
        self.git('commit', '-qm', 'later')
        restored = capture_workspace(str(self.repo), unavailable)
        self.assertEqual(restored['base_commit'], base)
        self.assertIn('changed after base', restored['diff'])

    def test_path_limit_does_not_double_count_tracked_changes(self):
        for index in range(110):
            (self.repo / f'file-{index:03}.txt').write_text('base\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'many files')
        for index in range(110):
            (self.repo / f'file-{index:03}.txt').write_text('change\n')
        snapshot = capture_workspace(str(self.repo))
        self.assertEqual(snapshot['file_count'], 110)
        self.assertEqual(len(snapshot['files']), 100)
        self.assertFalse(snapshot['complete'])

    def test_command_output_and_time_are_bounded(self):
        # A fake git process provides deterministic pressure without a huge repository.
        bin_path = self.root / 'bin'
        bin_path.mkdir()
        fake = bin_path / 'git'
        fake.write_text('#!' + sys.executable + '\nimport sys,time\nif "hang" in sys.argv: time.sleep(5)\nelse: sys.stdout.write("x"*1000000)\n')
        fake.chmod(0o700)
        with patch.dict(os.environ, {'PATH': str(bin_path)}):
            answer = _git(self.repo, [], timeout=.5, max_bytes=1024)
            self.assertTrue(answer['truncated'])
            self.assertLessEqual(len(answer['stdout']), 1024)
            started = time.monotonic()
            timed = _git(self.repo, ['hang'], timeout=.1)
            self.assertTrue(timed['timed_out'])
            self.assertLess(time.monotonic() - started, 2)
        self.assertFalse(any(thread.name.startswith('agenturm-git-') for thread in threading.enumerate()))

    @unittest.skipUnless(sys.platform == 'linux', 'uses Linux subreaping to verify descendant cleanup')
    def test_completed_parent_cannot_leave_descendants_or_pipe_readers_running(self):
        bin_path = self.root / 'descendant-bin'
        bin_path.mkdir()
        child_pid = self.root / 'child.pid'
        fake = bin_path / 'git'
        fake.write_text('#!' + sys.executable + '\nimport subprocess,sys\nfrom pathlib import Path\n'
                        'child=subprocess.Popen([sys.executable,"-c","import time; time.sleep(30)"])\n'
                        'Path(' + repr(str(child_pid)) + ').write_text(str(child.pid))\n')
        fake.chmod(0o700)
        # Subreaping is confined to this helper so the test also reaps the killed
        # orphan instead of depending on the machine's PID 1 cleanup behavior.
        script = '''import ctypes,json,os,signal,sys,threading,time
from pathlib import Path
from observatory.task_workspace import _git
assert ctypes.CDLL(None).prctl(36,1,0,0,0) == 0
os.environ['PATH'] = sys.argv[1]
started = time.monotonic()
answer = _git(sys.argv[2], [], timeout=.2)
assert answer['timed_out'], answer
pid = int(Path(sys.argv[3]).read_text())
_, status = os.waitpid(pid, 0)
assert os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGKILL, status
assert not any(thread.name.startswith('agenturm-git-') for thread in threading.enumerate())
assert time.monotonic() - started < 2
print('clean')
'''
        answer = subprocess.run([sys.executable, '-c', script, str(bin_path), str(self.repo), str(child_pid)],
                                text=True, capture_output=True, timeout=5)
        self.assertEqual((answer.returncode, answer.stdout.strip()), (0, 'clean'), answer.stderr)

