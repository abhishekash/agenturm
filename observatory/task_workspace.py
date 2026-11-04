"""Bounded, read-only Git observations for explicitly saved tasks."""
from __future__ import annotations

import hashlib
import os
from pathlib import Path
import re
import selectors
import signal
import subprocess
import threading
import time

MAX_DIFF_BYTES = 128 * 1024
MAX_FILES = 100
SNAPSHOT_SECONDS = 8


def display_text(value):
    """Keep captured terminal control characters inert in every renderer."""
    return ''.join(char if char in '\n\t' or ord(char) >= 32 and not 127 <= ord(char) <= 159
                   else f'\\x{ord(char):02x}' for char in str(value))


def _git(workspace, arguments, *, timeout=2, max_bytes=16384, config=()):
    env = {key: value for key, value in os.environ.items()
           if not key.startswith('GIT_')}
    # The empty protocol allowlist also covers older Git versions that ignore
    # GIT_NO_LAZY_FETCH. Missing promisor objects must stay missing during review.
    env.update(GIT_OPTIONAL_LOCKS='0', GIT_TERMINAL_PROMPT='0', GIT_PAGER='cat', LC_ALL='C',
               GIT_NO_LAZY_FETCH='1', GIT_ALLOW_PROTOCOL='', GIT_PROTOCOL_FROM_USER='0')
    command = ['git', '--no-optional-locks', '-c', 'core.fsmonitor=false',
               '-c', 'core.pager=cat', '-c', 'diff.external=', '-c', 'diff.relative=false']
    for setting in config:
        command.extend(('-c', setting))
    command.extend(('-C', str(workspace), *arguments))
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               stdin=subprocess.DEVNULL, env=env,
                               start_new_session=os.name == 'posix')
    buffers = [bytearray(), bytearray()]
    clipped = threading.Event()
    read_stop = threading.Event()
    deadline = time.monotonic() + max(.05, timeout)

    def stop():
        try:
            if os.name == 'posix':
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
        except ProcessLookupError:
            pass

    def drain(stream, target, limit):
        watcher = selectors.DefaultSelector() if os.name == 'posix' else None
        try:
            if watcher:
                watcher.register(stream, selectors.EVENT_READ)
            while not read_stop.is_set():
                if watcher and not watcher.select(.05):
                    continue
                chunk = os.read(stream.fileno(), 8192)
                if not chunk:
                    break
                remaining = max(0, limit - len(target))
                target.extend(chunk[:remaining])
                if len(chunk) > remaining:
                    clipped.set()
                    stop()
                    break
        finally:
            if watcher:
                watcher.close()
            stream.close()

    readers = [threading.Thread(target=drain, args=(process.stdout, buffers[0], max_bytes), daemon=True, name='agenturm-git-stdout'),
               threading.Thread(target=drain, args=(process.stderr, buffers[1], 4096), daemon=True, name='agenturm-git-stderr')]
    for reader in readers:
        reader.start()
    timed_out = False
    try:
        process.wait(timeout=max(.001, deadline - time.monotonic()))
    except subprocess.TimeoutExpired:
        timed_out = True
        stop()
        process.wait(timeout=1)
    except BaseException:
        stop()
        process.wait(timeout=1)
        raise
    finally:
        for reader in readers:
            reader.join(timeout=max(0, deadline - time.monotonic()))
        if any(reader.is_alive() for reader in readers):
            timed_out = True
        # A completed parent can leave descendants holding the pipes open.
        # Stop the whole group, then release readers even if a pipe never closes.
        stop()
        read_stop.set()
        for reader in readers:
            reader.join(timeout=.5)
    return {'stdout': bytes(buffers[0]), 'stderr': display_text(buffers[1].decode('utf-8', 'replace')),
            'returncode': process.returncode, 'truncated': clipped.is_set(), 'timed_out': timed_out}


