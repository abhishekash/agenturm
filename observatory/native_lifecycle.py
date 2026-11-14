"""Explicit, metadata-only native lifecycle protocol and durable send queue.

A binding is configured against recorded receiver identities. Sender JSON cannot
choose another actor, session or turn. Native hooks never return approval JSON.
"""
from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import stat
import sys
import time
from urllib.parse import urlencode

from .workflow_store import WorkflowStore,WorkflowStoreError

MAX_BYTES=8192
MAX_EVENTS=1000
KINDS={'approval_requested','reply_requested','request_resolved','turn_started','turn_completed','turn_interrupted'}
IDENTITY=('receiver_id','service','agent_id','session_id','trace_id','turn_id','source_id','parent_agent_id')
BINDING_FIELDS={'schema','sender_id','endpoint','adapter','native_turn_id','source_clock','follow_recorded_turns',*IDENTITY}
EVENT_FIELDS={'schema','sender_id','event_id','sequence','kind','request_id','time_unix_nano',*IDENTITY}


class UncertainLifecycle(ValueError):pass


def fingerprint(value):return hashlib.sha256(json.dumps(value,sort_keys=True,separators=(',',':')).encode()).hexdigest()


def identifier(value,field,*,empty=False):
    if empty and value=='':return value
    if not isinstance(value,str) or not re.fullmatch(r'[A-Za-z0-9_.:-]{1,512}',value):raise UncertainLifecycle('Invalid '+field+' identity')
    return value


def bounded_json(raw):
    if not isinstance(raw,(str,bytes)) or len(raw)>MAX_BYTES:raise UncertainLifecycle('Native input exceeds the 8 KiB limit')
    if isinstance(raw,str):raw=raw.encode()
    if len(raw)>MAX_BYTES:raise UncertainLifecycle('Native input exceeds the 8 KiB limit')
    def pairs(values):
        result={}
        for key,value in values:
            if key in result:raise UncertainLifecycle('Duplicate native JSON field')
            result[key]=value
        return result
    def invalid_constant(_):raise UncertainLifecycle('Non-finite native JSON value')
    try:value=json.loads(raw,object_pairs_hook=pairs,parse_constant=invalid_constant)
    except (ValueError,TypeError,RecursionError,UnicodeError) as exc:raise UncertainLifecycle('Invalid native JSON') from exc
    if not isinstance(value,dict):raise UncertainLifecycle('Native input must be an object')
    return value


def validate_binding(value):
    if not isinstance(value,dict) or set(value)!=BINDING_FIELDS or value.get('schema')!='agenturm-native-binding-v1':raise UncertainLifecycle('Unsupported native binding')
    result=dict(value)
    for key in ('sender_id','native_turn_id',*IDENTITY):identifier(result.get(key),key,empty=key in {'native_turn_id','turn_id','parent_agent_id'})
    if not re.fullmatch(r'ag1_[0-9a-f]{32}',result['agent_id']):raise UncertainLifecycle('Binding requires a canonical recorded actor')
    if not re.fullmatch(r'[0-9a-f]{32}',result['trace_id']):raise UncertainLifecycle('Binding requires one recorded trace')
    if result['turn_id'] and not re.fullmatch(r'[0-9a-f]{32}',result['turn_id']):raise UncertainLifecycle('Invalid recorded turn')
    if type(result.get('follow_recorded_turns')) is not bool:raise UncertainLifecycle('Invalid follow-recorded-turns binding')
    if not isinstance(result.get('endpoint'),str) or len(result['endpoint'])>1024:raise UncertainLifecycle('Invalid native endpoint')
    if result['adapter'] not in {'generic-v1','codex-notify-v1','claude-hook-v1','pi-extension-v1','opencode-event-v2'}:raise UncertainLifecycle('Unsupported native adapter')
    if result['follow_recorded_turns'] and result['adapter']!='codex-notify-v1':raise UncertainLifecycle('Follow-recorded-turns is qualified only for Codex notify')
    if result['adapter']=='codex-notify-v1' and not result['follow_recorded_turns'] and not result['native_turn_id']:raise UncertainLifecycle('A fixed Codex binding requires its native turn identity')
    if result['source_clock'] not in {'unknown','receiver'}:raise UncertainLifecycle('Unsupported native source clock')
    from .terminal import Api
    Api(result['endpoint']) # Same credentials/redirect restrictions as normal API reads.
    return result


