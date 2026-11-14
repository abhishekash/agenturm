"""Isolated native-reader + receiver + real-terminal monitoring soak (Linux/tmux).

Run from the checkout with its dependencies installed:
  PYTHONPATH=. .venv/bin/python scripts/soak_monitor.py --output /tmp/agenturm-soak-new

Default: one hour, one three-worker cycle every 30 seconds. No provider calls.
The output directory must not exist. It contains only generated native sources,
private receiver credentials, telemetry, tmux captures and measurements. Do not
publish the directory wholesale; results.json contains no credentials. The child
processes replace Path.home only within this harness and inherit a minimal env;
native discovery cannot reach the user's transcripts/configuration. Host process
inventory is real; project configuration discovery is restricted to the fixture.
"""
from __future__ import annotations

import argparse
from contextlib import ExitStack
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import socket
import sqlite3
import statistics
import subprocess
import sys
import time
import traceback
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from scripts.validation_support import (application_revision,bind_application,harness_manifest,
    isolated_env,load_harness,published_heartbeat,nearest_rank,disk_gate,create_owned_directory,clone_seed,storage_snapshot,
    fresh_schema_manifest,prepare_raw_clone,verify_named_indexes)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def append(path, value):
    with path.open('a') as stream:
        stream.write(json.dumps(value, separators=(',', ':')) + '\n')


def source_revision(app_root=None):
    root=Path(app_root or os.environ.get('AGENTURM_VALIDATION_APP_ROOT',ROOT)).resolve()
    application=application_revision(root);harness=harness_manifest()
    return {**application,'application_dirty':application['dirty'],
        'dirty':application['dirty'] or harness['revision']['dirty'],'harness':harness}


def seed_history(path, *, traces=5751, span_count=31000, log_count=41000):
    load_harness('benchmark_history.py').build(path,traces=traces,span_count=span_count,log_count=log_count)
    # The full corpus remains retained, but its newest outcome predates the
    # default completion inbox. Exact review receipts then cover new turns only.
    offset=2*86400*10**9
    with sqlite3.connect(path) as db:
        db.execute('UPDATE spans SET start_ns=start_ns-?,end_ns=end_ns-?',(offset,offset))
        db.execute('UPDATE logs SET time_ns=time_ns-?',(offset,))
        db.execute('UPDATE trace_activity SET last_ns=last_ns-?',(offset,))


def event(kind, payload, *, ago=0):
    return dict(type=kind, timestamp=datetime.fromtimestamp(
        time.time()-ago, timezone.utc).isoformat(), payload=payload)


def native(path, kind, **payload):
    append(path, event(kind, payload))


