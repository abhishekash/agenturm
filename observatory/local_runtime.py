"""Local host and known agent process metadata; no environment or arguments."""
from __future__ import annotations

import os
import platform
import socket
import time
from itertools import islice
from pathlib import Path


def project_directories(names, proc_root=Path('/proc'), uid=None):
    """Return working directories for selected same-user processes, for local discovery only."""
    user_id = os.getuid() if uid is None else uid
    wanted = {str(name).casefold() for name in names}
    result = set()
    if not Path(proc_root).is_dir():
        return result
    for proc in Path(proc_root).iterdir():
        if not proc.name.isdigit():
            continue
        try:
            if proc.stat().st_uid != user_id or (proc / 'comm').read_text().strip().casefold() not in wanted:
                continue
            cwd = Path(os.readlink(proc / 'cwd'))
            if cwd.is_absolute() and cwd.is_dir():
                result.add(str(cwd))
        except OSError:
            continue
    return result


def _work(proc, ticks):
    stat = (proc / 'stat').read_text().rsplit(')', 1)[1].split()
    io = {}
    try:
        io = dict(line.split(':', 1) for line in (proc / 'io').read_text().splitlines())
    except OSError:
        pass
    return ((int(stat[11]) + int(stat[12])) / ticks,
            sum(int(io.get(key, 0)) for key in ('rchar', 'wchar')), stat[19])


def snapshot(session_paths=None, proc_root=Path('/proc'), uid=None):
    """Match only exact open session files, never commands, environments, or cwd guesses."""
    proc_root = Path(proc_root)
    user_id = os.getuid() if uid is None else uid
    paths = {str(Path(path).resolve()): source for path, source in (session_paths or {}).items()}
    result = {"host": socket.gethostname(), "os": platform.system(), "architecture": platform.machine(),
              "collector_pid": os.getpid(), "processes": [], "sessions": {},
              "process_inventory_available": proc_root.is_dir()}
    try:
        result["load_average"] = list(os.getloadavg())
    except (OSError, AttributeError):
        pass
    if not result["process_inventory_available"]:
        return result
    allowed = {"codex", "claude", "claude-code", "opencode", "pi", "gemini", "ollama", "cursor"}
    ticks = os.sysconf('SC_CLK_TCK')
    for proc in proc_root.iterdir():
        if not proc.name.isdigit():
            continue
        try:
            if proc.stat().st_uid != user_id:
                continue
            name = (proc/'comm').read_text().strip()
            if name.lower() not in allowed:
                continue
            fields = {}
            for line in (proc/'status').read_text().splitlines():
                key, _, value = line.partition(':')
                fields[key] = value.strip()
            cpu, io, started = _work(proc, ticks)
            info = {"pid": int(proc.name), "name": name, "state": fields.get('State', ''),
                    "parent_pid": int(fields.get('PPid', 0)),
                    "memory_rss_kib": int(fields.get('VmRSS', '0 kB').split()[0]),
                    "cpu_seconds": round(cpu, 2)}
            try:
                info['project'] = Path(os.readlink(proc/'cwd')).name
            except OSError:
                pass
            result['processes'].append(info)
            if paths and not str(info['state']).startswith(('Z', 'X')):
                matched = set()
                try:
                    for fd in islice((proc / 'fd').iterdir(), 1024):
                        try:
                            source = paths.get(os.readlink(fd))
                            if source:
                                matched.add(source)
                        except OSError:
                            continue
                except OSError:
                    pass
                # Include tool subprocess work. A sleeping agent can be waiting
                # on a compiler; do not mislabel that session as stuck.
                pending, seen = [proc.name], set()
                while matched and pending and len(seen) < 128:
                    pid = pending.pop()
                    if pid in seen:
                        continue
                    seen.add(pid)
                    child = proc_root / pid
                    try:
                        if child.stat().st_uid != user_id:
                            continue
                        if pid != proc.name:
                            child_cpu, child_io, _ = _work(child, ticks)
                            cpu += child_cpu
                            io += child_io
                        pending.extend((child / 'task' / pid / 'children').read_text().split())
                    except (OSError, ValueError, IndexError):
                        continue
                for source in matched:
                    sample = result['sessions'].setdefault(source, {'pids': [], 'identity': [], 'cpu_seconds': 0, 'io_chars': 0})
                    sample['pids'].append(int(proc.name))
                    sample['identity'].append((int(proc.name), started))
                    sample['cpu_seconds'] += cpu
                    sample['io_chars'] += io
        except (OSError, ValueError, IndexError):
            continue
    result['processes'] = sorted(result['processes'], key=lambda item:item['pid'])[:50]
    for sample in result['sessions'].values():
        sample['pids'].sort()
        sample['identity'].sort()
    result['observed_at_ns'] = time.time_ns()
    return result