def read_binding(path):
    descriptor=os.open(Path(path).expanduser(),os.O_RDONLY|getattr(os,'O_NOFOLLOW',0)|getattr(os,'O_NONBLOCK',0))
    with os.fdopen(descriptor,'rb') as stream:
        metadata=os.fstat(stream.fileno())
        if not stat.S_ISREG(metadata.st_mode) or os.name!='nt' and metadata.st_mode&0o077:raise UncertainLifecycle('Native binding must be an owner-only regular file')
        return validate_binding(bounded_json(stream.read(MAX_BYTES+1)))


def verify_binding(api,binding,event=None):
    health=api.get('/api/health')
    if health.get('receiver_id')!=binding['receiver_id']:raise UncertainLifecycle('Native receiver identity changed')
    selected={**binding,**({key:event[key] for key in ('trace_id','turn_id')} if event is not None else {})}
    params={key:selected[key] for key in ('service','agent_id','session_id','trace_id')};params.update(limit=1,subtree=0)
    page=api.get('/api/activity?'+urlencode(params))
    if page.get('receiver_id')!=binding['receiver_id']:raise UncertainLifecycle('Native source response identity changed')
    if not any(all(row.get(key)==selected[key] for key in ('service','agent_id','session_id','trace_id')) for row in page.get('activity',())):
        raise UncertainLifecycle('Configured actor/session/trace has no matching recorded source')
    if selected['turn_id'] and not any(row.get('turn_id')==selected['turn_id'] for row in page['activity']):raise UncertainLifecycle('Recorded turn contradicts native binding')
    graph=api.get('/api/hierarchy?limit=10000&include_reviews=1')
    if graph.get('receiver_id')!=binding['receiver_id']:raise UncertainLifecycle('Native hierarchy response identity changed')
    node=next((row for row in graph.get('nodes',()) if row.get('agent_id')==binding['agent_id']),{})
    native=next((row for row in (health.get('local_agents') or {}).get('sessions',()) if
        all(row.get(key)==binding[key] for key in ('service','agent_id','session_id'))),{})
    if binding['follow_recorded_turns'] and native.get('source_id')!=binding['source_id']:raise UncertainLifecycle('Follow mode requires the exact recorded native provider source')
    if binding['source_id'] not in {node.get('source_id'),native.get('source_id')}:raise UncertainLifecycle('Native source binding is not recorded for this actor')
    if binding['parent_agent_id'] and (node.get('parent_agent_id')!=binding['parent_agent_id'] or node.get('relationship_state')!='confirmed'):
        raise UncertainLifecycle('Native parent relationship is not confirmed')
    return health


def normalize_event(value,binding):
    if not isinstance(value,dict) or set(value)-EVENT_FIELDS or value.get('schema')!='agenturm-native-event-v1':raise UncertainLifecycle('Unsupported native event schema')
    for key in ('sender_id',*IDENTITY):
        if binding['follow_recorded_turns'] and key in {'trace_id','turn_id'}:continue
        if value.get(key)!=binding[key]:raise UncertainLifecycle('Native '+key+' contradicts configured binding')
    result={key:value.get(key) for key in EVENT_FIELDS}
    identifier(result['event_id'],'event');identifier(result.get('request_id') or '','request',empty=True)
    transport_only=binding['adapter']=='codex-notify-v1' and binding['follow_recorded_turns'] and result['kind']=='turn_completed'
    if type(result['sequence']) is not int or not (0 if transport_only else 1)<=result['sequence']<2**63:raise UncertainLifecycle('Invalid native event order')
    if not isinstance(result.get('trace_id'),str) or not re.fullmatch(r'[0-9a-f]{32}',result['trace_id']):raise UncertainLifecycle('Invalid recorded native trace')
    if result.get('turn_id') and (not isinstance(result['turn_id'],str) or not re.fullmatch(r'[0-9a-f]{32}',result['turn_id'])):raise UncertainLifecycle('Invalid recorded native turn')
    result['source_order']='transport-only' if transport_only else 'sender-sequence'
    if result['kind'] not in KINDS:raise UncertainLifecycle('Unsupported native lifecycle event')
    if result['kind'] in {'approval_requested','reply_requested','request_resolved'} and not result.get('request_id'):raise UncertainLifecycle('Request identity unavailable; attention is uncertain')
    stamp=result.get('time_unix_nano') or 0
    if type(stamp) is not int or not 0<=stamp<2**63:raise UncertainLifecycle('Invalid native source timestamp')
    result['time_unix_nano']=stamp;result['source_clock']=binding['source_clock']
    return result


