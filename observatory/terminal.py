"""Receiver lifecycle and live data for the terminal cockpit."""
from __future__ import annotations
from copy import deepcopy
import math
import hashlib
import secrets
from .http_transport import urlopen as receiver_urlopen

import asyncio
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import textwrap
import unicodedata
import urllib.error
import urllib.request
from dataclasses import dataclass, field, replace
from pathlib import Path
from urllib.parse import urlencode, urlsplit

from .feed_requests import ConnectionProgress, FeedScope, RequestBatch
from .shutdown import complete_cleanup


class CockpitError(Exception):
    pass


def clean(value):
    text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", str(value))
    return "".join(char for char in text if unicodedata.category(char) not in {"Cc", "Cf"})


def clipped(value, width):
    result, used = [], 0
    for char in clean(value):
        size = 0 if unicodedata.combining(char) else 2 if unicodedata.east_asian_width(char) in {"W", "F"} else 1
        if used + size > max(0, width):
            break
        result.append(char)
        used += size
    return "".join(result)


def number(value):
    value = float(value or 0)
    if abs(value) >= 1_000_000:
        return f"{value / 1_000_000:.1f}m"
    if abs(value) >= 10_000:
        return f"{value / 1_000:.1f}k"
    return f"{value:,.0f}"


def trace_name(trace):
    if trace.get("task"):
        return trace["task"]
    if trace.get("models"):
        return ", ".join(trace["models"])
    return trace.get("name") or trace.get("trace_id", "Agent run")[:12]


class Api:
    MAX_RESPONSE_BYTES = 16 * 1024 * 1024
    HEALTH_TIMEOUT = 1
    REQUEST_TIMEOUT = 3

    def __init__(self, endpoint, token=None):
        parsed = urlsplit(endpoint)
        if parsed.scheme not in {"http", "https"} or not parsed.hostname or parsed.username is not None or parsed.password is not None or parsed.query or parsed.fragment:
            raise CockpitError("Use a receiver HTTP address for --endpoint.")
        self.endpoint = endpoint.rstrip("/")
        from .receiver_state import endpoint_token
        try:self.token = endpoint_token(endpoint, token)
        except (ValueError, OSError) as exc:raise CockpitError(str(exc)) from exc

    def get(self, path):
        return self._get(path, self.MAX_RESPONSE_BYTES)

    def get_review(self, path):
        return self._get(path, 2 * 1024 * 1024)

    def _get(self, path, max_bytes):
        headers = {"Authorization": f"Bearer {self.token}"} if self.token else {}
        request = urllib.request.Request(self.endpoint + path, headers=headers)
        timeout=self.HEALTH_TIMEOUT if path=='/api/health?probe=1' else self.REQUEST_TIMEOUT
        with receiver_urlopen(request, timeout=timeout) as response:
            raw = response.read(max_bytes + 1)
            headers = getattr(response, 'headers', None)
            receiver = headers.get('X-Agenturm-Receiver-ID') if headers is not None else None
        if len(raw) > max_bytes:
            raise CockpitError(f'Receiver response exceeded the {max_bytes // (1024 * 1024)} MiB limit. Narrow the selected scope.')
        result = json.loads(raw)
        if receiver and isinstance(result, dict):
            if result.get('receiver_id') and result['receiver_id'] != receiver:
                raise CockpitError('Receiver identity does not match the response body. Retry the request.')
            result['receiver_id'] = receiver
        return result


class LocalReceiver:
    def __init__(self, db_path, port, token, local_agents=True, metadata_only=False, managed_db=False,
                 retention_days=None, collector_registry=None):
        from .core import Store
        from .server import BoundedHTTPServer, handler_for
        self.store = Store(db_path, async_derived_refresh=True)
        try:
            self.server = BoundedHTTPServer(("127.0.0.1", port), handler_for(self.store, token, quiet=True))
        except OSError:
            self.store.close()
            raise
        from .receiver_state import retention_policy
        from .server import _start_retention
        policy = retention_policy(self.store, managed=managed_db, requested=retention_days)
        if policy is not None:self.store.prune(policy)
        self.maintenance = _start_retention(self.store, policy) if policy is not None else None
        self.thread = threading.Thread(target=self.server.serve_forever, name="observatory-receiver", daemon=True)
        self.local = None
        if local_agents:
            from .collector_runtime import start_local_collector
            self.local = start_local_collector(self.store, capture_details=not metadata_only,
                                               registry=collector_registry)
        self.store.local_agent_monitor = self.local
        self.thread.start()

    def close(self):
        if getattr(self,"maintenance",None):
            self.maintenance[0].set();self.maintenance[1].join(timeout=2)
        complete_cleanup(
            *([self.local.close] if self.local else []),
            self.server.shutdown, self.server.server_close, self.thread.join,
            self.store.close)


@dataclass
class Snapshot:
    totals: dict = field(default_factory=dict)
    agents: list = field(default_factory=list)
    traces: list = field(default_factory=list)
    detail: dict = field(default_factory=dict)
    findings: list = field(default_factory=list)
    finding_results: list = field(default_factory=list)
    error: str = ""
    updated: float = 0
    local_status: dict = field(default_factory=dict)
    activity: list = field(default_factory=list)
    sessions: list = field(default_factory=list)
    mcp: list = field(default_factory=list)
    mcp_summary: dict = field(default_factory=dict)
    mcp_available: bool = True
    metric_catalog: list = field(default_factory=list)
    metric_points: list = field(default_factory=list)
    usage_series: list = field(default_factory=list)
    usage_service: str | None = None
    usage_session_id: str | None = None
    metric_history: list = field(default_factory=list)
    metrics_available: bool = True
    activity_results: list | None = None
    mcp_results: list | None = None
    queries: dict = field(default_factory=dict)
    pagination: dict = field(default_factory=dict)
    activity_prompts: bool = False
    hierarchy: dict = field(default_factory=dict)
    activity_errors: bool = False
    view_scope: tuple = ()
    sessions_error: str = ""
    sessions_updated: float = 0
    read_errors: dict = field(default_factory=dict)
    read_updated: dict = field(default_factory=dict)
    receiver_id: str = ""
    receiver_time_ns: int = 0
    indexing_ready: bool = False
    review_trace_filter: bool = False
    native_lifecycle: list = field(default_factory=list)
    native_lifecycle_warning: str = ""
    native_lifecycle_prepared: bool = False
    native_bootstrap: bool = False
    bootstrap_updated: float = 0


_POLICY_ID_FIELDS=('source_id','service','agent_id','session_id','runtime','source_namespace',
    'native_session_id','native_worker_id','parent_agent_id','trace_id','turn_id')
_POLICY_METADATA_FIELDS=(*_POLICY_ID_FIELDS,'run_state','state_since_ns','last_record_ns',
    'last_update_ns','run_start_ns','request_id','request_time_ns','outcome_id','outcome_time_ns',
    'relationship_kind','relationship_state','discovery_state','coverage','observed_ns',
    'capture_content','capture_revision','capture_safety_revision','collector_updated_ns')


def _capture_identity(row):
    """A malformed explicit actor must never turn into a session-wide policy."""
    if not isinstance(row,dict):return None
    def text(key):
        value=row.get(key)
        return value if isinstance(value,str)and 0<len(value)<=512 else None
    service=text('service');actor=text('agent_id');session=text('session_id')
    if row.get('agent_id')not in (None,'')and actor is None:return None
    if row.get('session_id')not in (None,'')and session is None:return None
    return (service,actor or '',session or '')if service and(actor or session)else None


def _capture_metadata(row):
    return {key:value for key,value in row.items()if key in _POLICY_METADATA_FIELDS and
        (isinstance(value,str)and len(value)<=512 or type(value)is bool or
         type(value)is int and 0<=value<2**63 or type(value)is float and math.isfinite(value)and 0<=value<2**63)}


