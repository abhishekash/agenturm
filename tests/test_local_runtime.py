"""Process association uses exact same-user open transcripts, not shared cwd."""
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from observatory.local_agents import LocalAgents
from observatory.local_runtime import snapshot


class RuntimeTests(unittest.TestCase):
    def test_exact_file_matches_and_tool_child_cpu_are_available_without_arguments(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            transcript = root / 'session.jsonl'
            transcript.touch()
            proc = root / 'proc'

            def process(pid, name, ticks, children=''):
                folder = proc / str(pid)
                (folder / 'fd').mkdir(parents=True)
                (folder / 'task' / str(pid)).mkdir(parents=True)
                (folder / 'task' / str(pid) / 'children').write_text(children)
                (folder / 'comm').write_text(name)
                (folder / 'status').write_text('State:\tS (sleeping)\nPPid:\t1\nVmRSS:\t123 kB\n')
                fields = ['0'] * 22
                fields[0] = 'S'
                fields[11] = str(ticks)
                fields[19] = str(pid * 10)
                (folder / 'stat').write_text(f'{pid} ({name}) ' + ' '.join(fields))
                (folder / 'io').write_text('rchar: 100\nwchar: 20\n')
                (folder / 'cwd').symlink_to(root)
                (folder / 'cmdline').write_text('SECRET-ARGUMENT')
                (folder / 'environ').write_text('SECRET-TOKEN')
                return folder

            owner = process(101, 'codex', 100, '102')
            process(102, 'compiler', 200)
            process(201, 'codex', 300)  # Same cwd, but not the same session.
            (owner / 'fd' / '4').symlink_to(transcript)
            result = snapshot({transcript: 'source'}, proc_root=proc, uid=os.getuid())
            sample = result['sessions']['source']
            self.assertEqual(sample['pids'], [101])
            self.assertAlmostEqual(sample['cpu_seconds'], 300 / os.sysconf('SC_CLK_TCK'))
            self.assertEqual(sample['io_chars'], 240)
            self.assertEqual(len(result['processes']), 2)
            self.assertNotIn('SECRET', json.dumps(result))
            other_user = snapshot({transcript: 'source'}, proc_root=proc, uid=os.getuid()+1)
            self.assertEqual(other_user['sessions'], {})
            self.assertEqual(other_user['processes'], [])

    def test_unavailable_inventory_does_not_fabricate_liveness(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = snapshot(proc_root=Path(tmp) / 'missing')
            self.assertFalse(result['process_inventory_available'])
            self.assertEqual(result['sessions'], {})

    def test_progress_uses_deltas_and_process_restart_does_not_inherit_old_progress(self):
        monitor = LocalAgents(object())

        def sample(started, cpu):
            return {'sessions': {'source': {'pids': [123], 'identity': [(123, started)],
                                            'cpu_seconds': cpu, 'io_chars': 10}}}

        with patch('observatory.local_runtime.snapshot', side_effect=[sample(1, 1), sample(1, 2), sample(2, 3)]):
            for now, expected in ((100, 0), (200, 200), (300, 0)):
                monitor.runtime_updated = 0
                with patch('observatory.local_agents.time.time_ns', return_value=now):
                    monitor.status()
                self.assertEqual(monitor.process_samples['source']['last_progress_ns'], expected)
