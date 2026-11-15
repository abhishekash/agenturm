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
    disk_gate,isolated_env,monitor_budgets,storage_snapshot,strict_heartbeat_errors,
    fresh_schema_manifest,prepare_raw_clone,verify_named_indexes)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--history',action='store_true')
    parser.add_argument('--first-frame-only',action='store_true',help='separate early-UI startup observation; never an idle resource budget')
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
    if args.first_frame_only and not args.seed:parser.error('First-frame observations require an explicit frozen generated --seed')
    if args.first_frame_only and args.profile:parser.error('First-frame observation is not a profiling mode')
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


def idle_publishable(args,source,errors):
    """Short/diagnostic runs cannot become the registered 300s idle budget."""
    return bool(not errors and not source['dirty'] and not args.profile and
        not args.smoke and not args.allow_dirty and args.seconds>=300 and args.warmup>=60)


def useful_agents_heartbeat(row,receiver_id):
    """Published visible rows, not merely an internal source inventory."""
    return bool(not strict_heartbeat_errors(row,receiver_id) and len(row.get('states',[]))>=4 and
        type(row.get('visible_agent_rows'))is int and row['visible_agent_rows']>=1)


FIRST_FRAME_PRIVATE='PRIVATE_FIRST_FRAME_PAYLOAD'
FIRST_FRAME_SESSIONS={'soak-parent','soak-child','soak-stale','soak-opencode'}


def first_frame_kind(row,frame,*,pid,receiver,observed,private_markers=(FIRST_FRAME_PRIVATE,)):
    """Require both already-published metadata and a real terminal capture.

    This deliberately does not reuse coherent readiness for partial frames.
    Missing history is unknown, never an empty/successful historical view.
    """
    def finite(value):return type(value)in(int,float)and math.isfinite(value)
    if not isinstance(row,dict)or not isinstance(frame,str):return None
    if 'History indexing'in frame and any(marker and marker in frame for marker in private_markers):
        raise ValueError('Partial frame exposed synthetic source content')
    states=row.get('states');sessions=row.get('native_sessions')
    if not isinstance(states,list)or any(not isinstance(item,dict)for item in states)or not isinstance(sessions,list):return None
    if (row.get('harness_version')!=2 or row.get('pid')!=pid or
            row.get('receiver_id')!=receiver or row.get('display_receiver_id')!=receiver or
            not receiver or row.get('screen')!='ObservatoryScreen' or
            row.get('terminal_size')!=[120,36] or row.get('focus')!='agent-table' or
            type(row.get('visible_agent_rows'))is not int or row['visible_agent_rows']<1 or
            not finite(row.get('monotonic_at')) or not 0<=observed-row['monotonic_at']<=2.5 or
            row.get('error') or row.get('warning') or row.get('published_sources_clipped') or
            not FIRST_FRAME_SESSIONS.issubset(sessions) or
            not FIRST_FRAME_SESSIONS.issubset({item.get('session')for item in states}) or
            row.get('initial_poll_complete')is not True or 'Agents'not in frame):return None
    if row.get('native_bootstrap')is True and row.get('display_native_bootstrap')is True:
        if FIRST_FRAME_PRIVATE in frame:raise ValueError('Partial frame exposed synthetic source content')
        times=[row.get(key)for key in ('at','bootstrap_updated','display_bootstrap_updated')]
        if (any(not finite(row.get(key))or row[key]!=0 for key in ('updated','display_updated'))or row.get('indexing_ready')is not False or
                any(not finite(value)or value<=0 for value in times) or
                any(not 0<=times[0]-value<=6 for value in times[1:]) or
                'History indexing'not in frame or 'Native observations'not in frame or
                any('session '+name not in frame for name in ('soak-parent','soak-child'))):return None
        return 'native_partial'
    if (row.get('native_bootstrap')is False and row.get('display_native_bootstrap')is False and
            useful_agents_heartbeat(row,receiver) and 'History indexing'not in frame):return 'coherent'
    return None