def child(mode, directory, port):
    base = Path(directory).resolve()
    # No HOME/CODEX_HOME override; only this isolated process sees the fixture
    # home through the platform API. No user transcript/config roots are read.
    with ExitStack() as stack:
        stack.enter_context(patch.object(Path, 'home', return_value=base/'fixture-home'))
        imported=bind_application(os.environ.get('AGENTURM_VALIDATION_APP_ROOT',ROOT))
        role=os.environ.get('AGENTURM_VALIDATION_ROLE',mode)
        append(base/'imports.jsonl',dict(role=role,pid=os.getpid(),application=imported,harness=harness_manifest()))
        if os.environ.get('AGENTURM_SOAK_EAGER_TLS')=='1':
            from observatory import http_transport
            import urllib.request
            def eager(request, *, timeout):
                return urllib.request.build_opener(http_transport.NoRedirect()).open(request,timeout=timeout)
            stack.enter_context(patch.object(http_transport,'urlopen',eager))
        from observatory import local_runtime
        original = local_runtime.project_directories
        def fixture_projects(*args, **kwargs):
            return {path for path in original(*args, **kwargs)
                    if Path(path).is_relative_to(base)}
        stack.enter_context(patch.object(local_runtime, 'project_directories', fixture_projects))
        if mode == 'actor':
            import ctypes
            ctypes.CDLL(None).prctl(15, b'codex', 0, 0, 0)
            files = [stack.enter_context((base/'fixture-home/.codex/sessions'/name).open())
                     for name in ('parent.jsonl', 'stale.jsonl')]
            while True:
                for stream in files:
                    stream.seek(0); stream.read(256)
                hashlib.pbkdf2_hmac('sha256', b'synthetic', b'soak', 3000)
                time.sleep(.5)
        if mode == 'receiver':
            from observatory.core import Store
            close = Store.close
            def observed_close(store):
                before=len(store._derived_pending)
                close(store)
                append(base/'receiver-exits.jsonl',dict(pending_before=before,pending_after=len(store._derived_pending)))
            stack.enter_context(patch.object(Store,'close',observed_close))
            from observatory.__main__ import main
            sys.argv = ['agenturm', '--db', str(base/'telemetry.db'), 'serve', '--port', str(port)]
            if os.environ.get('AGENTURM_SOAK_PROFILE')=='1':
                import cProfile
                profiler=cProfile.Profile();profiler.enable()
                try:main()
                finally:
                    profiler.disable();profiler.dump_stats(str(base/f'receiver-{os.getpid()}.prof'))
            else:main()
            return
        def notification(title,body):
            append(base/'notification-events.jsonl',dict(role=role,pid=os.getpid(),at=time.time(),title=title,body=body))
            return 'Desktop notification delivered'
        if mode=='watcher':
            import threading
            from observatory.attention_cli import run_watch
            from observatory.terminal import Api
            stop=threading.Event()
            for signum in (signal.SIGINT,signal.SIGTERM):signal.signal(signum,lambda *_:stop.set())
            token=(base/'state/agenturm'/f'receiver-{port}.token').read_text().strip()
            args=argparse.Namespace(endpoint=f'http://127.0.0.1:{port}',state=None,interval=1,desktop=True,once=False)
            with (base/'watcher-events.jsonl').open('a') as stream:
                return run_watch(args,api=Api(args.endpoint,token),stop=stop,notify=notification,stdout=stream)
        if mode not in {'ui','ui-observer'}:
            raise ValueError(mode)
        from observatory.terminal_ui import ObservatoryApp
        from observatory.ui_workflow_controller import WorkflowController
        from observatory.terminal import Api, Feed, _run_terminal_app
        telemetry = base/('ui-events.jsonl' if role=='ui' else 'ui-events-'+role+'.jsonl')
        if mode=='ui-observer':
            from observatory import ui_workflow
            stack.enter_context(patch.object(ui_workflow,'desktop_notify',notification))
        original_mount = ObservatoryApp.on_mount
        original_enqueue = WorkflowController._enqueue
        def enqueue(controller, job):
            accepted = not controller._closed
            result = original_enqueue(controller, job)
            if accepted and job[0] == 'review_completions' and job[1][0]:
                append(telemetry, dict(kind='accepted_reviews', ids=list(job[1][0]), at=time.time()))
            return result
        def mount(app):
            original_mount(app)
            if mode=='ui-observer':app.workflow_notify=True
            previous = [time.monotonic()]
            def heartbeat():
                now = time.monotonic()
                append(telemetry,{**published_heartbeat(app), 'interval':now-previous[0],
                    'focus':app.focused.id if app.focused else None,
                    'terminal_size':[app.size.width,app.size.height]})
                previous[0] = now
            app.set_interval(1, heartbeat)
        stack.enter_context(patch.object(WorkflowController, '_enqueue', enqueue))
        stack.enter_context(patch.object(ObservatoryApp, 'on_mount', mount))
        token = (base/'state/agenturm'/f'receiver-{port}.token').read_text().strip()
        feed = Feed(Api(f'http://127.0.0.1:{port}', token))
        profiler = None
        if os.environ.get('AGENTURM_SOAK_PROFILE') == '1':
            import cProfile
            profiler=cProfile.Profile();profiler.enable()
        app=ObservatoryApp(feed)
        try:
            code=_run_terminal_app(app)
            append(telemetry,dict(kind='exit',pid=os.getpid(),code=code,warning=app.workflow_controller.warning))
            return code
        finally:
            if profiler:
                profiler.disable();profiler.dump_stats(str(base/f'cockpit-{os.getpid()}.prof'))
            feed.close()