def _capture_denial(row,key):
    identity={'source_id','service','agent_id','session_id','runtime','source_namespace','native_session_id','native_worker_id','capture_revision'}
    result={field:value for field,value in _capture_metadata(row).items()if field in identity}
    result.setdefault('source_id','capture-policy:'+hashlib.sha256(json.dumps(key).encode()).hexdigest()[:32])
    result['capture_content']=False
    return result


def _capture_may_match(row,key,denial,*,aggregate=False):
    """Unknown ownership cannot establish that captured content is unrelated."""
    if not isinstance(row,dict):return True
    owner=row.get('review_owner')or row.get('scope_owner')or {}
    if not isinstance(owner,dict):owner={}
    service=owner.get('producer')or row.get('service')or row.get('producer')
    services=row.get('services')
    producers=set(services)if isinstance(services,list)and all(isinstance(value,str)for value in services)else set()
    if service:producers.add(service)
    denied_producers={key[0]}
    if key[0]=='codex-review'and denial.get('runtime')=='Codex':denied_producers.add('codex')
    if producers and producers.isdisjoint(denied_producers):return False
    proven=not aggregate or bool(owner)
    actor=(owner.get('agent_id')or row.get('agent_id'))if proven else None
    session=(owner.get('session_id')or row.get('session_id'))if proven else None
    if key[1]and actor and actor!=key[1]:return False
    if not key[1]and actor:return False
    if key[2]and session and session!=key[2]:return False
    return True


def _capture_policy_matches(row,key,denial):
    producer=row.get('service')
    if producer!=key[0]and not(producer=='codex'and key[0]=='codex-review'and denial.get('runtime')=='Codex'):
        return False
    if key[1]:
        if row.get('agent_id')!=key[1]:return False
    elif row.get('agent_id'):return False
    return not key[2]or row.get('session_id')==key[2]


def _capture_purge(snapshot,local_status):
    """A negative policy update is not a successful data refresh."""
    return replace(snapshot,local_status=local_status,totals={},agents=[],traces=[],detail={},findings=[],
        finding_results=[],activity=[],sessions=[],mcp=[],mcp_summary={},metric_catalog=[],metric_points=[],
        usage_series=[],metric_history=[],activity_results=[],mcp_results=[],pagination={},read_updated={},
        read_errors={},sessions_updated=0,sessions_error='',
        native_lifecycle=[],native_lifecycle_warning='',native_lifecycle_prepared=False,indexing_ready=False)