def observe_first_frames(observe,confirm,*,started,timeout=600,clock=time.monotonic,pause=time.sleep):
    """Observe actual captures before querying full readiness, on one lifetime.

    Callbacks perform the production tmux capture and authenticated health read;
    tests replace only those IO boundaries, not this admission/state machine.
    """
    deadline=started+timeout;receiver=None;identity=None;partial=None;full=None
    while clock()<deadline:
        value=observe();now=clock()
        current=value['identity']
        if (not isinstance(current,tuple)or len(current)!=2 or
                any(type(part)is not int or part<=0 for part in current)):raise ValueError('Invalid cockpit process identity')
        if identity is None:identity=current
        if current!=identity:raise ValueError('Cockpit process identity changed')
        row=value.get('heartbeat')or {};candidate=row.get('receiver_id')
        if candidate:
            if not isinstance(candidate,str)or len(candidate)>512:raise ValueError('Invalid receiver identity')
            if receiver is not None and receiver!=candidate:raise ValueError('Receiver identity changed')
            receiver=candidate
        kind=first_frame_kind(row,value.get('frame'),pid=identity[0],receiver=receiver,observed=now,
            private_markers=value.get('private_markers',(FIRST_FRAME_PRIVATE,)))
        if kind:
            proof=dict(observed_seconds=now-started,heartbeat_seconds=row['monotonic_at']-started,
                pid=identity[0],start_ticks=identity[1],receiver_id=receiver,visible_agent_rows=row['visible_agent_rows'])
            if kind=='native_partial'and partial is None:
                partial=proof;value['retain']('first-native-partial-screen.txt',proof)
            if full is None:
                health=confirm(max(.01,deadline-clock()))
                if (health.get('receiver_id')!=receiver or health.get('ok')is not True or
                        health.get('indexing',{}).get('ready')is not True or health.get('indexing',{}).get('error') or
                        health.get('local_agents',{}).get('initial_poll_complete')is not True):
                    raise ValueError('Independent full readiness failed')
                full=clock()-started
            if kind=='coherent':
                # The health read can take time; obtain a new captured coherent
                # frame rather than returning the pre-health observation.
                if now-started<full:continue
                value['retain']('first-coherent-screen.txt',proof)
                return dict(receiver_id=receiver,first_native_partial=partial,
                    partial_observation='observed'if partial else'unobserved',
                    full_readiness_seconds=full,first_coherent=proof,
                    timing='Monotonic terminal capture upper bounds; one-second heartbeat observation granularity.')
        pause(.1)
    raise TimeoutError('No admissible coherent terminal frame before the startup deadline')