def runtime_event(value,binding):
    """Only documented complete identities are adapted; absence stays uncertain."""
    if binding['adapter']=='generic-v1':return normalize_event(value,binding)
    if binding['adapter']=='codex-notify-v1':
        if value.get('type')!='agent-turn-complete':raise UncertainLifecycle('Codex notify does not expose this event type')
        native_turn=value.get('turn-id')
        if value.get('thread-id')!=binding['session_id'] or not isinstance(native_turn,str) or not native_turn or (not binding['follow_recorded_turns'] and native_turn!=binding['native_turn_id']):
            raise UncertainLifecycle('Codex notification session/turn contradicts binding')
        from .local_agents import identity
        event=dict(schema='agenturm-native-event-v1',sender_id=binding['sender_id'],**{key:binding[key] for key in IDENTITY},
            event_id='codex-complete:'+native_turn,sequence=0 if binding['follow_recorded_turns'] else 1,kind='turn_completed',request_id='',time_unix_nano=0)
        if binding['follow_recorded_turns']:event.update(trace_id=identity(binding['source_id'],native_turn),turn_id='')
        return normalize_event(event,binding)
    raise UncertainLifecycle('This native hook lacks a verified exact request/order mapping; use an explicit generic-v1 bridge with authoritative IDs')


def enqueue(store,binding,event):
    event=normalize_event({key:event.get(key) for key in EVENT_FIELDS},binding)
    digest=fingerprint(event);key=fingerprint((binding['sender_id'],event['event_id']))
    event={**event,'received_time_unix_nano':time.time_ns()}
    with store._transaction() as db:
        known=db.execute('SELECT * FROM workflow_native_senders_v1 WHERE sender_id=?',(binding['sender_id'],)).fetchone()
        binding_hash=fingerprint(binding)
        if known and known['binding_hash']!=binding_hash:raise UncertainLifecycle('Sender binding changed; create a new explicit binding')
        if not known:
            if db.execute('SELECT COUNT(*) FROM workflow_native_senders_v1').fetchone()[0]>=MAX_EVENTS:raise WorkflowStoreError('Native sender capacity reached')
            db.execute('INSERT INTO workflow_native_senders_v1(sender_id,namespace,binding_hash) VALUES(?,?,?)',(binding['sender_id'],binding['receiver_id'],binding_hash))
        previous=db.execute('SELECT event_hash FROM workflow_native_events_v1 WHERE event_key=?',(key,)).fetchone()
        if previous:
            if previous[0]!=digest:raise UncertainLifecycle('Native event identity conflicts with prior metadata')
            return False
        if event.get('source_order')=='transport-only' and event['kind']=='turn_completed':event['sequence']=(known['sequence'] if known else 0)+1
        if known and event['sequence']<=known['sequence']:raise UncertainLifecycle('Stale or reordered native event')
        if db.execute('SELECT COUNT(*) FROM workflow_native_events_v1').fetchone()[0]>=MAX_EVENTS:
            # Sent events can be evicted only after their sender's ordering fence
            # has advanced; pending outbox entries are never silently discarded.
            db.execute('DELETE FROM workflow_native_events_v1 WHERE event_key IN (SELECT event_key FROM workflow_native_events_v1 WHERE sent=1 AND NOT EXISTS (SELECT 1 FROM workflow_native_requests_v1 request WHERE request.event_key=workflow_native_events_v1.event_key) ORDER BY rowid LIMIT 100)')
            if db.execute('SELECT COUNT(*) FROM workflow_native_events_v1').fetchone()[0]>=MAX_EVENTS:raise WorkflowStoreError('Native event outbox capacity reached')
        if event['kind']=='request_resolved':
            request=db.execute('SELECT resolved,kind FROM workflow_native_requests_v1 WHERE sender_id=? AND request_id=?',(binding['sender_id'],event['request_id'])).fetchone()
            if request is None or request[0]:raise UncertainLifecycle('No matching unresolved native request')
            event['request_kind']=request['kind']
        if event['kind'] in {'approval_requested','reply_requested'}:
            event['request_kind']=event['kind']
            if db.execute('SELECT 1 FROM workflow_native_requests_v1 WHERE sender_id=? AND request_id=?',(binding['sender_id'],event['request_id'])).fetchone():
                raise UncertainLifecycle('Native request identity was reused')
            if db.execute('SELECT COUNT(*) FROM workflow_native_requests_v1').fetchone()[0]>=MAX_EVENTS:raise WorkflowStoreError('Native request capacity reached')
        db.execute('INSERT INTO workflow_native_events_v1 VALUES(?,?,?,?,?,0)',(key,binding['sender_id'],event['sequence'],digest,json.dumps(event,separators=(',',':'))))
        db.execute('UPDATE workflow_native_senders_v1 SET sequence=? WHERE sender_id=?',(event['sequence'],binding['sender_id']))
        if event['kind'] in {'approval_requested','reply_requested'}:
            db.execute('INSERT INTO workflow_native_requests_v1 VALUES(?,?,?,0,?,?)',
                (event['sender_id'],event['request_id'],event['kind'],key,json.dumps(event,separators=(',',':'))))
        elif event['kind']=='request_resolved':
            db.execute('UPDATE workflow_native_requests_v1 SET resolved=1,event_key=?,payload=? WHERE sender_id=? AND request_id=?',
                (key,json.dumps(event,separators=(',',':')),event['sender_id'],event['request_id']))
    return True


