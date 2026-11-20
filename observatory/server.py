"""Small local HTTP receiver and API for the terminal cockpit."""
from __future__ import annotations

import hmac
import hashlib
import gzip
import io
import json
import os
import sqlite3
import secrets
import threading
import time
import weakref
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from .agent_hierarchy import AGENT_ID, CAPABILITIES, HierarchyGraph
from .capability_policy import capture_sources
from .core import Store, IndexNotReady, normalize_native, normalize_otlp, TRACE_ID
from .otlp_proto import decode_trace_request
from .logs import normalize_otlp_logs, decode_log_request
from .metrics import normalize_otlp_metrics, decode_metric_request
from .watch import start_watchers
from .receiver import BoundedHTTPServer, DeadlineHandler
from .response_cache import ResponseCache

# Preserve the historical server import while exposing the explicit bounded name.
ThreadingHTTPServer = BoundedHTTPServer

MAX_BODY = 8 * 1024 * 1024
_CAPABILITY_PROCESS_INSTANCE = secrets.token_hex(16)
_CAPABILITY_MONITOR_TOKENS = weakref.WeakKeyDictionary()
_CAPABILITY_MONITOR_LOCK = threading.Lock()


def _capability_monitor_token(monitor):
    """Give each live collector instance a nonce without retaining it forever."""
    try:
        with _CAPABILITY_MONITOR_LOCK:
            token = _CAPABILITY_MONITOR_TOKENS.get(monitor)
            if token is None:
                token = secrets.token_hex(16)
                _CAPABILITY_MONITOR_TOKENS[monitor] = token
            return token
    except TypeError:
        return None


def _capability_revision_watch(path):
    """Sample committed SQLite changes without pinning a read transaction."""
    watch = sqlite3.connect(Path(path).resolve().as_uri() + '?mode=ro',
                            uri=True, timeout=0.1)
    try:
        revision = watch.execute('PRAGMA data_version').fetchone()[0]
        if watch.in_transaction:
            raise RuntimeError('capability revision watch pinned a read transaction')
        return watch, revision
    except BaseException:
        watch.close()
        raise


