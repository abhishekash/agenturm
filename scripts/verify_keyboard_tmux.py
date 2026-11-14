"""Verify prompt keys, resize, help and terminal restoration in isolated tmux.

Run: uv run python scripts/verify_keyboard_tmux.py
Requires a POSIX development checkout and tmux. Uses synthetic fixtures, a private
socket and temporary config/state. Never attaches to the user's tmux or agents.
Prints JSON; failures exit nonzero and include the captured terminal frame.
"""
import argparse
from contextlib import nullcontext
import json
import os
from pathlib import Path
import re
import signal
import shutil
import subprocess
import sys
import tempfile
import time
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from scripts.validation_support import isolated_env,disk_gate,create_owned_directory,harness_manifest,application_revision,file_hash


CHILD = '''import json, os, signal, sys, termios
from pathlib import Path
from unittest.mock import patch
sys.path.insert(0, sys.argv[1])
fixture_home=patch.object(Path,'home',return_value=Path(sys.argv[2]).parent/'fixture-home')
fixture_home.start()
from tests.test_inspector_layout import ManualFeed, prompt
from observatory.terminal_ui import ObservatoryApp
from observatory.terminal import _run_terminal_app
feed = ManualFeed()
feed.api.activity.extend([
    dict(prompt(i), detail=f'Recorded request body {i}\\n' + ('Instructions café 界.\\n' * 70))
    for i in range(1, 9)
])
feed.refresh()
app = ObservatoryApp(feed, persist_preferences=False)
if int(sys.argv[3]):
    original_mount=app.on_mount
    def startup_signal():
        original_mount()
        os.kill(os.getpid(),int(sys.argv[3]))
    app.on_mount=startup_signal
before = termios.tcgetattr(0)
code = None
try:
    code=_run_terminal_app(app)
finally:
    Path(sys.argv[2]).write_text(json.dumps({
        'return_code': code,
        'termios_restored': before == termios.tcgetattr(0),
        'module': __import__('observatory').__file__,
        'fixture_helper': __import__('tests.test_inspector_layout',fromlist=['x']).__file__,
    }))
    print('AGENTURM_TERMINAL_RESTORED',flush=True)
'''


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app-root',type=Path,default=ROOT)
    parser.add_argument('--minimum-free-mib',type=int,default=256)
    parser.add_argument('--output',type=Path,help='new private artifact directory; preserves frames/results')
    parser.add_argument('--shutdown',choices=('q','sigint','sigterm'),default='q')
    parser.add_argument('--signal-at',choices=('reader','startup'),default='reader')
    args=parser.parse_args()
    if os.name != 'posix' or not shutil.which('tmux'):
        raise SystemExit('This optional developer check requires POSIX and tmux.')
    if args.signal_at=='startup' and args.shutdown=='q':parser.error('Startup signal requires sigint or sigterm')
    repo = args.app_root.resolve()
    if args.minimum_free_mib<1:parser.error('Disk reserve must be positive')
    disk_gate(args.output or '/tmp',reserve_bytes=args.minimum_free_mib*1024**2)
    if args.output:
        create_owned_directory(args.output)
    context=nullcontext(str(args.output.resolve())) if args.output else tempfile.TemporaryDirectory(prefix='agenturm-keyboard-tmux-')
    manifest=dict(application=application_revision(repo),harness=harness_manifest())
    with context as temporary:
        root = Path(temporary)
        socket, result, child = root/'tmux.sock', root/'result.json', root/'app.py'
        child.write_text(CHILD)
        checks = []

        def tmux(*args):
            return subprocess.check_output(
                ['tmux', '-S', str(socket), '-f', '/dev/null', *args], text=True,cwd=root)

        def capture():
            frame=tmux('capture-pane', '-p', '-t', 'keys:0.0')
            with (root/'frames.jsonl').open('a') as stream:stream.write(json.dumps(dict(at=time.time(),text=frame))+'\n')
            return frame

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

        env=isolated_env(root,repo);env['AGENTURM_UI_PREFERENCES']='0'
        startup_signum=int(signal.SIGINT if args.shutdown=='sigint' else signal.SIGTERM) if args.signal_at=='startup' else 0
        command=[sys.executable,str(child),str(repo),str(result),str(startup_signum)]
        try:
            subprocess.run(['tmux', '-S', str(socket), '-f', '/dev/null',
                            'new-session', '-d', '-s', 'keys', '-x', '120', '-y', '36', *command],
                           env=env,cwd=root, check=True)
            tmux('set-option', '-g', 'remain-on-exit', 'on')
            if not startup_signum:
                until(lambda s:'Agents' in s and 'Show:' in s,'monitor startup')
                keys('C-p')
                tmux('send-keys','-l','-t','keys:0.0','Show History')
                keys('Enter')
                until(lambda s: 'Recorded request body 1' in s, 'agent list history fixture')
                tmux('resize-window','-t','keys:0','-x','50','-y','14')
                until(lambda s:'Terminal too small' in s,'explicit too-small frame')
                tmux('resize-window','-t','keys:0','-x','120','-y','36')
                until(lambda s:'Recorded request body 1' in s,'wide layout restored')
                keys('Escape','Enter')
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
                if args.shutdown=='q':keys('q')
                else:
                    pid=int(tmux('display-message','-p','-t','keys:0.0','#{pane_pid}').strip())
                    os.kill(pid,signal.SIGINT if args.shutdown=='sigint' else signal.SIGTERM)
            deadline = time.monotonic() + 15
            while not result.exists() and time.monotonic() < deadline:
                time.sleep(.1)
            assert result.exists(), 'Application did not exit\n' + capture()
            outcome = json.loads(result.read_text())
            expected={'q':0,'sigint':130,'sigterm':143}[args.shutdown]
            assert outcome['return_code']==expected and outcome['termios_restored'] is True,outcome
            until(lambda frame:'AGENTURM_TERMINAL_RESTORED' in frame,'terminal output after restoration')
            assert Path(outcome['module']).resolve().is_relative_to(repo),outcome
            helper=Path(outcome['fixture_helper']);outcome['fixture_helper_sha256']=file_hash(helper)
            assert manifest==dict(application=application_revision(repo),harness=harness_manifest()),'Application/harness changed'
            outcome.update(tmux=subprocess.check_output(['tmux', '-V'], text=True).strip(),
                           sizes=['120x36', '50x14', '80x24', '60x18'] if not startup_signum else ['120x36'], checks=checks,
                           steps=len(checks), passed=True,source=manifest,shutdown=args.shutdown,signal_at=args.signal_at)
            (root/'results.json').write_text(json.dumps(outcome,indent=2)+'\n')
            print(json.dumps(outcome, indent=2))
        except Exception as exc:
            (root/'failure.json').write_text(json.dumps(dict(error=repr(exc),source=manifest,checks=checks),indent=2)+'\n')
            raise
        finally:
            subprocess.run(['tmux', '-S', str(socket), 'kill-server'],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == '__main__':
    main()
