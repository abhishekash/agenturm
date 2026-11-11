"""Opt-in, bounded observations; captured repetition is not a correctness verdict."""
from __future__ import annotations
from collections import defaultdict
import hashlib
import json
import re
import shlex

from .findings import _finding, _safe_text

KINDS = ('repeated_compaction', 'repeated_equivalent_call', 'completion_unresolved_error')
MAX_EVENTS = 500
_POLL = re.compile(r'(?:^|[._/ -])(?:sleep|wait|poll|watch|tail|stream|status|progress)(?:$|[._/ -])', re.I)


def _fingerprint(value, *, sanitize=True):
    if isinstance(value, str):
        if len(value) > 16000:
            return None
        try:
            value = json.loads(value)
        except ValueError:
            pass
    try:
        text = json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False, allow_nan=False)
    except (TypeError, ValueError, RecursionError):
        return None
    if len(text) > 16000:
        return None
    if not sanitize and '[REDACTED' in text.upper():
        return None
    return hashlib.sha256((_safe_text(text) if sanitize else text).encode()).hexdigest()


def _polling_arguments(value):
    if isinstance(value, str):
        if len(value) > 16000:
            return False
        try:
            value = json.loads(value)
        except ValueError:
            pass
    command = next((value[k] for k in ('cmd', 'command') if isinstance(value.get(k), str)), '') if isinstance(value, dict) else value if isinstance(value, str) else ''
    try:
        words = shlex.split(command[:16000])
    except ValueError:
        return False
    return any(word.rsplit('/', 1)[-1] in {'sleep', 'wait', 'watch', 'tail', 'poll', 'status', 'stream'}
               for word in words)