def event_log(event):
    action={'approval_requested':'approval_request','reply_requested':'input_request','request_resolved':'request_resolved',
        'turn_started':'started','turn_completed':'completed','turn_interrupted':'interrupted'}[event['kind']]
    attrs={'agent.id':event['agent_id'],'session.id':event['session_id'],'agent.turn.id':event['turn_id'],
        'agent.activity.domain':'agent','agent.activity.action':action,'agent.activity.subject':event['kind'].replace('_',' ').capitalize(),
        'agent.activity.result':'observed','observatory.record_id':fingerprint((event['sender_id'],event['event_id'])),
        'observatory.native.schema':1,'observatory.native.time_basis':'source' if event['time_unix_nano'] else 'bridge-receipt',
        **{'observatory.native.'+key:event.get(key) or '' for key in ('receiver_id','sender_id','event_id','kind','request_id','request_kind','source_clock')},
        'observatory.native.sequence':event['sequence'],'observatory.native.source_time_ns':event['time_unix_nano'],
        'observatory.native.source_order':event.get('source_order') or 'sender-sequence'}
    # Unknown source time remains unknown; transport timestamp is required by
    # OTLP and never enters request-time/eligibility evidence in the payload.
    return dict(trace_id=event['trace_id'],span_id=None,time_unix_nano=event['time_unix_nano'] or event['received_time_unix_nano'],
        service=event['service'],severity_number=9,severity_text='INFO',event_name='agent.activity',
        attributes=attrs,resource_attributes={'observatory.source_id':event['source_id']},
        body='',scope_name='observatory.native-lifecycle.v1')


def flush(store,binding,post):
    with store._transaction(write=False) as db:
        pending=db.execute('SELECT event_key,payload FROM workflow_native_events_v1 WHERE sender_id=? AND sent=0 ORDER BY sequence LIMIT 100',(binding['sender_id'],)).fetchall()
    for row in pending:
        event=json.loads(row['payload'])
        if post(event_log(event)) is not True:raise OSError('Native lifecycle transport did not confirm acceptance')
        with store._transaction() as db:
            db.execute('UPDATE workflow_native_events_v1 SET sent=1 WHERE event_key=?',(row['event_key'],))
    return len(pending)


