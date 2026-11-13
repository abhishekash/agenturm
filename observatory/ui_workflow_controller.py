"""Background-only workflow persistence, acknowledgement and notification delivery."""
from __future__ import annotations
from collections import OrderedDict
import threading
import time

from .attention import AttentionLedger, classify, fresh_observation
from .ui_monitoring import task_run_usage
from .workflow_evidence import scope_for, safe_text
from .workflow_store import WorkflowStore, WorkflowStoreError
from .error_reminders import event_receipt


def prepared_item(item, snapshot):
    selected = dict(item)
    usage = task_run_usage(snapshot.sessions, item, snapshot.traces)
    if any(key in getattr(snapshot, 'read_errors', {}) for key in ('sessions', 'traces')):
        usage = {}
    if selected.get('trace_id') and usage.get('trace_id') != selected['trace_id']:
        usage = {}
    if usage.get('trace_id') and not selected.get('trace_id'):
        selected['trace_id'] = usage['trace_id']
    if usage.get('trace_id') and not usage.get('start_unix_nano'):
        trace = next((t for t in snapshot.traces if t.get('trace_id') == usage['trace_id'] and
                      (not t.get('service') or t['service'] == item.get('service'))), {})
        usage = {**usage, 'start_unix_nano': trace.get('start_unix_nano')}
    turn_owner=usage.get('scope_owner') or {}
    if (usage.get('turn_id') and turn_owner.get('producer')==selected.get('service') and
            turn_owner.get('agent_id')==selected.get('agent_id') and turn_owner.get('session_id')==selected.get('session_id')):
        selected['turn_id']=usage['turn_id']
    selected['_workflow_usage'] = usage
    return selected