class Workload:
    def __init__(self, base):
        self.base = base
        self.paths = {}
        directory = base/'fixture-home/.codex/sessions'
        directory.mkdir(parents=True)
        for name in ('parent', 'child', 'stale'):
            path = directory/(name+'.jsonl'); self.paths[name] = path
            payload = dict(id='soak-'+name, cwd=str(base/'project'))
            if name == 'child':
                payload.update(parent_thread_id='soak-parent', source={'subagent': {
                    'thread_spawn': {'parent_thread_id': 'soak-parent', 'depth': 1}}})
            append(path, event('session_meta', payload, ago=3600 if name=='stale' else 0))
        append(self.paths['stale'], event('event_msg', dict(type='task_started', turn_id='stale-turn'), ago=3600))
        # File mtime must not masquerade as current session work.
        stamp=time.time()-3600;os.utime(self.paths['stale'], (stamp,stamp))
        (base/'project').mkdir()
        self.database=base/'opencode.db'
        with sqlite3.connect(self.database) as db:
            db.executescript('''
                PRAGMA journal_mode=WAL;
                CREATE TABLE project(id TEXT PRIMARY KEY,name TEXT,worktree TEXT);
                CREATE TABLE session(id TEXT PRIMARY KEY,project_id TEXT,title TEXT,directory TEXT,
                  time_created INTEGER,time_updated INTEGER,agent TEXT,model TEXT,tokens_input INTEGER,tokens_output INTEGER);
                CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT);
                CREATE TABLE part(id TEXT PRIMARY KEY,session_id TEXT,message_id TEXT,time_created INTEGER,data TEXT);
            ''')
            now=time.time_ns()//1_000_000
            db.execute('INSERT INTO project VALUES(?,?,?)', ('p','soak-project',str(base/'project')))
            db.execute('INSERT INTO session VALUES(?,?,?,?,?,?,?,?,?,?)',
                ('soak-opencode','p','Validate OpenCode turn boundaries',str(base/'project'),now,now,'build','model-synthetic',0,0))
        self.requests=[];self.completed=[];self.turns={}

    def phase(self, cycle, phase):
        for name in ('parent','child'):
            path=self.paths[name];turn=f'{name}-turn-{cycle}'
            if phase==0:
                native(path,'event_msg',type='task_started',turn_id=turn)
                native(path,'event_msg',type='item_completed',item=dict(type='UserMessage',id=f'{name}-prompt-{cycle}',
                    content=[dict(type='text',text=f'Validate {name} cycle {cycle}')]))
                self.turns[(name,cycle)]=turn
            elif phase==1 and name=='parent':
                request=f'approval-{cycle}'
                native(path,'event_msg',type='approval_request',request_id=request,prompt='Approve synthetic check')
                self.requests.append((cycle,request))
            elif phase==2:
                native(path,'event_msg',type='task_started',turn_id=turn)
                native(path,'event_msg',type='item_completed',item=dict(type='CommandExecution',id=f'check-{cycle}',
                    command=['/usr/bin/zsh','-lc','npm run check'],status='completed',aggregated_output=f'{name} check {cycle} passed',exit_code=0))
            elif phase==3:
                native(path,'event_msg',type='item_completed',item=dict(type='FileChange',id=f'edit-{cycle}',status='completed',
                    changes={f'src/{name}_{cycle}.py':dict(type='update',diff='+ synthetic correction')}))
                native(path,'token_usage_record',turn_id=turn,response_id=f'{name}-{cycle}',usage=dict(input_tokens=100+cycle,output_tokens=10,cached_input_tokens=50))
            elif phase==4:
                native(path,'event_msg',type='item_completed',item=dict(type='AgentMessage',id=f'answer-{cycle}',
                    phase='final_answer',content=[dict(type='Text',text=f'Validated {name} cycle {cycle}')]))
                native(path,'event_msg',type='task_complete',turn_id=turn)
                self.completed.append((name,cycle))
        now=time.time_ns()//1_000_000
        uid=f'open-user-{cycle}';mid=f'open-step-{cycle}'
        with sqlite3.connect(self.database) as db:
            if phase==0:
                for key,data in [(uid,dict(role='user',content=[dict(type='text',text=f'Validate OpenCode cycle {cycle}')])),
                    (mid,dict(role='assistant',parentID=uid,modelID='model-synthetic',time=dict(created=now),tokens=dict(input=30,output=5)))]:
                    stamp=now+(key==mid)
                    db.execute('INSERT INTO message VALUES(?,?,?,?,?)',(key,'soak-opencode',stamp,stamp,json.dumps(data)))
            elif phase==2:
                data=dict(type='tool',tool='bash',state=dict(status='completed',input=dict(command='pytest -q'),output=f'OpenCode cycle {cycle} passed',metadata=dict(exit=0),time=dict(start=now-10,end=now)))
                db.execute('INSERT INTO part VALUES(?,?,?,?,?)',(f'open-tool-{cycle}','soak-opencode',mid,now,json.dumps(data)))
            elif phase in (3,4):
                # Text-part-only streaming must be seen without touching the message row.
                data=dict(type='text',text=f'Validated OpenCode cycle {cycle}' if phase==4 else f'Checking OpenCode cycle {cycle}')
                db.execute('INSERT OR REPLACE INTO part VALUES(?,?,?,?,?)',(f'open-text-{cycle}','soak-opencode',mid,now,json.dumps(data)))
                if phase==4:
                    data=json.loads(db.execute('SELECT data FROM message WHERE id=?',(mid,)).fetchone()[0])
                    data.update(finish='stop',time=dict(created=now-1000,completed=now))
                    db.execute('UPDATE message SET data=?,time_updated=? WHERE id=?',(json.dumps(data),now,mid))
                    self.completed.append(('opencode',cycle))
            db.execute('UPDATE session SET time_updated=?',(now,))


def process_sample(pid):
    path=Path('/proc')/str(pid)
    try:
        fields=path.joinpath('stat').read_text().rsplit(')',1)[1].split()
        status=dict(line.split(':',1) for line in path.joinpath('status').read_text().splitlines() if ':' in line)
        return dict(pid=pid,cpu_seconds=(int(fields[11])+int(fields[12]))/os.sysconf('SC_CLK_TCK'),
                    start_ticks=int(fields[19]),
                    sampled_at=time.monotonic(),
                    rss_mib=int(status.get('VmRSS','0 kB').split()[0])/1024,
                    open_fds=len(list((path/'fd').iterdir())),threads=int(status.get('Threads','0').strip()),
                    peak_rss_mib=int(status.get('VmHWM','0 kB').split()[0])/1024)
    except (OSError,ValueError,IndexError):
        return None


def percentile(values, fraction):
    return nearest_rank(values,fraction)