def record_uncertain(store,binding,reason):
    with store._transaction() as db:
        if not db.execute('SELECT 1 FROM workflow_native_senders_v1 WHERE sender_id=?',(binding['sender_id'],)).fetchone() and db.execute('SELECT COUNT(*) FROM workflow_native_senders_v1').fetchone()[0]>=MAX_EVENTS:
            raise WorkflowStoreError('Native sender capacity reached')
        db.execute('INSERT INTO workflow_native_senders_v1(sender_id,namespace,binding_hash) VALUES(?,?,?) ON CONFLICT DO NOTHING',
            (binding['sender_id'],binding['receiver_id'],fingerprint(binding)))
        db.execute('UPDATE workflow_native_senders_v1 SET rejected=rejected+1,last_reason=? WHERE sender_id=?',
            (str(reason)[:300],binding['sender_id']))


def post_event(api,binding,log):
    """Existing authenticated OTLP transport plus an atomic receiver precondition."""
    import urllib.request
    from .http_transport import urlopen
    def attrs(values):
        return [dict(key=key,value={'boolValue':value} if type(value) is bool else
            {'intValue':str(value)} if type(value) is int else {'stringValue':str(value)}) for key,value in values.items()]
    record=dict(timeUnixNano=str(log['time_unix_nano']),traceId=log['trace_id'],severityNumber=9,severityText='INFO',
        eventName='agent.activity',attributes=attrs(log['attributes']),body={'stringValue':log['body']})
    payload={'resourceLogs':[{'resource':{'attributes':attrs({'service.name':log['service'],**log['resource_attributes']})},
        'scopeLogs':[{'scope':{'name':log['scope_name']},'logRecords':[record]}]}]}
    headers={'Content-Type':'application/json','X-Agenturm-Expected-Receiver-ID':binding['receiver_id']}
    if api.token:headers['Authorization']='Bearer '+api.token
    request=urllib.request.Request(api.endpoint+'/v1/logs',data=json.dumps(payload,separators=(',',':')).encode(),headers=headers,method='POST')
    with urlopen(request,timeout=3) as response:
        tagged=response.headers.get('X-Agenturm-Receiver-ID')
        if tagged is not None and tagged!=binding['receiver_id']:raise UncertainLifecycle('Native write response receiver changed')
        raw=response.read(8193)
    if len(raw)>8192:raise OSError('Native write response exceeded limit')
    result=json.loads(raw)
    if result.get('accepted')!=1 or (result.get('partialSuccess') or {}).get('rejectedLogRecords',0):raise OSError('Native lifecycle record was not accepted')
    if api.get('/api/health').get('receiver_id')!=binding['receiver_id']:raise UncertainLifecycle('Native receiver changed after acceptance; outbox retained')
    return True


def overlay_observations(items,observations,receiver):
    """Only one exactly owned qualified request can override a captured state.

    Missing/foreign/multiple pending requests remain uncertain. An exact
    resolution suppresses only its own request and never approves an operation.
    """
    from .workflow_evidence import scope_for
    from types import SimpleNamespace
    snapshot=SimpleNamespace(receiver_id=receiver,local_status={})
    groups={};resolved=[]
    for event in observations:
        if event.get('receiver_id')!=receiver:continue
        key=tuple(event.get(field) or '' for field in ('service','agent_id','session_id','trace_id','turn_id'))
        if event.get('resolved'):resolved.append(event)
        else:groups.setdefault(key,[]).append(event)
    result=[]
    for original in items:
        item=dict(original);key=tuple(item.get(field) or '' for field in ('service','agent_id','session_id','trace_id','turn_id'))
        current=groups.get(key,[])
        if len(current)==1:
            event=current[0];kind=event.get('request_kind')
            # A stronger captured request is not replaced by an unproved alias.
            if item.get('state')=='Waiting' and item.get('request_id') and item['request_id']!=event.get('request_id'):
                result.append(item);continue
            if item.get('state') in {'Catching up','Monitor delayed','Unconfirmed','Offline','Spawn recorded'}:
                result.append(item);continue
            terminal=item.get('run_state') in {'completed','interrupted','failed','error','incomplete'} and bool(item.get('diagnostics'))
            terminal_stamp=item.get('outcome_time_ns') or item.get('state_since_ns') or 0
            newer=(event.get('source_clock')=='receiver' and type(terminal_stamp) is int and terminal_stamp>0
                and event.get('time_unix_nano',0)>terminal_stamp)
            if terminal and not newer:
                result.append(item);continue
            if kind not in {'approval_requested','reply_requested'}:continue
            item.update(state='Waiting',waiting_for='Approval' if kind=='approval_requested' else 'User input',
                request_id=event['request_id'],request_time_ns=event['time_unix_nano'],
                request_clock=event['source_clock'],_native_request_key=fingerprint((event['sender_id'],event['request_id'])))
        elif any(tuple(event.get(field) or '' for field in ('service','agent_id','session_id','trace_id','turn_id'))==key and
                item.get('request_id')==event.get('request_id') for event in resolved):
            # Do not invent a running/idle state from a resolved prompt.
            item.update(state='Unknown',waiting_for='',request_id='')
        result.append(item)
    resolutions=[]
    for event in resolved:
        scope=scope_for(event,snapshot)
        kind='approval' if event.get('request_kind')=='approval_requested' else 'reply'
        resolutions.append((scope,kind,event['request_id'],fingerprint((event['sender_id'],event['request_id']))))
    return result,resolutions


