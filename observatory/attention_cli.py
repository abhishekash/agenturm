"""Headless attention against an existing authenticated Agenturm receiver."""
from __future__ import annotations
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import sys
import threading
import time
from types import SimpleNamespace

from .attention_runner import AttentionRunner,desktop_notify,inventory_ready
from .workflow_store import WorkflowStore,WorkflowStoreError,default_path


def add_attention_parser(sub):
    parser=sub.add_parser('attention',help='observe durable attention without opening a cockpit')
    commands=parser.add_subparsers(dest='attention_command',required=True)
    watch=commands.add_parser('watch',help='watch an existing receiver; never launches an agent or approves a request')
    watch.add_argument('--endpoint',default=os.environ.get('OBSERVATORY_ENDPOINT','http://127.0.0.1:14318'))
    watch.add_argument('--state',type=Path,help='private workflow store shared with cockpits')
    watch.add_argument('--interval',type=float,default=2,help='poll interval in seconds (1–60; default2)')
    watch.add_argument('--desktop',action='store_true',help='opt in desktop notification attempts for new eligible incidents')
    watch.add_argument('--once',action='store_true',help='observe one bounded snapshot and exit; incomplete reads exit2')
    status=commands.add_parser('status',help='read durable attention records without contacting a receiver')
    status.add_argument('--state',type=Path)
    status.add_argument('--receiver',help='exact receiver namespace')
    from .native_lifecycle import add_native_parsers
    add_native_parsers(commands)
    return parser


def collect_snapshot(api):
    """Bounded reads; bind every response and final health to one receiver."""
    first=api.get('/api/health')
    receiver=first.get('receiver_id')
    if not isinstance(receiver,str) or not receiver:raise ValueError('Receiver identity is unavailable; update the receiver.')
    paths={'agents':'/api/connections','sessions':'/api/sessions?limit=500','activity':'/api/activity?limit=200'}
    values={};errors={}
    with ThreadPoolExecutor(max_workers=3,thread_name_prefix='agenturm-attention') as pool:
        pending={kind:pool.submit(api.get,path) for kind,path in paths.items()}
        for kind,future in pending.items():
            try:
                value=future.result()
                if not isinstance(value,dict) or value.get('receiver_id')!=receiver:raise ValueError('Receiver response identity changed')
                field='connections' if kind=='agents' else kind
                if not isinstance(value.get(field),list):raise ValueError('Receiver returned invalid '+kind)
                values[kind]=value[field]
            except Exception:errors[kind]='Read unavailable or receiver identity changed'
    last=api.get('/api/health')
    if last.get('receiver_id')!=receiver:raise ValueError('Receiver identity changed during attention read')
    now=time.time();stamp=last.get('observed_time_unix_nano')
    stamp=stamp if type(stamp) is int and 0<stamp<2**63 else 0
    return SimpleNamespace(receiver_id=receiver,receiver_time_ns=stamp,
        indexing_ready=(first.get('indexing') or {}).get('ready') is True and (last.get('indexing') or {}).get('ready') is True,
        local_status=last.get('local_agents') or {},agents=values.get('agents',[]),sessions=values.get('sessions',[]),
        activity=values.get('activity',[]),traces=[],updated=now,error='',read_errors=errors,
        read_updated={kind:now for kind in values},sessions_error=errors.get('sessions',''),sessions_updated=now if 'sessions' in values else 0)


def status_payload(records,*,receiver='',ready=None,warning='',error=''):
    pending=[]
    for record in records:
        if record.get('resolved_at') is not None or receiver and record.get('namespace')!=receiver:continue
        ref=record.get('execution') or {}
        pending.append({key:record.get(key) for key in ('id','namespace','kind','reason','source_time_ns','first_observed_time',
            'acknowledgement','delivery_status','delivery_attempts','attention_eligible','attention_eligibility_reason')} |
            {key:ref.get(key) for key in ('producer','agent_id','session_id','run_id','turn_id')})
    return dict(format='agenturm-attention-v1',receiver_id=receiver or None,ready=ready,pending=pending,
        warning=warning or None,error=error or None,note='Recorded requests are handled in their native agent terminal. Missing observations do not resolve pending requests.')


def run_watch(args,*,api=None,stop=None,notify=desktop_notify,items_for=None,stdout=None,stderr=None):
    from .terminal import Api,CockpitError
    stdout=stdout or sys.stdout;stderr=stderr or sys.stderr
    if not 1<=args.interval<=60:raise ValueError('--interval must be between1 and60 seconds')
    api=api or Api(args.endpoint);stop=stop or threading.Event()
    store=WorkflowStore(args.state);runner=AttentionRunner();runner.ledger.restore(store.load())
    if items_for is None:
        from .source_projection import attention_items
        items_for=attention_items
    previous=None
    while not stop.is_set():
        ready=False;error=''
        try:
            snapshot=collect_snapshot(api)
            snapshot.native_lifecycle=store.native_observations(snapshot.receiver_id)
            runner.update(items_for(snapshot),snapshot,{},endpoint=api.endpoint,store=store)
            runner.persist(store)
            ready=inventory_ready(snapshot)
            if args.desktop and ready:
                runner.deliver(store,runner.delivery_candidates(),notify=notify,stopped=stop.is_set)
            if not ready:error='Relevant inventory is incomplete; no baseline or notification delivery was accepted.'
        except (OSError,ValueError,CockpitError) as exc:
            error='Attention read unavailable: '+str(exc)
            # Keep durable pending records instead of presenting an empty success.
            print(error,file=stderr)
        records=store.load()
        payload=status_payload(records,receiver=getattr(runner.latest[1],'receiver_id','') if runner.latest else '',
            ready=ready,warning=runner.warning,error=error)
        encoded=json.dumps(payload,ensure_ascii=False,sort_keys=True)
        if args.once or encoded!=previous:print(encoded,file=stdout,flush=True);previous=encoded
        if args.once:return 0 if ready else 2
        stop.wait(args.interval)
    return 0


def run_attention_command(args):
    try:
        if args.attention_command=='status':
            path=args.state or default_path()
            records=WorkflowStore(path).load(args.receiver) if Path(path).exists() else ()
            payload=status_payload(records,receiver=args.receiver or '',ready=None)
            payload.update(state_exists=Path(path).exists(),live_read_performed=False,
                native_inputs=WorkflowStore(path).native_status(args.receiver) if Path(path).exists() else [])
            print(json.dumps(payload,ensure_ascii=False))
            return 0
        if args.attention_command=='watch':return run_watch(args)
        from .native_lifecycle import run_native_command
        return run_native_command(args)
    except KeyboardInterrupt:return 130
    except (OSError,ValueError,WorkflowStoreError) as exc:
        print('Agenturm attention: '+str(exc),file=sys.stderr);return 2
