"""Bounded explanations of captured windows, never attention or correctness claims."""
from collections import defaultdict
from itertools import islice
import math
import re
import shlex

from .operation_facts import operation_facts,operation_key,operation_stamp,safe_value

MAX_EVENTS=500
MAX_REFS=20
_POLL=re.compile(r'(?:^|[._/ -])(?:sleep|wait|poll|watch|tail|stream|status|progress)(?:$|[._/ -])',re.I)


def number(value):
    if type(value) not in (int,float):return None
    try:result=float(value)
    except (ValueError,OverflowError):return None
    return result if math.isfinite(result) and result>=0 else None


def count(value):return value if type(value) is int and 0<=value<=10**18 else None


def label(value):
    if not isinstance(value,str) or not value or value in {'Not reported','Unknown'} or len(value)>512:return None
    text,complete=safe_value(value)
    return text if complete else None


def reference(row):
    return {key:row[key] for key in ('id','source_id','trace_id','span_id','receiver_id','service','agent_id','session_id','turn_id','time_unix_nano')
        if isinstance(row.get(key),str) and label(row[key]) is not None or key=='time_unix_nano' and type(row.get(key)) is int and row[key]>=0}


def _call_facts(row):
    attrs={'mcp.server.name':row.get('server'),'mcp.tool.name':row.get('tool'),'rpc.system':'mcp'}
    if row.get('output_reported') is True or 'output' in row and row.get('output_reported') is not False:attrs['output.value']=row.get('output')
    if row.get('is_error') is True:attrs['mcp.is_error']=True
    status=row.get('status');status=status.lower() if isinstance(status,str) else ''
    result='success' if status in {'ok','success','passed','completed'} else 'error' if status in {'error','failed','denied','blocked'} else 'unknown'
    return operation_facts(dict(domain='mcp',action=row.get('method') or 'call',result=result,attributes=attrs))


def configured_servers(configurations):
    """Project bounded server names from the existing file-shaped inventory."""
    result=[];seen=set()
    for entry in configurations[:100] if isinstance(configurations,(list,tuple)) else []:
        if isinstance(entry,str):names=[entry];producer=None
        elif isinstance(entry,dict):
            names=entry.get('servers')
            names=names[:100] if isinstance(names,(list,tuple)) else [entry.get('server') or entry.get('name')]
            producer=label(entry.get('service'))
        else:continue
        for value in names:
            server=label(value)
            if server and (producer,server) not in seen:
                seen.add((producer,server));result.append(dict(service=producer,server=server))
                if len(result)>=100:return result
    return result