class WorkflowController:
    """A serialized worker keeps filesystem and subprocess work off the UI thread."""
    def __init__(self, owner, *, store_path=None):
        self.owner = owner
        self.store_path = store_path
        self.store = None
        self.ledger = AttentionLedger()
        self.receipts = {}
        self.error_seen = set()
        self._pending_error_seen = set()
        self._error_seen = OrderedDict()
        self.warning = ''
        self._lock = threading.RLock()
        self._drained = threading.Condition(self._lock)
        self._queue = []
        self._running = False
        self._closed = False
        self._store_attempted = False
        self._latest = None
        self._memory = False
        self._storage_error = ''

    def close(self):
        # Stop observations/delivery; preserve already accepted user receipts.
        with self._drained:
            self._closed = True
            self._queue = [job for job in self._queue if job[0] in {'ack', 'error_seen','review_completions'}]
            deadline = time.monotonic()+3
            while self._running:
                remaining = deadline-time.monotonic()
                if remaining<=0:
                    self.warning = 'Workflow receipt flush timed out; durability is not confirmed.'
                    import sys
                    print(self.warning, file=sys.stderr)
                    return
                self._drained.wait(remaining)

    def update(self, items, snapshot, limits):
        if self._closed:
            return
        options = dict(endpoint=self.owner.feed.api.endpoint,
            notify=bool(self.owner.workflow_notify and not self.owner.is_headless),
            snooze=self.owner.workflow_snooze,
            persist=bool(self.owner.preferences.enabled and not self.owner.is_headless))
        self._enqueue(('update', (list(items), snapshot, dict(limits), options)))

    def _enqueue(self, job):
        with self._lock:
            if self._closed:
                return
            if job[0] == 'update':
                self._queue = [queued for queued in self._queue if queued[0] != 'update']
            self._queue.append(job)
            if self._running:
                return
            self._running = True
        self.owner.run_worker(self._drain, thread=True, group='workflow-state', exit_on_error=False)

    def _drain(self):
        while True:
            with self._lock:
                if not self._queue:
                    self._running = False
                    self._drained.notify_all()
                    return
                kind, args = self._queue.pop(0)
            message = ''
            try:
                if kind == 'update':
                    self._observe(*args)
                elif kind == 'ack':
                    self._ack(*args)
                elif kind == 'review_completions':
                    self._review_completions(args[0])
                elif kind == 'error_seen':
                    self._open_storage(args[1])
                    self._remember_errors(args[0])
                    if self.store:
                        self.store.mark_error_seen(args[0])
                    elif not self._memory:
                        raise WorkflowStoreError('Error was marked seen for this monitor; durable storage is unavailable')
                elif kind == 'retry':
                    self._retry(*args)
                elif kind == 'cleanup':
                    if not self.store and self._latest and self._latest[3]['persist']:
                        self.store = WorkflowStore(self.store_path)
                        self.ledger.restore(self.store.load())
                        self._remember_errors(self.store.load_error_seen())
                        self.store.mark_error_seen(self._error_seen)
                        self._persist()
                    if self.store:
                        self.store.cleanup()
                        self.warning = 'Retention cleanup completed. Unresolved incidents were kept.'
                    else:
                        self.warning = 'Durable workflow storage is unavailable; no files changed.'
            except (WorkflowStoreError, OSError, ValueError) as exc:
                self.warning = 'Workflow action unavailable: ' + str(exc) + '. Monitoring continues.'
                message = self.warning
                if self._closed:
                    import sys
                    print(self.warning,file=sys.stderr)
            if self._closed:
                continue
            rows = list(self.ledger.rows)
            receipts = {key: dict(value) for key, value in self.ledger.records.items()}
            try:
                self.owner.call_from_thread(self._publish, rows, receipts, self.warning, message, set(self._error_seen))
            except RuntimeError:
                # UI teardown must not abandon accepted durable actions.
                continue

    def _publish(self, rows, receipts, warning, message, error_seen=None):
        if self._closed:
            return
        def presentation(values):
            return tuple(sorted((key, r.get('scope_key'), r.get('acknowledgement'),
                                 r.get('delivery_status')) for key, r in values.items()
                                if r.get('acknowledgement') or r.get('delivery_status')))
        changed = (presentation(self.receipts) != presentation(receipts)
                   or getattr(self, '_published_warning', '') != warning
                   or bool(set(error_seen or ()) - self.error_seen))
        self._pending_error_seen.difference_update(error_seen or ())
        self.error_seen=set(error_seen or ()) | self._pending_error_seen
        self._published_warning = warning
        self.receipts = receipts
        self.warning = warning
        # Observation timestamps advance on every poll. Only user-visible receipt
        # changes require a second monitor render after the feed's own update.
        self.owner.workflow_ledger.rows = rows
        if changed:
            self.owner.last_update = -1
        if message:
            self.owner.notify(message, severity='warning', timeout=8)
        from .ui_workflow import WorkflowScreen
        if isinstance(self.owner.screen, WorkflowScreen):
            self.owner.screen.render_report()

    def _observe(self, items, snapshot, limits, options):
        self._latest = (items, snapshot, limits, options)
        self._open_storage(options['persist'])
        self.ledger.update([prepared_item(item, snapshot) for item in items], snapshot, limits,
                           endpoint=options['endpoint'])
        self._persist()
        if self.store and options['notify'] and time.time() >= options['snooze'] and not self._closed:
            self._deliver(self.ledger.new_incidents)

    def _open_storage(self, persist):
        if not self._store_attempted:
            self._store_attempted = True
            self._memory = not persist
            if persist:
                try:
                    self.store = WorkflowStore(self.store_path)
                    self.ledger.restore(self.store.load())
                    self._remember_errors(self.store.load_error_seen())
                except WorkflowStoreError as exc:
                    self._storage_error = str(exc)
                    self.store = None

    def _persist(self):
        if self.store:
            result = self.store.observe((i for i in self.ledger.incidents.values() if i.persistent and
                                        i.scope_key in (self.ledger._observed_scopes or set())),
                                        resolved=self.ledger.resolved)
            self.ledger.restore(result.records)
            for identity in result.unpersisted:
                self.ledger.records[identity] = {**self.ledger.incidents[identity].to_record(), 'memory_only': True}
            overflow=sum(bool(record.get('memory_only')) for identity,record in self.ledger.records.items()
                         if identity in self.ledger.incidents)
            self.warning = (f'Workflow storage capacity reached; {overflow} incident(s) are memory only. '
                            'Acknowledgement and desktop delivery are unavailable for them.' if overflow else '')
        elif not self._memory:
            for identity,incident in self.ledger.incidents.items():
                self.ledger.records[identity]={**incident.to_record(),'memory_only':True}
            self.warning = ('Durable workflow state is unavailable: ' + self._storage_error +
                '. Incidents remain visible; acknowledgements and alerts are disabled. Settings → Cleanup retries storage.')
        if not self.store:
            self.ledger.records={key:record for key,record in self.ledger.records.items() if key in self.ledger.incidents}
        self.ledger.render_rows(snapshot=self._latest[1] if self._latest else None)

    def reviewed(self, item):
        kind = {'Completed': 'completion', 'Interrupted': 'interruption', 'Error': 'failure',
                'Failed': 'failure', 'Incomplete': 'failure', 'Possibly stalled': 'stall'}.get(item.get('state'))
        if not kind:
            return False
        scope = scope_for(prepared_item(item, self.owner.feed.snapshot),
                          self.owner.feed.snapshot, self.owner.feed.api.endpoint)
        key = scope.scope_key
        return any(r.get('scope_key') == key and r.get('kind') == kind and r.get('acknowledgement')
                   for r in self.receipts.values())

    def seen(self, item):
        selected = prepared_item(item, self.owner.feed.snapshot)
        scope = scope_for(selected, self.owner.feed.snapshot, self.owner.feed.api.endpoint)
        requests = classify(selected, scope, {}, {}, now=time.time(), events=self.owner.feed.snapshot.activity)
        return any(self.receipts.get(incident.id, {}).get('acknowledgement') for incident in requests
                   if incident.kind in {'approval', 'reply'})

    def acknowledge(self, item, *, action='seen'):
        prepared = prepared_item(item, self.owner.feed.snapshot)
        scope = scope_for(prepared, self.owner.feed.snapshot, self.owner.feed.api.endpoint)
        self._enqueue(('ack', (scope.scope_key, prepared.get('_attention_id'), action)))

    def review_completions(self, incident_ids):
        # Freeze exact selected completion identities; later arrivals are excluded.
        self._enqueue(('review_completions',(tuple(incident_ids),)))

    def _review_completions(self, identities):
        chosen=[self.ledger.incidents[identity] for identity in dict.fromkeys(identities)
            if identity in self.ledger.incidents and self.ledger.incidents[identity].kind=='completion'
            and not self.ledger.records.get(identity,{}).get('acknowledgement')]
        if self.store:
            if any(not incident.persistent or self.ledger.records.get(incident.id,{}).get('memory_only') for incident in chosen):
                raise WorkflowStoreError('Selected completion lacks durable identity or capacity')
            self.store.review_completions((incident.namespace,incident.id,incident.scope_key) for incident in chosen)
            self.ledger.restore(self.store.load())
        else:
            for incident in chosen:self._ack(incident.scope_key,incident.id,'reviewed')
        self.ledger.render_rows(snapshot=self._latest[1] if self._latest else None)

    def error_displayed(self, events, *, snapshot=None):
        """Only call after actual error details are opened, never for a count."""
        keys = {key for event in events if (key := event_receipt(
            event, snapshot if snapshot is not None else self.owner.feed.snapshot, self.owner.feed.api.endpoint))}
        keys -= self.error_seen
        if not keys:
            return
        self.error_seen.update(keys)
        self._pending_error_seen.update(keys)
        self.owner.last_update = -1
        persist=bool(self.owner.preferences.enabled and not self.owner.is_headless)
        self._enqueue(('error_seen', (keys,persist)))

    def _remember_errors(self, identities):
        from .workflow_store import MAX_INCIDENTS
        for identity in identities:
            self._error_seen[identity]=None
        while len(self._error_seen)>MAX_INCIDENTS:
            self._error_seen.popitem(last=False)

    def _ack(self, scope_key, incident_id, action):
        chosen = [i for i in self.ledger.incidents.values() if i.scope_key == scope_key and
                  (i.id == incident_id if incident_id else i.kind in ({'completion'} if action=='reviewed' else {'approval','reply','failure','interruption','stall'}))]
        if not incident_id and chosen:chosen=[min(chosen,key=lambda i:(i.priority,-i.source_time_ns))]
        if not chosen:
            raise WorkflowStoreError('No recorded attention incident is available for this execution')
        for incident in chosen:
            record = self.ledger.records.get(incident.id, {})
            if not incident.persistent or record.get('memory_only'):
                raise WorkflowStoreError('No durable run/request identity or available capacity for this incident')
            enabled = not bool(record.get('acknowledgement'))
            kind = 'seen' if incident.kind in {'approval', 'reply'} else action
            if self.store:
                self.store.acknowledge(incident.namespace, incident.id, incident.scope_key,
                                       action=kind, enabled=enabled)
            elif self._memory:
                self.ledger.records[incident.id] = {**incident.to_record(), 'acknowledgement': kind if enabled else None}
            else:
                raise WorkflowStoreError('Durable workflow storage is unavailable')
        if self.store:
            self.ledger.restore(self.store.load())
        self.ledger.render_rows(snapshot=self._latest[1] if self._latest else None)

    def retry(self, item):
        self._enqueue(('retry', (item.get('_attention_id'),)))

    def cleanup(self):
        self._enqueue(('cleanup', ()))

    def _retry(self, incident_id):
        incident = self.ledger.incidents.get(incident_id)
        if not incident or not self.store:
            raise WorkflowStoreError('This incident has no persisted failed delivery to retry')
        if not self._latest or not self._latest[3]['notify'] or time.time() < self._latest[3]['snooze']:
            raise WorkflowStoreError('Enable desktop alerts and end snooze before retrying')
        item = self.ledger.items.get(incident.scope_key, {})
        if not fresh_observation(item, self._latest[1], time.time()):
            raise WorkflowStoreError('Capture is stale; wait for a fresh observation before retrying')
        self._deliver([incident], retry=True)

    def _deliver(self, incidents, *, retry=False):
        claimed = []
        for incident in incidents:
            if self._closed:
                break
            if self.store.claim(incident.namespace, incident.id, incident.scope_key, retry=retry):
                claimed.append(incident)
        if not claimed or self._closed:
            return
        projects = []
        for incident in claimed:
            item = self.ledger.items.get(incident.scope_key, {})
            projects.append(safe_text(item.get('project') or item.get('runtime') or 'Local agent').replace('\n', ' ')[:40])
        project = ', '.join(dict.fromkeys(projects))[:120]
        from .ui_workflow import desktop_notify
        result = desktop_notify('Agenturm · ' + project,
            str(len(claimed)) + ' recorded attention incident(s). Open Workflow (w) to inspect.')
        status = 'delivered' if 'delivered' in result else 'unavailable' if 'unavailable' in result else 'failed'
        for incident in claimed:
            self.store.finish_delivery(incident.namespace, incident.id, status=status, detail=result)
        self.ledger.restore(self.store.load())
        self.ledger.render_rows(snapshot=self._latest[1])
        if not self._closed:
            self.owner.call_from_thread(setattr, self.owner, 'workflow_delivery', result)