class Feed:
    POLL_SECONDS = 2
    EXTRA_POLL_SECONDS = 5

    def __init__(self, api):
        self.api = api
        self.snapshot = Snapshot()
        self.service = ""
        self.query = ""
        self.activity_query = ""
        self.activity_prompts = False
        self.activity_errors = False
        self.mcp_query = ""
        self.errors_only = False
        self.selected_trace = None
        self.selected_span_id = None
        # Non-UI consumers retain full snapshots. The cockpit opts into
        # fetching trace payloads only while its trace inspector is visible.
        self.detail_enabled = True
        self.include_global_totals = True
        self.active_page = None
        self.session_id = ''
        self.include_reviews = False
        self.pinned_trace = False
        self.scope_agent_id = ''
        self.scope_subtree = False
        self.hierarchy_revision = ''
        self.generation = 0
        self.extra_cache = None
        self.extra_updated = 0
        self.extra_scope = None
        # Stored session history is global, independent of the selected view.
        # Keep its refresh budget and freshness separate from scoped extras.
        self.sessions_cache = None
        self.sessions_attempted = None
        self.sessions_updated = 0
        self.sessions_error = ''
        self.cache_receiver_id = None
        self.paused = False
        self.history = {page: [None] for page in ('activity','mcp','traces','findings')}
        self.history_scopes = {}
        self.findings_cache = None
        self.findings_scope = None
        self.findings_updated = 0
        self.refresh_once = False
        self.stop = threading.Event()
        self.wake = threading.Event()
        self.thread = threading.Thread(target=self._loop, name="observatory-terminal-feed", daemon=True)
        self._requests = None
        self.connection = ConnectionProgress(started=time.monotonic())
        self.connection_failures = 0
        self.retry_requested = False
        self.prepare = None
        self._capture_lock=threading.RLock()
        # A new Feed cannot accidentally reuse another lifetime's first
        # revocation token and revive a persisted frozen source.
        self._capture_epoch=secrets.randbits(62)+1
        self._capture_receiver=''
        self._capture_global_denied=False
        self._capture_overflow=False
        self._capture_denials={}
        self._capture_revisions={}
        self._capture_global_revision=0

    def _clear_content_caches(self):
        self.sessions_cache=None;self.sessions_attempted=None;self.sessions_updated=0;self.sessions_error=''
        self.extra_cache=None;self.extra_scope=None;self.extra_updated=0
        self.findings_cache=None;self.findings_scope=None;self.findings_updated=0

    def _capture_local(self,local,*,metadata=False):
        if (not metadata and isinstance(local,dict)and not self._capture_global_denied and not self._capture_denials
            and not self._capture_global_revision and not self._capture_revisions and not any(key in local for key in
                ('capture_policy_schema','capture_policy_revisions','capture_global_revision'))):
            return local
        local=deepcopy(local)if isinstance(local,dict)else {}
        rows=local.get('sessions')if isinstance(local.get('sessions'),list)else []
        if metadata:
            local={key:value for key,value in local.items()if type(value)in(bool,int,float)}
            rows=[_capture_metadata(row)for row in rows if isinstance(row,dict)]
        known=set()
        for row in rows:
            matching={key for key,denial in self._capture_denials.items()if _capture_policy_matches(row,key,denial)}
            known.update(matching)
            if self._capture_global_denied or matching:row['capture_content']=False
        source_ids={row.get('source_id')for row in rows}
        for key,denial in self._capture_denials.items():
            if key not in known:
                tombstone=dict(denial)
                if tombstone.get('source_id')in source_ids:
                    tombstone['source_id']='capture-policy:'+hashlib.sha256(json.dumps(key).encode()).hexdigest()[:32]
                rows.append(tombstone);source_ids.add(tombstone['source_id'])
        local['sessions']=rows
        if self._capture_global_denied:local['capture_content']=False
        if self._capture_overflow:local['capture_policy_warning']='Capture-policy scope limit reached; receiver content is withheld.'
        for key in ('capture_policy_schema','capture_policy_revisions','capture_global_revision'):local.pop(key,None)
        if self._capture_global_revision or self._capture_revisions:
            local.update(capture_policy_schema=1,capture_global_revision=self._capture_global_revision,
                capture_policy_revisions=[dict(row)for row in self._capture_revisions.values()])
            from .review_facts import capture_safety_revision
            from types import SimpleNamespace
            policy=SimpleNamespace(receiver_id=self._capture_receiver,local_status=local)
            for row in rows:
                scope=SimpleNamespace(receiver=self._capture_receiver,producer=row.get('service'),
                    agent_id=row.get('agent_id'),session_id=row.get('session_id'))
                revision=capture_safety_revision(policy,scope)
                if revision is not None:row['capture_safety_revision']=revision
                else:row.pop('capture_safety_revision',None)
        return local

    def _capture_note_revisions(self,new_keys,new_global):
        for key in new_keys:
            if len(self._capture_revisions)>=500 and key not in self._capture_revisions:
                self._capture_overflow=True;self._capture_global_denied=True;continue
            denial=self._capture_denials[key]
            self._capture_revisions[key]={field:value for field,value in denial.items()if field in
                ('service','agent_id','session_id','runtime')}
            self._capture_revisions[key]['capture_safety_revision']=self._capture_epoch
        if new_global or self._capture_overflow:self._capture_global_revision=self._capture_epoch

    def _capture_from_health(self,health):
        """Publish new DENY only, independently of data readiness/navigation."""
        if not isinstance(health,dict):return False
        receiver=health.get('receiver_id');local=health.get('local_agents')
        with self._capture_lock:
            if not receiver or receiver!=self.snapshot.receiver_id or not isinstance(local,dict):return False
            if self._capture_receiver!=receiver:
                self._capture_receiver=receiver;self._capture_global_denied=False;self._capture_denials={};self._capture_overflow=False
                self._capture_revisions={};self._capture_global_revision=0
            previous=(self._capture_global_denied,set(self._capture_denials))
            changed=False
            if local.get('capture_content')is False and not self._capture_global_denied:
                self._capture_global_denied=True;changed=True
            rows=local.get('sessions')if isinstance(local.get('sessions'),list)else []
            for row in rows:
                if not isinstance(row,dict)or row.get('capture_content')is not False:continue
                for key in ((_capture_identity(row),) if _capture_identity(row) else ()):
                    if key in self._capture_denials:continue
                    if len(self._capture_denials)>=500:
                        self._capture_overflow=True
                        if not self._capture_global_denied:self._capture_global_denied=True;changed=True
                        break
                    self._capture_denials[key]=_capture_denial({**row,**({'session_id':key[2]}if key[2]else {})},key);changed=True
            if not changed:return False
            self._capture_epoch+=1;self.generation+=1
            self._capture_note_revisions(set(self._capture_denials)-previous[1],self._capture_global_denied and not previous[0])
            old_local=self._capture_local(self.snapshot.local_status,metadata=True)
            self.snapshot=_capture_purge(self.snapshot,old_local)
            self.snapshot.error='Capture policy changed; waiting for a coherent receiver refresh.'
            self._clear_content_caches();self.wake.set()
            return True

    def _capture_guard(self,epoch):
        with self._capture_lock:
            if epoch!=self._capture_epoch:raise InterruptedError('Capture policy changed during this refresh')

    def _capture_commit(self,candidate,health,epoch,*,complete):
        with self._capture_lock:
            if epoch!=self._capture_epoch:return False
            receiver=candidate.receiver_id
            bootstrap_owners={tuple(row.get(key)or ''for key in ('source_id','service','agent_id','session_id'))
                for row in candidate.local_status.get('sessions',())}if candidate.native_bootstrap else set()
            namespace_changed=bool(self.snapshot.receiver_id and self.snapshot.receiver_id!=receiver)
            if self._capture_receiver!=receiver:
                self._capture_receiver=receiver;self._capture_global_denied=False;self._capture_denials={};self._capture_overflow=False
                self._capture_revisions={};self._capture_global_revision=0
            previous=(self._capture_global_denied,set(self._capture_denials))
            local=health.get('local_agents')or {}
            if not complete and not candidate.native_bootstrap and(self._capture_global_denied or self._capture_denials):
                self.snapshot=_capture_purge(self.snapshot,self._capture_local(self.snapshot.local_status,metadata=True))
                self.snapshot.error=self.snapshot.error or 'Capture policy changed; waiting for a coherent receiver refresh.'
                self._clear_content_caches()
                return False
            if complete:
                if local.get('capture_content')is True and not self._capture_overflow:self._capture_global_denied=False
                for row in local.get('sessions',[]):
                    key=_capture_identity(row)
                    if key and row.get('capture_content')is True:self._capture_denials.pop(key,None)
            # Bootstrap/new namespace policies apply only to their own fully
            # attributed snapshot, never to the preceding receiver's content.
            if local.get('capture_content')is False:self._capture_global_denied=True
            for row in local.get('sessions',[]):
                if isinstance(row,dict)and row.get('capture_content')is False:
                    for key in ((_capture_identity(row),) if _capture_identity(row) else ()):
                        if key in self._capture_denials:continue
                        if len(self._capture_denials)<500:self._capture_denials[key]=_capture_denial({**row,**({'session_id':key[2]}if key[2]else {})},key)
                        else:self._capture_global_denied=True;self._capture_overflow=True
            if namespace_changed or (self._capture_global_denied and not previous[0])or set(self._capture_denials)-previous[1]:
                self._capture_epoch+=1;self.generation+=1
                self._capture_note_revisions(set(self._capture_denials)-previous[1],self._capture_global_denied and not previous[0])
            candidate.local_status=self._capture_local(candidate.local_status)
            if self._capture_global_denied or self._capture_denials:
                def allowed(row,aggregate=False):
                    return not self._capture_global_denied and not any(_capture_may_match(row,key,value,aggregate=aggregate)
                        for key,value in self._capture_denials.items())
                for field in ('agents','activity','activity_results','mcp','mcp_results','findings','finding_results',
                              'metric_catalog','metric_points','metric_history','usage_series','native_lifecycle'):
                    setattr(candidate,field,[row for row in getattr(candidate,field)or []if allowed(row)])
                candidate.sessions=[row for row in candidate.sessions if allowed(row,True)]
                candidate.traces=[row for row in candidate.traces if allowed(row,True)]
                if candidate.detail and not allowed(candidate.detail.get('summary')or {},True):candidate.detail={}
                for row in candidate.local_status['sessions']:
                    if not allowed(row):
                        safe=_capture_metadata(row);row.clear();row.update(safe)
                if self._capture_global_denied:candidate.totals={};candidate.mcp_summary={}
                # Keep only the sanitized cache generation. Repeated native
                # metadata-only sessions must not force a new history scan.
                self.sessions_cache=candidate.sessions
                if self.extra_cache is not None:
                    old_page=self.extra_cache[3]
                    page={key:old_page[key]for key in ('next_cursor','agent_scope','receiver_id')if key in old_page}
                    page.update(calls=candidate.mcp_results,summary=candidate.mcp_summary)
                    self.extra_cache=(candidate.mcp,candidate.mcp_results,candidate.mcp_available,page,candidate.mcp_summary,
                        candidate.metric_catalog,candidate.metric_points,candidate.metrics_available,candidate.usage_series,
                        candidate.usage_service,candidate.usage_session_id)
                if self.findings_cache is not None:
                    self.findings_cache={key:self.findings_cache[key]for key in ('next_cursor','agent_scope','receiver_id')if key in self.findings_cache}
                    self.findings_cache['findings']=candidate.finding_results
            if candidate.native_bootstrap:
                from .native_bootstrap import published_metadata
                candidate.local_status=published_metadata(candidate.local_status)
                candidate.local_status['sessions']=[row for row in candidate.local_status['sessions']
                    if tuple(row.get(key)or ''for key in ('source_id','service','agent_id','session_id'))in bootstrap_owners]
            self.snapshot=candidate
            return True

    def _publish_native_bootstrap(self,initial,confirmed,scope,epoch):
        from .native_bootstrap import native_metadata
        local=native_metadata(confirmed)
        receiver=confirmed.get('receiver_id')
        if local is None or receiver!=initial.get('receiver_id'):return False
        # Both authenticated observations can revoke; neither partial positive
        # is allowed to restore an existing Feed denial.
        policy=dict(local);denials=[]
        for health in (initial,confirmed):
            source=health.get('local_agents')or {}
            if source.get('capture_content')is False:policy['capture_content']=False
            rows=source.get('sessions')or []
            if len(rows)>500:policy['capture_content']=False
            denials.extend(_capture_denial(row,key)for row in rows[:500]if isinstance(row,dict)
                and row.get('capture_content')is False and(key:=_capture_identity(row)))
        policy['sessions']=[*local['sessions'],*denials]
        with self._capture_lock:
            if epoch!=self._capture_epoch or self.stop.is_set()or scope!=FeedScope.capture(self):return False
            # A concurrent coherent refresh wins even when navigation did not
            # change. An old bootstrap callback may never replace that frame.
            if self.snapshot.updated:return False
            if self.snapshot.receiver_id and self.snapshot.receiver_id!=receiver:return False
            self._clear_content_caches()
            candidate=Snapshot(local_status=local,receiver_id=receiver,
                receiver_time_ns=confirmed['observed_time_unix_nano'],native_bootstrap=True,
                bootstrap_updated=time.time(),native_lifecycle_prepared=True,
                sessions_error='History indexing · native observations only',
                read_errors={'history':'Initial history indexing is incomplete.'})
            return self._capture_commit(candidate,{'local_agents':policy},epoch,complete=False)

    def connection_stage(self, stage):
        self.connection=replace(self.connection,stage=stage)

    def retry_delay(self):
        return min(2.0, .5 * max(1,self.connection_failures))

    def refresh(self):
        retry = self.retry_requested
        self.retry_requested=False
        self.connection=ConnectionProgress(
            phase='reconnecting' if self.snapshot.error else 'loading' if self.snapshot.updated else 'connecting',
            attempt=self.connection_failures+1,started=time.monotonic())
        with self._capture_lock:
            requested_scope = FeedScope.capture(self)
            capture_epoch=self._capture_epoch
        load_mcp=requested_scope.active_page in {None,'mcp'}
        load_metrics=requested_scope.active_page in {None,'metrics'}
        load_findings=requested_scope.active_page in {None,'findings'}
        service, query, errors_only, selected = (requested_scope.service, requested_scope.query,
                                                requested_scope.errors_only, requested_scope.selected_trace)
        activity_query, mcp_query = requested_scope.activity_query, requested_scope.mcp_query
        activity_prompts = requested_scope.activity_prompts
        activity_errors = requested_scope.activity_errors
        requested_span = requested_scope.selected_span_id
        session_id, include_reviews = requested_scope.session_id, requested_scope.include_reviews
        scope_agent, scope_subtree = requested_scope.scope_agent_id, requested_scope.scope_subtree
        for page, text in (('activity',activity_query),('mcp',mcp_query),('traces',query),
                           ('findings','')):
            scope=(service,session_id,text,include_reviews,scope_agent,scope_subtree,
                   requested_scope.hierarchy_revision)
            if page=='activity':
                scope+=(activity_prompts,activity_errors)
            elif page=='traces':
                scope+=(errors_only,)
            if self.history_scopes.get(page)!=scope:
                self.history[page]=[None]
                self.history_scopes[page]=scope
        history={page:tuple(cursors) for page,cursors in self.history.items()}
        view_scope=(service,session_id,scope_agent,scope_subtree,include_reviews,
                    activity_query,mcp_query,query,activity_prompts,activity_errors,
                    tuple((page,cursors) for page,cursors in history.items()),errors_only)
        pagination={}
        def page_result(page, response):
            pagination[page]={'page':len(history[page]), 'next_cursor':response.get('next_cursor'),
                              'supported':'next_cursor' in response}
        requests = RequestBatch(self.api, self.stop, self.connection_stage)
        self._requests = requests
        raw_get = requests.get
        read_errors,read_updated = {},{}
        old=self.snapshot
        receiver_id=old.receiver_id
        receiver_confirmed=False
        def get(path):
            response=raw_get(path)
            if (receiver_confirmed and path.split('?',1)[0]!='/api/health' and
                    isinstance(response,dict) and response.get('receiver_id') and
                    response['receiver_id']!=receiver_id):
                raise ValueError('Receiver identity changed during this refresh; retrying')
            return response
        def evidence_get(path,kind,fallback):
            try:
                response=get(path)
            except OSError as exc:
                transient=(500<=exc.code<600 if isinstance(exc,urllib.error.HTTPError)
                           else not isinstance(exc,InterruptedError))
                if requested_scope.active_page!='agents' or not transient:
                    raise
                read_errors[kind]=clean(str(exc))
                same_scope=(old.receiver_id==receiver_id and (kind=='connections' or
                            (old.view_scope==view_scope and
                             str(old.hierarchy.get('revision') or '')==self.hierarchy_revision)))
                read_updated[kind]=old.read_updated.get(kind,0) if same_scope else 0
                return fallback if same_scope else {kind:[]}
            if isinstance(response,dict) and response.get('receiver_id') and response['receiver_id']!=receiver_id:
                raise ValueError('Receiver identity changed during '+kind+' read; retrying')
            values=response.get(kind) if isinstance(response,dict) else None
            if not isinstance(values,list) or any(not isinstance(row,dict) for row in values):
                raise ValueError('Invalid '+kind+' response')
            if kind not in read_errors:read_updated[kind]=time.time()
            return response
        scope_revisions = []
        history_due = (retry or self.sessions_attempted is None or
                       time.monotonic()-self.sessions_attempted>self.EXTRA_POLL_SECONDS)
        try:
            prepared = False
            if self.prepare is not None:
                prepared = self.prepare(self)
                self.prepare = None
            if self.stop.is_set():
                raise InterruptedError('Receiver startup cancelled')
            activity_params = {"limit": 200, "service": service, "session_id": session_id,
                               "include_reviews":int(include_reviews)}
            params = {"limit": 100, "service": service, "session_id": session_id, "q": query,
                      "cursor":history['traces'][-1] or '', 'include_reviews':int(include_reviews)}
            if scope_agent:
                activity_params.update(agent_id=scope_agent, subtree=int(scope_subtree))
                params.update(agent_id=scope_agent, subtree=int(scope_subtree))
                if requested_scope.hierarchy_revision:
                    activity_params['scope_revision']=requested_scope.hierarchy_revision
                    params['scope_revision']=requested_scope.hierarchy_revision
            if errors_only:
                params["status"] = "ERROR"
            trace_path = "/api/traces?" + urlencode(params)
            # During startup/outage, fail fast on one short health probe instead
            # of launching a whole batch and waiting for every timeout to drain.
            recovering=bool(self.snapshot.error) or not self.snapshot.updated
            if recovering and not prepared:get('/api/health?probe=1')
            initial_paths=['/api/health', '/api/connections',
                           '/api/activity?' + urlencode(activity_params), trace_path]
            if requested_scope.include_global_totals:initial_paths.append('/api/metrics')
            if history_due:initial_paths.append('/api/sessions?limit=500')
            requests.prefetch(['/api/health'] if not old.updated else initial_paths)
            health=get('/api/health')
            if self._capture_from_health(health):return
            receiver_id=str(health.get('receiver_id') or '')
            receiver_confirmed=True
            receiver_changed=self.cache_receiver_id is not None and self.cache_receiver_id!=receiver_id
            self.cache_receiver_id=receiver_id
            if receiver_changed:
                # Nothing cached under the preceding receiver namespace may be
                # relabelled with the new health identity, even during timeouts.
                self.sessions_cache=None;self.sessions_attempted=None
                self.sessions_updated=0;self.sessions_error=''
                self.extra_cache=None;self.extra_scope=None;self.extra_updated=0
                self.findings_cache=None;self.findings_scope=None;self.findings_updated=0
                self.history_scopes.clear()
                self.selected_trace=None;self.selected_span_id=None;self.pinned_trace=False
                selected=None;requested_span=None
                requested_scope=replace(requested_scope,selected_trace=None,selected_span_id=None,pinned_trace=False)
                # Discard reads begun before the identity was confirmed. They
                # may still finish, but their results cannot enter this snapshot.
                for path,future in tuple(requests.pending.items()):
                    if path!='/api/health':
                        future.cancel();requests.pending.pop(path,None)
                if any(cursors!=(None,) for cursors in history.values()):
                    self.history={page:[None] for page in self.history}
                    self.wake.set();return
                history_due=True
            if health.get('indexing',{}).get('ready')is False:
                from .native_bootstrap import ready_native
                initial=health
                if (not old.updated or old.receiver_id!=receiver_id)and ready_native(initial):
                    # A RequestBatch Future is cached; this independent health
                    # response is the only positive bootstrap observation used.
                    from .feed_requests import shared_get
                    confirmed=shared_get(self.api,'/api/health',cancelled=self.stop.is_set)
                    if confirmed.get('receiver_id')!=receiver_id:
                        raise ValueError('Receiver identity changed during native bootstrap; retrying')
                    if self._capture_from_health(confirmed):return
                    if confirmed.get('indexing',{}).get('ready')is False:
                        if self._publish_native_bootstrap(initial,confirmed,requested_scope,capture_epoch):
                            self.connection_failures=0
                            self.connection=replace(self.connection,phase='loading',stage='History indexing · native observations available',retry_at=0)
                        elif old.updated and self.snapshot is old:
                            raise InterruptedError('Indexing stored history; retaining the previous coherent snapshot')
                        return
                    if confirmed.get('indexing',{}).get('ready')is True:health=confirmed
                    else:raise InterruptedError('History readiness is unavailable')
                else:
                    if not old.updated:
                        from .native_bootstrap import indexing_in_progress
                        note=('History indexing failed; native observations are unavailable.'if health['indexing'].get('error')else
                            'History indexing; native discovery is incomplete.'if indexing_in_progress(health['indexing'])else
                            'History readiness is unavailable; native observations are withheld.')
                        raise InterruptedError(note)
                    raise InterruptedError('Indexing stored history · '+health['indexing'].get('stage','pending'))
            requests.prefetch([path for path in initial_paths if path!='/api/health'])
            local_status = health.get("local_agents", {})
            new_revision = str((local_status.get("hierarchy") or {}).get("revision") or "")
            if new_revision and self.hierarchy_revision and new_revision != self.hierarchy_revision:
                # Descendant membership changed.  Never mix a pre-change page
                # with a new hierarchy revision; restart the bounded read.
                self.hierarchy_revision = new_revision
                self.wake.set()
                return
            if new_revision:
                self.hierarchy_revision = new_revision
            totals = get('/api/metrics') if requested_scope.include_global_totals else {}
            agents = evidence_get('/api/connections','connections',{'connections':old.agents})['connections']
            try:
                activity_page = evidence_get('/api/activity?'+urlencode(activity_params),'activity',
                    {'activity':old.activity,**old.pagination.get('activity',{})})
                activity = activity_page['activity']
                result_params = dict(activity_params)
                if activity_prompts:
                    result_params.update(domain='message', action='user')
                if activity_errors:
                    result_params['errors_only']=1
                if activity_query or history['activity'][-1] or activity_prompts or activity_errors:
                    activity_page = evidence_get('/api/activity?'+urlencode({**result_params,
                        'q':activity_query,'cursor':history['activity'][-1] or ''}),'activity',
                        {'activity':old.activity_results if old.activity_results is not None else old.activity,
                         **old.pagination.get('activity',{})})
                activity_results = activity_page['activity']
                if isinstance(activity_page.get('agent_scope'),dict):
                    scope_revisions.append(activity_page['agent_scope'].get('revision'))
                page_result('activity',activity_page)
            except urllib.error.HTTPError as exc:
                if exc.code != 404:
                    raise
                activity, activity_results = [], []
            if history_due:
                try:
                    history_response = get('/api/sessions?limit=500')
                except urllib.error.HTTPError as exc:
                    if not 500 <= exc.code < 600:
                        raise
                    self.sessions_error = f'Stored session history returned HTTP {exc.code}.'
                except OSError as exc:
                    if isinstance(exc,InterruptedError):
                        raise
                    self.sessions_error = clean(str(exc)) or 'Stored session history could not be loaded.'
                else:
                    # Malformed responses and authentication failures remain
                    # fatal; only transient transport/server errors are partial.
                    sessions = history_response['sessions']
                    if not isinstance(sessions,list) or any(not isinstance(row,dict) for row in sessions):
                        raise ValueError('Invalid session history response')
                    with self._capture_lock:
                        self._capture_guard(capture_epoch)
                        self.sessions_cache = sessions
                        self.sessions_updated = time.time()
                        self.sessions_error = ''
                with self._capture_lock:
                    self._capture_guard(capture_epoch)
                    self.sessions_attempted = time.monotonic()
            sessions = self.sessions_cache if self.sessions_cache is not None else []
            scope = (service, session_id, mcp_query, include_reviews, history['mcp'],requested_scope.active_page)
            if self.extra_cache is None or scope != self.extra_scope or time.monotonic()-self.extra_updated>self.EXTRA_POLL_SECONDS:
                mcp_params = {'limit':100,'service':service,'session_id':session_id,
                              'include_reviews':int(include_reviews)}
                if scope_agent:
                    mcp_params.update(agent_id=scope_agent, subtree=int(scope_subtree))
                    if requested_scope.hierarchy_revision:
                        mcp_params['scope_revision']=requested_scope.hierarchy_revision
                metric_params = {'limit': 500, 'latest': 1}
                if service:
                    metric_params['service'] = service
                usage_params = {'days': 7, 'step': 'day'}
                if service:
                    usage_params['service'] = service
                if session_id:
                    usage_params['session_id'] = session_id
                extra_paths=[]
                if load_mcp:extra_paths.append('/api/mcp?'+urlencode(mcp_params))
                if load_metrics:extra_paths.extend(('/api/metric-catalog?limit=500',
                    '/api/measurements?'+urlencode(metric_params),'/api/usage-series?'+urlencode(usage_params)))
                requests.prefetch(extra_paths)
                mcp,mcp_results,mcp_available,mcp_page,mcp_summary=[],[],True,{},{}
                metric_catalog,metric_points,metrics_available=[],[],True
                usage_series,usage_service,usage_session_id=[],None,None
                if load_mcp:
                    try:
                        mcp_page = get('/api/mcp?'+urlencode(mcp_params))
                        mcp = mcp_page['calls']
                        mcp_summary = mcp_page.get('summary', {})
                        if mcp_query or history['mcp'][-1]:
                            mcp_page = get('/api/mcp?'+urlencode({**mcp_params,'q':mcp_query,
                                'cursor':history['mcp'][-1] or ''}))
                        mcp_results = mcp_page['calls']
                        if isinstance(mcp_page.get('agent_scope'),dict):
                            scope_revisions.append(mcp_page['agent_scope'].get('revision'))
                        mcp_available = True
                    except urllib.error.HTTPError as exc:
                        if exc.code!=404:
                            raise
                        mcp,mcp_results,mcp_available,mcp_page,mcp_summary=[],[],False,{},{}
                if load_metrics:
                    try:
                        metric_catalog = get('/api/metric-catalog?limit=500')['metrics']
                        metric_points = get('/api/measurements?' + urlencode(metric_params))['points']
                        metrics_available = True
                    except urllib.error.HTTPError as exc:
                        if exc.code != 404:
                            raise
                        metric_catalog, metric_points, metrics_available = [], [], False
                    try:
                        usage = get('/api/usage-series?' + urlencode(usage_params))
                        usage_series = usage.get('points', [])
                        usage_service = usage.get('service')
                        usage_session_id = usage.get('session_id')
                    except urllib.error.HTTPError as exc:
                        if exc.code != 404:
                            raise
                        usage_series, usage_service, usage_session_id = [], None, None
                with self._capture_lock:
                    self._capture_guard(capture_epoch)
                    self.extra_cache = (mcp,mcp_results,mcp_available,mcp_page,mcp_summary,
                                        metric_catalog,metric_points,metrics_available,usage_series,usage_service,
                                        usage_session_id)
                    self.extra_scope,self.extra_updated = scope,time.monotonic()
            (mcp,mcp_results,mcp_available,mcp_page,mcp_summary,
             metric_catalog,metric_points,metrics_available,usage_series,usage_service,
             usage_session_id)=self.extra_cache
            page_result('mcp',mcp_page)
            metric_history=[]
            trace_page = evidence_get(trace_path,'traces',{'traces':old.traces,**old.pagination.get('traces',{})})
            traces = trace_page['traces']
            if isinstance(trace_page.get('agent_scope'),dict):
                scope_revisions.append(trace_page['agent_scope'].get('revision'))
            page_result('traces',trace_page)
            if not include_reviews:
                traces = [item for item in traces if 'codex-review' not in item.get('services',[])]
                activity = [item for item in activity if item.get('service')!='codex-review']
                activity_results = [item for item in activity_results if item.get('service')!='codex-review']
                mcp = [item for item in mcp if item.get('service')!='codex-review']
                mcp_results = [item for item in mcp_results if item.get('service')!='codex-review']
            traces,activity = traces[:100],activity[:200]
            if not requested_scope.pinned_trace and selected not in {item["trace_id"] for item in traces}:
                selected = traces[0]["trace_id"] if traces else None
            detail = get("/api/traces/" + selected + "?span_limit=100") if selected and requested_scope.detail_enabled and 'traces' not in read_errors else {}
            if selected and requested_span and requested_scope.detail_enabled and 'traces' not in read_errors:
                wanted=requested_span
                known={span['span_id'] for span in detail.get('spans',[])}
                for _ in range(8):
                    if not wanted or wanted in known:break
                    page=get('/api/traces/'+selected+'/spans?'+urlencode({'limit':100,'q':wanted}))
                    span=next((item for item in page['spans'] if item['span_id']==wanted),None)
                    if not span:break
                    detail['spans'].append(span);known.add(wanted)
                    wanted=span.get('parent_span_id')
            findings = detail.get("findings", [])
            if not selected and load_findings:
                findings = get("/api/findings?limit=8")["findings"]
            finding_page={}
            if load_findings:
                findings_scope=history['findings']
                if (self.findings_cache is None or self.findings_scope!=findings_scope or
                        time.monotonic()-self.findings_updated>self.EXTRA_POLL_SECONDS):
                    finding_params={'limit':100,'cursor':findings_scope[-1] or ''}
                    if scope_agent:
                        finding_params.update(agent_id=scope_agent, subtree=int(scope_subtree))
                        if requested_scope.hierarchy_revision:
                            finding_params['scope_revision']=requested_scope.hierarchy_revision
                    finding_page=get('/api/findings?'+urlencode(finding_params))
                    if isinstance(finding_page.get('agent_scope'),dict):
                        scope_revisions.append(finding_page['agent_scope'].get('revision'))
                    with self._capture_lock:
                        self._capture_guard(capture_epoch)
                        self.findings_cache=finding_page
                        self.findings_scope=findings_scope
                        self.findings_updated=time.monotonic()
                else:
                    finding_page=self.findings_cache
            page_result('findings',finding_page)
            if self.stop.is_set():
                return
            revisions = [revision for revision in scope_revisions if revision]
            if not requested_scope.hierarchy_revision and revisions:
                # A receiver without a local monitor can still report the
                # durable hierarchy revision through scoped API responses.
                self.hierarchy_revision = revisions[0]
                self.wake.set()
                return
            newer = next((revision for revision in revisions
                          if requested_scope.hierarchy_revision and
                          revision != requested_scope.hierarchy_revision), None)
            if newer:
                self.hierarchy_revision = newer
                self.wake.set()
                return
            # A filter changed while the HTTP reads were in flight; discard the older view.
            current_scope=FeedScope.capture(self)
            if not requested_scope.hierarchy_revision and current_scope.hierarchy_revision:
                # The first read learns the durable revision from health. Keep
                # that first useful snapshot; the next poll will reset paging
                # because the revision differs from the bootstrap request.
                requested_scope=replace(requested_scope,
                    hierarchy_revision=current_scope.hierarchy_revision)
            if requested_scope != current_scope:
                self.wake.set()
                return
            if history!={page:tuple(cursors) for page,cursors in self.history.items()}:
                self.wake.set()
                return
            from .native_lifecycle import merge_observations,inventory_observations
            inventory=[];inventory_warning=''
            if health.get('native_lifecycle_v1') is True:
                try:inventory,inventory_warning=inventory_observations(get('/api/native-lifecycle'),receiver_id)
                except (OSError,ValueError,CockpitError):inventory_warning='Native lifecycle inventory read is unavailable.'
                # RequestBatch caches the prefetched health Future; this must
                # be an independent read after the native inventory response.
                from .feed_requests import shared_get
                confirmed=shared_get(self.api,'/api/health',cancelled=self.stop.is_set)
                if confirmed.get('receiver_id')!=receiver_id:
                    raise ValueError('Receiver identity changed during native inventory read; retrying')
                if self._capture_from_health(confirmed):return
                if confirmed.get('indexing',{}).get('ready')is False:
                    raise InterruptedError('Indexing stored history; metadata refresh deferred')
                if confirmed.get('native_lifecycle_v1') is not True:
                    inventory_warning='Native lifecycle inventory capability changed during the read.'
                health=confirmed;local_status=health.get('local_agents') or {}
            if self.stop.is_set():return
            if requested_scope!=FeedScope.capture(self) or history!={page:tuple(cursors) for page,cursors in self.history.items()}:
                self.wake.set();return
            clock=health.get('observed_time_unix_nano')
            receiver_time=clock if type(clock) is int and 0<clock<2**63 else 0
            indexing=health.get('indexing')
            native,native_warning=merge_observations(receiver_id,
                old.native_lifecycle if old.receiver_id==receiver_id else (),inventory,activity=activity)
            native_warning=inventory_warning or native_warning
            if old.receiver_id==receiver_id and old.native_lifecycle_warning.startswith('Native lifecycle evidence reached'):
                native_warning=native_warning or old.native_lifecycle_warning
            candidate = Snapshot(totals, agents, traces, detail, findings, updated=time.time(), local_status=local_status, activity=activity,
                                     sessions=sessions,receiver_id=health.get("receiver_id") or "",
                                     receiver_time_ns=receiver_time,indexing_ready=isinstance(indexing,dict) and indexing.get("ready") is True,
                                     native_lifecycle=native,native_lifecycle_warning=native_warning,native_lifecycle_prepared=True,
                                     review_trace_filter=health.get("review_trace_filter") is True,
                                     sessions_error=self.sessions_error,
                                     sessions_updated=self.sessions_updated,
                                     mcp=mcp,mcp_available=mcp_available,mcp_summary=mcp_summary,
                                     metric_catalog=metric_catalog,metric_points=metric_points,
                                     usage_series=usage_series,usage_service=usage_service,
                                     usage_session_id=usage_session_id,
                                     metric_history=metric_history,
                                     metrics_available=metrics_available,
                                     activity_results=activity_results[:200],mcp_results=mcp_results,
                                     finding_results=finding_page.get('findings',[]),
                                      queries={'activity':activity_query,'mcp':mcp_query,'traces':query},pagination=pagination,
                                      activity_prompts=activity_prompts,
                                      activity_errors=activity_errors,
                                      view_scope=view_scope,read_errors=read_errors,read_updated=read_updated,
                                      hierarchy=local_status.get('hierarchy',{}))
            complete=(health.get('indexing',{}).get('ready')is True and not read_errors and not self.sessions_error)
            if not self._capture_commit(candidate,health,capture_epoch,complete=complete):
                self.wake.set();return
            self.selected_trace = selected
            self.connection_failures=0
            self.connection=replace(self.connection,phase='live',stage='Connected',retry_at=0)
        except Exception as exc:
            if self.stop.is_set():
                return
            if isinstance(exc, urllib.error.HTTPError) and exc.code in {401,403}:
                message = "Receiver requires authentication. Set OBSERVATORY_TOKEN and restart."
            elif isinstance(exc,TimeoutError) and self.connection.stage!='Checking receiver':
                message = 'Loading delayed: '+clean(str(exc))+'. Retrying.'
            elif isinstance(exc,InterruptedError)and not old.updated:
                message=clean(str(exc))
            else:
                message = "Receiver unavailable. Retrying: " + clean(str(exc))
            with self._capture_lock:
                if capture_epoch!=self._capture_epoch:
                    self.wake.set();return
                if not old.updated and self.snapshot.updated:return
                if getattr(self.snapshot,'native_bootstrap',False)and self.snapshot is not old:return
                old=self.snapshot
                if old.receiver_id==self._capture_receiver and(self._capture_global_denied or self._capture_denials):
                    old=_capture_purge(old,self._capture_local(old.local_status,metadata=True))
                    self._clear_content_caches()
                self.snapshot=replace(old,error=message)
            self.extra_cache=None
            self.findings_cache=None
            self.connection_failures+=1
            self.connection=replace(self.connection,phase='auth' if isinstance(exc,urllib.error.HTTPError) and exc.code in {401,403}
                                    else 'reconnecting',stage='Finishing pending requests')
        finally:
            requests.close()
            self._requests = None
            if self.snapshot.error:
                self.connection=replace(self.connection,
                    phase='auth' if self.connection.phase=='auth' else 'waiting',
                    stage='Waiting to retry',retry_at=time.monotonic()+self.retry_delay())

    def metric_history(self, metric, limit=100):
        params={'limit':max(1,min(500,int(limit)))}
        for key in ('service','name','kind','temporality','scope_name','unit','description'):
            if metric.get(key) is not None:
                params[key]=metric[key]
        return self.api.get('/api/measurements?'+urlencode(params))['points']

    def request_refresh(self):
        # Invalidate any in-flight read so an A -> B -> A selection sequence
        # cannot publish the earlier A response over newer context.
        self.generation += 1
        self.refresh_once = True
        self.wake.set()

    def retry_now(self):
        self.retry_requested=True
        self.request_refresh()

    def navigate_history(self, page, direction, *, snapshot=None):
        cursors=self.history[page]
        next_cursor=(snapshot or self.snapshot).pagination.get(page,{}).get('next_cursor')
        if direction=='older' and next_cursor:
            self.history[page]=[*cursors,next_cursor]
        elif direction=='newer' and len(cursors)>1:
            self.history[page]=cursors[:-1]
        elif direction=='latest':
            self.history[page]=[None]
        else:
            return False
        if page=='traces':
            self.selected_trace=None;self.selected_span_id=None;self.pinned_trace=False
        self.request_refresh()
        return True

    def _loop(self):
        while not self.stop.is_set():
            # Clear before work: a wake raised during a request must survive so
            # navigation and Retry now do not incur another polling interval.
            self.wake.clear()
            # Pause freezes the cockpit's view, not collection or its knowledge
            # of arriving records. Explicit navigation still gets fresh evidence.
            self.refresh_once = False
            self.refresh()
            self.wake.wait(self.retry_delay() if self.snapshot.error else self.POLL_SECONDS)

    def request_stop(self):
        """Stop admitting reads without waiting on the terminal's event thread."""
        self.stop.set()
        self.wake.set()
        requests = self._requests
        if requests is not None:
            requests.cancel()

    def close(self):
        self.request_stop()
        if getattr(self.thread, 'ident', None) is not None:
            self.thread.join(timeout=4)


