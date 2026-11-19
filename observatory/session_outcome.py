"""Exact execution evidence for session outcomes and trace navigation."""
import json

from .local_activity import safe
from .session_prompts import event_in_scope, prompt_scope
from .workflow_evidence import scope_for


def current_turn_events(item, events, snapshot=None):
    scope = scope_for(item, snapshot)
    if not scope.run_anchor:
        return []
    return sorted((event for event in events if scope.matches(event)),
                  key=lambda event: event.get('time_unix_nano') or 0, reverse=True)


def interruption_event(item, events, snapshot=None):
    if item.get('state') != 'Interrupted' or item.get('capture_content') is False:
        return None
    stamp = item.get('outcome_time_ns') or item.get('task_outcome_ns') or item.get('state_since_ns')
    return next((event for event in current_turn_events(item, events, snapshot)
                 if event.get('domain') == 'agent' and event.get('action') == 'interrupted'
                 and (not stamp or event.get('time_unix_nano') == stamp)), None)


def interruption_reason(event):
    detail = (event or {}).get('detail') or ''
    try:
        payload = json.loads(detail) if isinstance(detail, str) else detail
    except (ValueError, RecursionError):
        payload = {}
    payload = payload if isinstance(payload, dict) else {}
    native = payload.get('event')
    native = native if native in {'turn_aborted', 'task_aborted'} else 'an interruption event'
    reason = payload.get('reason')
    reason = safe(reason, 300).strip() if isinstance(reason, str) else ''
    return native, reason


def linked_trace(item, events, traces):
    """Prefer the selected execution; never infer identity from task text."""
    scope = prompt_scope(item)
    if not scope:
        return None

    def owned(trace):
        # Trace summaries may use services rather than service.
        service = trace.get('service')
        services = trace.get('services') or []
        if not service and len(services) == 1:
            service = services[0]
        return event_in_scope({**trace, 'service': service}, scope)

    def compatible(trace):
        # An explicit link already establishes identity. Older summary formats
        # omit ownership fields; reject contradictions, not absent metadata.
        service, session, agent, _ = scope
        services = trace.get('services') or []
        return (all(not expected or not trace.get(key) or trace[key] == expected
                    for key, expected in [('service', service), ('session_id', session), ('agent_id', agent)])
                and (not service or not services or service in services))

    by_id = {trace.get('trace_id'): trace for trace in traces}
    explicit = item.get('trace_id') or item.get('latest_trace_id')
    if explicit:
        # A native lifecycle link can be fetched even outside the loaded page.
        if explicit not in by_id or compatible(by_id[explicit]):
            return {'trace_id': explicit}
        return None
    if item.get('state') == 'Interrupted':
        return None
    for event in sorted(events, key=lambda row: row.get('time_unix_nano') or 0, reverse=True):
        trace_id = event.get('trace_id')
        if trace_id and event_in_scope(event, scope) and (trace_id not in by_id or compatible(by_id[trace_id])):
            return {'trace_id': trace_id, 'span_id': event.get('span_id')}
    matches = [trace for trace in traces if owned(trace)]
    if matches:
        trace = max(matches, key=lambda row: row.get('last_unix_nano') or row.get('end_unix_nano') or 0)
        return {'trace_id': trace['trace_id']}
    return None