def analyze_patterns(spans, logs, outcome=None):
    """Partition by exact recorded agent or labeled legacy identity within a run.

    Retain at most 500 observations and only safe fingerprints/source references.
    No raw argument payload is added to a finding or persisted separately.
    """
    from .core import _agent_id_from_attributes, SESSION_KEYS
    candidates = []
    for span in spans:
        candidates.append((span.get('end_unix_nano') or span.get('end_ns') or
                           span.get('start_unix_nano') or span.get('start_ns') or 0,
                           'span', span.get('span_id'), span))
    for log in logs:
        candidates.append((log.get('time_unix_nano') or log.get('time_ns') or 0,
                           'log', log.get('id') or log.get('digest'), log))
    candidates.sort(key=lambda row: (row[0], str(row[2])), reverse=True)
    partial = len(candidates) > MAX_EVENTS
    groups = defaultdict(list)
    for stamp, source, source_id, row in reversed(candidates[:MAX_EVENTS]):
        attrs = row.get('attributes') or {}
        resource = row.get('resource_attributes') or {}
        if not isinstance(attrs, dict) or not isinstance(resource, dict):
            partial = True
            continue
        agent = attrs['agent.id'] if 'agent.id' in attrs else resource.get('agent.id')
        session_values = [values[k] for k in SESSION_KEYS for values in (attrs, resource)
                          if k in values and values[k] not in (None, '')]
        session = session_values[0] if session_values else None
        valid_text = lambda value: isinstance(value, str) and bool(value.strip()) and len(value) <= 512
        if (not source_id or not valid_text(row.get('trace_id')) or not valid_text(row.get('service'))
                or agent is not None and not valid_text(agent)
                or any(not valid_text(value) for value in session_values)
                or not (valid_text(agent) or valid_text(session))):
            partial = True
            continue
        owner = _agent_id_from_attributes(attrs, resource, row['service']) if agent else None
        if agent and not owner:
            partial = True
            continue
        scope = (row['trace_id'], row['service'], 'agent' if owner else 'legacy', owner or session,
                 session if valid_text(session) else None)
        tool = attrs.get('tool.name') or attrs.get('gen_ai.tool.name') or attrs.get('mcp.tool.name') or ''
        if not isinstance(tool, str) or len(tool) > 256:
            partial = True
            tool = ''
        arguments = next((attrs[k] for k in ('tool.arguments', 'gen_ai.tool.call.arguments', 'input.value') if k in attrs), None)
        failed = row.get('status') == 'ERROR' or attrs.get('tool.result_status') == 'error' or bool(attrs.get('tool.error')) or attrs.get('agent.activity.result') in ('error','failed')
        success = row.get('status') == 'OK' or attrs.get('tool.result_status') == 'success' or attrs.get('agent.activity.result') == 'success'
        error = attrs.get('tool.error_message') or attrs.get('tool.error')
        error = _safe_text(error) if isinstance(error, str) and error else None
        name = str(row.get('name') or row.get('event_name') or attrs.get('agent.activity.action') or '').casefold()
        groups[scope].append(dict(stamp=stamp, ref={'type':source,'id':source_id},tool=tool,
            args=_fingerprint(arguments, sanitize=False) if arguments is not None else None,
            polling=bool(_POLL.search(tool)) or _polling_arguments(arguments),
            error=_fingerprint(error) if error else None,failed=failed,success=success,
            compact='compact' in name or attrs.get('agent.activity.action') == 'compaction',
            completed=(name == 'agent.run' and (attrs.get('agent.stopped_reason') == 'completed'
                       or attrs.get('agent.run.state') == 'completed'))))
    results=[]
    for scope, events in groups.items():
        def emit(kind,title,rows,detail):
            item=_finding(scope[0],kind,title,'medium',[r['ref'] for r in rows],detail)
            item.update(workflow_pattern=True,analysis_partial=partial,scope_kind=scope[2],
                        agent_id=scope[3] if scope[2]=='agent' else None, producer=scope[1],
                        session_id=scope[4],
                        detail=item['detail']+' Observation over captured evidence; recovery or intentional repetition may be unobserved.')
            results.append(item)
        failures=defaultdict(list);compactions=[];chain=[];unresolved={}
        for event in events:
            stamp=event['stamp']
            if event['compact'] and stamp:
                compactions=[r for r in compactions if stamp-r['stamp']<=600_000_000_000]+[event]
                if len(compactions)==3:
                    emit('repeated_compaction','Repeated recorded compaction',compactions,'Three compaction records within ten minutes. This does not establish context loss.')
            if not event['tool']:
                continue
            key=(event['tool'],event['args'])
            if event['failed']:
                unresolved.setdefault(key, []).append(event)
                fkey=(event['tool'],event['error'])
                failures[fkey]=[r for r in failures[fkey] if stamp and r['stamp'] and stamp-r['stamp']<=300_000_000_000]+[event]
                if len(failures[fkey])==3 and stamp and event['error']:
                    emit('repeated_tool_error','Repeated errors from '+_safe_text(event['tool']),failures[fkey], 'Three matching sanitized error fingerprints within five minutes; retries may be intentional.')
            elif event['success'] and event['args'] is not None:
                unresolved.pop(key,None)
            if event['args'] is not None and not event['polling']:
                chain=(chain+[event]) if chain and (chain[-1]['tool'],chain[-1]['args'])==key else [event]
                if len(chain)==5:
                    emit('repeated_equivalent_call','Repeated equivalent tool calls',chain,'Five consecutive captured calls share a canonical tool/argument fingerprint. No claim of wasted work is made.')
            else:
                chain=[]
        completed=max((e['stamp'] for e in events if e['completed'] and e['stamp']), default=0)
        pending=[failure for failures in unresolved.values() for failure in failures
                 if failure['stamp'] and failure['stamp'] <= completed]
        # An outcome without an explicit agent identity cannot be attributed to a
        # worker in a multi-worker trace. A recorded per-agent run ending can.
        if completed and pending:
            emit('completion_unresolved_error','Completion with recovery unobserved',pending[:20],
                 'Agent reported completion; matching successful operations were not observed for these errors in the loaded window.')
    return results[:32]
