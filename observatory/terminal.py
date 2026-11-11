"""Receiver lifecycle and live data for the terminal cockpit."""
from __future__ import annotations

import json
import os
import re
import shlex
import shutil
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
        self.token = token

    def get(self, path):
        return self._get(path, self.MAX_RESPONSE_BYTES)

    def get_review(self, path):
        return self._get(path, 2 * 1024 * 1024)

    def _get(self, path, max_bytes):
        headers = {"Authorization": f"Bearer {self.token}"} if self.token else {}
        request = urllib.request.Request(self.endpoint + path, headers=headers)
        timeout=self.HEALTH_TIMEOUT if path=='/api/health?probe=1' else self.REQUEST_TIMEOUT
        with urllib.request.urlopen(request, timeout=timeout) as response:
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
    def __init__(self, db_path, port, token, local_agents=True, metadata_only=False):
        from .core import Store
        from .server import BoundedHTTPServer, handler_for
        self.store = Store(db_path, async_derived_refresh=True)
        try:
            self.server = BoundedHTTPServer(("127.0.0.1", port), handler_for(self.store, token, quiet=True))
        except OSError:
            self.store.close()
            raise
        self.thread = threading.Thread(target=self.server.serve_forever, name="observatory-receiver", daemon=True)
        self.thread.start()
        self.local = None
        if local_agents:
            from .local_agents import LocalAgents
            self.local = LocalAgents(self.store, capture_details=not metadata_only).start()
        self.store.local_agent_monitor = self.local

    def close(self):
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
    review_trace_filter: bool = False


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
        requested_scope = FeedScope.capture(self)
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
            if recovering:get('/api/health?probe=1')
            initial_paths=['/api/health', '/api/connections',
                           '/api/activity?' + urlencode(activity_params), trace_path]
            if requested_scope.include_global_totals:initial_paths.append('/api/metrics')
            if history_due:initial_paths.append('/api/sessions?limit=500')
            requests.prefetch(initial_paths)
            health=get('/api/health')
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
                requests.prefetch([path for path in initial_paths if path!='/api/health']+['/api/sessions?limit=500'])
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
                    self.sessions_cache = sessions
                    self.sessions_updated = time.time()
                    self.sessions_error = ''
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
            self.selected_trace = selected
            self.snapshot = Snapshot(totals, agents, traces, detail, findings, updated=time.time(), local_status=local_status, activity=activity,
                                     sessions=sessions,receiver_id=health.get("receiver_id") or "",
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
            self.connection_failures=0
            self.connection=replace(self.connection,phase='live',stage='Connected',retry_at=0)
        except Exception as exc:
            if self.stop.is_set():
                return
            if isinstance(exc, urllib.error.HTTPError) and exc.code in {401,403}:
                message = "Receiver requires authentication. Set OBSERVATORY_TOKEN and restart."
            elif isinstance(exc,TimeoutError) and self.connection.stage!='Checking receiver':
                message = 'Loading delayed: '+clean(str(exc))+'. Retrying.'
            else:
                message = "Receiver unavailable. Retrying: " + clean(str(exc))
            old = self.snapshot
            self.snapshot = replace(old,error=message)
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

    def close(self):
        self.stop.set()
        self.wake.set()
        requests = self._requests
        if requests is not None:
            requests.cancel()
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


def print_snapshot(feed):
    feed.refresh()
    snapshot = feed.snapshot
    totals = snapshot.totals
    def token_count(record, key):
        if record.get(key + '_reported') is False or record.get(key) is None:
            return 'Not reported'
        return number(record.get(key))
    print("AGENTURM  " + feed.api.endpoint)
    if snapshot.local_status.get("enabled"):
        mode = "bounded, redacted content" if snapshot.local_status.get('capture_content') else "metadata only"
        opencode_count=sum(entry.get('runtime')=='OpenCode' for entry in snapshot.local_status.get('sessions',[]))
        print(f"Sources: local Codex / Claude Code logs · {snapshot.local_status.get('sources', 0)} files · "
              f"OpenCode · {opencode_count} recent sessions · {mode}")
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


def run_cockpit(*, db_path="observatory.db", port=14318, endpoint=None,
                once=False, new_window=False, local_agents=True, metadata_only=False):
    if not 1 <= port <= 65535:
        raise CockpitError("Choose a port between 1 and 65535.")
    if new_window:
        command = [sys.executable, "-m", "observatory", "--db", str(Path(db_path).resolve()), "cockpit", "--port", str(port)]
        if endpoint:
            command.extend(["--endpoint", endpoint])
        if not local_agents:
            command.append("--no-local-agents")
        if metadata_only:
            command.append("--metadata-only")
        if once:
            command.append("--once")
        open_terminal(command)
        return
    if not once and (not sys.stdin.isatty() or not sys.stdout.isatty()):
        raise CockpitError("Run this command in an interactive terminal. Use --new-window to open one, or --once for a snapshot.")
    if not once:
        try:
            from .terminal_ui import ObservatoryApp
        except ImportError as exc:
            raise CockpitError("The terminal UI needs Textual. Reinstall the CLI, or install this checkout with: python -m pip install .") from exc
    token = os.environ.get("OBSERVATORY_TOKEN")
    receiver = feed = None
    api = Api(endpoint or f"http://127.0.0.1:{port}", token)
    try:
        if not once:
            print('Connecting to receiver '+api.endpoint+'…',file=sys.stderr,flush=True)
        try:
            health = api.get("/api/health?probe=1")
            if not health.get("ok"):
                raise CockpitError("The endpoint is not a ready Agenturm receiver.")
            if metadata_only:health=api.get('/api/health')
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
                    receiver = LocalReceiver(db_path, port, token, local_agents=local_agents, metadata_only=metadata_only)
                except OSError as bind_error:
                    raise CockpitError(f"Cannot start a local receiver on port {port}: {bind_error}") from bind_error
        feed = Feed(api)
        if once:
            print_snapshot(feed)
        else:
            ObservatoryApp(feed).run()
    except KeyboardInterrupt:
        pass
    finally:
        complete_cleanup(*([feed.close] if feed is not None else []),
                         *([receiver.close] if receiver is not None else []))