def agent_rows(snapshot):
    rows = [("", "ALL AGENTS", "")]
    known = set()
    local_groups = {}
    for agent in snapshot.agents:
        service = agent["service"]
        known.add(service)
        local = "local-session-log" in agent.get("integrations", [])
        if local:
            local_groups.setdefault(service, []).append(agent)
            continue
        state = ("RECENT" if agent["state"] == "running" else "IDLE") if local else (
            "LIVE" if agent["state"] == "running" else "DONE" if agent["state"] == "stopped" else "OFFLINE")
        rows.append((service, service, f"{state} · {number(agent['span_count'])} spans"))
    for service, agents in local_groups.items():
        state = "RECENT" if any(agent["state"] == "running" for agent in agents) else "IDLE"
        rows.append((service, service, f"{state} · {len(agents)} sessions · {number(sum(agent['span_count'] for agent in agents))} spans"))
    for service in snapshot.totals.get("services", []):
        if service not in known:
            rows.append((service, service, "OTLP / imported"))
    return rows


def event_rows(snapshot):
    rows = []
    detail = snapshot.detail
    for event in detail.get("progress", []):
        rows.append((event.get("time_ns", 0), event.get("stage", "Progress"), event.get("message", "")))
    for log in detail.get("logs", [])[:30]:
        body = log.get("body", "")
        rows.append((log.get("time_unix_nano", 0), log.get("severity_text") or log.get("event_name") or "LOG",
                     body if isinstance(body, str) else json.dumps(body, ensure_ascii=False)))
    for finding in snapshot.findings[:10]:
        rows.append((finding.get("time_unix_nano", 0), finding.get("severity", "finding").upper(), finding.get("title", "Review finding")))
    for span in detail.get("spans", []):
        if span.get("scope_name") == "observatory.local_agents":
            rows.append((span["end_unix_nano"], span["status"], span["name"]))
    rows.sort(key=lambda item: item[0], reverse=True)
    return [f"{time.strftime('%H:%M:%S', time.localtime(stamp/1e9)) if stamp else '--:--:--'} {label}  {body}" for stamp, label, body in rows]


