"""Opt-in, bounded observations; captured repetition is not a correctness verdict."""
from __future__ import annotations
from collections import defaultdict
import re

from .findings import _finding

KINDS = ('repeated_compaction', 'repeated_equivalent_call', 'completion_unresolved_error')
MAX_EVENTS = 500
_POLL = re.compile(r'(?:^|[._/ -])(?:sleep|wait|poll|watch|tail|stream|status|progress)(?:$|[._/ -])', re.I)


def _fingerprint(value, *, sanitize=True):
    """Compatibility entry point: no flag can bypass bounded redaction."""
    from .operation_facts import decode,safe_value,digest
    if value in (None,''):return None
    value,complete=safe_value(decode(value))
    return digest(value) if complete else None


def analyze_patterns(spans, logs, outcome=None, *, receiver='', activity=None):
    """Opt-in semantic observations, qualified by shared operation facts.

    No raw argument hash, generic tool label, or completion status proves an
    equivalent operation or recovery. An absent namespace remains unknown.
    """
    from .core import _activity_for,_agent_id_from_attributes,_activity_session
    from .operation_facts import operation_facts,operation_key,operation_stamp
    from .evidence_explanations import progress_observations,label
    candidates=[('span',row) for row in spans if isinstance(row,dict)]+[('log',row) for row in logs if isinstance(row,dict)]
    def stamp(pair):
        row=pair[1];value=row.get('end_unix_nano') or row.get('time_unix_nano') or row.get('start_unix_nano') or 0
        return value if type(value) is int and value>0 else 0
    candidates=sorted(candidates,key=stamp,reverse=True)
    partial=len(candidates)>MAX_EVENTS
    selected=[]
    for kind,row in candidates[:MAX_EVENTS]:
        if not isinstance(row.get('attributes') or {},dict) or not isinstance(row.get('resource_attributes') or {},dict) or not label(row.get('trace_id')) or not label(row.get('service')) or not label(row.get('span_id') if kind=='span' else row.get('id') or row.get('digest')):
            partial=True;continue
        selected.append((kind,row))
    source_spans=[dict(row,events=row.get('events') or []) for kind,row in selected if kind=='span']
    source_logs=[dict(row,id=row.get('id') or row.get('digest'),body=row.get('body',''),resource_attributes=row.get('resource_attributes') or {})
        for kind,row in selected if kind=='log']
    # Canonical events already perform same-owner, source-linked supersession.
    activity=_activity_for(source_spans,source_logs) if activity is None else activity[:MAX_EVENTS]
    source_map=defaultdict(list)
    for kind,row in selected:
        source_map[(kind,row.get('span_id') if kind=='span' else row.get('id') or row.get('digest'))].append(row)
    rows=[]
    for event in activity:
        key=(event['source'],event.get('span_id') if event['source']=='span' else event['source_id'])
        sources=source_map.get(key,[])
        if any(not label(row.get('service')) or not isinstance(row.get('attributes') or {},dict)
            or any(key in (row.get('attributes') or {}) and not label(row['attributes'][key])
                   for key in ('tool.name','gen_ai.tool.name','mcp.tool.name')) for row in sources):
            partial=True;continue
        traces={row.get('trace_id') for row in sources}
        if len(traces)!=1:partial=True;continue
        trace=next(iter(traces))
        recorded_namespaces={row['receiver_id'] for row in sources if label(row.get('receiver_id'))}
        if len(recorded_namespaces)>1 or receiver and any(value!=receiver for value in recorded_namespaces):partial=True;continue
        namespace=receiver or next(iter(recorded_namespaces),None)
        if event.get('trace_id') and event['trace_id']!=trace or event.get('receiver_id') and event['receiver_id']!=namespace:partial=True;continue
        rows.append({**event,'trace_id':trace,'receiver_id':namespace,'id':str(trace)+':'+event['source_id']})
    for kind,row in selected:
        attrs=row.get('attributes') or {};resource=row.get('resource_attributes') or {}
        if not isinstance(attrs,dict) or not isinstance(resource,dict):partial=True;continue
        agent=attrs.get('agent.id',resource.get('agent.id'))
        if not isinstance(agent,str) or not agent:continue
        owner=_agent_id_from_attributes(attrs,resource,row.get('service'));session=_activity_session(attrs,resource)
        if receiver and row.get('receiver_id') and row['receiver_id']!=receiver:partial=True;continue
        namespace=receiver or row.get('receiver_id')
        if not all(label(value) for value in (namespace,row.get('service'),owner,session,row.get('trace_id'))):partial=True;continue
        name=str(row.get('name') or row.get('event_name') or '').casefold()
        compact='compact' in name or attrs.get('agent.activity.action') in {'compaction','compact'}
        completed=name=='agent.run' and (attrs.get('agent.stopped_reason')=='completed' or attrs.get('agent.run.state')=='completed')
        if not compact and not completed:continue
        source_id=row.get('span_id') if kind=='span' else row.get('id') or row.get('digest')
        if not source_id:continue
        rows.append(dict(receiver_id=namespace,service=row['service'],agent_id=owner,session_id=session,trace_id=row['trace_id'],
            turn_id=attrs.get('agent.turn.id') or '',source=kind,source_id=source_id,id=str(row['trace_id'])+':'+str(source_id),
            time_unix_nano=stamp((kind,row)),domain='context' if compact else 'agent',action='compaction' if compact else 'complete',
            result='observed' if compact else 'success',subject='Recorded compaction' if compact else 'Agent reported completion'))
    explanation=progress_observations(rows,receiver=receiver)
    partial=partial or explanation['coverage']['input_clipped'] or bool(explanation['coverage']['unknown_scope_records'])
    results=[]
    def emit(kind,selected,note,basis):
        first=selected[0];trace=first.get('trace_id')
        if not trace:return
        refs=[dict(type=row.get('source') or 'activity',id=row.get('source_id') or row.get('id')) for row in selected[:20]]
        title={'repeated_compaction':'Repeated recorded compaction','repeated_tool_error':'Repeated matching operation failures',
            'repeated_equivalent_call':'Repeated equivalent tool calls','completion_unresolved_error':'Completion with recovery unobserved'}[kind]
        item=_finding(trace,kind,title,'medium',refs,note+' Observation over captured evidence; recovery or intentional repetition may be unobserved.')
        item.update(workflow_pattern=True,analysis_partial=partial,scope_kind='agent',agent_id=first.get('agent_id'),
            producer=first.get('service'),session_id=first.get('session_id'),group_basis=basis,attention=False)
        results.append(item)
    by_reference={(row.get('id') or row.get('source_id')):row for row in rows}
    for observation in explanation['observations']:
        kind=observation['kind']
        if kind not in {'repeated_compaction','repeated_tool_error','repeated_equivalent_call'}:continue
        selected_rows=[by_reference.get(ref.get('id') or ref.get('source_id')) for ref in observation['references']]
        selected_rows=[row for row in selected_rows if row]
        if selected_rows:emit(kind,selected_rows,observation['note'],'semantic observation' if kind=='repeated_compaction' else 'complete sanitized operation equivalence')
    groups=defaultdict(list)
    for row in rows:
        values=(row.get('receiver_id'),row.get('service'),row.get('agent_id'),row.get('session_id'),row.get('turn_id') or row.get('trace_id'))
        if all(label(value) for value in values):groups[values].append(row)
    for scope,events in groups.items():
        completed=max((operation_stamp(row) or 0 for row in events if row.get('domain')=='agent' and row.get('action')=='complete'),default=0)
        if not completed:continue
        successes={}
        for row in events:
            facts=operation_facts(row);key=operation_key(row,receiver=receiver,facts=facts);time=operation_stamp(row)
            if key and time and facts['result']=='passed' and not facts['result_conflict']:successes[key]=max(time,successes.get(key,0))
        pending=[]
        for row in events:
            facts=operation_facts(row);time=operation_stamp(row);key=operation_key(row,receiver=receiver,facts=facts)
            if facts['result']=='failed' and not facts['expected_nonzero'] and time and time<completed and (not key or successes.get(key,0)<=time):pending.append(row)
        if pending:emit('completion_unresolved_error',pending[:20],
            'Agent reported completion; strictly later identical successful operations were not observed for these errors in the loaded window.',
            'semantic observation; no recovery inference from completion')
    return results[:32]
