"""Pure attention incidents: recorded identity, conservative resolution, no I/O."""
from __future__ import annotations

from dataclasses import asdict, dataclass, field, replace
import hashlib
import json
import math
import time

from .workflow_evidence import ExecutionScope, scope_for

REQUESTS = {'approval', 'reply'}
OUTCOMES = {'failure', 'interruption', 'completion'}
REASONS = {'approval': 'Approval requested', 'reply': 'Reply requested',
           'failure': 'Reported task failure', 'interruption': 'Task interrupted',
           'completion': 'Completion ready for review',
           'stall': 'Possible stall · inspect activity and collector health',
           'tokens': 'Task token threshold reached', 'cost': 'Reported task cost threshold reached',
           'minutes': 'Task elapsed-time threshold reached'}
PRIORITY = {'approval': 0, 'reply': 0, 'failure': 1, 'interruption': 1,
            'stall': 2, 'tokens': 3, 'cost': 3, 'minutes': 3, 'completion': 4}


def stable_id(*values):
    return hashlib.sha256(json.dumps(values, separators=(',', ':'), sort_keys=True).encode()).hexdigest()


def fresh_observation(item, snapshot, now):
    updated = getattr(snapshot, 'updated', 0) or 0
    if not updated or getattr(snapshot, 'error', '') or now - updated > 15:
        return False
    if getattr(snapshot, 'read_errors', {}) and not ('diagnostics' in item and 'run_state' in item):
        # A successful health read can accompany retained, failed database reads.
        return False
    diagnostics = item.get('diagnostics') or {}
    read = diagnostics.get('last_success_ns') or item.get('collector_updated_ns')
    return not (diagnostics.get('backlog_bytes', 0) or diagnostics.get('read_error') or
                (read and now - read / 1e9 > 15) or
                item.get('state') in {'Catching up', 'Monitor delayed', 'Unconfirmed'})


@dataclass(frozen=True)
class Incident:
    id: str
    namespace: str
    scope_key: str
    kind: str
    anchor: str
    source_time_ns: int
    first_observed_time: float
    priority: int
    reason: str
    persistent: bool
    alertable: bool
    execution: dict = field(default_factory=dict)

    def to_record(self):
        return asdict(self)


def classify(item, scope, usage, limits, *, now, stall_anchor='', events=()):
    """Each cause gets an independent identity; display/reason changes do not reset it."""
    state = item.get('state')
    waiting = str(item.get('waiting_for') or '').lower()
    candidates = []
    if state == 'Waiting' and any(x in waiting for x in ('approval', 'permission', 'user input', 'input request')):
        kind = 'approval' if 'approval' in waiting or 'permission' in waiting else 'reply'
        request = str(item.get('request_id') or item.get('request_event_id') or '')
        stamp = item.get('request_time_ns') or 0
        if not request:
            exact = [e for e in events if scope.matches(e) and e.get('action') in
                     ({'approval_request', 'permission_request'} if kind == 'approval' else {'request_user_input', 'input_request'})]
            if exact:
                event = max(exact, key=lambda e: e.get('time_unix_nano') or 0)
                request = str(event.get('source_id') or event.get('id') or '')
                stamp = event.get('time_unix_nano') or 0
        candidates.append((kind, 'request:' + request if request else 'unanchored-request', stamp, bool(request)))
    if state in {'Error', 'Failed', 'Incomplete', 'Interrupted', 'Completed'}:
        kind = 'completion' if state == 'Completed' else 'interruption' if state == 'Interrupted' else 'failure'
        anchor = 'outcome:' + str(item.get('outcome_id') or scope.run_anchor or 'unavailable')
        candidates.append((kind, anchor, item.get('outcome_time_ns') or item.get('task_outcome_ns') or item.get('state_since_ns') or 0, True))
    if state == 'Possibly stalled' and stall_anchor:
        candidates.append(('stall', stall_anchor, 0, True))
    for kind in ('tokens', 'cost', 'minutes'):
        threshold = limits.get(kind, 0)
        if not isinstance(threshold, (float, int)) or not math.isfinite(threshold) or threshold <= 0:
            continue
        if kind == 'tokens':
            value = sum(usage.get(k, 0) or 0 for k in ('input_tokens', 'output_tokens')
                        if usage.get(k + '_reported') is not False)
        elif kind == 'cost':
            value = usage.get('known_cost_usd')
        else:
            start = scope.run_start_ns or usage.get('start_unix_nano')
            value = max(0, now - start / 1e9) / 60 if start and item.get('active') else None
        if value is not None and value >= threshold:
            # A policy edit affects only this threshold's receipt, not a request.
            candidates.append((kind, 'policy:' + stable_id('v1', kind, float(threshold)), 0, True))
    incidents = []
    for kind, anchor, stamp, anchored in candidates:
        persistent = scope.persistent and anchored
        # An unanchored wait stays visible, but cannot obtain a durable receipt.
        identity = stable_id(scope.scope_key, kind, anchor)
        incidents.append(Incident(identity, scope.namespace or 'unavailable', scope.scope_key, kind,
            anchor, int(stamp) if isinstance(stamp, int) and stamp > 0 else 0, now,
            PRIORITY[kind], REASONS[kind], persistent, persistent and kind != 'completion',
            dict(producer=scope.producer,agent_id=scope.agent_id,session_id=scope.session_id,
                 run_id=scope.run_id,run_start_ns=scope.run_start_ns,run_end_ns=scope.run_end_ns,turn_id=scope.turn_id,kind=scope.kind,
                 endpoint_fallback=scope.endpoint_fallback,source_id=str(item.get('id') or ''))))
    return incidents


