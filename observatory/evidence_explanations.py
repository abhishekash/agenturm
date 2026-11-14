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
        chain=[];compactions=[];failures=defaultdict(list);recovery_failures=defaultdict(list);successful={}
        def emit(kind,selected,note):
            if len(observations)<32:observations.append(dict(kind=kind,scope=scope,references=[reference(item['row']) for item in selected[:MAX_REFS]],
                count=len(selected),note=note,attention=False))
        for item in items:
            stamp=item['stamp'];facts=item['facts'];key=item['key']
            if key and not item['polling'] and facts['result']=='passed' and not facts['result_conflict']:
                successful[key]=item
            if item['progress']:
                chain=[previous for previous in chain if previous['stamp']>=stamp]
                compactions=[previous for previous in compactions if previous['stamp']>=stamp]
                failures=defaultdict(list,{key:[previous for previous in values if previous['stamp']>=stamp] for key,values in failures.items()})
            if item['compact']:
                compactions=[previous for previous in compactions if stamp-previous['stamp']<=600_000_000_000]+[item]
                if len(compactions)==3:emit('repeated_compaction',compactions,'Three recorded compactions within ten minutes; context loss or stalled work is not established.')
            if item['progress'] or not key or item['polling'] or facts['expected_nonzero']:continue
            if facts['result']=='failed' and facts['error_fingerprint']:
                signature=(key,facts['error_fingerprint']);recovery_failures[signature].append(item);failures[signature]=[old for old in failures[signature] if stamp-old['stamp']<=300_000_000_000]+[item]
                if len(failures[signature])==3:emit('repeated_tool_error',failures[signature],'Three complete matching failures within five minutes; retries may be intentional.')
            chain=chain+[item] if chain and chain[-1]['key']==key and stamp>chain[-1]['stamp'] else [item]
            if len(chain)==5 and not any(old['progress'] for old in chain):
                emit('repeated_equivalent_call',chain,'Five consecutive complete matching calls without intervening captured edit/check progress; no waste or stalled-work claim is made.')
        # A later matching success is only an observation. Do not infer task repair.
        for (key,_),failed in recovery_failures.items():
            later=successful.get(key)
            matched=[item for item in failed if later is not None and later['stamp']>item['stamp']]
            if matched:emit('later_matching_success',[*matched[:MAX_REFS-1],later],'A strictly later matching success was recorded in this exact scope; consequences or full recovery are not established.')
    return dict(observations=observations,coverage=dict(limit=MAX_EVENTS,input_clipped=len(events)>MAX_EVENTS if isinstance(events,(list,tuple)) else False,
        unknown_scope_records=unknown_scope,unknown_timestamp_records=unknown_time,
        note='Loaded captured evidence only. Explanations never create explicit attention incidents or desktop notifications.'))