def run(args):
    bind_application(getattr(args,'app_root',ROOT))
    from observatory.terminal import Api
    from observatory.local_agents import identity
    from observatory.review_cli import collect_review
    from observatory.workflow_evidence import ExecutionScope
    from observatory.attention import classify
    revision_start=source_revision(getattr(args,'app_root',ROOT))
    disk_gate(args.output,required_bytes=0,reserve_bytes=int(getattr(args,'minimum_free_mib',4096))*1024**2)
    base=args.output.resolve();create_owned_directory(base)
    workload=Workload(base)
    preparation_started=time.monotonic();seed=None;raw_preparation=None;expected_schema=None;index_check=None
    if getattr(args,'seed',None):
        seed=clone_seed(args.seed,base/'telemetry.db',reserve_bytes=int(args.minimum_free_mib)*1024**2)
        from observatory.core import Store
        expected_schema=fresh_schema_manifest(base,Store)
        raw_preparation=prepare_raw_clone(base,'telemetry.db',schema_meta_sql=expected_schema['schema_meta_sql'])
    elif args.history:
        seed_history(base/'telemetry.db')
    preparation_seconds=time.monotonic()-preparation_started
    channel='agenturm-soak-'+str(os.getpid())
    env=isolated_env(base,getattr(args,'app_root',ROOT))
    if args.profile_ui:env['AGENTURM_SOAK_PROFILE']='1'
    if args.eager_tls:env['AGENTURM_SOAK_EAGER_TLS']='1'
    with socket.socket() as sock:
        sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
    endpoint=f'http://127.0.0.1:{port}'
    command=[sys.executable,str(Path(__file__).resolve())]
    receiver=None;actor=None;streams=[];errors=[];samples=[];latencies={};verified=[];reconnects=[]
    expected_reviews=set();rendered_approvals=set();ui_pids=[];exit_statuses=[];approval_latencies=[]
    checked_receivers=set()
    def spawn(mode):
        log=(base/(mode+'.log')).open('a');streams.append(log)
        return subprocess.Popen([*command,'_child',mode,str(base),str(port),'app-server'],env=env,cwd=base/'project',stdout=log,stderr=subprocess.STDOUT)
    def tmux(*values, capture=False):
        result=subprocess.run(['tmux','-L',channel,'-f','/dev/null',*values],env=env,cwd=base/'project',text=True,capture_output=True,timeout=5)
        if result.returncode:raise RuntimeError('tmux: '+result.stderr)
        return result.stdout if capture else None
    def screen(label):
        text=tmux('capture-pane','-p','-t','monitor',capture=True)
        (base/(label+'.txt')).write_text(text)
        return text
    def keys(*values):
        # Separate Escape bytes from following keys; a terminal otherwise
        # correctly decodes the concatenated sequence as an Alt chord.
        for value in values:
            tmux('send-keys','-t','monitor',value);time.sleep(.25)
    def start_ui(compact=False):
        width,height=('60','18') if compact else ('120','36')
        # Multiple command arguments execute Python directly, without the user's
        # default shell or its startup files. This server reads no tmux config.
        tmux('new-session','-d','-s','monitor','-x',width,'-y',height,*command,'_child','ui',str(base),str(port))
        tmux('set-option','-t','monitor','remain-on-exit','on')
        pid=ui_pid();ui_pids.append(pid)
        # A freshly started compact cockpit may not have installed input yet.
        # Observe its published frames before sending workflow or quit keys.
        deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            telemetry=base/'ui-events.jsonl'
            if telemetry.exists():
                with telemetry.open('rb') as stream:
                    stream.seek(0,os.SEEK_END);size=stream.tell();stream.seek(max(0,size-128*1024))
                    lines=stream.read().splitlines()
                if size>128*1024:lines=lines[1:]
                beats=[]
                for line in lines:
                    try:row=json.loads(line)
                    except (ValueError,UnicodeDecodeError):continue
                    if (row.get('kind')=='heartbeat' and row.get('pid')==pid and
                            row.get('screen')=='ObservatoryScreen' and row.get('visible_agent_rows',0)>0 and
                            row.get('focus')=='agent-table' and row.get('terminal_size')==[int(width),int(height)] and
                            not row.get('error')):beats.append(row)
                if len(beats)>=3:return
            dead=tmux('display-message','-p','-t','monitor','#{pane_dead}',capture=True).strip()
            if dead=='1':raise RuntimeError('Cockpit exited before sustained published frames')
            time.sleep(.15)
        raise RuntimeError('Cockpit did not publish sustained input-ready frames')
    def ui_pid():
        return int(tmux('display-message','-p','-t','monitor','#{pane_pid}',capture=True).strip())
    def stop_ui():
        keys('Escape','Escape','q')
        deadline=time.monotonic()+8
        while time.monotonic()<deadline:
            dead,status,signum=tmux('display-message','-p','-t','monitor',
                '#{pane_dead}:#{pane_dead_status}:#{pane_dead_signal}',capture=True).strip().split(':')
            # PTY closure can be visible before waitpid supplies exit status.
            # Require an actual status; a signal or unknown status is no pass.
            if dead=='1' and (status or signum):
                exit_statuses.append(dict(role='cockpit',status=status,signal=signum))
                if status!='0' or signum:errors.append(f'Cockpit exited unsuccessfully: status={status}, signal={signum}')
                return
            time.sleep(.1)
        errors.append('cockpit did not finish graceful exit within eight seconds')
    def wait_ready(api, timeout=25):
        deadline=time.monotonic()+timeout
        while time.monotonic()<deadline:
            try:
                health=api.get('/api/health')
                if health.get('indexing',{}).get('ready') and health.get('local_agents',{}).get('initial_poll_complete'):return health
            except OSError:pass
            time.sleep(.2)
        raise RuntimeError('Receiver readiness deadline exceeded')
    def stop_receiver():
        nonlocal receiver
        if not receiver or receiver.pid in checked_receivers:return
        if receiver and receiver.poll() is None:
            receiver.send_signal(signal.SIGINT)
            try:receiver.wait(timeout=20)
            except subprocess.TimeoutExpired:
                receiver.kill();receiver.wait();errors.append('receiver required forced shutdown')
        checked_receivers.add(receiver.pid)
        exit_statuses.append(dict(role='receiver',status=receiver.returncode))
        if receiver.returncode!=0:errors.append(f'Receiver exited unsuccessfully: {receiver.returncode}')
    def request(api, path):
        before=time.monotonic()
        try:return api.get(path)
        finally:latencies.setdefault(path,[]).append((time.monotonic()-before)*1000)
    started=time.monotonic();last_phase=-1;mid_restart=False;receiver_restart=False
    try:
        receiver=spawn('receiver');actor=spawn('actor')
        token_file=base/'state/agenturm'/f'receiver-{port}.token'
        for _ in range(100):
            if token_file.exists():break
            if receiver.poll() is not None:raise RuntimeError('Receiver failed; see receiver.log')
            time.sleep(.1)
        api=Api(endpoint,token_file.read_text().strip());wait_ready(api,timeout=600 if args.history or getattr(args,'seed',None) else 25)
        initial_readiness_seconds=time.monotonic()-started
        if expected_schema:
            index_check=verify_named_indexes(base/'telemetry.db',expected_schema)
            if not index_check['equal']:raise RuntimeError('Initialized indexes differ from this source tree production schema')
        start_ui()
        # Timed workload begins after first receiver readiness.
        started=time.monotonic();next_sample=started;sample_interval=args.phase_seconds
        previous={};pending_cycle=None;next_phase=started
        while time.monotonic()-started<args.duration or last_phase%6!=5:
            now=time.monotonic();elapsed=now-started
            phase_index=last_phase+1
            if now>=next_phase:
                cycle,phase=divmod(phase_index,6)
                if phase_index!=last_phase+1:errors.append(f'workload skipped phases: {last_phase} -> {phase_index}')
                workload.phase(cycle,phase);last_phase=phase_index
                if phase==1:
                    pending_cycle=cycle
                    # Inspect the actual pending inbox, not just received data.
                    issued=time.monotonic();keys('w')
                    while 'Approval requested' not in screen(f'approval-{cycle}'):
                        if time.monotonic()-issued>=10:raise RuntimeError(f'Approval not rendered within ten seconds for cycle {cycle}')
                        time.sleep(.2)
                    approval_latencies.append((time.monotonic()-issued)*1000)
                    rendered_approvals.add(cycle);keys('Escape')
                if phase==5:
                    keys('Escape','Escape','1','w')
                    time.sleep(1.2)
                    text=screen(f'workflow-{cycle}')
                    if 'Shift+R' not in text:raise RuntimeError('Workflow did not open before review key')
                    keys('R');time.sleep(.4);keys('Escape')
                    for name in ('parent','child'):
                        # Match authoritative identity from the observed inventory.
                        health=api.get('/api/health')
                        observed=next(row for row in health['local_agents']['sessions'] if row['session_id']=='soak-'+name)
                        projection=collect_review(api,service='codex',agent_id=observed['agent_id'],session_id='soak-'+name,trace_id=observed['trace_id'],pages=5)
                        scope=ExecutionScope(**projection['scope'])
                        incidents=classify({**observed,'state':'Completed'},scope,{}, {},now=time.time())
                        expected_reviews.update(incident.id for incident in incidents if incident.kind=='completion')
                        if (len(projection['checks'])!=1 or len(projection['edits'])!=1
                            or projection['edits'][0]['path']!=f'src/{name}_{cycle}.py'
                            or projection['usage']['values']['input_tokens']!=100+cycle
                            or projection['request']['text']!=f'Validate {name} cycle {cycle}'):
                            errors.append(f'Codex reconstruction mismatch: {name} cycle {cycle}')
                        else:verified.append(f'{name}:{cycle}')
                    trace=identity('opencode-trace','soak-opencode',f'open-step-{cycle}')
                    rollup=api.get('/api/review-turn?trace_id='+trace)['turn']
                    owner=rollup.get('review_owner') or {}
                    projection=collect_review(api,service='opencode',agent_id=owner.get('agent_id'),session_id='soak-opencode',trace_id=trace,pages=5,whole_turn=True)
                    observed=next(row for row in api.get('/api/health')['local_agents']['sessions'] if row['session_id']=='soak-opencode')
                    scope=ExecutionScope(**projection['scope'])
                    expected_reviews.update(incident.id for incident in classify({**observed,'state':'Completed'},scope,{}, {},now=time.time()) if incident.kind=='completion')
                    if (len(projection['checks'])!=1 or projection['usage']['values']['input_tokens']!=30
                        or projection['request']['text']!=f'Validate OpenCode cycle {cycle}'
                        or projection['agent_update']['text']!=f'Validated OpenCode cycle {cycle}'):
                        errors.append(f'OpenCode reconstruction mismatch: cycle {cycle}')
                    else:verified.append(f'opencode:{cycle}')
                    if cycle%5==0:screen(f'cycle-{cycle}')
                next_phase=time.monotonic()+args.phase_seconds
            if now>=next_sample:
                disk_gate(base,reserve_bytes=int(getattr(args,'minimum_free_mib',4096))*1024**2)
                health=request(api,'/api/health');local=health.get('local_agents',{})
                if pending_cycle is not None:
                    row=next((row for row in local.get('sessions',[]) if row.get('session_id')=='soak-parent'),{})
                    expected=identity(row.get('source_id'),f'parent-turn-{pending_cycle}','request',f'approval-{pending_cycle}')
                    if row.get('request_id')==expected:
                        append(base/'observed-requests.jsonl',dict(cycle=pending_cycle,id=row['request_id']))
                        pending_cycle=None
                for path in ('/api/sessions','/api/activity?limit=50','/api/metrics','/api/mcp?limit=20'):
                    request(api,path)
                record=dict(elapsed=elapsed,backlog_bytes=local.get('backlog_bytes'),
                    opencode_backlog=local.get('opencode_diagnostics',{}).get('backlog_records'),
                    read_errors=local.get('read_errors'),schema_errors=local.get('schema_errors'),
                    pending_storage_records=sum((item.get('diagnostics') or {}).get('pending_storage_records',0) for item in local.get('sessions',())),
                    hierarchy_write_errors=(local.get('hierarchy') or {}).get('write_errors'),
                    opencode_read_errors=local.get('opencode_read_errors'),opencode_schema_errors=(local.get('opencode_diagnostics') or {}).get('schema_errors'),
                    indexing_ready=(health.get('indexing') or {}).get('ready'),
                    database_bytes=(base/'telemetry.db').stat().st_size,
                    wal_bytes=(base/'telemetry.db-wal').stat().st_size if (base/'telemetry.db-wal').exists() else 0,
                    processes={})
                for role,pid in [('receiver',receiver.pid),('cockpit',ui_pid()),('synthetic_daemon',actor.pid),('driver',os.getpid())]:
                    value=process_sample(pid)
                    if value:
                        old=previous.get(role)
                        if old and old[0]==(pid,value['start_ticks']):
                            value['sample_seconds']=value['sampled_at']-old[2]
                            value['cpu_delta_seconds']=value['cpu_seconds']-old[1]
                            value['cpu_percent_one_core']=value['cpu_delta_seconds']/value['sample_seconds']*100
                        previous[role]=((pid,value['start_ticks']),value['cpu_seconds'],value['sampled_at']);record['processes'][role]=value
                samples.append(record);append(base/'samples.jsonl',record)
                next_sample=now+sample_interval
            # Restart at phase 5 boundaries, never skip a pending input phase.
            if last_phase%6==5 and elapsed>args.duration*.33 and not receiver_restart:
                before=time.monotonic();old_id=health['receiver_id'];stop_receiver()
                time.sleep(8);receiver=spawn('receiver');new=wait_ready(api)
                if new['receiver_id']!=old_id:errors.append('Receiver identity changed on restart')
                reconnects.append(dict(kind='receiver',at=elapsed,seconds=time.monotonic()-before));receiver_restart=True
                next_phase=time.monotonic()+args.phase_seconds
            if last_phase%6==5 and elapsed>args.duration*.66 and not mid_restart:
                screen('before-cockpit-restart');stop_ui()
                tmux('kill-session','-t','monitor');start_ui(compact=True);mid_restart=True
                next_phase=time.monotonic()+args.phase_seconds
                reconnects.append(dict(kind='cockpit',at=elapsed))
            time.sleep(.15)
        screen('final');stop_ui()
        stop_receiver()
    except Exception as exc:
        (base/'failure.txt').write_text(traceback.format_exc())
        errors.append(f'{type(exc).__name__}: {exc}')
    except KeyboardInterrupt:
        errors.append('Interrupted before the configured soak completed')
    finally:
        stop_receiver()
        if actor and actor.poll() is None:actor.terminate();actor.wait(timeout=5)
        subprocess.run(['tmux','-L',channel,'-f','/dev/null','kill-server'],capture_output=True)
        for stream in streams:stream.close()
    duration=time.monotonic()-started
    ui_events=[json.loads(line) for line in (base/'ui-events.jsonl').read_text().splitlines()] if (base/'ui-events.jsonl').exists() else []
    accepted={identity for row in ui_events if row['kind']=='accepted_reviews' for identity in row['ids']}
    heartbeat=[row for row in ui_events if row['kind']=='heartbeat']
    observed_requests=[json.loads(line) for line in (base/'observed-requests.jsonl').read_text().splitlines()] if (base/'observed-requests.jsonl').exists() else []
    persisted=set()
    workflow=base/'state/agenturm/workflow.sqlite3'
    if workflow.exists():
        with sqlite3.connect(workflow) as db:
            persisted={row[0] for row in db.execute('SELECT id FROM workflow_acknowledgements_v1')}
    missing=accepted-persisted
    if missing:errors.append(f'{len(missing)} accepted reviews missing after shutdown')
    if not accepted:errors.append('No keyboard review acknowledgements were accepted')
    if len(expected_reviews)!=len(workload.completed):errors.append('Completion identities do not cover every generated outcome')
    if accepted!=expected_reviews:errors.append(f'Review acceptance differs from expected outcomes: missing={len(expected_reviews-accepted)}, extra={len(accepted-expected_reviews)}')
    if persisted!=expected_reviews:errors.append(f'Durable reviews differ from expected outcomes: missing={len(expected_reviews-persisted)}, extra={len(persisted-expected_reviews)}')
    if len(verified)<3:errors.append('No complete three-worker cycle reconstructed')
    if len(verified)!=len(workload.completed):errors.append('Not every completed turn was reconstructed')
    missing_requests={cycle for cycle,_ in workload.requests}-{row['cycle'] for row in observed_requests}
    if missing_requests:errors.append(f'Unobserved approval request cycles: {sorted(missing_requests)}')
    false_working=[row for row in heartbeat if any(item['session']=='soak-stale' and item['state'] in {'Working','Working (quiet)','Processing','Possibly stalled'} for item in row['states'])]
    if false_working:errors.append(f'Shared daemon falsely advanced stale session in {len(false_working)} observations')
    if not receiver_restart or not mid_restart:errors.append('Required receiver/cockpit restart did not occur')
    if len(heartbeat)<args.duration*.65:errors.append('Insufficient heartbeat coverage')
    for pid in ui_pids:
        pid_beats=[row for row in heartbeat if row['pid']==pid]
        if len(pid_beats)<3:errors.append(f'No sustained heartbeat for sampled cockpit process {pid}')
        if not any(any(item['session']=='soak-stale' for item in row['states']) for row in pid_beats):errors.append('Stale-session observation missing after cockpit start/restart')
    for role in ('receiver','cockpit','synthetic_daemon','driver'):
        if sum(role in row['processes'] for row in samples)<args.duration/args.phase_seconds*.5:errors.append(f'Insufficient {role} process samples')
    if any(any(row.get(key) for key in ('read_errors','schema_errors','pending_storage_records','hierarchy_write_errors','opencode_read_errors','opencode_schema_errors')) or row.get('indexing_ready') is not True for row in samples):
        errors.append('Source/storage/index readiness failures occurred')
    storage_after=storage_snapshot(base/'telemetry.db')
    if any(count for name,count in storage_after.get('rows',{}).items() if 'dirty' in name):errors.append('Derived/index queues remain after shutdown')
    exits=[json.loads(line) for line in (base/'receiver-exits.jsonl').read_text().splitlines()] if (base/'receiver-exits.jsonl').exists() else []
    if len(exits)!=2 or any(row['pending_after'] for row in exits):errors.append('Derived work was not confirmed drained at both receiver shutdowns')
    if len([row for row in exit_statuses if row['role']=='receiver'])!=2:errors.append('Two receiver exit statuses were not confirmed')
    request_ids={row['id'] for row in observed_requests}
    ui_requests={request for row in heartbeat for request in row.get('approval_requests',[]) if request}
    if request_ids-ui_requests:errors.append('Not every observed native approval appeared in the cockpit')
    if receiver_restart and (not any(row['error'] for row in heartbeat) or not heartbeat or heartbeat[-1]['error']):
        errors.append('Cockpit disconnect/recovery was not observed')
    if rendered_approvals!={cycle for cycle,_ in workload.requests}:errors.append('Not every approval had a rendered pending inbox')
    ui_exits=[row for row in ui_events if row['kind']=='exit']
    if len(ui_exits)!=2 or any(row['code']!=0 or row['warning'] for row in ui_exits):errors.append('Two successful warning-free cockpit shutdowns were not confirmed')
    revision_end=source_revision(getattr(args,'app_root',ROOT))
    if revision_start!=revision_end or revision_start['dirty']:errors.append('Measured source was dirty or changed during the run')
    resources={}
    for role in ('receiver','cockpit','synthetic_daemon','driver'):
        values=[row['processes'][role] for row in samples if role in row['processes']]
        cpus=[row['cpu_percent_one_core'] for row in values if 'cpu_percent_one_core' in row]
        resources[role]=dict(cpu_median=statistics.median(cpus) if cpus else None,cpu_p95=percentile(cpus,.95),
            cpu_mean_weighted=(sum(row.get('cpu_delta_seconds',0) for row in values)/sum(row.get('sample_seconds',0) for row in values)*100) if any(row.get('sample_seconds') for row in values) else None,
            rss_max_mib=max((row['rss_mib'] for row in values),default=0),peak_rss_mib=max((row['peak_rss_mib'] for row in values),default=0))
        segments=[]
        for pid in sorted({row['pid'] for row in values}):
            points=[(row['elapsed'],row['processes'][role]['rss_mib']) for row in samples
                    if role in row['processes'] and row['processes'][role]['pid']==pid]
            start=points[0][0];end=points[-1][0]
            warm=[rss for elapsed,rss in points if start+60<=elapsed<=start+300]
            tail=[rss for elapsed,rss in points if elapsed>=end-60]
            segments.append(dict(observed_seconds=end-start,
                warm_rss_median_mib=statistics.median(warm) if warm else None,
                last_minute_rss_median_mib=statistics.median(tail),
                warm_to_tail_delta_mib=statistics.median(tail)-statistics.median(warm) if warm else None))
        resources[role]['rss_segments']=segments
    result=dict(schema_version=1,source_start=revision_start,source_end=revision_end,
        host=dict(system=platform.system(),architecture=platform.machine(),python=platform.python_version(),logical_cpus=os.cpu_count()),
        configured_seconds=args.duration,phase_seconds=args.phase_seconds,elapsed_seconds=duration,eager_tls=args.eager_tls,profile_ui=args.profile_ui,
        active_workload_comparison='descriptive only: phases follow completion time, not fixed absolute cadence',
        common_seed=seed,raw_schema_preparation=raw_preparation,
        production_indexes=index_check,
        storage_after=storage_after,
        seeded_history=dict(traces=5751,spans=31000,logs=41000,days=35,newest_age_days=2) if args.history else None,
        fixture_preparation_seconds=preparation_seconds,initial_readiness_seconds=locals().get('initial_readiness_seconds'),
        completed_turns=len(workload.completed),verified_turns=len(verified),approval_requests=len(workload.requests),observed_approval_requests=len({row['cycle'] for row in observed_requests}),
        accepted_review_ids=len(accepted),persisted_review_ids=len(persisted),missing_review_ids=len(missing),
        expected_review_ids=len(expected_reviews),rendered_approval_inboxes=len(rendered_approvals),
        approval_visible_latency_ms=dict(median=statistics.median(approval_latencies) if approval_latencies else None,
            p95=percentile(approval_latencies,.95),max=max(approval_latencies,default=0)),
        shared_daemon_false_progress=len(false_working),heartbeat_samples=len(heartbeat),
        heartbeat_interval_p95=percentile([row['interval'] for row in heartbeat],.95),
        heartbeat_interval_max=max((row['interval'] for row in heartbeat),default=0),
        connection_error_observations=sum(bool(row['error']) for row in heartbeat),
        recovered_after_outage=bool(any(row['error'] for row in heartbeat) and heartbeat and not heartbeat[-1]['error']),
        resources=resources,latency_ms={key:dict(median=statistics.median(values),p95=percentile(values,.95),max=max(values)) for key,values in latencies.items()},
        reconnects=reconnects,receiver_shutdowns=exits,exit_statuses=exit_statuses,sample_count=len(samples),errors=errors,
        storage=dict(database_bytes=(base/'telemetry.db').stat().st_size,
            peak_database_plus_wal_bytes=max((row['database_bytes']+row['wal_bytes'] for row in samples),default=0),
            max_native_backlog_bytes=max((row.get('backlog_bytes') or 0 for row in samples),default=0),
            max_opencode_backlog_records=max((row.get('opencode_backlog') or 0 for row in samples),default=0)),
        limitations=['Synthetic native records; no live provider or agent runtime.','Host process inventory is real; project config reads restricted to synthetic project.',
            'Heartbeat and review-queue instrumentation add small overhead.','No daily-use or human reconstruction-pilot conclusion.'])
    write_json(base/'results.json',result);print(json.dumps(result,indent=2),flush=True)
    return 1 if errors else 0