class AttentionLedger:
    """Missing observations retain incidents. Only fresh positive evidence resolves them."""
    def __init__(self):
        self.rows = []
        self.incidents = {}
        self.records = {}
        self.items = {}
        self.scopes = {}
        self.resolved = set()
        self.new_incidents = []
        self.initialized_namespaces = set()
        self.initialized_scopes = set()
        self.observed_ids = set()
        self._visible_namespaces = None
        self._observed_scopes = None

    def restore(self, records):
        previous_records = self.records
        self.records = {record['id']: dict(record) for record in records}
        # Capacity overflow is deliberately memory-only; keep its minimal receipt
        # while it is still an unresolved incident in this cockpit.
        self.records.update({key:record for key,record in previous_records.items()
                             if record.get('memory_only') and key in self.incidents and key not in self.records})
        for record in records:
            self.observed_ids.add(record['id'])
            incident = self.incidents.get(record['id'])
            if incident:
                self.incidents[incident.id] = replace(incident, first_observed_time=record['first_observed_time'])
            elif record.get('resolved_at') is None:
                ref = record.get('execution') or {}
                if not ref:
                    continue
                scope = ExecutionScope(record['namespace'],ref.get('producer',''),ref.get('agent_id',''),
                    ref.get('session_id',''),ref.get('run_id',''),ref.get('run_start_ns'),
                    ref.get('kind','unavailable'),bool(ref.get('endpoint_fallback')),
                    ref.get('run_end_ns'),str(ref.get('turn_id') or ''))
                if scope.scope_key != record['scope_key']:
                    continue
                self.incidents[record['id']] = Incident(**{k:record[k] for k in Incident.__dataclass_fields__})
                self.scopes[scope.scope_key] = scope
                state = {'approval':'Waiting','reply':'Waiting','failure':'Failed','interruption':'Interrupted',
                         'completion':'Completed','stall':'Possibly stalled'}.get(record['kind'],'Unknown')
                self.items.setdefault(scope.scope_key,dict(id=ref.get('source_id') or record['id'],
                    service=scope.producer,agent_id=scope.agent_id,session_id=scope.session_id,
                    trace_id=scope.run_id,run_start_ns=scope.run_start_ns,turn_id=scope.turn_id,state=state,
                    task='Retained '+record['reason'].lower(),waiting_for='Approval' if record['kind']=='approval' else
                    'User input' if record['kind']=='reply' else ''))

    def update(self, items, snapshot, limits, now=None, *, endpoint=''):
        now = time.time() if now is None else now
        notifications = []
        self.new_incidents = []
        self.resolved = set()
        visible_namespaces = {scope_for({}, snapshot, endpoint).namespace or 'unavailable'}
        observed_scopes = set()
        fresh_scopes = set()
        by_scope = {}
        pending_by_agent = {}
        for incident in self.incidents.values():
            by_scope.setdefault(incident.scope_key,[]).append(incident)
            scope = self.scopes.get(incident.scope_key)
            if scope and incident.kind in REQUESTS | {'stall'}:
                key=(scope.namespace,scope.producer,scope.agent_id,scope.session_id)
                pending_by_agent.setdefault(key,[]).append(incident)
        for original in items:
            item = dict(original)
            scope = scope_for(item, snapshot, endpoint)
            visible_namespaces.add(scope.namespace or 'unavailable')
            observed_scopes.add(scope.scope_key)
            self.items[scope.scope_key] = item
            self.scopes[scope.scope_key] = scope
            fresh = fresh_observation(item, snapshot, now)
            if fresh:fresh_scopes.add(scope.scope_key)
            # Recover an ongoing stall episode across cockpit restarts.
            previous_stall = next((i.to_record() for i in by_scope.get(scope.scope_key,()) if i.kind=='stall'),None)
            stall_anchor = (previous_stall['anchor'] if previous_stall else
                            'episode:' + stable_id(scope.run_anchor, now)) if fresh else ''
            current = classify(item, scope, item.get('_workflow_usage') or {}, limits, now=now,
                               stall_anchor=stall_anchor, events=getattr(snapshot, 'activity', ()))
            current_ids = {i.id for i in current}
            if fresh:
                # Exact same run positively advanced or replaced its pending request.
                candidates = {i.id:i for i in by_scope.get(scope.scope_key,())}
                candidates.update((i.id,i) for i in pending_by_agent.get(
                    (scope.namespace,scope.producer,scope.agent_id,scope.session_id),()))
                for old in candidates.values():
                    same_run = old.scope_key == scope.scope_key
                    old_scope = self.scopes.get(old.scope_key)
                    new_run = old_scope and old_scope.namespace == scope.namespace and old_scope.producer == scope.producer and \
                        (old_scope.agent_id, old_scope.session_id) == (scope.agent_id, scope.session_id) and \
                        old_scope.run_anchor != scope.run_anchor and bool(scope.run_anchor)
                    if old.kind in REQUESTS | {'stall'} and (same_run or new_run) and old.id not in current_ids:
                        # An unknown state or collector delay never proves resolution.
                        if item.get('state') not in {'Unknown', 'Offline', 'Spawn recorded', 'Unconfirmed'}:
                            self.resolved.add((old.namespace, old.id))
                            self.incidents.pop(old.id, None)
                            if old.id in self.records:
                                self.records[old.id]['resolved_at'] = now
            for incident in current:
                old = self.incidents.get(incident.id)
                record = self.records.get(incident.id, {})
                first = old.first_observed_time if old else record.get('first_observed_time', now)
                incident = replace(incident, first_observed_time=first)
                self.incidents[incident.id] = incident
                if (incident.id not in self.observed_ids and incident.namespace in self.initialized_namespaces
                        and incident.scope_key in self.initialized_scopes and fresh and incident.alertable):
                    self.new_incidents.append(incident)
                    notifications.append((item, [incident.reason]))
                self.observed_ids.add(incident.id)
        self.initialized_namespaces.update(visible_namespaces)
        # Records are bounded by the durable store; old payloads and dedup keys
        # need not survive after resolution or a reviewed item leaves the roster.
        for identity, incident in list(self.incidents.items()):
            record = self.records.get(identity, {})
            if incident.scope_key not in observed_scopes and (record.get('resolved_at') is not None or
                    record.get('acknowledgement') and incident.kind not in REQUESTS or
                    record.get('memory_only') and incident.kind not in REQUESTS):
                del self.incidents[identity]
        retained = observed_scopes | {i.scope_key for i in self.incidents.values()}
        self.items = {key: value for key, value in self.items.items() if key in retained}
        self.scopes = {key: value for key, value in self.scopes.items() if key in retained}
        self.observed_ids.intersection_update(set(self.records) | set(self.incidents))
        # Discovery can finish after an initially empty health response. First
        # inventory for each execution is silent, including restored receipts.
        self.initialized_scopes.intersection_update(retained)
        self.initialized_scopes.update(fresh_scopes)
        self._visible_namespaces = visible_namespaces
        self._observed_scopes = observed_scopes
        # Memory-only overflow never retains a missing agent's task/command text.
        for incident in self.incidents.values():
            if incident.scope_key not in observed_scopes and self.records.get(incident.id, {}).get('memory_only'):
                old = self.items.get(incident.scope_key, {})
                self.items[incident.scope_key] = {key: old[key] for key in
                    ('id', 'service', 'runtime', 'agent_id', 'session_id', 'trace_id', 'run_start_ns', 'state') if key in old}
        self.render_rows(visible_namespaces, observed_scopes, snapshot, now)
        return notifications

    def render_rows(self, namespaces=None, observed_scopes=None, snapshot=None, now=None):
        namespaces = self._visible_namespaces if namespaces is None else namespaces
        observed_scopes = self._observed_scopes if observed_scopes is None else observed_scopes
        rows = []
        for incident in sorted(self.incidents.values(), key=lambda i: (i.priority, i.first_observed_time, i.id)):
            if namespaces is not None and incident.namespace not in namespaces:
                continue
            item = self.items.get(incident.scope_key)
            if item is None:
                continue
            record = self.records.get(incident.id, {})
            ack = record.get('acknowledgement')
            if ack and incident.kind not in REQUESTS:
                continue
            stale = observed_scopes is not None and incident.scope_key not in observed_scopes
            if snapshot is not None:
                stale = stale or not fresh_observation(item, snapshot, time.time() if now is None else now)
            prepared = dict(item, _attention_id=incident.id, _attention_kind=incident.kind,
                _attention_scope=incident.scope_key, _attention_namespace=incident.namespace,
                _attention_seen=bool(ack), _attention_persistent=incident.persistent and not record.get('memory_only'),
                _attention_source_time_ns=incident.source_time_ns, _attention_stale=stale,
                _attention_delivery=record.get('delivery_status') or '')
            reason = incident.reason + (' · seen; still pending' if ack else '')
            rows.append((prepared, [reason], incident.first_observed_time))
        self.rows = rows