def mcp_window(calls,*,configured=(),window=None,complete=False):
    """Per-tool recorded outcomes/durations for the supplied explicit loaded page."""
    input_size=len(calls) if isinstance(calls,(list,tuple)) else 0
    rows=calls[:MAX_EVENTS] if isinstance(calls,(list,tuple)) else []
    window=window if isinstance(window,dict) else {}
    partial=input_size>MAX_EVENTS or complete is not True
    groups={};seen=set();unknown_time=0;outside=0;unplaced=0;other_methods=0
    start=window.get('start_ns');end=window.get('end_ns')
    bounded_time=type(start) is int and start>=0 and type(end) is int and end>=start
    observed_servers=set()
    for ordinal,row in enumerate(rows[:MAX_EVENTS]):
        if not isinstance(row,dict):partial=True;continue
        stamp=operation_stamp(row)
        if not stamp:unknown_time+=1
        if bounded_time and not stamp:unplaced+=1;continue
        if bounded_time and not start<=stamp<=end:outside+=1;continue
        namespace=label(row.get('receiver_id')) or label(window.get('receiver_id'))
        producer=label(row.get('service'));server=label(row.get('server'));tool=label(row.get('tool'))
        method=label(row.get('method'))
        if method and method!='tools/call':other_methods+=1;continue
        if server:observed_servers.add((producer,server))
        identity=row.get('id') or row.get('source_id')
        occurrence=(label(row.get('receiver_id')),producer,label(row.get('agent_id')),label(row.get('session_id')),identity,server,tool)
        if namespace and label(identity):
            if occurrence in seen:continue
            seen.add(occurrence)
        key=(namespace,producer,server,tool) if all((namespace,producer,server,tool)) else ('incomplete',ordinal)
        item=groups.setdefault(key,dict(receiver_id=namespace,producer=producer,server=server,tool=tool,identity_complete=all((namespace,producer,server,tool)),
            calls=0,passed=0,failed=0,unknown=0,durations=[],duration_unknown=0,references=[],failure_references=[],unknown_references=[]))
        item['calls']+=1;facts=_call_facts(row)
        result=facts['result'] if not facts['result_conflict'] else 'unknown'
        item[result if result in {'passed','failed'} else 'unknown']+=1
        duration=number(row.get('duration_ms')) if row.get('duration_reported') is not False else None
        if duration is None:item['duration_unknown']+=1
        else:item['durations'].append(duration)
        if len(item['references'])<MAX_REFS:item['references'].append(reference({**row,'receiver_id':namespace}))
        target='failure_references' if result=='failed' else 'unknown_references' if result not in {'passed','failed'} else None
        if target and len(item[target])<MAX_REFS:item[target].append(reference({**row,'receiver_id':namespace}))
    result=[]
    for item in groups.values():
        durations=sorted(item.pop('durations'));known=item['passed']+item['failed'];n=len(durations)
        item.update(outcome_known=known,outcome_unknown=item['unknown'],duration_known=n,
            failure_fraction=item['failed']/known if known else None,
            p50_ms=durations[max(0,math.ceil(n*.5)-1)] if n else None,
            p95_ms=durations[max(0,math.ceil(n*.95)-1)] if n else None,
            duration_method='nearest-rank empirical quantiles over known durations',
            small_sample=item['calls']<20,references_clipped=item['calls']>MAX_REFS,
            outcome_meaning='Recorded protocol/tool result; response correctness is not evaluated.')
        item['next_step']=('Inspect the exact recorded failures in this window.' if item['failed'] else
            'Inspect unknown outcomes before interpreting reliability.' if item['unknown'] else
            'No failure was recorded among these known outcomes; this is not a correctness guarantee.')
        result.append(item)
    unobserved=[]
    for config in configured[:100] if isinstance(configured,(list,tuple)) else []:
        if isinstance(config,str):producer=None;server=label(config)
        elif isinstance(config,dict):producer=label(config.get('service'));server=label(config.get('server') or config.get('name'))
        else:continue
        if not server:continue
        if (producer,server) not in observed_servers and not (producer is None and any(name==server for _,name in observed_servers)):
            unobserved.append(dict(producer=producer,server=server,
                note='No matching captured tool calls in this loaded window. Capture/filter coverage may be incomplete; absence does not justify removing configuration.'))
    return dict(tools=sorted(result,key=lambda row:(-row['failed'],-row['unknown'],str(row['producer']),str(row['server']),str(row['tool'])))[:100],
        configured_unobserved=unobserved,coverage=dict(basis=label(window.get('label')) or 'Explicit supplied loaded call page',
        receiver_id=label(window.get('receiver_id')),
        requested_start_ns=start if bounded_time else None,requested_end_ns=end if bounded_time else None,
        input_limit=MAX_EVENTS,loaded_records=len(rows),input_clipped=input_size>MAX_EVENTS,
        partial=partial,unknown_timestamp_records=unknown_time,unplaced_timestamp_records=unplaced,outside_requested_window=outside,
        non_tool_method_records=other_methods,tools_clipped=len(result)>100,
        note='Denominators cover supplied captured records only; neither all-history reliability nor configuration completeness is inferred.'))


def validate_rates(value):
    """Explicit exact model labels; no provider defaults, aliases or rate fetching."""
    if not isinstance(value,dict):return {}
    result={}
    for model,rate in islice(value.items(),16):
        if not label(model) or len(model)>200 or not isinstance(rate,dict):continue
        if rate.get('unit')!='USD_per_million_tokens' or rate.get('input_accounting') not in {'flat_input','split_cache'}:continue
        required=('input','output','cache_read','cache_write') if rate['input_accounting']=='split_cache' else ('input','output')
        if any(number(rate.get(key)) is None or number(rate[key])>10**9 for key in required):continue
        result[model]=dict(unit=rate['unit'],input_accounting=rate['input_accounting'],**{key:float(rate[key]) for key in required})
    return result