def main():
    if len(sys.argv)>1 and sys.argv[1]=='_child':
        return child(sys.argv[2],sys.argv[3],int(sys.argv[4])) or 0
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,required=True,help='new private artifact directory')
    parser.add_argument('--duration',type=float,default=3600,help='seconds after receiver readiness')
    parser.add_argument('--phase-seconds',type=float,default=5,help='six phases per completed workload cycle')
    parser.add_argument('--profile-ui',action='store_true',help='diagnostic cProfile; do not compare its CPU timings to unprofiled runs')
    parser.add_argument('--eager-tls',action='store_true',help='diagnostic baseline: restore pre-fix per-request TLS handler initialization in the cockpit')
    parser.add_argument('--history',action='store_true',help='seed the existing synthetic 35-day / 31k-span / 41k-log fixture before timing the live monitor')
    parser.add_argument('--app-root',type=Path,default=ROOT,help='frozen application checkout; harness remains this script revision')
    parser.add_argument('--seed',type=Path,help='frozen common raw seed; copy/checkpoint occurs outside timing')
    parser.add_argument('--minimum-free-mib',type=int,default=4096,help='disk reserve; full runs default to 4 GiB')
    args=parser.parse_args()
    if args.duration<args.phase_seconds*6 or args.phase_seconds<2:parser.error('Use at least one cycle and phases >=2 seconds')
    if sys.platform!='linux' or not shutil.which('tmux'):parser.error('This measurement harness requires Linux /proc and tmux')
    if args.output.exists():parser.error('Output directory must not exist')
    # Coordinator reads are loopback-only too; do not inherit proxy routing for
    # the generated local token or synthetic prompt/check payloads.
    with patch('urllib.request.getproxies',return_value={}):
        return run(args)


if __name__=='__main__':
    raise SystemExit(main())
