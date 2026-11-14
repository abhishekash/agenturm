"""Isolated real-terminal idle budget (Linux/tmux), with optional retained history.

PYTHONPATH=. .venv/bin/python scripts/benchmark_monitor_idle.py --output /tmp/new-idle --history

Reuses the native soak's child isolation; no provider calls, personal transcripts
or user configuration. No workload, keyboard input or API probes run during the
timed idle interval. This is an idle budget, not the active-soak measurement.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import time
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from scripts.soak_monitor import Workload, process_sample, seed_history, source_revision
from scripts.validation_support import (bind_application,clone_seed,create_owned_directory,
    disk_gate,isolated_env,monitor_budgets,storage_snapshot,strict_heartbeat_errors)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--history',action='store_true')
    parser.add_argument('--seconds',type=float,default=300)
    parser.add_argument('--warmup',type=float,default=60)
    parser.add_argument('--app-root',type=Path,default=ROOT)
    parser.add_argument('--seed',type=Path,help='frozen common raw seed; prepared once, copied outside timing')
    parser.add_argument('--minimum-free-mib',type=int,default=4096)
    parser.add_argument('--expected-history-sessions',type=int,default=36)
    parser.add_argument('--smoke',action='store_true',help='short non-publishable fixture check, not a budget')
    parser.add_argument('--profile',action='store_true',help='diagnostic cProfile; resulting timings are not an idle budget')
    parser.add_argument('--allow-dirty',action='store_true',help='diagnostics only; mark results non-publishable')
    args=parser.parse_args()
    if any(not math.isfinite(v) or v<5 for v in (args.seconds,args.warmup)):
        parser.error('Use finite intervals of at least five seconds')
    if sys.platform!='linux' or not shutil.which('tmux'):parser.error('Linux /proc and tmux required')
    if args.output.exists():parser.error('Output directory must not exist')
    if args.minimum_free_mib<1 or args.expected_history_sessions<1:parser.error('Disk reserve/session count must be positive')
    if revision(args.app_root)['dirty'] and not args.allow_dirty:parser.error('Commit source before measuring; --allow-dirty is for non-publishable diagnostics')
    if args.seed:args.history=True
    with patch('urllib.request.getproxies',return_value={}):
        return measure(args)


def revision(app_root=ROOT):
    value=source_revision(app_root)
    value['harness_sha256']=hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    return value


def validate_idle(events, start, end, pid, receiver_id, history,minimum_history=36):
    """Cheap idle cannot pass by losing the receiver or freezing first data."""
    errors=[]
    rows=[row for row in events if row.get('kind')=='heartbeat' and start<=row.get('at',0)<=end]
    stamps=[start,*[row['at'] for row in rows],end]
    if not rows or max(b-a for a,b in zip(stamps,stamps[1:]))>2.5:
        errors.append('Timed cockpit heartbeat coverage is incomplete')
    expected={'soak-parent','soak-child','soak-stale','soak-opencode'}
    for row in rows:
        if row.get('harness_version')==2:errors.extend(strict_heartbeat_errors(row,receiver_id))
        if (row.get('pid')!=pid or row.get('receiver_id')!=receiver_id or
                row.get('error') or row.get('read_errors') or row.get('sessions_error') or
                not row.get('initial_poll_complete') or row.get('native_read_errors') or row.get('native_schema_errors') or
                row.get('opencode_read_errors') or any(row.get('opencode_diagnostics',{}).get(key,0) for key in ('read_errors','schema_errors')) or
                not 0<=row['at']-row.get('updated',0)<=6 or not expected.issubset(row.get('native_sessions',[])) or
                history and row.get('session_count',0)<minimum_history):
            errors.append('Timed cockpit lost fresh, correctly scoped fixture data');break
    return errors


def measure(args):
    bind_application(args.app_root)
    from observatory.terminal import Api
    reserve=args.minimum_free_mib*1024**2;disk=disk_gate(args.output,reserve_bytes=reserve)
    base=args.output.resolve();create_owned_directory(base)
    source=revision(args.app_root);workload=Workload(base);seed=None
    if args.seed:seed=clone_seed(args.seed,base/'telemetry.db',reserve_bytes=reserve)
    elif args.history:seed_history(base/'telemetry.db')
    before_storage=storage_snapshot(base/'telemetry.db')
    env=isolated_env(base,args.app_root)
    if args.profile:env['AGENTURM_SOAK_PROFILE']='1'
    with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
    channel='agenturm-idle-'+str(os.getpid())
    command=[sys.executable,str(ROOT/'scripts/soak_monitor.py'),'_child']
    def tmux(*values):
        result=subprocess.run(['tmux','-L',channel,'-f','/dev/null',*values],env=env,
            cwd=base/'project',text=True,capture_output=True,timeout=5,check=True)
        return result.stdout
    def pane_state():
        return tmux('display-message','-p','-t','monitor','#{pane_dead}:#{pane_dead_status}:#{pane_dead_signal}').strip().split(':')
    receiver=None;log=None;errors=[];samples=[];exit_status={};health={};ready_seconds=None
    idle_start=idle_end=None;ui=None;receiver_id=None
    try:
        log=(base/'receiver.log').open('w')
        started=time.monotonic()
        receiver=subprocess.Popen([*command,'receiver',str(base),str(port)],env=env,
            cwd=base/'project',stdout=log,stderr=subprocess.STDOUT)
        token=base/'state/agenturm'/f'receiver-{port}.token'
        deadline=started+600
        api=None
        while time.monotonic()<deadline:
            if receiver.poll() is not None:raise RuntimeError('Receiver exited before readiness')
            if token.exists():
                api=Api(f'http://127.0.0.1:{port}',token.read_text().strip())
                try:
                    health=api.get('/api/health')
                    if health.get('indexing',{}).get('ready') and health.get('local_agents',{}).get('initial_poll_complete'):break
                except OSError:pass
            time.sleep(.2)
        else:raise RuntimeError('Receiver readiness deadline exceeded')
        ready_seconds=time.monotonic()-started
        receiver_id=health['receiver_id']
        # Verify retained data before timing. Agents deliberately does not poll
        # the hidden MCP view; requiring its counter in idle heartbeats would
        # either reject a healthy cockpit or accidentally widen idle work.
        if args.history:
            if len(api.get('/api/sessions?limit=500').get('sessions',[]))<args.expected_history_sessions:
                raise RuntimeError('Retained session fixture is missing')
            if api.get('/api/mcp?limit=20').get('summary',{}).get('calls',0)<1:
                raise RuntimeError('Retained MCP fixture is missing')
        tmux('new-session','-d','-s','monitor','-x','120','-y','36',*command,'ui',str(base),str(port))
        tmux('set-option','-t','monitor','remain-on-exit','on')
        ui=int(tmux('display-message','-p','-t','monitor','#{pane_pid}').strip())
        useful_deadline=time.monotonic()+120
        while time.monotonic()<useful_deadline:
            events=[json.loads(line) for line in (base/'ui-events.jsonl').read_text().splitlines()] if (base/'ui-events.jsonl').exists() else []
            beats=[row for row in events if row.get('kind')=='heartbeat' and row.get('pid')==ui]
            if beats and not strict_heartbeat_errors(beats[-1],receiver_id) and len(beats[-1].get('states',[]))>=4:break
            time.sleep(.2)
        else:raise RuntimeError('Useful nonempty Agents frame did not arrive')
        useful_seconds=time.monotonic()-started
        warm_until=time.monotonic()+args.warmup
        while time.monotonic()<warm_until:time.sleep(.2)
        roles={'receiver':receiver.pid,'cockpit':ui,'driver':os.getpid()}
        # /proc sampling only: no receiver API or terminal activity in this window.
        started=time.monotonic();idle_start=time.time()
        while True:
            row={role:process_sample(pid) for role,pid in roles.items()}
            if any(value is None for value in row.values()):raise RuntimeError('A monitored process exited during idle')
            samples.append(row)
            if time.monotonic()-started>=args.seconds:break
            time.sleep(min(1,max(.01,args.seconds-(time.monotonic()-started))))
        idle_end=time.time()
        health=api.get('/api/health')
        (base/'final-health.json').write_text(json.dumps(health,indent=2)+'\n')
        (base/'final-screen.txt').write_text(tmux('capture-pane','-p','-t','monitor'))
        for key in ('Escape','Escape','q'):
            tmux('send-keys','-t','monitor',key);time.sleep(.25)
        deadline=time.monotonic()+10
        while time.monotonic()<deadline:
            dead,status,signum=pane_state()
            if dead=='1' and (status or signum):
                exit_status['cockpit']={'status':status,'signal':signum}
                if status!='0' or signum:errors.append('Cockpit did not exit normally')
                break
            time.sleep(.1)
        else:errors.append('Cockpit shutdown deadline exceeded')
    except Exception as exc:
        errors.append(type(exc).__name__+': '+str(exc))
    finally:
        if receiver is not None:
            if receiver.poll() is None:
                receiver.send_signal(signal.SIGINT)
                try:receiver.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    receiver.kill();receiver.wait();errors.append('Receiver required forced termination')
            exit_status['receiver']=receiver.returncode
            if receiver.returncode!=0:errors.append('Receiver did not exit normally')
        subprocess.run(['tmux','-L',channel,'-f','/dev/null','kill-server'],env=env,capture_output=True)
        if log is not None:log.close()
    budgets={};whole=None
    try:budgets,whole=monitor_budgets(samples)
    except ValueError as exc:errors.append(str(exc))
    events=[json.loads(line) for line in (base/'ui-events.jsonl').read_text().splitlines()] if (base/'ui-events.jsonl').exists() else []
    if idle_start is None or idle_end is None:errors.append('Idle window did not complete')
    else:
        errors.extend(validate_idle(events,idle_start,idle_end,ui,receiver_id,args.history,args.expected_history_sessions))
        if any(row.get('harness_version')!=2 for row in events if row.get('kind')=='heartbeat' and idle_start<=row.get('at',0)<=idle_end):errors.append('Missing v2 validation heartbeat')
    if not any(row.get('kind')=='exit' and row.get('code')==0 and not row.get('warning') for row in events):errors.append('No clean cockpit receipt drain observed')
    receiver_exits=[json.loads(line) for line in (base/'receiver-exits.jsonl').read_text().splitlines()] if (base/'receiver-exits.jsonl').exists() else []
    if not receiver_exits or any(row.get('pending_after') for row in receiver_exits):errors.append('Derived work did not drain')
    if source!=revision(args.app_root):errors.append('Source changed during measurement')
    if health.get('local_agents',{}).get('read_errors') or health.get('local_agents',{}).get('schema_errors'):
        errors.append('Native fixture read/schema errors')
    local=health.get('local_agents',{})
    if local.get('opencode_read_errors') or any(local.get('opencode_diagnostics',{}).get(key,0) for key in ('read_errors','schema_errors')):
        errors.append('OpenCode fixture read/schema errors')
    after_storage=storage_snapshot(base/'telemetry.db')
    if any(count for name,count in after_storage.get('rows',{}).items() if 'dirty' in name):errors.append('Derived/index queues remain after shutdown')
    result=dict(source=source,publishable=not source['dirty'] and not args.profile and not args.smoke and not errors,
        smoke=args.smoke,disk_headroom=disk,seed=seed,storage_before=before_storage,storage_after=after_storage,
        terminal_size='120x36',history=args.history,profiling=args.profile,warmup_seconds=args.warmup,
        initial_readiness_seconds=ready_seconds,first_useful_agents_seconds=locals().get('useful_seconds'),requested_idle_seconds=args.seconds,processes=budgets,
        whole_monitor=whole,
        whole_monitor_idle_cpu_one_core_percent=sum(budgets.get(role,{}).get('cpu_one_core_percent',0) for role in ('receiver','cockpit')) if budgets else None,
        no_timed_api_or_keyboard_work=True,no_provider_calls=True,exit_status=exit_status,
        native_read_errors=health.get('local_agents',{}).get('read_errors'),errors=errors)
    (base/'samples.json').write_text(json.dumps(samples,indent=2)+'\n')
    (base/'results.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result,indent=2));return 1 if errors else 0


if __name__=='__main__':raise SystemExit(main())
