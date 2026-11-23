"""Verify persisted Workflow retries and stale-proof rejection through the real terminal UI."""
import argparse
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--checkout', type=Path, default=Path(__file__).resolve().parents[1])
parser.add_argument('--bin-dir', type=Path, required=True)
options = parser.parse_args()
sys.path.insert(0, str(options.checkout / 'scripts'))
from validate_rust_default_cockpit import Cockpit

APP = options.bin_dir.resolve(strict=True) / 'agenturm'
SOURCE = 'harness.codex.workflow-retry'
SESSION = 'workflow-retry-session'
SECRET = 'PRIVATE_RETRY_BODY_NEVER_DISPLAYED_OR_NOTIFIED'
GENERIC = '--app-name=Agenturm -- Agenturm attention 1 recorded attention request(s). Open Agenturm to inspect and respond in your agent terminal.'


def fact(kind, turn, **fields):
    return {'type': 'event_msg', 'timestamp': datetime.now(timezone.utc).isoformat(),
            'payload': {'type': kind, 'thread_id': SESSION, 'turn_id': turn, **fields}}


def acceptance(width, height, failure):
    with tempfile.TemporaryDirectory(prefix='agenturm-workflow-retry-') as folder:
        root = Path(folder)
        db = root / 'history.sqlite'
        raw = root / 'codex.jsonl'
        catalog = root / 'catalog.json'
        config = root / 'sources.json'
        tools = root / 'bin'
        tools.mkdir()
        logfile = root / 'notification-argv'
        mode = root / 'notifier-mode'
        notifier = tools / 'notify-send'
        adapter = APP.with_name('agenturm-codex-harness')
        catalog.write_text(json.dumps({'version': 1, 'plugins': [{
            'plugin_id': 'example.rust-codex-harness', 'adapter': str(adapter),
            'sha256': hashlib.sha256(adapter.read_bytes()).hexdigest(), 'protocol': 1,
            'features': ['source-adapter/1', 'configured-root/1'],
            'observation_format': 'agenturm-observations/1',
        }]}))
        config.write_text(json.dumps({'version': 2, 'plugin_host': str(APP.with_name('agenturm-plugin-host')),
            'plugin_catalog': str(catalog), 'sources': [{'plugin_id': 'example.rust-codex-harness',
                'source_id': SOURCE, 'source_root': str(raw)}]}))
        for path in (catalog, config):
            path.chmod(0o600)
        env = {**os.environ, 'HOME': folder, 'XDG_STATE_HOME': str(root / 'state'),
               'XDG_CONFIG_HOME': str(root / 'config'), 'XDG_DATA_HOME': str(root / 'data'),
               'PATH': str(tools), 'TERM': 'xterm-256color', 'NO_COLOR': '1',
               'AGENTURM_TEST_NOTIFY_LOG': str(logfile), 'AGENTURM_TEST_NOTIFY_MODE': str(mode)}

        def run(*args):
            call = subprocess.run([APP, *args], cwd=root, env=env, capture_output=True,
                                  text=True, timeout=10)
            assert call.returncode == 0, (args, call.stdout, call.stderr)
            assert SECRET not in call.stdout and SECRET not in call.stderr
            return call.stdout

        def install(exit_code):
            mode.write_text(f'{exit_code}\n')
            notifier.write_text('#!/bin/sh\nprintf \'%s\\n\' "$*" >> "$AGENTURM_TEST_NOTIFY_LOG"\nread -r result < "$AGENTURM_TEST_NOTIFY_MODE"\nexit "$result"\n')
            notifier.chmod(0o700)

        def append(*rows):
            with raw.open('a') as stream:
                stream.writelines(json.dumps(row) + '\n' for row in rows)
            run('collect', '--config', config, '--db', db)

        def recorded():
            return logfile.read_text().splitlines() if logfile.exists() else []

        def poll():
            return json.loads(run('attention', 'watch', '--once', '--desktop', '--db', db))

        append({'type': 'session_meta', 'payload': {'id': SESSION, 'source': 'cli'}},
               fact('task_started', 'baseline'))
        if failure == 'failed':
            install(1)
        transcript = io.BytesIO()
        pty = Cockpit([APP, 'cockpit', '--db', db], env, transcript, width=width, height=height)

        def frame(label, *parts):
            return pty.frame(label, lambda state: SECRET not in state and all(
                part.casefold() in state.casefold() for part in parts))

        companion = None
        try:
            frame('ready cockpit', 'Agenturm command center', 'codex.workflow-retry session', 'Working')
            pty.send(b'w')
            frame('workflow list', 'Workflow', 'S settings')
            pty.send(b'S')
            frame('workflow desktop preference', 'Desktop notifications', 'Use watch configuration')
            pty.send(b'\r')
            pty.frame('explicit desktop enable', lambda state: any(
                'On' in line.partition('Desktop notifications')[2].split()
                for line in state.splitlines() if 'Desktop notifications' in line))
            pty.send(b'\x1b')
            frame('return to Workflow', 'Workflow', 'S settings')
            pty.send(b'p')
            frame('freeze first terminal selection', 'Auto-refresh paused')

            companion = Cockpit([APP, 'cockpit', '--db', db], env, io.BytesIO(), width=120, height=32)
            companion.frame('other cockpit ready', lambda state: 'codex.workflow-retry session' in state)
            companion.send(b'w')
            companion.frame('second workflow list', lambda state: 'S settings' in state)
            companion.send(b'p')
            companion.frame('freeze second terminal selection', lambda state: 'Auto-refresh paused' in state)

            assert poll().get('desktop_delivery') is None
            append(fact('task_started', 'retry-turn'), fact('approval_request', 'retry-turn',
                request_id='request-to-retry', message=SECRET))
            assert poll()['desktop_delivery'] == 'failed'
            assert recorded() == [GENERIC]
            for pane in (pty, companion):
                pane.send(b'R')
                pane.frame('both observe failed attempt one', lambda state: 'T retry' in state and 'Failed' in state and SECRET not in state)

            pty.send(b'T')
            frame('first monitor observes failed attempt two', 'Notification retry', 'Failed')
            assert recorded() == [GENERIC, GENERIC]
            companion.send(b'T')
            companion.frame('old attempt receipt rejected', lambda state: SECRET not in state and 'Retry needs' in state)
            assert recorded() == [GENERIC, GENERIC]

            install(0)
            pty.send(b'T')
            frame('current attempt receipt can be retried', 'Notification retry', 'Delivered')
            assert recorded() == [GENERIC, GENERIC, GENERIC]
            companion.send(b'R')
            companion.frame('other monitor refreshes actual delivered state', lambda state: 'Delivered' in state and 'T retry' not in state)
            assert poll().get('desktop_delivery') is None
            assert recorded() == [GENERIC, GENERIC, GENERIC]
            pty.quit()
            companion.quit()
            assert SECRET.encode() not in transcript.getvalue()
            print('PASS two actual monitors cannot retry the same failed receipt after the attempt changes', flush=True)
        except Exception:
            print(pty.screen.contents(), file=sys.stderr)
            if companion is not None:
                print(companion.screen.contents(), file=sys.stderr)
            raise
        finally:
            pty.close()
            if companion is not None:
                companion.close()


acceptance(120, 32, 'failed')
print('App SHA-256', hashlib.sha256(APP.read_bytes()).hexdigest())
