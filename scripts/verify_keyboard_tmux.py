"""Verify prompt keys, resize, help and terminal restoration in isolated tmux.

Run: uv run python scripts/verify_keyboard_tmux.py
Requires a POSIX development checkout and tmux. Uses synthetic fixtures, a private
socket and temporary config/state. Never attaches to the user's tmux or agents.
Prints JSON; failures exit nonzero and include the captured terminal frame.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time


CHILD = '''import json, sys, termios
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from tests.test_inspector_layout import ManualFeed, prompt
from observatory.terminal_ui import ObservatoryApp
feed = ManualFeed()
feed.api.activity.extend([
    dict(prompt(i), detail=f'Recorded request body {i}\\n' + ('Instructions café 界.\\n' * 70))
    for i in range(1, 9)
])
feed.refresh()
app = ObservatoryApp(feed, persist_preferences=False)
before = termios.tcgetattr(0)
try:
    app.run()
finally:
    Path(sys.argv[2]).write_text(json.dumps({
        'return_code': app.return_code,
        'termios_restored': before == termios.tcgetattr(0),
    }))
'''


def main():
    argparse.ArgumentParser(description=__doc__).parse_args()
    if os.name != 'posix' or not shutil.which('tmux'):
        raise SystemExit('This optional developer check requires POSIX and tmux.')
    repo = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix='agenturm-keyboard-tmux-') as temporary:
        root = Path(temporary)
        socket, result, child = root/'tmux.sock', root/'result.json', root/'app.py'
        child.write_text(CHILD)
        checks = []

        def tmux(*args):
            return subprocess.check_output(
                ['tmux', '-S', str(socket), '-f', '/dev/null', *args], text=True)

        def capture():
            return tmux('capture-pane', '-p', '-t', 'keys:0.0')

        def until(predicate, label):
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                frame = capture()
                if predicate(frame):
                    checks.append(label)
                    return frame
                time.sleep(.1)
            raise AssertionError(label + '\n' + capture())

        def keys(*names):
            for name in names:
                tmux('send-keys', '-t', 'keys:0.0', name)
                time.sleep(.15)

        def request_number(frame):
            match = re.search(r'Request (\d+) of (\d+)', frame)
            return tuple(map(int, match.groups())) if match else None

        env = {**os.environ, 'XDG_CONFIG_HOME': str(root/'config'),
               'XDG_STATE_HOME': str(root/'state'), 'AGENTURM_UI_PREFERENCES': '0'}
        command = shlex.join([sys.executable, str(child), str(repo), str(result)])
        try:
            subprocess.run(['tmux', '-S', str(socket), '-f', '/dev/null',
                            'new-session', '-d', '-s', 'keys', '-x', '120', '-y', '32', command],
                           env=env, check=True)
            tmux('set-option', '-g', 'remain-on-exit', 'on')
            until(lambda s: 'Review migration' in s, 'agent list startup')
            keys('Enter')
            until(lambda s: '←→ Sections' in s, 'inspector tabs focus')
            keys('Right')
            frame = until(lambda s: '↑↓ Prompts' in s and request_number(s), 'prompt section')
            latest, total = request_number(frame)
            keys('Up')
            until(lambda s: '↑↓ Choose' in s and request_number(s) == (latest-1, total),
                  'Up from prompt tabs enters older request')
            keys('End')
            until(lambda s: '↑↓ Choose' in s and request_number(s) == (latest, total),
                  'wide prompt navigator')
            keys('Up')
            until(lambda s: request_number(s) == (latest-1, total), 'Up chooses older')
            keys('Down')
            until(lambda s: request_number(s) == (latest, total), 'Down chooses newer')
            keys('Home')
            until(lambda s: request_number(s) == (1, total), 'Home chooses first')
            keys('End')
            until(lambda s: request_number(s) == (total, total), 'End chooses latest')
            keys('Enter')
            until(lambda s: '↑↓ Scroll' in s, 'Enter reads')
            keys('C-Right')
            until(lambda s: 'Ctrl+R Review' in s, 'Ctrl+Right changes inspector section')
            keys('C-Left')
            until(lambda s: '↑↓ Choose' in s, 'Ctrl+Left returns to prompt navigator')
            keys('Right')
            until(lambda s: '↑↓ Scroll' in s, 'Right reads')
            keys('Left')
            until(lambda s: '↑↓ Choose' in s, 'Left returns to index')
            tmux('resize-window', '-t', 'keys:0', '-x', '80', '-y', '24')
            until(lambda s: '↑↓/Enter Choose' in s, 'compact navigator focus')
            keys('Up', 'Up', 'Enter')
            until(lambda s: '↑↓ Scroll' in s and request_number(s) == (total-1, total),
                  'compact arrows confirm and read')
            keys('Left', 'Enter', 'Enter')
            until(lambda s: '↑↓ Scroll' in s, 'unchanged compact choice confirms and reads')
            keys('F1')
            until(lambda s: 'Keyboard navigation' in s, 'F1 opens help')
            tmux('resize-window', '-t', 'keys:0', '-x', '60', '-y', '18')
            keys('Escape')
            until(lambda s: '↑↓ Scroll' in s, 'help returns to reader after resize')
            keys('Left')
            until(lambda s: '↑↓/Enter Choose' in s, 'compact navigator after help')
            keys('q')
            deadline = time.monotonic() + 15
            while not result.exists() and time.monotonic() < deadline:
                time.sleep(.1)
            assert result.exists(), 'Application did not exit\n' + capture()
            outcome = json.loads(result.read_text())
            assert outcome == {'return_code': 0, 'termios_restored': True}, outcome
            outcome.update(tmux=subprocess.check_output(['tmux', '-V'], text=True).strip(),
                           sizes=['120x32', '80x24', '60x18'], checks=checks,
                           steps=len(checks), passed=True)
            print(json.dumps(outcome, indent=2))
        finally:
            subprocess.run(['tmux', '-S', str(socket), 'kill-server'],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == '__main__':
    main()