def next_event_for_log(store,binding,log):
    return dict(trace_id=log['trace_id'],turn_id=log['attributes'].get('agent.turn.id') or '')


def add_native_parsers(commands):
    native=commands.add_parser('native',help='explicit metadata-only native lifecycle bridge')
    subs=native.add_subparsers(dest='native_command',required=True)
    bind=subs.add_parser('bind',help='create a private binding after checking recorded identities')
    bind.add_argument('--endpoint',required=True);bind.add_argument('--output',type=Path,required=True)
    for key in IDENTITY:bind.add_argument('--'+key.replace('_','-'),required=key not in {'turn_id','parent_agent_id'},default='')
    bind.add_argument('--native-turn-id',default='');bind.add_argument('--adapter',choices=('generic-v1','codex-notify-v1'),default='generic-v1')
    bind.add_argument('--follow-recorded-turns',action='store_true',help='follow this exact native source only after independently proving each recorded turn')
    bind.add_argument('--receiver-clock',action='store_true',help='explicitly attest event timestamps use the receiver host clock; default unknown')
    for command in ('send','hook','retry'):
        child=subs.add_parser(command,help='forward exact lifecycle metadata without native decisions')
        child.add_argument('--binding',type=Path,required=True);child.add_argument('--state',type=Path)
        if command!='retry':child.add_argument('payload',nargs='?',help='one bounded JSON argument; otherwise stdin')