def configured_estimate(usage,*,model='',rates=None,allocation_proven=False):
    def unavailable(reason):return dict(available=False,reason=reason,kind='user-configured estimate; not reported cost or a bill')
    rates=validate_rates(rates)
    if not rates:return unavailable('No valid explicit rates configured.')
    if not model or model not in rates:return unavailable('No exact-model rate entry; model aliases and current-model guesses are not used.')
    if allocation_proven is not True or usage.get('usage_allocation') or usage.get('aggregation_ambiguous') or usage.get('usage_complete') is not True:
        return unavailable('Complete unambiguous source allocation is required.')
    models=usage.get('models')
    if usage.get('models_complete') is not True or not isinstance(models,list) or models!=[model]:
        return unavailable('The reported usage must belong completely to this one exact model.')
    tokens={key:count(usage.get(key)) if usage.get(key+'_reported') is not False else None for key in ('input_tokens','output_tokens','cache_read_input_tokens','cache_write_input_tokens')}
    if tokens['input_tokens'] is None or tokens['output_tokens'] is None:return unavailable('Input/output token counts are missing or invalid.')
    rate=rates[model];parts={};inputs=tokens['input_tokens'];outputs=tokens['output_tokens']
    if rate['input_accounting']=='split_cache':
        read=tokens['cache_read_input_tokens'];write=tokens['cache_write_input_tokens']
        if read is None or write is None or read+write>inputs:return unavailable('Split-cache accounting needs complete compatible cache read/write breakdowns.')
        parts.update(uncached_input=(inputs-read-write)*rate['input']/1e6,cache_read=read*rate['cache_read']/1e6,cache_write=write*rate['cache_write']/1e6)
        assumption='Reported input includes cache reads/writes; those breakdowns are subtracted before pricing uncached input.'
    else:
        parts['reported_input']=inputs*rate['input']/1e6
        assumption='All reported input is priced at the configured input rate; cache discounts are not modeled.'
    parts['reported_output']=outputs*rate['output']/1e6
    return dict(available=True,model=model,estimated_usd=round(sum(parts.values()),8),components_usd=parts,
        rates=rate,assumption=assumption,
        kind='User-configured USD estimate; separate from reported cost; not provider billing or subscription cost.',
        note='Reasoning is an output breakdown and is not priced again. No provider prices or model defaults were supplied by Agenturm.')


def usage_explanation(usage,*,scope='session',context=None,scope_owner=None):
    input_tokens=count(usage.get('input_tokens'));cached=count(usage.get('cache_read_input_tokens'))
    cache_known=(input_tokens is not None and cached is not None and usage.get('input_tokens_reported') is not False
        and usage.get('cache_read_input_tokens_reported') is not False and usage.get('usage_complete') is not False and cached<=input_tokens)
    occupancy=dict(available=False,reason='No authoritative context occupancy observation supplied; cumulative usage is not occupancy.')
    if isinstance(context,dict) and context.get('authoritative') is True:
        used=count(context.get('used_tokens'));capacity=count(context.get('capacity_tokens'));source=context.get('source')
        owner=scope_owner if isinstance(scope_owner,dict) else {}
        ref=reference(source) if isinstance(source,dict) else {}
        owned=all(isinstance(owner.get(key),str) and owner[key] and ref.get(key)==owner[key] for key in ('receiver_id','service','agent_id','session_id'))
        owned=owned and all(ref.get(key)==owner[key] for key in ('trace_id','turn_id') if owner.get(key))
        if used is not None and capacity is not None and capacity>0 and used<=capacity and (ref.get('id') or ref.get('source_id')) and owned:
            occupancy=dict(available=True,used_tokens=used,capacity_tokens=capacity,fraction=used/capacity,source=reference(source),
                note='Explicit authoritative runtime observation; not inferred from cumulative tokens.')
    return dict(scope=scope,cache=dict(known=cache_known,cached_input_fraction=cached/input_tokens if cache_known and input_tokens>0 else None,
        note='Cached input is a token breakdown, not a request hit rate; cache/reasoning breakdowns are not added to totals.'),context=occupancy,
        retry_cost=dict(known=False,note='Aggregate usage cannot attribute tokens or cost to individual retries without exact per-operation usage.'))