def span_rows(snapshot):
    rows = []
    for span in snapshot.detail.get("spans", []):
        duration = (span["end_unix_nano"] - span["start_unix_nano"]) / 1e6
        timing = "  unknown" if span.get("attributes", {}).get("observatory.duration_unknown") else f"{duration:7.1f}ms"
        rows.append(f"{span['status'][:7]:7} {timing}  {span['name']}")
    return rows


def activity_rows(snapshot):
    rows = []
    for item in snapshot.activity:
        stamp = time.strftime('%H:%M:%S', time.localtime(item['time_unix_nano']/1e9))
        state = str(item.get('result') or 'observed').upper()
        rows.append(f"{stamp} {item['domain'][:7].upper():7} {state[:7]:7} {item['action']}  {item['subject']}")
    return rows


def wrapped(text, width):
    lines = []
    for line in str(text).splitlines():
        lines.extend(textwrap.wrap(line, width=max(10, width), replace_whitespace=False) or [''])
    return lines


def print_snapshot(feed, *, refresh=True):
    if refresh:feed.refresh()
    if feed.snapshot.error:raise CockpitError(feed.snapshot.error)
    snapshot = feed.snapshot
    totals = snapshot.totals
    def token_count(record, key):
        if record.get(key + '_reported') is False or record.get(key) is None:
            return 'Not reported'
        return number(record.get(key))
    print("AGENTURM  " + feed.api.endpoint)
    if snapshot.local_status.get("enabled"):
        from .source_coverage import coverage_summary
        mode = "bounded, redacted content" if snapshot.local_status.get('capture_content') else "metadata only"
        print(f"Sources: {coverage_summary(snapshot.local_status)} · {mode}")
    known_cost = totals.get('known_cost_usd')
    cost_text = 'Not reported' if known_cost is None else f'${known_cost:.4f}'
    print(f"Runs {number(totals.get('runs'))}  Spans {number(totals.get('spans'))}  "
          f"Tokens {token_count(totals, 'input_tokens')} in / {token_count(totals, 'output_tokens')} out  "
          f"Errors {number(totals.get('errors'))}  Known cost {cost_text}")
    for title, rows in (
        ("CONNECTED AGENTS", [f"{label}  {state}" for _, label, state in agent_rows(snapshot)[1:]]),
        ("RECENT TRACES", [f"{item['status']:10} {trace_name(item)}  {token_count(item, 'input_tokens')} in / {token_count(item, 'output_tokens')} out" for item in snapshot.traces[:10]]),
        ("SELECTED TRACE / SPANS", span_rows(snapshot)[:10]),
        ("LIVE PROGRESS / ERRORS", event_rows(snapshot)[:8]),
        ("RECENT ACTIVITY", activity_rows(snapshot)[:12]),
    ):
        print("\n+-- " + title + " " + "-" * max(0, 66-len(title)))
        for line in rows or ["Waiting for telemetry. Press c in the cockpit for connection commands."]:
            print("| " + clipped(line, 100))
        print("+" + "-" * 72)
    if snapshot.error:
        print(snapshot.error)
        raise CockpitError(snapshot.error)