def run_native_command(args):
    from .terminal import Api
    binding=None;store=None
    try:
        if args.native_command=='bind':
            binding=dict(schema='agenturm-native-binding-v1',sender_id=secrets.token_hex(16),endpoint=args.endpoint,
                adapter=args.adapter,native_turn_id=args.native_turn_id,follow_recorded_turns=args.follow_recorded_turns,source_clock='receiver' if args.receiver_clock else 'unknown',
                **{key:getattr(args,key) for key in IDENTITY})
            binding=validate_binding(binding);verify_binding(Api(args.endpoint),binding)
            descriptor=os.open(args.output,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
            with os.fdopen(descriptor,'w') as stream:json.dump(binding,stream,indent=2);stream.write('\n')
            return 0
        binding=read_binding(args.binding);api=Api(binding['endpoint'])
        store=WorkflowStore(args.state)
        if args.native_command!='retry':
            raw=args.payload if args.payload is not None else sys.stdin.buffer.read(MAX_BYTES+1)
            event=runtime_event(bounded_json(raw),binding);verify_binding(api,binding,event);enqueue(store,binding,event)
        def send(log):
            event_meta=next_event_for_log(store,binding,log)
            verify_binding(api,binding,event_meta)
            return post_event(api,binding,log)
        flush(store,binding,send)
        return 0
    except (OSError,ValueError) as exc:
        if binding is not None and store is not None and isinstance(exc,UncertainLifecycle):
            try:record_uncertain(store,binding,str(exc))
            except WorkflowStoreError:pass
        print('Agenturm native observation uncertain or unavailable: '+str(exc),file=sys.stderr)
        # A hook observer must never block or approve the native operation.
        return 0 if args.native_command=='hook' else 2


def native_metadata(attrs,resource=None):
    """Whitelist a typed native envelope; it contains no claimed actor fields.

    Actual producer/actor/session/trace/turn must come independently from the
    normalized source row. Receiver matching occurs at consumer snapshot time.
    """
    if not isinstance(attrs,dict) or type(attrs.get('observatory.native.schema')) is not int or attrs['observatory.native.schema']!=1:return None
    result={'schema':1}
    for key in ('receiver_id','sender_id','event_id','kind','request_id','request_kind','source_clock'):
        value=attrs.get('observatory.native.'+key)
        if not isinstance(value,str) or len(value)>512:return None
        result[key]=value
    if result['kind'] not in KINDS or result['source_clock'] not in {'receiver','unknown'}:return None
    try:
        for key in ('receiver_id','sender_id','event_id'):identifier(result[key],key)
        identifier(result['request_id'],'request',empty=True)
    except UncertainLifecycle:return None
    if result['kind'] in {'approval_requested','reply_requested','request_resolved'}:
        if not result['request_id'] or result['request_kind'] not in {'approval_requested','reply_requested'}:return None
        if result['kind']!='request_resolved' and result['request_kind']!=result['kind']:return None
    for source,target in (('sequence','sequence'),('source_time_ns','time_unix_nano')):
        value=attrs.get('observatory.native.'+source)
        if type(value) is not int or not 0<=value<2**63 or source=='sequence' and value==0:return None
        result[target]=value
    result['source_order']=attrs.get('observatory.native.source_order','sender-sequence')
    if result['source_order'] not in {'sender-sequence','transport-only'}:return None
    if result['source_order']=='transport-only' and result['kind']!='turn_completed':return None
    source=(resource or {}).get('observatory.source_id')
    try:result['source_id']=identifier(source,'native source')
    except UncertainLifecycle:return None
    return result


def activity_observations(events,receiver):
    """Reconstruct bounded native request states without borrowing row identity."""
    from itertools import islice
    states={};senders={};conflicted=set();sequences={}
    for row in islice(events,1000):
        if not isinstance(row,dict) or row.get('receiver_id')!=receiver:continue
        metadata=row.get('native_lifecycle')
        if not isinstance(metadata,dict) or metadata.get('schema')!=1 or metadata.get('receiver_id')!=receiver:continue
        attrs={'observatory.native.'+key:metadata.get(key) for key in ('schema','receiver_id','sender_id','event_id','sequence','kind','request_id','request_kind','source_clock')}
        attrs['observatory.native.source_time_ns']=metadata.get('time_unix_nano')
        attrs['observatory.native.source_order']=metadata.get('source_order','sender-sequence')
        metadata=native_metadata(attrs,{'observatory.source_id':metadata.get('source_id')})
        if metadata is None:continue
        if not all(isinstance(row.get(key),str) and row[key] for key in ('service','agent_id','session_id','trace_id')):continue
        if not re.fullmatch(r'ag1_[0-9a-f]{32}',row['agent_id']) or not re.fullmatch(r'[0-9a-f]{32}',row['trace_id']):continue
        sender=metadata.get('sender_id');sequence=metadata.get('sequence')
        if not isinstance(sender,str) or type(sequence) is not int or sequence<=0:continue
        scope=tuple(row.get(key) or '' for key in ('service','agent_id','session_id','trace_id','turn_id'))
        if sender in senders and senders[sender]!=scope:conflicted.add(sender)
        senders[sender]=scope
        event={**metadata,**dict(zip(('service','agent_id','session_id','trace_id','turn_id'),scope)),
            'event_source_id':row.get('source_id') or row.get('id') or '',
            'resolved':metadata.get('kind')=='request_resolved'}
        identity=(sender,sequence);value=fingerprint({key:value for key,value in event.items() if key!='event_source_id'})
        if identity in sequences and sequences[identity]!=value:conflicted.add(sender)
        sequences[identity]=value
        request=event.get('request_id')
        if not request or event.get('kind') not in {'approval_requested','reply_requested','request_resolved'}:continue
        key=(sender,request)
        if key not in states or sequence>states[key]['sequence']:states[key]=event
    return [event for (sender,_),event in states.items() if sender not in conflicted]
