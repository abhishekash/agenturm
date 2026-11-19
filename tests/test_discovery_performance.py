"""Discovery keeps freshness and selection semantics while streaming directories."""
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch

from observatory.local_agents import LocalAgents
from observatory.native_sources import NativeSourceSpec


class DiscoveryTests(unittest.TestCase):
    def test_registered_source_controls_discovery_without_core_runtime_branch(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            (root / 'skip').mkdir()
            (root / 'keep').mkdir()
            (root / 'skip' / 'ignored.jsonl').touch()
            accepted = root / 'keep' / 'accepted.jsonl'
            accepted.touch()
            (root / 'keep' / 'rejected.jsonl').touch()
            spec = NativeSourceSpec('fixture', 'Fixture', lambda: root,
                accepts_file=lambda path: Path(path).name == 'accepted.jsonl',
                accepts_directory=lambda name: name != 'skip')
            monitor = LocalAgents(None, source_specs=(spec,))
            monitor.discover()
            self.assertEqual(list(monitor.sessions), [accepted])
            self.assertEqual(monitor.sessions[accepted].runtime, 'Fixture')
            self.assertEqual(monitor.discovered_sources, 1)
            with self.assertRaisesRegex(ValueError, 'duplicate native source ID'):
                LocalAgents(None, source_specs=(spec, spec))
            with self.assertRaisesRegex(ValueError, 'invalid native source ID'):
                NativeSourceSpec('', 'Fixture', lambda: root)

    def test_bounded_selection_finds_new_files_and_revived_old_sessions(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            folder = root / 'codex' / 'sessions' / 'nested'
            folder.mkdir(parents=True)
            now = time.time()
            for index in range(60):
                path = folder / f'{index}.jsonl'
                path.touch()
                os.utime(path, (now - index, now - index))
            old = folder / 'old.jsonl'
            old.touch()
            os.utime(old, (now - 7 * 86400, now - 7 * 86400))
            (folder / 'ignored.txt').touch()
            (folder / 'cycle').symlink_to(folder, target_is_directory=True)
            env = {'CODEX_HOME': str(root / 'codex'), 'CLAUDE_CONFIG_DIR': str(root / 'claude'),
                   'PI_CODING_AGENT_DIR': str(root / 'pi'), 'KIRO_HOME': str(root / 'kiro')}
            with patch.dict(os.environ, env):
                monitor = LocalAgents(None)
                monitor.discover()
                self.assertEqual(monitor.discovered_sources, 60)
                self.assertEqual(monitor.omitted_sources, 10)
                self.assertEqual({path.name for path in monitor.sessions}, {f'{i}.jsonl' for i in range(50)})
                kept = monitor.sessions[folder / '0.jsonl']
                os.utime(old, (now + 1, now + 1))
                new = folder / 'new.jsonl'
                new.touch()
                monitor.discover()
                self.assertIn(old, monitor.sessions)
                self.assertIn(new, monitor.sessions)
                self.assertIs(monitor.sessions[folder / '0.jsonl'], kept)
                self.assertEqual(monitor.discovered_sources, 62)
                self.assertEqual(len(monitor.sessions), 50)
                monitor.stop.set()
                with patch('observatory.local_agents.os.scandir') as scan:
                    monitor.discover()
                    scan.assert_not_called()