def open_terminal(command):
    if sys.platform == "darwin":
        script = tempfile.NamedTemporaryFile(mode="w", prefix="observatory-", suffix=".command", delete=False)
        with script:
            script.write("#!/bin/sh\nrm -- " + shlex.quote(script.name) + "\ncd " + shlex.quote(os.getcwd()) + "\nexec " + shlex.join(command) + "\n")
        os.chmod(script.name, 0o700)
        subprocess.Popen(["open", "-a", "Terminal", script.name])
        return
    if os.name == "nt":
        terminal = shutil.which("wt.exe")
        if terminal:
            subprocess.Popen([terminal, "--title", "Agenturm", "--startingDirectory", os.getcwd(), *command])
            return
    elif os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY"):
        for executable, prefix in (("gnome-terminal", ["--working-directory", os.getcwd(), "--"]),
                                   ("konsole", ["--workdir", os.getcwd(), "-e"]),
                                   ("x-terminal-emulator", ["-e"]), ("xterm", ["-e"])):
            terminal = shutil.which(executable)
            if terminal:
                subprocess.Popen([terminal, *prefix, *command])
                return
    raise CockpitError("No graphical terminal is available. Run the cockpit in your current terminal, or use --once.")


def _run_terminal_app(app):
    """Let Textual restore the terminal before reporting signals or failures."""
    async def run():
        loop = asyncio.get_running_loop()
        previous = {}
        registered = os.name == 'posix' and threading.current_thread() is threading.main_thread()
        try:
            if registered:
                # Register before the driver enters raw mode, including startup.
                # Textual consumes task cancellation, so explicit exit codes are
                # needed for external SIGINT as well as SIGTERM.
                for signum in (signal.SIGINT, signal.SIGTERM):
                    handler = signal.getsignal(signum)
                    loop.add_signal_handler(signum, lambda code=128+signum: app.exit(return_code=code))
                    previous[signum] = handler
            await app.run_async()
            return app.return_code
        finally:
            for signum, handler in previous.items():
                loop.remove_signal_handler(signum)
                signal.signal(signum, handler)

    return asyncio.run(run())