def handler_for(store: Store, token: str | None = None, quiet=False, allowed_hosts=()):
    cache = ResponseCache(store.path)
    class Handler(DeadlineHandler, BaseHTTPRequestHandler):
        response_cache = cache

        def _json(self, code, value):
            data = json.dumps(value, ensure_ascii=False).encode()
            tagged = value.get('receiver_id') if isinstance(value, dict) else None
            self._json_bytes(code, data, cache_result=True, body_receiver=tagged)

        def _json_bytes(self, code, data, *, cache_result=False, body_receiver=None):
            receiver = getattr(self, '_get_receiver_id', None) if self.command == 'GET' and code == 200 else None
            if receiver and code == 200:
                try:
                    current = store.receiver_namespace()
                    changed = current != receiver or body_receiver and body_receiver != receiver
                    identity=getattr(self,'_mcp_file_identity',None)
                    if identity is not None:changed=changed or store._mcp_file_identity()!=identity
                    identity=getattr(self,'_activity_file_identity',None)
                    if identity is not None:changed=changed or store._source_file_identity('Activity')!=identity
                    identity=getattr(self,'_metrics_file_identity',None)
                    if identity is not None:changed=changed or store._source_file_identity('Metrics')!=identity
                    identity=getattr(self,'_sessions_file_identity',None)
                    if identity is not None:changed=changed or store._source_file_identity('Sessions')!=identity
                except (OSError, ValueError, sqlite3.Error):
                    changed = True
                if changed:
                    code = 409
                    data = json.dumps({'error': 'Receiver identity changed during this read. Retry the request.'}).encode()
                    receiver = None
            if code == 200 and cache_result and getattr(self, '_cache_version', None) is not None:
                cache.put(self.path, self._cache_version, data)
            guard = getattr(self, '_capability_guard', None)
            if code == 200 and guard is not None and not guard():
                code = 409
                data = json.dumps({'error': 'Capability capture policy changed during this read.'}).encode()
                receiver = None
            self.send_response(code)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            if receiver:
                self.send_header('X-Agenturm-Receiver-ID', receiver)
            self.end_headers()
            self.wfile.write(data)

        def _protobuf_ok(self):
            # Empty OTLP export response means the full batch was accepted.
            self.send_response(200)
            self.send_header("Content-Type", "application/x-protobuf")
            self.send_header("Content-Length", "0")
            self.end_headers()

        def _authorized(self):
            if not token:
                return True
            header = self.headers.get("Authorization", "")
            return hmac.compare_digest(header, f"Bearer {token}")

        def _route(self):
            route = urlparse(self.path)
            from .receiver_state import request_authority
            try:request_authority(self.headers,allowed_hosts)
            except ValueError as exc:
                self._json(403,{'error':str(exc)})
                return None
            # The root is a static, non-sensitive hint for someone who opens
            # the receiver URL in a browser. Keep every data/API route gated.
            if route.path != "/" and not self._authorized():
                self._json(401, {"error": "unauthorized"})
                return None
            return route

        def do_GET(self):
            try:
                self._read_GET()
            except IndexNotReady as exc:
                self._json(503, {'error': str(exc), 'indexing': exc.indexing})

        def _read_GET(self):
            self._cache_version = None
            self._capability_guard = None
            self._get_receiver_id = None
            self._mcp_file_identity = None
            self._activity_file_identity = None
            self._metrics_file_identity = None
            self._sessions_file_identity = None
            route = self._route()
            if route is None:
                return
            path = route.path
            capability_activity = (path == '/api/activity' and
                'capture_scope' in parse_qs(route.query, keep_blank_values=True))
            # Scope selector presence is conservative, including blank/false
            # values and duplicates. Only raw inventory has this dependency.
            certified_mcp=(path=='/api/mcp'and callable(getattr(store,'certify_initial_mcp',None)))
            raw_mcp=(certified_mcp and
                not {'agent_id','subtree','scope_revision'}.intersection(parse_qs(route.query,keep_blank_values=True)))
            certified_activity=(path=='/api/activity'and callable(getattr(store,'certify_initial_activity',None)))
            indexed_activity=(certified_activity and not
                {'agent_id','subtree','scope_revision','trace_id','turn_id','review'}.intersection(parse_qs(route.query,keep_blank_values=True)))
            certified_metrics=(path=='/api/metrics'and callable(getattr(store,'certify_initial_metrics',None)))
            certified_sessions=(path=='/api/sessions'and callable(getattr(store,'certify_initial_sessions',None)))
            independent_read=raw_mcp or indexed_activity or certified_metrics or certified_sessions
            independent_cache_ready=True
            if path not in {'/','/api/health'} and path.startswith('/api/') and hasattr(store,'index_status'):
                indexing=store.index_status()
                independent_cache_ready=indexing.get('ready')is True
                if not independent_read and not indexing.get('ready') and callable(getattr(store,'catch_up_incoming',None)):
                    indexing=store.catch_up_incoming(indexing)
                if not independent_read and not indexing.get('ready'):
                    self._json(503,{'error':'History index is not ready','indexing':indexing})
                    return
            # The fast recovery probe deliberately avoids SQLite. Full health
            # and all data reads bind their response to one receiver identity.
            if path != '/' and not (path == '/api/health' and parse_qs(route.query).get('probe') == ['1']):
                namespace = getattr(store, 'receiver_namespace', None)
                if callable(namespace):
                    try:
                        self._get_receiver_id = namespace()
                    except (OSError, ValueError, sqlite3.Error):
                        self._json(503, {'error': 'Receiver identity is temporarily unavailable. Retry the request.'})
                        return
            if certified_mcp:self._mcp_file_identity=store.certify_initial_mcp(self._get_receiver_id,strict=not raw_mcp)
            def certificate(read,**options):
                nonlocal independent_cache_ready
                try:return read(self._get_receiver_id,**options)
                except IndexNotReady as exc:
                    # Preserve the existing single 75 ms worker-demand budget
                    # when this endpoint's own incoming dependencies are dirty.
                    # The failed certificate released its read lease already;
                    # unrelated work never arms this wait, and stale data is
                    # still rejected by a fresh certificate after the budget.
                    catch_up=getattr(store,'catch_up_incoming',None)
                    if (not independent_read or exc.indexing.get('error')or
                            exc.indexing.get('stage')!='Indexing incoming source changes'or not callable(catch_up)):
                        raise
                    independent_cache_ready=False
                    catch_up(exc.indexing)
                    return read(self._get_receiver_id,**options)
            if certified_activity:self._activity_file_identity=certificate(store.certify_initial_activity,strict=not indexed_activity)
            if certified_metrics:self._metrics_file_identity=certificate(store.certify_initial_metrics)
            if certified_sessions:self._sessions_file_identity=certificate(store.certify_initial_sessions)
            # A capability grant is live receiver state, not just SQLite data.
            # Never serve or retain one through the ordinary response cache.
            if not capability_activity and cache.eligible(path)and(not independent_read or independent_cache_ready):
                self._cache_version, cached = cache.get(self.path)
                if cached is not None:
                    self._json_bytes(200, cached)
                    return
            if path == "/":
                data = ("Agenturm is terminal-first. Run `agenturm` in a terminal "
                        "to monitor local sessions. This HTTP service receives OTLP and serves the local API.\n").encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/plain; charset=utf-8")
                self.send_header("Content-Length", str(len(data)))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(data)
            elif path == "/api/health":
                if parse_qs(route.query).get('probe')==['1']:
                    self._json(200, {"ok": True})
                    return
                indexing=store.index_status() if hasattr(store,'index_status') else {"ready":True}
                if not indexing.get('ready') and callable(getattr(store,'catch_up_incoming',None)):
                    indexing=store.catch_up_incoming(indexing)
                local = getattr(store, "local_agent_monitor", None)
                local_status = local.status() if local else {"enabled": False}
                try:
                    plugin_status = store.plugin_sessions()
                    local_status = dict(local_status)
                    supervisor = getattr(store, "plugin_supervisor", None)
                    workers = supervisor.snapshot() if supervisor is not None else []
                    from .source_coverage import process_source_availability
                    availability = process_source_availability(workers, plugin_status["sessions"])
                    local_status["process_source_availability"] = availability
                    by_source = {item["source_id"]: item for item in availability}
                    projected_sessions = []
                    for session in plugin_status["sessions"]:
                        projected = dict(session)
                        source = by_source.get(projected.get("plugin_source_id"))
                        if source is not None and source["availability"] != "ready":
                            projected["source_availability"] = source["availability"]
                            projected["source_health_code"] = source["reason_code"]
                        projected_sessions.append(projected)
                    if projected_sessions:
                        local_status["sessions"] = [*(local_status.get("sessions") or ()),
                                                    *projected_sessions]
                        local_status["enabled"] = True
                    local_status["plugin_projection"] = {
                        "pending_sources": plugin_status["pending_sources"],
                        "omitted_sessions": plugin_status["omitted_sessions"],
                        "retention_gaps": plugin_status["retention_gaps"]}
                    if supervisor is not None:
                        local_status["plugin_workers"] = workers
                    if (plugin_status["sessions"] or plugin_status["omitted_sessions"] or
                            plugin_status["pending_sources"] or plugin_status["retention_gaps"] or
                            local_status.get("plugin_workers")):
                        omitted = plugin_status["omitted_sessions"]
                        partial = bool(omitted or plugin_status["pending_sources"] or
                                       plugin_status["retention_gaps"] or
                                       any(item["availability"] != "ready" for item in availability))
                        local_status["source_coverage"] = [
                            *(local_status.get("source_coverage") or ()),
                            {"label": "Process adapters", "unit": "sessions",
                             "active": len(projected_sessions),
                             "omitted": 1 if omitted else 0,
                             "omitted_exact": not omitted,
                             "pending": plugin_status["pending_sources"],
                             "state": "partial" if partial else "ready"}]
                except (sqlite3.Error, ValueError) as exc:
                    local_status = dict(local_status)
                    local_status["plugin_projection"] = {"error": type(exc).__name__}
                    local_status["source_coverage"] = [
                        *(local_status.get("source_coverage") or ())[:23],
                        {"label": "Process adapters", "unit": "sessions",
                         "active": None, "omitted": None, "state": "unavailable"}]
                self._json(200, {"ok": True,"observed_time_unix_nano":time.time_ns(), "receiver_id": store.receiver_namespace() if hasattr(store,"receiver_namespace") else None, "review_trace_filter": True, "review_turn_filter": True, "review_run_summary": True,
                    "investigation_index_v1":getattr(store,'investigation_index_v1',False),
                    "native_lifecycle_v1":callable(getattr(store,'native_lifecycle_inventory',None)),
                    "security":{"authentication_required":bool(token),"host_validation":True},
                    "storage":{"retention_days":getattr(store,"retention_days",None)},
                    "indexing":indexing,
                    "local_agents": local_status})
            elif path == "/api/connections":
                self._json(200, {"connections": store.connections()})
            elif path == "/api/hierarchy":
                query = parse_qs(route.query)
                try:
                    limit = max(1, min(10000, int(query.get("limit", [2000])[0])))
                    include_reviews = query.get("include_reviews", ["1"])[0] not in {"0", "false", "False"}
                    nodes = store.agent_nodes(limit=limit, include_reviews=include_reviews)
                    graph = HierarchyGraph(nodes)
                    self._json(200, {
                        "nodes": nodes,
                        "capabilities": CAPABILITIES,
                        "revision": store.hierarchy_revision(),
                        "diagnostics": graph.diagnostics,
                        "coverage": ("complete" if not graph.diagnostics["missing_parents"] and
                                     not graph.diagnostics["announced"] and
                                     all(node.get("coverage") == "complete" for node in nodes)
                                     else "partial"),
                    })
                except ValueError as exc:
                    self._json(400, {"error": str(exc)})
            elif path == "/api/review-run":
                query=parse_qs(route.query)
                try:self._json(200,{'run':store.review_run(query.get('trace_id',[''])[0])})
                except ValueError as exc:self._json(400,{'error':str(exc)})
            elif path == "/api/review-turn":
                query=parse_qs(route.query)
                try:self._json(200,{'turn':store.review_turn(query.get('trace_id',[''])[0])})
                except ValueError as exc:self._json(400,{'error':str(exc)})
            elif path == "/api/activity":
                query = parse_qs(route.query, keep_blank_values=True)
                provenance_watch = None
                try:
                    capability_scope = 'capture_scope' in query
                    capture_token = None
                    source_ids = None
                    if capability_scope:
                        allowed = {'capture_scope','limit','service','session_id',
                                   'agent_id','subtree','include_reviews'}
                        if (set(query) != allowed or any(len(values) != 1 for values in query.values())
                                or query['capture_scope'] != ['1'] or query['subtree'] != ['0']
                                or query['include_reviews'] != ['0']
                                or not query['service'][0] or len(query['service'][0]) > 200
                                or not query['session_id'][0] or len(query['session_id'][0]) > 200
                                or not AGENT_ID.fullmatch(query['agent_id'][0])
                                or not 1 <= int(query['limit'][0]) <= 200):
                            raise ValueError('invalid capability Activity scope')
                        monitor = getattr(store, 'local_agent_monitor', None)
                        def policy_token():
                            if monitor is None or getattr(store, 'local_agent_monitor', None) is not monitor:
                                return None
                            try:
                                sources = capture_sources(monitor.status(),
                                    service=query['service'][0], session_id=query['session_id'][0],
                                    agent_id=query['agent_id'][0])
                                source_token = (store.capability_source_token(sources)
                                                if sources is not None else None)
                            except Exception:
                                # An adapter status failure must deny this
                                # optional capability, never widen its scope.
                                return None
                            return (monitor, sources, source_token) if source_token is not None else None
                        capture_token = policy_token()
                        if capture_token is None:
                            self._json(403, {'error': 'Capability capture denied for exact scope.'})
                            return
                        source_ids = capture_token[1]
                        # This connection keeps no read transaction. Its
                        # data_version detects any commit between admission
                        # and the final response guard, including writes by
                        # older or external receivers.
                        provenance_watch, initial_data_version = _capability_revision_watch(store.path)
                    certificate=({'_initial_receiver':self._get_receiver_id,'_initial_file':self._activity_file_identity,
                                  '_strict_read':not indexed_activity}if certified_activity else {})
                    page = store.activity_page(limit=int(query.get("limit", [100])[0]),
                        service=query.get("service", [None])[0], domain=query.get("domain", [None])[0],
                        query=query.get("q", [None])[0], session_id=query.get("session_id", [None])[0],
                        cursor=query.get("cursor", [None])[0],
                        include_reviews=query.get("include_reviews", ["1"])[0]!="0",
                        errors_only=query.get("errors_only", ["0"])[0]=="1",
                        action=query.get("action", [None])[0],
                        agent_id=query.get("agent_id", [None])[0],
                        subtree=query.get("subtree", ["0"])[0]=="1",
                        scope_revision=query.get("scope_revision", [None])[0],
                        trace_id=query.get("trace_id", [None])[0], turn_id=query.get("turn_id", [None])[0],
                        review=query.get("review", ["0"])[0]=="1",
                        _capture_source_ids=source_ids,**certificate)
                    if capability_scope:
                        # Opaque to UI consumers. This identifies the receiver
                        # process and durable capture-policy generation, not
                        # the Activity page's contents or source IDs.
                        monitor_token = _capability_monitor_token(capture_token[0])
                        if monitor_token is not None:
                            revision = json.dumps((self._get_receiver_id,
                                _CAPABILITY_PROCESS_INSTANCE, monitor_token,
                                capture_token[1], capture_token[2]), separators=(',', ':'))
                            page = {**page, 'capture_revision':
                                hashlib.sha256(revision.encode()).hexdigest()}
                        def current_capability_revision():
                            try:
                                return (policy_token() == capture_token and
                                    not provenance_watch.in_transaction and
                                    provenance_watch.execute('PRAGMA data_version').fetchone()[0]
                                    == initial_data_version)
                            except (sqlite3.Error, OSError):
                                return False
                        self._capability_guard = current_capability_revision
                    self._json(200, page)
                except ValueError as exc:
                    self._json(400, {"error": str(exc)})
                except (sqlite3.Error, OSError):
                    self._json(503, {"error": "Capability source revision unavailable. Retry the request."})
                finally:
                    self._capability_guard = None
                    if provenance_watch is not None:
                        provenance_watch.close()
            elif path == "/api/plugin-activity-proof":
                from .plugin_activity import export_proof
                query = parse_qs(route.query, keep_blank_values=True)
                try:
                    if set(query) != {'event_id'} or len(query['event_id']) != 1:
                        raise ValueError('one exact plugin Activity event is required')
                    # The revision watch is tied to its original inode. A DB
                    # replacement can keep the same receiver ID and leave its
                    # data_version unchanged, so the response also certifies
                    # the Activity file identity in _json_bytes.
                    self._activity_file_identity = store._source_file_identity('Activity')
                    watch, initial_version = _capability_revision_watch(store.path)
                    try:
                        proof = export_proof(store, query['event_id'][0])
                        if proof is not None and proof['receiver_id'] != self._get_receiver_id:
                            self._json(409, {'error': 'Plugin source receiver changed.'})
                            return
                        self._capability_guard = lambda: (not watch.in_transaction and
                            watch.execute('PRAGMA data_version').fetchone()[0] == initial_version)
                        self._json(200, {'receiver_id': self._get_receiver_id, 'proof': proof})
                    finally:
                        self._capability_guard = None
                        watch.close()
                except ValueError as exc:
                    self._json(400, {'error': str(exc)})
                except (sqlite3.Error, OSError):
                    self._json(503, {'error': 'Plugin Activity source revision unavailable.'})
            elif path == "/api/plugin-content":
                # Explicit, exact-event text access. The active receiver owns
                # the source grant and never trusts a path supplied by HTTP.
                from .plugin_activity import _EVENT_KEY
                from .plugin_pi_content_read import Available, read_exact_pi_user_text
                from .receiver_state import loopback_host
                from .workflow_evidence import safe_text
                if not loopback_host(self.server.server_address[0]) or \
                        not loopback_host(self.client_address[0]):
                    self._json(403, {'error': 'Captured text requires a local receiver.'})
                    return
                query = parse_qs(route.query, keep_blank_values=True)
                if set(query) != {'event_id'} or len(query['event_id']) != 1:
                    self._json(400, {'error': 'One exact plugin Activity event is required.'})
                    return
                event_id = query['event_id'][0]
                if not _EVENT_KEY.fullmatch(event_id):
                    self._json(400, {'error': 'Invalid plugin Activity event.'})
                    return
                selected = read_exact_pi_user_text(store, event_id)
                if not isinstance(selected, Available):
                    self._json(404, {'error': 'Captured text unavailable for this exact event.'})
                    return
                # Bound the response independently of the retained source row.
                clean = safe_text(selected.redacted_text)
                encoded = clean.encode('utf-8')
                clipped = len(encoded) > 4096
                preview = encoded[:4096].decode('utf-8', 'ignore') if clipped else clean
                proof = selected.proof
                try:self._activity_file_identity = store._source_file_identity('Activity')
                except (OSError, ValueError, sqlite3.Error):
                    self._json(503, {'error': 'Plugin source revision unavailable.'})
                    return
                self._cache_version = None
                self._capability_guard = lambda: read_exact_pi_user_text(store, event_id) == selected
                try:
                    data = json.dumps({
                        'receiver_id': proof.receiver_id, 'event_id': event_id,
                        'redacted_text': preview, 'clipped': clipped,
                        'revision': [proof.lease_epoch, proof.policy_epoch,
                                     proof.source_dirty_seq],
                    }, ensure_ascii=False).encode('utf-8')
                    self._json_bytes(200, data, body_receiver=proof.receiver_id)
                finally:
                    self._capability_guard = None
            elif path == "/api/plugin-command-content":
                # One opt-in Pi bash request, selected by its projected Activity
                # event. A started fact records the request, not execution.
                from .plugin_activity import _EVENT_KEY
                from .plugin_pi_command_read import Available, read_exact_pi_requested_command
                from .plugin_wire import ProtocolError
                from .receiver_state import loopback_host
                from .rust_shadow_snapshot import _store_file_identity
                from .workflow_evidence import safe_text
                if not loopback_host(self.server.server_address[0]) or \
                        not loopback_host(self.client_address[0]):
                    self._json(403, {'error': 'Captured text requires a local receiver.'})
                    return
                query = parse_qs(route.query, keep_blank_values=True)
                if set(query) != {'event_id'} or len(query['event_id']) != 1:
                    self._json(400, {'error': 'One exact plugin Activity event is required.'})
                    return
                event_id = query['event_id'][0]
                if not _EVENT_KEY.fullmatch(event_id):
                    self._json(400, {'error': 'Invalid plugin Activity event.'})
                    return
                try:
                    store_identity = _store_file_identity(store)
                except (OSError, ValueError, sqlite3.Error, ProtocolError):
                    self._json(503, {'error': 'Plugin source revision unavailable.'})
                    return
                selected = read_exact_pi_requested_command(store, event_id)
                if not isinstance(selected, Available):
                    self._json(404, {'error': 'Requested command unavailable for this exact event.'})
                    return
                clean = safe_text(selected.redacted_command)
                encoded = clean.encode('utf-8')
                clipped = len(encoded) > 4096
                preview = encoded[:4096].decode('utf-8', 'ignore') if clipped else clean
                proof = selected.proof
                try:
                    if _store_file_identity(store) != store_identity:
                        self._json(409, {'error': 'Plugin source revision changed.'})
                        return
                    self._activity_file_identity = store._source_file_identity('Activity')
                except (OSError, ValueError, sqlite3.Error, ProtocolError):
                    self._json(503, {'error': 'Plugin source revision unavailable.'})
                    return
                self._cache_version = None

                def current_command():
                    try:
                        return (_store_file_identity(store) == store_identity and
                                read_exact_pi_requested_command(store, event_id) == selected and
                                _store_file_identity(store) == store_identity)
                    except (OSError, ValueError, sqlite3.Error, ProtocolError):
                        return False

                self._capability_guard = current_command
                try:
                    data = json.dumps({
                        'receiver_id': proof.receiver_id, 'event_id': event_id,
                        'redacted_text': preview, 'clipped': clipped,
                        'revision': [proof.lease_epoch, proof.policy_epoch,
                                     proof.source_dirty_seq],
                    }, ensure_ascii=False).encode('utf-8')
                    self._json_bytes(200, data, body_receiver=proof.receiver_id)
                finally:
                    self._capability_guard = None
            elif path == "/api/metrics":
                certificate=({'_initial_receiver':self._get_receiver_id,'_initial_file':self._metrics_file_identity}if certified_metrics else {})
                self._json(200, store.metrics(**certificate))
            elif path == "/api/mcp":
                query = parse_qs(route.query)
                try:
                    certificate=({'_initial_receiver':self._get_receiver_id,'_initial_file':self._mcp_file_identity,
                                  '_strict_read':not raw_mcp}if certified_mcp else {})
                    page = store.mcp_page(limit=int(query.get('limit', [100])[0]),
                        service=query.get('service', [None])[0], session_id=query.get('session_id', [None])[0],
                        query=query.get('q', [None])[0], cursor=query.get('cursor', [None])[0],
                        include_reviews=query.get('include_reviews', ['1'])[0]!='0',
                        agent_id=query.get('agent_id', [None])[0],
                        subtree=query.get('subtree', ['0'])[0]=='1',
                        scope_revision=query.get('scope_revision', [None])[0],**certificate)
                    self._json(200, {**page, 'scope': page.get('scope', 'stored MCP operations')})
                except ValueError as exc:
                    self._json(400, {'error': str(exc)})
            elif path == "/api/usage-series":
                query = parse_qs(route.query)
                try:
                    days = int(query.get("days", [7])[0])
                    step = query.get("step", ["day"])[0]
                    service = query.get("service", [None])[0]
                    session_id = query.get("session_id", [None])[0]
                    self._json(200, store.usage_series(days=days, step=step, service=service,
                                                       session_id=session_id))
                except ValueError as exc:
                    self._json(400, {"error": str(exc)})
            elif path == "/api/logs":
                self._json(200, {"logs": store.logs(limit=500)})
            elif path == "/api/measurements":
                query = parse_qs(route.query, keep_blank_values=True)
                try:
                    limit = max(1, min(500, int(query.get("limit", [500])[0])))
                    if query.get("latest", ["0"])[0] == "1":
                        points = store.metric_latest_points(limit=limit,
                            service=query.get("service", [None])[0])
                        self._json(200, {"points": points})
                        return
                    temporality = query.get("temporality", [None])[0]
                    points = store.metric_points(limit=limit,
                        service=query.get("service", [None])[0], name=query.get("name", [None])[0],
                        kind=query.get("kind", [None])[0],
                        temporality=int(temporality) if temporality is not None else None,
                        scope_name=query.get("scope_name", [None])[0],
                        unit=query.get("unit", [None])[0],
                        description=query.get("description", [None])[0])
                except ValueError as exc:
                    self._json(400, {"error": str(exc)})
                    return
                self._json(200, {"points": points})
            elif path == "/api/metric-catalog":
                query = parse_qs(route.query)
                try:
                    limit = max(1, min(500, int(query.get("limit", [200])[0])))
                    self._json(200, {"metrics": store.metric_catalog(limit=limit)})
                except ValueError as exc:
                    self._json(400, {"error": str(exc)})
            elif path == '/api/investigations/source':
                query=parse_qs(route.query)
                try:self._json(200,store.investigation_source(query.get('event_id',[''])[0],query.get('receiver_id',[''])[0],query.get('reference',[''])[0]))
                except ValueError as exc:self._json(400,{'error':str(exc)})
            elif path == '/api/investigations':
                query=parse_qs(route.query)
                try:self._json(200,store.investigation_page(query.get('kind',[''])[0],query.get('q',[''])[0],
                    limit=int(query.get('limit',['50'])[0]),cursor=query.get('cursor',[None])[0],
                    service=query.get('service',[None])[0],failed=query.get('failed',['0'])[0]=='1',
                    agent_id=query.get('agent_id',[None])[0],subtree=query.get('subtree',['0'])[0]=='1',
                    scope_revision=query.get('scope_revision',[None])[0]))
                except ValueError as exc:self._json(400,{'error':str(exc)})
            elif path == '/api/native-lifecycle':
                self._json(200,store.native_lifecycle_inventory())
            elif path.startswith('/api/findings/') and path.endswith('/source'):
                try:self._json(200,store.finding_source(path[len('/api/findings/'):-len('/source')]))
                except ValueError as exc:self._json(400,{'error':str(exc)})
            elif path == "/api/findings":
                query = parse_qs(route.query)
                try:
                    limit = max(1, min(500, int(query.get("limit", [100])[0])))
                    page = store.finding_page(limit=limit, cursor=query.get("cursor", [None])[0],
                                              agent_id=query.get("agent_id", [None])[0],
                                              subtree=query.get("subtree", ["0"])[0] == "1",
                                              scope_revision=query.get("scope_revision", [None])[0])
                except ValueError as exc:
                    self._json(400, {"error": str(exc)})
                    return
                self._json(200, page)
            elif path == "/api/sessions":
                query = parse_qs(route.query)
                try:
                    limit = max(1, min(500, int(query.get("limit", [100])[0])))
                    certificate=({'_initial_receiver':self._get_receiver_id,'_initial_file':self._sessions_file_identity}if certified_sessions else {})
                    self._json(200, {"sessions": store.sessions(limit=limit,**certificate)})
                except ValueError as exc:
                    self._json(400, {"error": str(exc)})
            elif path == "/api/endpoints":
                query = parse_qs(route.query)
                try:
                    limit = max(1, min(500, int(query.get("limit", [100])[0])))
                    self._json(200, {"endpoints": store.endpoints(limit=limit)})
                except ValueError as exc:
                    self._json(400, {"error": str(exc)})
            elif path == "/api/outcomes":
                self._json(200, {"outcomes": list(store.outcomes().values())})
            elif path == "/api/traces":
                query = parse_qs(route.query)
                try:
                    limit = max(1, min(500, int(query.get("limit", [100])[0])))
                except ValueError:
                    self._json(400, {"error": "invalid limit"})
                    return
                try:
                    page = store.trace_page(limit, query.get("service", [None])[0],
                                            query.get("status", [None])[0],
                                            query.get("cursor", [None])[0],
                                            query.get("session_id", [None])[0],
                                            query.get("q", [None])[0],
                                            query.get("endpoint_id", [None])[0],
                                            query.get("include_reviews", ["1"])[0]!="0",
                                            query.get("agent_id", [None])[0],
                                            query.get("subtree", ["0"])[0]=="1",
                                            query.get("scope_revision", [None])[0])
                except ValueError as exc:
                    self._json(400, {"error": str(exc)})
                    return
                self._json(200, page)
            elif path.startswith("/api/traces/") and path.endswith("/related"):
                trace_id = path.removeprefix("/api/traces/").removesuffix("/related")
                if not TRACE_ID.fullmatch(trace_id):
                    self._json(400, {"error": "invalid trace_id"})
                    return
                self._json(200, {"related": store.related_traces(trace_id)})
            elif path.startswith("/api/traces/") and path.endswith("/spans"):
                trace_id = path.removeprefix("/api/traces/").removesuffix("/spans")
                if not TRACE_ID.fullmatch(trace_id):
                    self._json(400, {"error": "invalid trace_id"})
                    return
                query = parse_qs(route.query, keep_blank_values=True)
                try:
                    limit = max(1, min(500, int(query.get("limit", [200])[0])))
                    page = store.span_page(trace_id, limit=limit,
                        cursor=query.get("cursor", [None])[0],
                        query=query.get("q", [""])[0].strip()[:200],
                        span_filter=query.get("filter", ["all"])[0])
                except ValueError as exc:
                    self._json(400, {"error": str(exc)})
                    return
                self._json(200, page)
            elif path.startswith("/api/traces/"):
                trace_id = path.removeprefix("/api/traces/")
                if not TRACE_ID.fullmatch(trace_id):
                    self._json(400, {"error": "invalid trace_id"})
                    return
                query = parse_qs(route.query, keep_blank_values=True)
                if "span_limit" in query:
                    try:
                        limit = max(1, min(500, int(query.get("span_limit", [200])[0])))
                        def read_trace(db):
                            result=store.trace_overview(trace_id,_db=db)
                            page=store.span_page(trace_id,limit=limit,
                                query=query.get("q",[""])[0].strip()[:200],
                                span_filter=query.get("filter",["all"])[0],_db=db)
                            return result,page
                        result,span_page=store._derived_snapshot(read_trace)
                    except ValueError as exc:
                        self._json(400, {"error": str(exc)})
                        return
                    result["spans"] = span_page["spans"]
                    result["span_page"] = {key: span_page[key] for key in
                                            ("total", "trace_total", "next_cursor", "start_unix_nano", "end_unix_nano")}
                else:
                    result = store.trace(trace_id)
                self._json(200 if result["summary"] or result["progress"] or result["logs"] or result["outcome"] else 404, result)
            else:
                self._json(404, {"error": "not found"})

        def do_POST(self):
            route = self._route()
            if route is None:
                return
            try:
                size = int(self.headers.get("Content-Length", "0"))
                if size <= 0 or size > MAX_BODY:
                    self._json(413, {"error": "body size must be 1 to 8 MiB"})
                    return
                raw = self.read_body(size)
                if len(raw) != size:
                    raise ValueError("truncated request body")
                encoding = self.headers.get("Content-Encoding", "identity").lower()
                if encoding == "gzip":
                    with gzip.GzipFile(fileobj=io.BytesIO(raw)) as stream:
                        raw = stream.read(MAX_BODY + 1)
                    if len(raw) > MAX_BODY:
                        self._json(413, {"error": "decompressed body exceeds 8 MiB"})
                        return
                elif encoding != "identity":
                    self._json(415, {"error": "unsupported content encoding"})
                    return
                content_type = self.headers.get("Content-Type", "").split(";")[0].strip()
                if route.path in ("/v1/traces", "/v1/logs", "/v1/metrics"):
                    if content_type == "application/x-protobuf":
                        if route.path == "/v1/traces":
                            store.ingest(decode_trace_request(raw))
                        elif route.path == "/v1/logs":
                            store.ingest_logs(decode_log_request(raw),expected_receiver=self.headers.get('X-Agenturm-Expected-Receiver-ID'))
                        else:
                            store.ingest_metrics(decode_metric_request(raw))
                        self._protobuf_ok()
                        return
                    if content_type != "application/json":
                        self._json(415, {"error": "OTLP requires application/json or application/x-protobuf"})
                        return
                body = json.loads(raw)
                if route.path == "/api/spans":
                    raw_spans = body if isinstance(body, list) else body.get("spans", [])
                    if not isinstance(raw_spans, list):
                        raise ValueError("spans must be an array")
                    service = body.get("service", "agent-harness") if isinstance(body, dict) else "agent-harness"
                    count = store.ingest([normalize_native(s, service) for s in raw_spans])
                    self._json(200, {"accepted": count})
                elif route.path == "/v1/traces":
                    count = store.ingest(normalize_otlp(body))
                    self._json(200, {"partialSuccess": {"rejectedSpans": 0}, "accepted": count})
                elif route.path == "/v1/logs":
                    count = store.ingest_logs(normalize_otlp_logs(body),expected_receiver=self.headers.get('X-Agenturm-Expected-Receiver-ID'))
                    self._json(200, {"partialSuccess": {"rejectedLogRecords": 0}, "accepted": count})
                elif route.path == "/v1/metrics":
                    count = store.ingest_metrics(normalize_otlp_metrics(body))
                    self._json(200, {"partialSuccess": {"rejectedDataPoints": 0}, "accepted": count})
                elif route.path == "/api/progress":
                    store.add_progress(body["trace_id"], body["stage"], body.get("message", ""),
                                       body.get("percent"), body.get("session_id"), body.get("service"))
                    self._json(200, {"accepted": 1})
                elif route.path == "/api/connections":
                    store.register_connection(body)
                    self._json(200, {"accepted": 1})
                elif route.path == "/api/outcomes":
                    records = body if isinstance(body, list) else body.get("results", [body])
                    count = store.ingest_outcomes(records, source="api")
                    self._json(200, {"accepted": count})
                else:
                    self._json(404, {"error": "not found"})
            except TimeoutError:
                raise
            except (ValueError, TypeError, KeyError, AttributeError, OSError, EOFError, OverflowError, RecursionError) as exc:
                if self._response_started:
                    raise
                self._json(400, {"error": str(exc)})

        def log_message(self, format, *args):
            if not quiet and os.environ.get("OBSERVATORY_QUIET") != "1":
                super().log_message(format, *args)

    return Handler