def reported_usage_context(spans,events,observation,*,receiver='',source_complete=False):
    """Reported leaf contributions inside a captured repeat interval, never deltas.

    The candidate/ancestor rule matches summarize's input/output leaf accounting.
    Additional completeness, ownership and timing gates make this explanation
    unavailable whenever the narrower interval cannot be accounted for safely.
    """
    from .core import INPUT_TOKEN_KEYS,OUTPUT_TOKEN_KEYS,_agent_id_from_attributes,_activity_session
    from .workflow_evidence import check_observation
    def unavailable(reason):return dict(available=False,reason=reason,attention=False)
    if source_complete is not True:return unavailable('The complete source window is not proved.')
    if not isinstance(spans,(list,tuple)) or len(spans)>MAX_EVENTS or not isinstance(events,(list,tuple)) or len(events)>MAX_EVENTS:
        return unavailable('The source/progress window exceeds its bounded input limit.')
    scope=observation.get('scope') if isinstance(observation,dict) else None
    if not isinstance(scope,(tuple,list)) or len(scope)!=5 or any(not label(value) for value in scope[:4]):
        return unavailable('Exact receiver, producer, actor and session are required.')
    execution=scope[4]
    if (not isinstance(execution,(tuple,list)) or len(execution)!=2 or execution[0] not in {'trace','turn'}
        or not label(execution[1]) or scope[0]!=receiver):return unavailable('Exact execution/receiver is unavailable.')
    wanted=tuple(scope[:4])+(tuple(execution),)
    references=observation.get('references') or []
    stamps=[ref.get('time_unix_nano') for ref in references if isinstance(ref,dict)]
    if (len(stamps)!=len(references) or len(stamps)<2 or any(type(stamp) is not int or stamp<=0 for stamp in stamps)
        or min(stamps)>=max(stamps)):return unavailable('A strict comparable observation interval is required.')
    lower,upper=min(stamps),max(stamps)
    traces={ref.get('trace_id') for ref in references}
    if len(traces)!=1 or not all(label(trace) for trace in traces):return unavailable('A single explicit trace is required for source navigation.')
    trace=next(iter(traces))
    def owner(row):
        attrs=row.get('attributes') or {};resource=row.get('resource_attributes') or {}
        if not isinstance(attrs,dict) or not isinstance(resource,dict):return None
        raw=attrs.get('agent.id',resource.get('agent.id'))
        if not label(raw):return None
        namespace=label(row.get('receiver_id')) or label(receiver)
        if namespace!=receiver:return None
        actor=_agent_id_from_attributes(attrs,resource,row.get('service'));session=_activity_session(attrs,resource)
        turn=attrs.get('agent.turn.id')
        anchor=('turn',turn) if turn else ('trace',row.get('trace_id'))
        values=(namespace,row.get('service'),actor,session)
        return (*values,anchor) if all(label(value) for value in values) and label(anchor[1]) else None
    by_id={};usage={};matching={}
    fields={'input_tokens':INPUT_TOKEN_KEYS,'output_tokens':OUTPUT_TOKEN_KEYS}
    for row in spans:
        if not isinstance(row,dict):return unavailable('Malformed source record.')
        key=(row.get('trace_id'),row.get('span_id'))
        if not all(isinstance(value,str) and value for value in key):return unavailable('Source execution/span identity is missing.')
        if key in by_id:return unavailable('Duplicate source span identity.')
        by_id[key]=row
        attrs=row.get('attributes') or {}
        if not isinstance(attrs,dict):return unavailable('Malformed source usage fields.')
        if row.get('name')=='agent.run' or not any(name in attrs for keys in fields.values() for name in keys):continue
        usage[key]=owner(row)
        if usage[key] is None:return unavailable('Usage ownership or receiver is unknown/contradictory.')
        if usage[key]!=wanted:continue
        if key[0]!=trace:return unavailable('Cross-trace turn contributions cannot use this per-trace source reader.')
        if not re.fullmatch(r'[0-9a-f]{16}',key[1]) or not re.fullmatch(r'[0-9a-f]{32}',key[0]):
            return unavailable('Exact source reference is unavailable.')
        values={}
        for field,keys in fields.items():
            present=[attrs[name] for name in keys if name in attrs]
            if present and (any(count(value) is None for value in present) or len(set(present))!=1):
                return unavailable('Reported token fields are invalid or contradictory.')
            if present:values[field]=present[0]
        matching[key]=values
    if not matching:return unavailable('No exactly owned reported usage sources were supplied.')
    # Full ancestry is required here. The general aggregate can remain useful
    # when ancestry is missing; this narrower explanatory interval cannot.
    ancestors={}
    for key,record_owner in usage.items():
        chain=[];seen={key};parent=by_id[key].get('parent_span_id')
        while parent:
            if not isinstance(parent,str):return unavailable('Source ancestry is malformed.')
            parent_key=(key[0],parent)
            if parent_key in seen:return unavailable('Source ancestry is cyclic.')
            if parent_key not in by_id:return unavailable('Source ancestry is incomplete.')
            seen.add(parent_key);chain.append(parent_key)
            if parent_key in usage and usage[parent_key]!=record_owner and (record_owner==wanted or usage[parent_key]==wanted):
                return unavailable('Cross-owner usage ancestry cannot allocate contributions.')
            parent=by_id[parent_key].get('parent_span_id')
        ancestors[key]=chain
    leaves={}
    for field in fields:
        candidates={key for key,values in matching.items() if field in values}
        wrappers={parent for key in candidates for parent in ancestors[key] if parent in candidates}
        leaves[field]=candidates-wrappers
    selected={};totals={field:0 for field in fields};present={field:False for field in fields}
    for key in set().union(*leaves.values()):
        row=by_id[key];start=row.get('start_unix_nano');end=row.get('end_unix_nano')
        if any(type(value) is not int or value<=0 for value in (start,end)) or end<start:
            return unavailable('Source contribution timing is unavailable.')
        if end<lower or start>upper:continue
        if start<=lower or end>=upper:return unavailable('A contribution crosses or equals the observation boundary.')
        if row.get('status') not in {'OK','ERROR','UNSET'} or row.get('capture_content') is False:
            return unavailable('Source terminal/capture status is uncertain.')
        selected[key]=row
        for field in fields:
            if key in leaves[field]:totals[field]+=matching[key][field];present[field]=True
    if (len(selected)<2 or len({row['end_unix_nano'] for row in selected.values()})<2
        or not all(present.values()) or not any(totals.values())):
        return unavailable('At least two ordered positive usage sources with both token directions are required.')
    for row in events:
        if not isinstance(row,dict):return unavailable('Malformed progress source.')
        namespace=label(row.get('receiver_id')) or label(receiver)
        anchor=('turn',row.get('turn_id')) if row.get('turn_id') else ('trace',row.get('trace_id'))
        if (namespace,row.get('service'),row.get('agent_id'),row.get('session_id'),anchor)!=wanted:continue
        relevant_file=row.get('domain')=='file' and row.get('action') in {'write','edit','add','update','delete','create','patch','rename','move'}
        check=check_observation(row) if row.get('domain')=='shell' else None
        if not relevant_file and (not check or check['result']=='failed'):continue
        stamp=operation_stamp(row)
        if stamp is None:return unavailable('Progress timing is unavailable.')
        if stamp in {lower,upper}:return unavailable('Progress ordering at an interval boundary is unknown.')
        if lower<stamp<upper:
            return unavailable('Captured edit/check progress or effect uncertainty occurs in this interval.')
    ordered=sorted(selected.values(),key=lambda row:(row['end_unix_nano'],row['trace_id'],row['span_id']))
    refs=[dict(type='span',id=row['span_id'],trace_id=row['trace_id'],service=row['service'],
        agent_id=wanted[2],session_id=wanted[3],receiver_id=receiver) for row in ordered[:MAX_REFS]]
    return dict(available=True,**totals,source_records=len(selected),references=refs,references_clipped=len(selected)>MAX_REFS,
        interval_start_ns=lower,interval_end_ns=upper,attention=False,
        note=f"{totals['input_tokens']:,} reported input and {totals['output_tokens']:,} output tokens from {len(selected)} completed leaf source records strictly inside this captured interval; no successful captured edit/check progress is established. These are per-record contributions, not cumulative-counter deltas, retry costs, context occupancy or proof of waste.")