def run_cockpit(*, db_path=None, port=14318, endpoint=None,
                once=False, new_window=False, local_agents=True, metadata_only=False, managed_db=False,
                retention_days=None, capability_config=()):
    if db_path is None:
        from .receiver_state import telemetry_path,prepare_state_directory
        prepare_state_directory();db_path=str(telemetry_path());managed_db=True
    if not 1 <= port <= 65535:
        raise CockpitError("Choose a port between 1 and 65535.")
    if capability_config and once:
        raise CockpitError("Capability panes require the interactive cockpit; omit --once.")
    if capability_config:
        from .capability_process import read_capability_config
        # Validate declarations before launching another terminal or any
        # receiver. Discovery never imports or starts a plugin executable.
        for path in capability_config:
            read_capability_config(path)
    if new_window:
        command = [sys.executable, "-m", "observatory", "--db", str(Path(db_path).resolve()), "cockpit", "--port", str(port)]
        if managed_db:
            command = [sys.executable,"-m","observatory","cockpit","--port",str(port)]
        if retention_days is not None:command.extend(["--retention-days",str(retention_days)])
        if endpoint:
            command.extend(["--endpoint", endpoint])
        if not local_agents:
            command.append("--no-local-agents")
        if metadata_only:
            command.append("--metadata-only")
        if once:
            command.append("--once")
        for path in capability_config:
            command.extend(["--capability-config", str(Path(path).resolve())])
        open_terminal(command)
        return
    if not once and (not sys.stdin.isatty() or not sys.stdout.isatty()):
        raise CockpitError("Run this command in an interactive terminal. Use --new-window to open one, or --once for a snapshot.")
    if not once:
        try:
            from .terminal_ui import ObservatoryApp
        except ImportError as exc:
            raise CockpitError("The terminal UI needs Textual. Reinstall the CLI, or install this checkout with: python -m pip install .") from exc
    token = os.environ.get("OBSERVATORY_TOKEN") or None
    receiver = feed = startup = None
    capability_activations = ()
    api = Api(endpoint or f"http://127.0.0.1:{port}", token)
    try:
        if not once:
            print('Connecting to receiver '+api.endpoint+'…',file=sys.stderr,flush=True)
            from .cockpit_startup import CockpitStartup
            startup = CockpitStartup(api, db_path, port, endpoint=endpoint,
                local_agents=local_agents, metadata_only=metadata_only,
                managed_db=managed_db, retention_days=retention_days)
            feed = Feed(api)
            feed.prepare = startup
            panes = ()
            if capability_config:
                from .capability_activation import activate_capabilities
                capability_activations, panes = activate_capabilities(tuple(capability_config), api, feed)
            return _run_terminal_app(ObservatoryApp(feed, ui_panes=panes))
        try:
            health = api.get("/api/health?probe=1")
            if not health.get("ok"):
                raise CockpitError("The endpoint is not a ready Agenturm receiver.")
            health=api.get('/api/health')
            if not (health.get('security') or {}).get('authentication_required'):
                print('Receiver authentication is disabled or unavailable. Restart the receiver to enable the private local token.',file=sys.stderr)
            if metadata_only and health.get("local_agents", {}).get("capture_content"):
                raise CockpitError("The existing receiver captures session content. Restart it with serve --metadata-only, then reconnect.")
        except urllib.error.HTTPError as exc:
            if exc.code == 401:
                raise CockpitError("Receiver requires authentication. Set OBSERVATORY_TOKEN before starting.") from exc
            raise CockpitError(f"The endpoint returned HTTP {exc.code}. Check the receiver address.") from exc
        except (urllib.error.URLError, OSError) as exc:
            if endpoint and (once or metadata_only):
                raise CockpitError("Cannot connect to the receiver at " + api.endpoint) from exc
            if not endpoint:
                if not once:
                    print('Starting local receiver and loading stored sessions…',file=sys.stderr,flush=True)
                try:
                    from .receiver_state import local_token
                    token = token or local_token(port, create=True)
                    api.token = token
                    receiver = LocalReceiver(db_path, port, token, local_agents=local_agents, metadata_only=metadata_only, managed_db=managed_db, retention_days=retention_days)
                except OSError as bind_error:
                    raise CockpitError(f"Cannot start a local receiver on port {port}: {bind_error}") from bind_error
        feed = Feed(api)
        if once:
            deadline=time.monotonic()+15
            while True:
                progress=api.get('/api/health')
                if progress.get('indexing',{}).get('ready') is False:
                    if time.monotonic()>=deadline:
                        print('Agenturm snapshot incomplete: history indexing did not become ready within 15 seconds. Restart to resume.',file=sys.stderr)
                        return 3
                    time.sleep(.1);continue
                feed.refresh()
                if feed.snapshot.error:raise CockpitError(feed.snapshot.error)
                if getattr(feed.snapshot,'native_bootstrap',False):
                    if time.monotonic()>=deadline:
                        print('Agenturm snapshot incomplete: native metadata is available but history indexing is not ready. No complete snapshot emitted.',file=sys.stderr)
                        return 3
                    time.sleep(.1);continue
                local=feed.snapshot.local_status
                discovery_ready=not local.get('enabled') or local.get('initial_poll_complete') is True
                derived_ready=receiver is None or not receiver.store._derived_pending
                if discovery_ready and not local.get('backlog_bytes') and derived_ready:
                    print_snapshot(feed,refresh=False)
                    return 0
                if time.monotonic()>=deadline:
                    print('Agenturm snapshot incomplete: discovery or indexing did not become ready within 15 seconds. No complete snapshot emitted.',file=sys.stderr)
                    return 3
                time.sleep(.1)
        else:
            return _run_terminal_app(ObservatoryApp(feed))
    except KeyboardInterrupt:
        return 130
    finally:
        complete_cleanup(*(closer for host, gate in reversed(capability_activations)
                           for closer in (host.close, gate.close)),
                         *([feed.close] if feed is not None else []),
                         *([startup.close] if startup is not None else []),
                         *([receiver.close] if receiver is not None else []))