def serve(db_path=None, host="127.0.0.1", port=4318, token=None,
          watch_jsonl=(), watch_service="agent-harness", retention_days=None, local_agents=True, metadata_only=False,
          managed_db=False, no_auth=False, allowed_hosts=(), collector_registry=None,
          plugin_config=None, rust_plugin_host=None, rust_plugin_host_sha256=None):
    if db_path is None:
        from .receiver_state import telemetry_path,prepare_state_directory
        prepare_state_directory();db_path=str(telemetry_path());managed_db=True
    if not 1<=port<=65535:raise ValueError("Choose a receiver port between 1 and 65535.")
    from .receiver_state import local_token,loopback_host,retention_policy
    if not loopback_host(host) and not allowed_hosts:raise ValueError("An external bind requires --allowed-host with the exact receiver hostname.")
    if not token and not no_auth:
        if not loopback_host(host):raise ValueError('An externally bound receiver requires an explicit token and allowed Host configuration.')
        token=local_token(port,create=True)
    plugin_sources = ()
    if rust_plugin_host is not None or rust_plugin_host_sha256 is not None:
        if plugin_config is None:
            raise ValueError("Rust plugin host requires --plugin-config")
        from .plugin_process import rust_host_artifact
        rust_host_artifact(str(rust_plugin_host) if rust_plugin_host is not None else None,
                           rust_plugin_host_sha256)
    if plugin_config is not None:
        if not loopback_host(host):
            raise ValueError("plugin adapters require a loopback receiver")
        from .plugin_config import read_plugin_config
        plugin_sources = read_plugin_config(plugin_config)
        # Activation validation is inert. Fail before opening the Store or
        # disabling a trusted source if a configured child cannot be admitted.
        from .plugin_process import PluginProcess
        for source in plugin_sources:
            child = PluginProcess(source.manifest, source_id=source.source_id,
                                  allowed_grants=source.allowed_grants, timeout=source.timeout,
                                  source_root=source.source_root,
                                  required_features=source.runtime_features,
                                  rust_host_executable=str(rust_plugin_host) if rust_plugin_host else None,
                                  rust_host_sha256=rust_plugin_host_sha256)
            if source.replaces:
                child.verify_artifact()
    replacements = tuple(name for source in plugin_sources for name in source.replaces)
    if replacements:
        if not local_agents:
            raise ValueError("built-in source replacement requires local collection")
        from .collector_runtime import CollectorRegistry
        selected = collector_registry if collector_registry is not None else CollectorRegistry.bundled()
        if not isinstance(selected, CollectorRegistry):
            raise TypeError("collector registry must be a CollectorRegistry")
        known_sources = {spec.id for spec in (*selected.sources, *selected.readers)}
        if not set(replacements) <= known_sources:
            raise ValueError("plugin replacement names an unavailable built-in harness")
        collector_registry = selected.without(*replacements)
    store = Store(db_path, async_derived_refresh=True)
    retention_days=retention_policy(store,managed=managed_db,requested=retention_days)
    if retention_days is not None:
        store.prune(retention_days)
    server = ThreadingHTTPServer((host, port), handler_for(store, token,allowed_hosts=allowed_hosts))
    local = None
    if local_agents and host in {"127.0.0.1", "localhost", "::1"}:
        from .collector_runtime import start_local_collector
        local = start_local_collector(store, capture_details=not metadata_only,
                                      registry=collector_registry)
    store.local_agent_monitor = local
    watch = start_watchers(store, watch_jsonl, watch_service) if watch_jsonl else None
    maintenance = _start_retention(store, retention_days) if retention_days is not None else None
    plugin_supervisor = None
    print(f"Agenturm receiver listening at http://{host}:{server.server_port} (database: {db_path})", flush=True)
    if local:
        print("Watching local Codex / Claude Code sessions · " + ("metadata only" if metadata_only else "activity and bounded, redacted content"), flush=True)
    try:
        if plugin_sources:
            from .plugin_supervisor import PluginSupervisor
            plugin_supervisor = PluginSupervisor(
                store, plugin_sources,
                rust_host_executable=str(rust_plugin_host) if rust_plugin_host else None,
                rust_host_sha256=rust_plugin_host_sha256).start()
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
        if plugin_supervisor:
            plugin_supervisor.close()
        if local:
            local.close()
        if watch:
            watch[0].set()
            watch[1].join(timeout=2)
        if maintenance:
            maintenance[0].set()
            maintenance[1].join(timeout=2)
        store.close()


def _start_retention(store, retention_days, interval_seconds=3600):
    stop = threading.Event()

    def maintain():
        while not stop.is_set():
            stop.wait(interval_seconds)
            if stop.is_set():
                break
            store.prune(retention_days)

    thread = threading.Thread(target=maintain, name="observatory-retention", daemon=True)
    thread.start()
    return stop, thread
