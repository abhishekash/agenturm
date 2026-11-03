"""Measure native discovery against synthetic old history and recent sessions."""
import json
import os
from pathlib import Path
import statistics
import sys
import tempfile
import time
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from observatory.local_agents import LocalAgents


def benchmark():
    with tempfile.TemporaryDirectory(prefix='agenturm-discovery-') as root:
        root = Path(root)
        sessions = root / 'codex' / 'sessions'
        old = time.time() - 7 * 86400
        for directory in range(100):
            folder = sessions / str(directory)
            folder.mkdir(parents=True)
            for index in range(100):
                path = folder / f'{index}.jsonl'
                path.touch()
                if index:
                    os.utime(path, (old, old))
        env = {'CODEX_HOME': str(root / 'codex'),
               'CLAUDE_CONFIG_DIR': str(root / 'claude'),
               'PI_CODING_AGENT_DIR': str(root / 'pi')}
        with patch.dict(os.environ, env):
            monitor = LocalAgents(None)
            monitor.discover()
            samples = []
            for _ in range(10):
                start = time.process_time()
                monitor.discover()
                samples.append((time.process_time() - start) * 1000)
            return {'files': 10000, 'directories': 100,
                    'recent_sources': monitor.discovered_sources,
                    'selected_sources': len(monitor.sessions),
                    'median_cpu_ms': round(statistics.median(samples), 3)}


if __name__ == '__main__':
    print(json.dumps(benchmark(), indent=2))