def measure_first_frame(args):
    """Separate instrumented early-UI workload; never a cold/idle comparison."""
    bind_application(args.app_root)
    from observatory.terminal import Api
    from scripts.soak_monitor import native,wait_ready,index_proof
    from scripts.validation_support import raw_manifest
    reserve=args.minimum_free_mib*1024**2;disk=disk_gate(args.output,reserve_bytes=reserve)
    base=args.output.resolve();create_owned_directory(base);source=revision(args.app_root)
    prepared=time.monotonic();workload=Workload(base)
    # Static fresh parent/child work makes the metadata view useful while its
    # source text sentinel must remain absent from every partial capture.
    for name in ('parent','child'):
        native(workload.paths[name],'event_msg',type='task_started',turn_id='first-frame-'+name)
        native(workload.paths[name],'event_msg',type='item_completed',item=dict(type='UserMessage',id='first-frame-'+name,
            content=[dict(type='text',text=FIRST_FRAME_PRIVATE)]))
    seed=raw=None;schema=None;indexes=None
    if args.seed:
        from observatory.core import Store
        seed=clone_seed(args.seed,base/'telemetry.db',reserve_bytes=reserve)
        schema=fresh_schema_manifest(base,Store)
        raw=prepare_raw_clone(base,'telemetry.db',schema_meta_sql=schema['schema_meta_sql'])
    elif args.history:seed_history(base/'telemetry.db')
    preparation_seconds=time.monotonic()-prepared
    env=isolated_env(base,args.app_root);channel='agenturm-first-frame-'+str(os.getpid())
    command=[sys.executable,str(ROOT/'scripts/soak_monitor.py'),'_child']
    def tmux(*values):
        return subprocess.run(['tmux','-L',channel,'-f','/dev/null',*values],env=env,cwd=base/'project',
            text=True,capture_output=True,timeout=5,check=True).stdout
    def events():
        path=base/'ui-events.jsonl'
        if not path.exists():return []
        with path.open('rb')as stream:
            stream.seek(0,2);size=stream.tell();stream.seek(max(0,size-128*1024));lines=stream.read().splitlines()
        if size>128*1024:lines=lines[1:]
        rows=[]
        for line in lines:
            try:rows.append(json.loads(line))
            except (ValueError,UnicodeDecodeError):continue
        return rows
    errors=[];exits={};receiver=log=None;ui=None;ui_identity=None;proof=None;health={};readiness=[];captures={}
    try:
        with socket.socket()as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
        log=(base/'receiver.log').open('w');started=time.monotonic()
        receiver=subprocess.Popen([*command,'receiver',str(base),str(port)],env=env,cwd=base/'project',stdout=log,stderr=subprocess.STDOUT)
        initial_receiver=process_sample(receiver.pid)
        if initial_receiver is None:raise RuntimeError('Receiver process is unavailable')
        token=base/'state/agenturm'/f'receiver-{port}.token'
        while not token.exists():
            if receiver.poll()is not None:raise RuntimeError('Receiver exited before token publication')
            if time.monotonic()-started>=600:raise TimeoutError('Receiver token deadline exceeded')
            time.sleep(.05)
        info=(base/'telemetry.db').stat();initial_file=(info.st_dev,info.st_ino)
        def same_file():
            info=(base/'telemetry.db').stat()
            if (info.st_dev,info.st_ino)!=initial_file:raise RuntimeError('Receiver file identity changed')
        api=Api(f'http://127.0.0.1:{port}',token.read_text().strip())
        # Intentionally before ANY full-readiness wait. Default measure() does
        # not enter this branch and still launches its UI after full readiness.
        tmux('new-session','-d','-s','monitor','-x','120','-y','36',*command,'ui',str(base),str(port))
        tmux('set-option','-t','monitor','remain-on-exit','on')
        ui=int(tmux('display-message','-p','-t','monitor','#{pane_pid}').strip())
        point=process_sample(ui)
        if point is None:raise RuntimeError('Cockpit process is unavailable')
        ui_identity=(ui,point['start_ticks'])
        def observe():
            same_file()
            if receiver.poll()is not None:raise RuntimeError('Receiver exited during frame observation')
            current_receiver=process_sample(receiver.pid);current=process_sample(ui)
            if (not current or not current_receiver or
                    current_receiver['start_ticks']!=initial_receiver['start_ticks']):raise RuntimeError('An owned process changed')
            pane,dead=tmux('display-message','-p','-t','monitor','#{pane_pid}:#{pane_dead}').strip().split(':')
            if int(pane)!=ui or dead!='0':raise RuntimeError('Cockpit pane exited or changed')
            if (ui,current['start_ticks'])!=ui_identity:raise RuntimeError('Cockpit process identity changed')
            rows=[row for row in events()if row.get('kind')=='heartbeat'and row.get('pid')==ui]
            frame=tmux('capture-pane','-p','-t','monitor')
            def retain(name,metadata):
                (base/name).write_text(frame);captures[name]=metadata
            return dict(identity=(ui,current['start_ticks']),heartbeat=rows[-1]if rows else {},frame=frame,
                retain=retain,private_markers=(FIRST_FRAME_PRIVATE,str(base/'project'),str(base/'fixture-home')))
        def confirm(timeout):
            same_file();value=wait_ready(api,timeout=timeout,observations=readiness);same_file();return value
        proof=observe_first_frames(observe,confirm,started=started)
        same_file()
        health=api.get('/api/health')
        same_file()
        local=health.get('local_agents')or {}
        if (health.get('receiver_id')!=proof['receiver_id']or health.get('ok')is not True or
                health.get('indexing',{}).get('ready')is not True or health.get('indexing',{}).get('error') or
                local.get('enabled')is not True or local.get('initial_poll_complete')is not True or
                any(local.get(key)for key in ('read_errors','schema_errors','opencode_read_errors'))or
                any((local.get('opencode_diagnostics')or {}).get(key)for key in ('read_errors','schema_errors','backlog_records'))or
                not FIRST_FRAME_SESSIONS.issubset({row.get('session_id')for row in local.get('sessions',[])if isinstance(row,dict)})):
            raise RuntimeError('Final readiness or namespace changed')
        final_proof=index_proof(base/'telemetry.db',proof['receiver_id'],health['receiver_id'])
        if not all(final_proof.get(key)is True for key in ('receiver_matches','health_matches','derived_marker_complete','file_unchanged')):
            raise RuntimeError('Final source proof is incomplete')
        if schema:indexes=verify_named_indexes(base/'telemetry.db',schema)
        if args.history:
            if len(api.get('/api/sessions?limit=500').get('sessions',[]))<args.expected_history_sessions:raise RuntimeError('Retained sessions missing')
            if api.get('/api/mcp?limit=20').get('summary',{}).get('calls',0)<1:raise RuntimeError('Retained MCP missing')
        for key in ('Escape','Escape','q'):tmux('send-keys','-t','monitor',key);time.sleep(.25)
        deadline=time.monotonic()+10
        while time.monotonic()<deadline:
            dead,status,signum=tmux('display-message','-p','-t','monitor','#{pane_dead}:#{pane_dead_status}:#{pane_dead_signal}').strip().split(':')
            if dead=='1'and(status or signum):
                exits['cockpit']=dict(status=status,signal=signum)
                if status!='0'or signum:raise RuntimeError('Cockpit exit failed')
                break
            time.sleep(.1)
        else:raise TimeoutError('Cockpit shutdown deadline exceeded')
    except Exception as exc:errors.append('First-frame observation failed: '+type(exc).__name__)
    finally:
        if receiver is not None:
            if receiver.poll()is None:
                receiver.send_signal(signal.SIGINT)
                try:receiver.wait(timeout=20)
                except subprocess.TimeoutExpired:receiver.kill();receiver.wait();errors.append('Forced receiver shutdown')
            exits['receiver']=receiver.returncode
            if receiver.returncode!=0:errors.append('Receiver exit failed')
        subprocess.run(['tmux','-L',channel,'-f','/dev/null','kill-server'],env=env,capture_output=True)
        if ui_identity:
            deadline=time.monotonic()+5
            while time.monotonic()<deadline:
                point=process_sample(ui)
                if point is None or (ui,point['start_ticks'])!=ui_identity:break
                time.sleep(.05)
            else:errors.append('Owned cockpit process survived cleanup')
        if log is not None:log.close()
    rows=events()
    if not any(row.get('kind')=='exit'and row.get('pid')==ui and row.get('code')==0 and not row.get('warning')for row in rows):errors.append('No clean cockpit receipt drain')
    path=base/'receiver-exits.jsonl'
    drained=[json.loads(line)for line in path.read_text().splitlines()]if path.exists()else []
    if len(drained)!=1 or drained[0].get('pending_after')!=0:errors.append('Receiver drain not confirmed')
    if source!=revision(args.app_root):errors.append('Source changed')
    if args.seed and raw_manifest(args.seed)!=raw['seed_after']:errors.append('Frozen seed changed')
    storage=storage_snapshot(base/'telemetry.db')
    if any(value for name,value in storage.get('rows',{}).items()if 'dirty'in name):errors.append('Queues remain after shutdown')
    valid=bool(proof and not errors and not source['dirty']and not args.profile and not args.allow_dirty and not args.smoke)
    result=dict(mode='first-frame-only',source=source,observation_valid=valid,publishable=False,
        idle_budget=False,startup_workload='Instrumented early UI; not comparable with default late-UI cold/idle timing.',
        retained_history_fixture=bool(seed and seed.get('counts')=={'spans':31000,'logs':41000,'trace_activity':5751}),
        proof=proof,captured_observations=captures,fixture_preparation_seconds=preparation_seconds,seed=seed,raw_schema_preparation=raw,
        production_indexes=indexes,disk_headroom=disk,readiness_observations=readiness,storage_after=storage,
        exit_status=exits,errors=errors,no_provider_calls=True)
    (base/'results.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result,indent=2));return 1 if errors else 0


def measure(args):
    if getattr(args,'first_frame_only',False):return measure_first_frame(args)
    bind_application(args.app_root)
    from observatory.terminal import Api
    reserve=args.minimum_free_mib*1024**2;disk=disk_gate(args.output,reserve_bytes=reserve)
    base=args.output.resolve();create_owned_directory(base)
    source=revision(args.app_root)
    preparation_started=time.monotonic();workload=Workload(base);seed=None;raw_preparation=None;expected_schema=None;index_check=None
    if args.seed:
        from observatory.core import Store
        seed=clone_seed(args.seed,base/'telemetry.db',reserve_bytes=reserve)
        expected_schema=fresh_schema_manifest(base,Store)
        raw_preparation=prepare_raw_clone(base,'telemetry.db',schema_meta_sql=expected_schema['schema_meta_sql'])
    elif args.history:seed_history(base/'telemetry.db')
    preparation_seconds=time.monotonic()-preparation_started
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
        if expected_schema:index_check=verify_named_indexes(base/'telemetry.db',expected_schema)
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
            if beats and useful_agents_heartbeat(beats[-1],receiver_id):break
            time.sleep(.2)
        else:raise RuntimeError('Useful nonempty Agents frame did not arrive')
        useful_seconds=time.monotonic()-started
        first_useful_visible_rows=beats[-1]['visible_agent_rows']
        (base/'first-useful-screen.txt').write_text(tmux('capture-pane','-p','-t','monitor'))
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
    result=dict(source=source,publishable=idle_publishable(args,source,errors),
        smoke=args.smoke,disk_headroom=disk,seed=seed,storage_before=before_storage,storage_after=after_storage,
        fixture_preparation_seconds=preparation_seconds,
        raw_schema_preparation=raw_preparation,production_indexes=index_check,
        terminal_size='120x36',history=args.history,profiling=args.profile,warmup_seconds=args.warmup,
        initial_readiness_seconds=ready_seconds,first_useful_agents_seconds=locals().get('useful_seconds'),requested_idle_seconds=args.seconds,processes=budgets,
        first_useful_visible_agent_rows=locals().get('first_useful_visible_rows'),
        whole_monitor=whole,
        whole_monitor_idle_cpu_one_core_percent=sum(budgets.get(role,{}).get('cpu_one_core_percent',0) for role in ('receiver','cockpit')) if budgets else None,
        no_timed_api_or_keyboard_work=True,no_provider_calls=True,exit_status=exit_status,
        native_read_errors=health.get('local_agents',{}).get('read_errors'),errors=errors)
    (base/'samples.json').write_text(json.dumps(samples,indent=2)+'\n')
    (base/'results.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result,indent=2));return 1 if errors else 0


if __name__=='__main__':raise SystemExit(main())