def capture_workspace(workspace, previous=None):
    """Capture changes relative to the task's first successful Git observation.

    Files, Git refs, the index, hooks, and filters are never written or executed.
    Untracked contents are deliberately not captured.
    """
    previous = previous or {}
    started = time.time_ns()
    result = {'captured_ns': started, 'workspace': workspace, 'available': False,
              'complete': False, 'files': [], 'diff': '', 'stat': '', 'warnings': [],
              'scope': 'Workspace changes relative to the saved base, including pre-existing or concurrent work. '
                       'This does not establish which agent made a change. Untracked contents are not captured.',
              'checks': 'Recorded checks are not verified against this workspace snapshot.'}
    # A transient missing path/Git executable must not silently reset the saved base.
    if previous.get('workspace') == workspace:
        for field in ('repo_root', 'base_commit'):
            if previous.get(field):
                result[field] = previous[field]
    if not workspace:
        result['error'] = 'Set a workspace directory to capture Git changes.'
        return result
    path = Path(workspace).expanduser()
    if not path.is_dir():
        result['error'] = 'Workspace directory is unavailable. Update the task workspace and try again.'
        return result
    deadline = time.monotonic() + SNAPSHOT_SECONDS
    disabled_filters = []

    def run(args, limit=16384, required=True):
        left = deadline - time.monotonic()
        if left <= 0:
            raise TimeoutError('Git capture reached its time limit; refresh to try again.')
        answer = _git(path, args, timeout=min(2, left), max_bytes=limit, config=disabled_filters)
        if answer['timed_out']:
            raise TimeoutError('Git capture timed out; refresh to try again.')
        if answer['truncated']:
            result['warnings'].append('Git output exceeded the capture limit; this snapshot is partial.')
        elif answer['returncode'] and required:
            raise ValueError(answer['stderr'].strip() or 'Git could not read this workspace.')
        return answer

    try:
        root = run(['rev-parse', '--show-toplevel'])['stdout'].decode('utf-8', 'replace').strip()
        path = Path(root)
        filters = run(['config', '--name-only', '--get-regexp', r'^filter\..*\.(clean|smudge|process|required)$'],
                      32768, required=False)
        if filters['truncated']:
            raise ValueError('Too many Git filter settings to disable safely; workspace capture was stopped.')
        for key in filters['stdout'].decode('utf-8', 'replace').splitlines():
            if not re.fullmatch(r'filter\.[^\x00-\x1f=]+\.(clean|smudge|process|required)', key):
                raise ValueError('A Git filter setting could not be disabled safely; workspace capture was stopped.')
            disabled_filters.append(key + ('=false' if key.endswith('.required') else '='))
        head = run(['rev-parse', '--verify', 'HEAD'])['stdout'].decode('ascii', 'replace').strip()
        if not re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', head):
            raise ValueError('Workspace has no readable Git commit yet. Create a commit, then refresh.')
        branch = run(['symbolic-ref', '--quiet', '--short', 'HEAD'], required=False)['stdout'].decode('utf-8', 'replace').strip()
        old_base = previous.get('base_commit') if previous.get('repo_root') == root else None
        base = old_base if isinstance(old_base, str) and re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', old_base) else head
        result.update(available=True, repo_root=root, head=head, branch=display_text(branch) or '(detached)', base_commit=base)
        run(['cat-file', '-e', base + '^{commit}'])
        status = run(['status', '--porcelain=v1', '-z', '--untracked-files=normal', '--ignore-submodules=all'], 32768)
        # A capped NUL stream may end halfway through a pathname; omit that tail.
        records = iter(status['stdout'].split(b'\0')[:-1])
        files = []
        known = set()
        count = 0
        for record in records:
            if len(record) < 4:
                continue
            state = record[:2].decode('ascii', 'replace')
            entry = {'status': state, 'path': display_text(record[3:].decode('utf-8', 'replace'))}
            if 'R' in state or 'C' in state:
                entry['from_path'] = display_text(next(records, b'').decode('utf-8', 'replace'))
            count += 1
            known.add(entry['path'])
            if len(files) < MAX_FILES:
                files.append(entry)
        common = ['diff', '--no-ext-diff', '--no-textconv', '--ignore-submodules=all', '--no-color']
        stat = run([*common, '--stat', base, '--'], 16384)
        diff = run([*common, base, '--'], MAX_DIFF_BYTES)
        # Include committed changes since the pinned base, which status alone omits.
        changed = run([*common, '--name-only', '-z', base, '--'], 32768)
        for raw in changed['stdout'].split(b'\0')[:-1]:
            name = display_text(raw.decode('utf-8', 'replace'))
            if name and name not in known:
                known.add(name)
                count += 1
                if len(files) < MAX_FILES:
                    files.append({'status': 'Δ', 'path': name})
        if count > MAX_FILES:
            result['warnings'].append(f'Only the first {MAX_FILES} changed paths are shown.')
        result.update(files=files, file_count=count,
                      file_count_complete=not status['truncated'] and not changed['truncated'],
                      diff=display_text(diff['stdout'].decode('utf-8', 'replace')),
                      stat=display_text(stat['stdout'].decode('utf-8', 'replace')),
                      complete=not result['warnings'])
        if result['complete']:
            result['fingerprint'] = hashlib.sha256(head.encode() + b'\0' + status['stdout'] + b'\0' + diff['stdout']).hexdigest()
        result['fingerprint_scope'] = 'Tracked Git diff and status paths only; excludes untracked contents and ignored files.'
        result['warnings'].append('Files are observed sequentially; active writers may change the workspace during or after capture.')
    except (OSError, ValueError, TimeoutError) as exc:
        result['error'] = display_text(str(exc))[:1200]
        result['complete'] = False
    result['finished_ns'] = time.time_ns()
    return result
