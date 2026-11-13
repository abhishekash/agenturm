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


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--history',action='store_true')
    parser.add_argument('--seconds',type=float,default=60)
    parser.add_argument('--warmup',type=float,default=15)
    parser.add_argument('--profile',action='store_true',help='diagnostic cProfile; resulting timings are not an idle budget')
    parser.add_argument('--allow-dirty',action='store_true',help='diagnostics only; mark results non-publishable')
    args=parser.parse_args()
    if any(not math.isfinite(v) or v<5 for v in (args.seconds,args.warmup)):
        parser.error('Use finite intervals of at least five seconds')
    if sys.platform!='linux' or not shutil.which('tmux'):parser.error('Linux /proc and tmux required')
    if args.output.exists():parser.error('Output directory must not exist')
    if revision()['dirty'] and not args.allow_dirty:parser.error('Commit source before measuring; --allow-dirty is for non-publishable diagnostics')
    with patch('urllib.request.getproxies',return_value={}):
        return measure(args)


def revision():
    value=source_revision()
    value['harness_sha256']=hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    value['untracked_code']=subprocess.check_output(['git','ls-files','--others','--exclude-standard','--','*.py','*.toml'],cwd=ROOT,text=True).splitlines()
    value['dirty']=value['dirty'] or bool(value['untracked_code'])
    return value


def validate_idle(events, start, end, pid, receiver_id, history):
    """Cheap idle cannot pass by losing the receiver or freezing first data."""
    errors=[]
    rows=[row for row in events if row.get('kind')=='heartbeat' and start<=row.get('at',0)<=end]
    stamps=[start,*[row['at'] for row in rows],end]
    if not rows or max(b-a for a,b in zip(stamps,stamps[1:]))>2.5:
        errors.append('Timed cockpit heartbeat coverage is incomplete')
    expected={'soak-parent','soak-child','soak-stale','soak-opencode'}
    for row in rows:
        if (row.get('pid')!=pid or row.get('receiver_id')!=receiver_id or
                row.get('error') or row.get('read_errors') or row.get('sessions_error') or
                not row.get('initial_poll_complete') or row.get('native_read_errors') or row.get('native_schema_errors') or
                not 0<=row['at']-row.get('updated',0)<=6 or not expected.issubset(row.get('native_sessions',[])) or
                history and (row.get('session_count',0)<36 or row.get('mcp_calls',0)<1)):
            errors.append('Timed cockpit lost fresh, correctly scoped fixture data');break
    return errors


def measure(args):
    from observatory.terminal import Api
    base=args.output.resolve();base.mkdir(mode=0o700)
    source=revision();workload=Workload(base)
    if args.history:seed_history(base/'telemetry.db')
    env={key:os.environ[key] for key in ('PATH','LANG','LC_ALL','LD_LIBRARY_PATH') if key in os.environ}
    env.update(XDG_STATE_HOME=str(base/'state'),XDG_CONFIG_HOME=str(base/'config'),
        XDG_DATA_HOME=str(base/'data'),OPENCODE_DB=str(workload.database),TERM='xterm-256color',
        GEMINI_CLI_SYSTEM_SETTINGS_PATH=str(base/'config/gemini-system.json'),PYTHONUNBUFFERED='1')
    if args.profile:env['AGENTURM_SOAK_PROFILE']='1'
    with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
    channel='agenturm-idle-'+str(os.getpid())
    command=[sys.executable,str(ROOT/'scripts/soak_monitor.py'),'_child']
    def tmux(*values):
        result=subprocess.run(['tmux','-L',channel,'-f','/dev/null',*values],env=env,
            text=True,capture_output=True,timeout=5,check=True)
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
        tmux('new-session','-d','-s','monitor','-x','120','-y','36',*command,'ui',str(base),str(port))
        tmux('set-option','-t','monitor','remain-on-exit','on')
        ui=int(tmux('display-message','-p','-t','monitor','#{pane_pid}').strip())
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
    budgets={}
    if len(samples)>1:
        for role in samples[0]:
            first,last=samples[0][role],samples[-1][role]
            seconds=last['sampled_at']-first['sampled_at']
            budgets[role]=dict(cpu_one_core_percent=(last['cpu_seconds']-first['cpu_seconds'])/seconds*100,
                sampled_seconds=seconds,peak_sampled_rss_mib=max(row[role]['rss_mib'] for row in samples))
    events=[json.loads(line) for line in (base/'ui-events.jsonl').read_text().splitlines()] if (base/'ui-events.jsonl').exists() else []
    if idle_start is None or idle_end is None:errors.append('Idle window did not complete')
    else:errors.extend(validate_idle(events,idle_start,idle_end,ui,receiver_id,args.history))
    if not any(row.get('kind')=='exit' and row.get('code')==0 and not row.get('warning') for row in events):errors.append('No clean cockpit receipt drain observed')
    receiver_exits=[json.loads(line) for line in (base/'receiver-exits.jsonl').read_text().splitlines()] if (base/'receiver-exits.jsonl').exists() else []
    if not receiver_exits or any(row.get('pending_after') for row in receiver_exits):errors.append('Derived work did not drain')
    if source!=revision():errors.append('Source changed during measurement')
    if health.get('local_agents',{}).get('read_errors') or health.get('local_agents',{}).get('schema_errors'):
        errors.append('Native fixture read/schema errors')
    result=dict(source=source,publishable=not source['dirty'] and not args.profile and not errors,
        history=args.history,profiling=args.profile,warmup_seconds=args.warmup,
        initial_readiness_seconds=ready_seconds,requested_idle_seconds=args.seconds,processes=budgets,
        whole_monitor_idle_cpu_one_core_percent=sum(budgets.get(role,{}).get('cpu_one_core_percent',0) for role in ('receiver','cockpit')) if budgets else None,
        no_timed_api_or_keyboard_work=True,no_provider_calls=True,exit_status=exit_status,
        native_read_errors=health.get('local_agents',{}).get('read_errors'),errors=errors)
    (base/'samples.json').write_text(json.dumps(samples,indent=2)+'\n')
    (base/'results.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result,indent=2));return 1 if errors else 0


if __name__=='__main__':raise SystemExit(main())