def progress_observations(events,*,receiver=''):
    """Conservative repeated-work context over at most 500 exact-owned records."""
    from .workflow_evidence import check_observation
    rows=events[:MAX_EVENTS] if isinstance(events,(list,tuple)) else []
    rows=sorted((row for row in rows[:MAX_EVENTS] if isinstance(row,dict)),key=lambda row:operation_stamp(row) or 0)
    groups=defaultdict(list);unknown_scope=0;unknown_time=0
    for row in rows:
        facts=operation_facts(row);key=operation_key(row,receiver=receiver,facts=facts)
        owned=tuple(row.get(key) for key in ('receiver_id','service','agent_id','session_id'))
        execution=('turn',row.get('turn_id')) if row.get('turn_id') else ('trace',row.get('trace_id'))
        namespace=label(row.get('receiver_id')) or label(receiver)
        if not namespace or any(not label(row.get(key)) for key in ('service','agent_id','session_id')) or not label(execution[1]):
            unknown_scope+=1;continue
        stamp=operation_stamp(row)
        if not stamp:unknown_time+=1;continue
        if receiver and row.get('receiver_id') and receiver!=row['receiver_id']:unknown_scope+=1;continue
        scope=(namespace,*owned[1:],execution)
        command=facts['command'] or ''
        arguments=facts.get('arguments')
        poll_command=command or (next((arguments[key] for key in ('cmd','command') if isinstance(arguments.get(key),str)), '') if isinstance(arguments,dict) else arguments if isinstance(arguments,str) else '')
        try:poll_words=shlex.split(poll_command[:16000])
        except ValueError:poll_words=[]
        polling=bool(_POLL.search(str(facts.get('tool') or ''))) or any(word.rsplit('/',1)[-1] in {'sleep','wait','watch','tail','poll','status','stream'} for word in poll_words)
        check=check_observation(row,facts) if row.get('domain')=='shell' else None
        progress=(row.get('domain')=='file' and row.get('action') in {'write','edit','add','update','delete','create','patch','rename'}
            and row.get('result') in {'success','ok','completed'}) or bool(check and check['result']=='passed')
        groups[scope].append(dict(row=row,facts=facts,key=key,stamp=stamp,polling=polling,progress=progress,
            compact=row.get('action') in {'compact','compaction','truncation','context_truncated'}))
    observations=[]
    for scope,items in groups.items():
        chain=[];compactions=[];failures=defaultdict(list);successful={}
        def emit(kind,selected,note):
            if len(observations)<32:observations.append(dict(kind=kind,scope=scope,references=[reference(item['row']) for item in selected[:MAX_REFS]],
                count=len(selected),note=note,attention=False))
        for item in items:
            stamp=item['stamp'];facts=item['facts'];key=item['key']
            if item['progress']:
                chain=[previous for previous in chain if previous['stamp']>=stamp]
                compactions=[previous for previous in compactions if previous['stamp']>=stamp]
                failures=defaultdict(list,{key:[previous for previous in values if previous['stamp']>=stamp] for key,values in failures.items()})
            if item['compact']:
                compactions=[previous for previous in compactions if stamp-previous['stamp']<=600_000_000_000]+[item]
                if len(compactions)==3:emit('repeated_compaction',compactions,'Three recorded compactions within ten minutes; context loss or stalled work is not established.')
            if item['progress'] or not key or item['polling'] or facts['expected_nonzero']:continue
            if facts['result']=='failed' and facts['error_fingerprint']:
                signature=(key,facts['error_fingerprint']);failures[signature]=[old for old in failures[signature] if stamp-old['stamp']<=300_000_000_000]+[item]
                if len(failures[signature])==3:emit('repeated_tool_error',failures[signature],'Three complete matching failures within five minutes; retries may be intentional.')
            if facts['result']=='passed' and not facts['result_conflict']:
                successful[key]=stamp
            chain=chain+[item] if chain and chain[-1]['key']==key and stamp>chain[-1]['stamp'] else [item]
            if len(chain)==5 and not any(old['progress'] for old in chain):
                emit('repeated_equivalent_call',chain,'Five consecutive complete matching calls without intervening captured edit/check progress; no waste or stalled-work claim is made.')
        # A later matching success is only an observation. Do not infer task repair.
        for (key,_),failed in failures.items():
            later=successful.get(key,0)
            matched=[item for item in failed if later>item['stamp']]
            if matched:emit('later_matching_success',matched,'A strictly later matching success was recorded in this exact scope; consequences or full recovery are not established.')
    return dict(observations=observations,coverage=dict(limit=MAX_EVENTS,input_clipped=len(events)>MAX_EVENTS if isinstance(events,(list,tuple)) else False,
        unknown_scope_records=unknown_scope,unknown_timestamp_records=unknown_time,
        note='Loaded captured evidence only. Explanations never create explicit attention incidents or desktop notifications.'))
