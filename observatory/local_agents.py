"""Observe local agent session metadata without changing or restarting agents.

Session files are read-only. Content capture is bounded and redacted; metadata-only
mode omits task text, arguments, and results. Agent log formats vary by version.
"""
from __future__ import annotations

import hashlib
import heapq
import json
import math
import os
import platform
import socket
import threading
import time
from datetime import datetime
from pathlib import Path

from .agent_hierarchy import canonical_agent_id, provenance
from .core import normalize_native
from .local_activity import mcp_attributes, mcp_failed, plaintext
from .local_state import RecentMap
from .native_json import loads
from .native_sources import NativeSourceSpec, builtin_sources, kiro_home
from .native_ports import NativeEvidenceFacade, NativeEvidencePort
from .native_adapter import NativeSessionAdapter, builtin_adapter

MAX_READ = 8 * 1024 * 1024
MAX_LINE = 2 * 1024 * 1024
MAX_SOURCES = 50
MAX_TASK_PREFIX_BYTES = 512 * 1024
MAX_TASK_PREFIX_RECORDS = 256
MAX_RECENT_TURNS = 64
MAX_RECENT_CALLS = 512
MAX_RECENT_USAGE = 256
CLAUDE_USAGE_FIELDS = ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")


def identity(*parts, size=32):
    return hashlib.sha256("\0".join(map(str, parts)).encode()).hexdigest()[:size]


def timestamp(value):
    if isinstance(value, (int, float)):
        if isinstance(value, float) and not math.isfinite(value):
            return 0
        magnitude = abs(value)
        factor = 1 if magnitude >= 1e17 else 1000 if magnitude >= 1e14 else 1_000_000 if magnitude >= 1e11 else 1_000_000_000
        stamp = max(0, int(value * factor))
        return stamp if stamp <= 2**63 - 1 else 0
    if isinstance(value, str):
        try:
            stamp = max(0, int(datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp() * 1e9))
            return stamp if stamp <= 2**63 - 1 else 0
        except (ValueError, OverflowError, OSError):
            pass
    return 0


def count(value):
    try:
        return max(0, int(value or 0))
    except (ValueError, TypeError, OverflowError):
        return 0


def discover_mcp_configurations(global_paths=None, project_directories=(), on_error=None):
    """Compatibility entry point for trusted adapter MCP config contributors."""
    from .mcp_config_registry import discover

    return discover(global_paths=global_paths, project_directories=project_directories,
                    on_error=on_error)


class Session:
    def __init__(self, store: NativeEvidencePort, path, runtime, capture_details=True,
                 adapter: NativeSessionAdapter | None = None):
        self.store, self.path, self.runtime = store, Path(path), runtime
        self.adapter = adapter if adapter is not None else builtin_adapter(runtime)
        if self.adapter is None:
            raise ValueError(f"no native adapter for {runtime}")
        self.service = runtime.lower().replace(" ", "-")
        self.source_id = identity(runtime, self.path.resolve())
        self.capture_details = capture_details
        self.internal = False
        self.session_id = self.path.stem
        # Hierarchy identity is deliberately separate from the file source ID.
        # A child discovered through a spawn result and later through its own
        # transcript resolves to this same canonical node.
        self.agent_namespace = getattr(self.adapter, "agent_namespace", runtime.casefold())
        self.native_session_id = self.session_id
        self.native_worker_id = None
        self.native_root_session_id = None
        self.agent_id = canonical_agent_id(runtime, self.agent_namespace, self.session_id)
        self.parent_agent_id = None
        self.relationship_kind = "unknown"
        self.relationship_state = "unresolved"
        self.relationship_provenance = provenance(runtime, "session path; no parent metadata")
        self.discovery_state = "observed"
        self.announced_agents = RecentMap(MAX_RECENT_CALLS)
        self.spawn_calls = RecentMap(MAX_RECENT_CALLS)
        self.offset = 0
        self.file_id = None
        self.pending = b""
        self.discarding_line = False
        self.turn = "session"
        self.model = None
        self.provider = None
        self.project = ""
        self.working_directory = ""
        self.branch = ""
        self.run_state = "unknown"
        self.state_since = 0
        self.waiting_for = ""
        self.request_id = ""
        self.request_time_ns = 0
        self.outcome_id = ""
        self.outcome_time_ns = 0
        self.pending_tools = RecentMap(MAX_RECENT_CALLS)
        self.pending_request_times = RecentMap(MAX_RECENT_CALLS)
        self.started = RecentMap(MAX_RECENT_TURNS)
        self.calls = RecentMap(MAX_RECENT_CALLS)
        self.usage = RecentMap(MAX_RECENT_USAGE)
        self.tasks = RecentMap(MAX_RECENT_TURNS)
        self.task_description = ''
        self.tool_inputs = RecentMap(MAX_RECENT_CALLS)
        self.modern_usage = False
        self.last_total = None
        self.last_heartbeat = 0
        self.last_state = None
        self.skipped = 0
        self.last_activity = None
        self.last_record_ns = 0
        self.last_success_ns = 0
        self.backlog_bytes = 0
        self.schema_errors = 0
        self.skipped_oversized = 0
        self.source_size_bytes = 0
        self.source_mtime_ns = 0
        self.records_last_poll = 0
        self.spans_last_poll = 0
        self.logs_last_poll = 0
        self.bytes_read_total = 0
        self.records_total = 0
        self.spans_total = 0
        self.logs_total = 0
        self._pending_ingest = None
        self._redaction_pending = False
        self._pending_spans_done = False
        self._pending_retry = False
        # State publication is constant-time and never holds this lock during
        # source, policy or SQLite I/O. Diagnostics must not combine fields
        # from two different batches while the collector is writing.
        self._storage_lock = threading.Lock()
        self._storage_epoch = identity(time.monotonic_ns(), id(self))
        self._storage_counter = 0
        self._storage_buffer = None
        self._storage_batch_id = None
        self._storage_started = self._storage_progress_at = None
        self._storage_progress_sequence = 0
        self._storage_attempt_in_flight = False
        self._storage_stage = 'idle'
        self._current_record_offset = 0
        initialize = getattr(self.adapter, "initialize", None)
        if initialize is not None:
            initialize(self)

    def _kiro_setup(self):
        return self.adapter.initialize(self)

    def kiro_reset_permissions(self):
        return self.adapter.reset_permissions(self)

    def kiro_permission(self, row, payload, stamp):
        return self.adapter.permission(self, row, payload, stamp)

    def kiro_permission_summary(self):
        return self.adapter.permission_summary(self)

    def kiro_refresh_metadata(self):
        return self.adapter.refresh_metadata(self)

    def kiro_refresh_rules(self, force=False):
        return self.adapter.refresh_rules(self, force)

    @property
    def last_activity(self):
        value=self._last_activity
        return {**value,'subject':'Approval review'} if self.internal and value else value

    @last_activity.setter
    def last_activity(self,value):
        self._last_activity={**value,'subject':'Approval review'} if self.internal and value else value

    def adopt_internal_policy(self):
        self.internal=True;self.capture_details=False;self.service='codex-review'
        self.tasks.clear();self.tool_inputs.clear();self.task_description=''
        self.last_activity=self.last_activity

    def refresh_capture_policy(self):
        if hasattr(self.store,'internal_source_ids') and self.source_id in self.store.internal_source_ids([self.source_id]):
            self.adopt_internal_policy()

    def native_position(self):
        if self.file_id is None:return {}
        return {'observatory.native_file_id':':'.join(map(str,self.file_id)),
            'observatory.native_file_offset':self._current_record_offset,
            'observatory.native_mtime_ns':self.source_mtime_ns}

    def hierarchy(self):
        """Return the structural observation persisted by the receiver."""
        coverage = "partial" if (self.schema_errors or self.skipped or self.backlog_bytes or self._pending_ingest is not None) else "complete"
        return {
            "agent_id": self.agent_id, "runtime": self.runtime,
            "source_namespace": self.agent_namespace, "source_id": self.source_id,
            "native_session_id": self.native_session_id,
            "native_worker_id": self.native_worker_id,
            "parent_agent_id": self.parent_agent_id,
            "relationship_kind": self.relationship_kind,
            "relationship_state": self.relationship_state,
            "relationship_provenance": self.relationship_provenance,
            "discovery_state": self.discovery_state,
            "coverage": coverage,
            "observed_ns": max(self.last_record_ns, self.last_success_ns),
            "review": bool(self.internal),
        }

    def announce(self, child_native_id, stamp, reference, parent_agent_id=None, namespace=None,
                 runtime=None):
        """Record an exact spawn result before the child transcript arrives."""
        child_native_id = _safe_identifier(child_native_id)
        if not child_native_id:
            return None
        runtime = runtime or self.runtime
        namespace = namespace or self.agent_namespace
        parent_agent_id = parent_agent_id or self.agent_id
        try:
            child_id = canonical_agent_id(runtime, namespace, child_native_id)
        except ValueError:
            return None
        if child_id == parent_agent_id:
            return None
        item = self.announced_agents.get(child_id) or {}
        item.update({
            "agent_id": child_id, "runtime": runtime, "source_namespace": namespace,
            "source_id": "announced:" + child_id,
            "native_session_id": child_native_id, "native_worker_id": child_native_id,
            "parent_agent_id": parent_agent_id,
            "relationship_kind": "delegated", "relationship_state": "confirmed",
            "relationship_provenance": provenance("spawn-result", reference, stamp),
            "discovery_state": "announced", "coverage": "unknown",
            "observed_ns": max(item.get("observed_ns") or 0, stamp or 0),
            "review": bool(self.internal),
        })
        self.announced_agents[child_id] = item
        return item

    def finish_turn(self):
        # Roots and model usage are durable in Store. Keep only the current
        # prompt/start for status; unresolved calls have their own bounded grace
        # window and must survive completion and subsequent turns.
        for key in list(self.started):
            if key != self.turn:
                del self.started[key]
        if self.turn in self.tasks:
            task = self.tasks[self.turn]
            self.tasks.clear()
            self.tasks[self.turn] = task
        else:
            self.tasks.clear()

    def _new_storage_batch(self, spans, logs):
        now = time.monotonic()
        with self._storage_lock:
            self._pending_ingest = (spans, logs)
            self._pending_spans_done = False
            self._pending_retry = False
            self._storage_counter += 1
            self._storage_batch_id = self._storage_epoch + ':' + str(self._storage_counter)
            self._storage_buffer = self._pending_ingest
            self._storage_started = self._storage_progress_at = now
            self._storage_progress_sequence = 0
            self._storage_attempt_in_flight = True
            self._storage_stage = 'prepared'

    def _storage_step(self, stage, *, progressed=False, spans_done=False):
        now = time.monotonic()
        with self._storage_lock:
            self._storage_stage = stage
            if progressed:
                self._storage_progress_sequence += 1
                self._storage_progress_at = now
            if spans_done:
                self._pending_spans_done = True

    def storage_diagnostics(self):
        """One coherent metadata snapshot; totals remain the buffered batch size.

        A retry may include spans already committed before its logs failed.
        Empty batches may also need a capture-policy retry, so the boolean
        failure state is authoritative independently of the record count.
        A retrying attempt stays ``retry`` while its in-flight flag is true.
        Progress advances after successful steps, not merely another attempt;
        batch age never resets on retry. Unknown state cannot certify delivery.
        """
        now = time.monotonic()
        with self._storage_lock:
            pending = self._pending_ingest
            total = sum(map(len, pending)) if pending is not None else 0
            known = pending is self._storage_buffer
            retry = bool(self._pending_retry)
            running = self._storage_attempt_in_flight
            state = ('idle' if pending is None and not retry and not running else
                     'retry' if known and pending is not None and retry else
                     'writing' if known and pending is not None and running else 'unknown')
            active = pending is not None
            return dict(storage_diagnostics_version=1, storage_state=state,
                pending_storage_records=total, storage_retry_pending=retry,
                storage_attempt_in_flight=running,
                storage_retry_records=total if state == 'retry' else 0,
                storage_in_flight_records=total if state == 'writing' else 0,
                storage_unknown_records=total if state == 'unknown' else 0,
                storage_batch_id=self._storage_batch_id if active and known else None,
                storage_batch_age_seconds=max(0, now-self._storage_started)
                    if active and known and self._storage_started is not None else None if active else 0,
                storage_progress_age_seconds=max(0, now-self._storage_progress_at)
                    if active and known and self._storage_progress_at is not None else None if active else 0,
                storage_progress_sequence=self._storage_progress_sequence if active and known else 0,
                storage_stage=self._storage_stage if known else 'unknown')

    def diagnostics(self):
        storage = self.storage_diagnostics()
        return {"source_path": str(self.path), "last_success_ns": self.last_success_ns,
                "backlog_bytes": self.backlog_bytes, "skipped_records": self.skipped,
                "source_size_bytes": self.source_size_bytes, "source_mtime_ns": self.source_mtime_ns,
                "records_last_poll": self.records_last_poll,
                "spans_last_poll": self.spans_last_poll, "logs_last_poll": self.logs_last_poll,
                "bytes_read_total": self.bytes_read_total, "records_total": self.records_total,
                "spans_total": self.spans_total, "logs_total": self.logs_total,
                "bytes_read": self.offset,
                "skipped_oversized_records": self.skipped_oversized,
                "schema_errors": self.schema_errors,
                **storage,
                "cache_evictions": sum(mapping.evictions for mapping in
                    (self.started, self.tasks, self.calls, self.tool_inputs, self.usage, self.pending_tools, self.pending_request_times)),
                "cache_tracked_bytes": sum(mapping.tracked_bytes for mapping in
                    (self.started, self.tasks, self.calls, self.tool_inputs, self.usage, self.pending_tools, self.pending_request_times)),
                "cache_max_bytes": sum(mapping.max_bytes for mapping in
                    (self.started, self.tasks, self.calls, self.tool_inputs, self.usage, self.pending_tools, self.pending_request_times)),
                "cache_dropped_oversized_entries": sum(mapping.dropped_oversized for mapping in
                    (self.started, self.tasks, self.calls, self.tool_inputs, self.usage, self.pending_tools, self.pending_request_times))}

    def observe_state(self, row, stamp):
        """Track explicit lifecycle records; absence of output is not an approval wait."""
        payload = (row.get("payload") or {}) if self.runtime == "Codex" else row
        if not isinstance(payload, dict):
            return
        cwd = payload.get("cwd")
        if cwd:
            self.project = Path(cwd).name
            self.working_directory = str(cwd) if self.capture_details else ""
        git = payload.get("git") or {}
        branch = payload.get("gitBranch") or (git.get("branch") if isinstance(git, dict) else None)
        if branch and self.capture_details:
            self.branch = str(branch)
        state = self.run_state
        event = payload.get("type")
        request_key = None
        result = self.adapter.observe_state(self, row, stamp)
        if result is None:
            return
        state, request_key = result
        if self.pending_tools:
            names = list(self.pending_tools.values())
            if any(name.rsplit(".", 1)[-1] in {"request_user_input", "AskUserQuestion"} for name in names):
                state, self.waiting_for = "waiting", "User input"
                request_key = next(key for key, name in self.pending_tools.items()
                    if name.rsplit('.', 1)[-1] in {'request_user_input', 'AskUserQuestion'})
            else:
                mcp = mcp_attributes(names[-1])
                label = mcp["mcp.server.name"] + " / " + mcp["mcp.tool.name"] if mcp else names[-1]
                self.waiting_for = "Tool response: " + label
                if len(names) > 1:
                    self.waiting_for += f" (+{len(names)-1} pending)"
        elif state != "waiting":
            self.waiting_for = ""
        if state == 'waiting' and request_key and str(request_key) != 'None':
            request_id = identity(self.source_id, self.turn, 'request', str(request_key))
            if request_id != self.request_id:
                self.request_id, self.request_time_ns = request_id, self.pending_request_times.get(str(request_key),stamp)
        elif state != 'waiting':
            self.request_id, self.request_time_ns = '', 0
        if state in {'completed', 'interrupted', 'failed', 'error', 'incomplete'}:
            if state != self.run_state or not self.outcome_id:
                self.outcome_id = identity(self.source_id, self.turn, 'outcome', state)
                self.outcome_time_ns = stamp
        else:
            self.outcome_id, self.outcome_time_ns = '', 0
        for key in list(self.pending_request_times):
            if key not in self.pending_tools:
                del self.pending_request_times[key]
        if state != self.run_state:
            self.state_since = stamp
        self.run_state = state

    def span(self, key, name, stamp, attrs=None, *, start=None, parent=True, error=False):
        if not stamp:
            return None
        relationship = {"agent.id": self.agent_id, "agent.runtime": self.runtime,
                        "agent.discovery_state": self.discovery_state,
                        "agent.relationship.kind": self.relationship_kind,
                        "agent.relationship.state": self.relationship_state,
                        "agent.native_session.id": self.native_session_id}
        if self.capture_details and not self.internal and self.task_description:
            relationship['agent.session.task'] = self.task_description
        if self.native_worker_id:
            relationship["agent.native_worker_id"] = self.native_worker_id
        if self.parent_agent_id:
            relationship["agent.parent_id"] = self.parent_agent_id
        attributes = {"session.id": self.session_id, "observatory.project": self.project,
                      "observatory.data_source": "local-session-log", **relationship,
                      **(attrs or {}), **self.native_position()}
        return normalize_native({
            "trace_id": identity(self.source_id, self.turn),
            "span_id": identity(self.source_id, key, size=16) if parent else identity(self.source_id, self.turn, "root", size=16),
            "parent_span_id": identity(self.source_id, self.turn, "root", size=16) if parent else None,
            "name": name, "start_unix_nano": min(start or stamp, stamp), "end_unix_nano": stamp,
            "status": "ERROR" if error else "OK", "attributes": attributes,
            "resource_attributes": {"service.name": self.service, "host.name": socket.gethostname(),
                "os.type": platform.system().lower(), "observatory.source_id": self.source_id,
                "observatory.capture": "local-session-log"},
            "scope_name": "observatory.local_agents",
        }, self.service)

    def root(self, stamp, state):
        attrs = {"agent.task": "Approval review" if self.internal else self.tasks.get(self.turn) or self.runtime + (" · " + self.project if self.project else "") + " · turn",
                 "agent.run.state": state}
        if state in {"completed", "interrupted"}:
            attrs["agent.stopped_reason"] = state
        return self.span((self.turn, "root"), "agent.run", stamp,
            attrs, start=self.started.get(self.turn), parent=False)

    def model_span(self, key, usage, stamp, *, claude=False):
        if not isinstance(usage, dict):
            return None
        attrs = {"gen_ai.operation.name": "chat",
                 "gen_ai.provider.name": "anthropic" if claude else "openai",
                 "observatory.duration_unknown": True}
        input_sources = ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens") if claude else ("input_tokens",)
        if any(name in usage for name in input_sources):
            attrs["gen_ai.usage.input_tokens"] = sum(count(usage.get(name)) for name in input_sources)
        if "output_tokens" in usage:
            attrs["gen_ai.usage.output_tokens"] = count(usage.get("output_tokens"))
        cache_read = "cache_read_input_tokens" if claude else (
            "cached_input_tokens" if "cached_input_tokens" in usage else "cache_read_input_tokens")
        if cache_read in usage:
            attrs["gen_ai.usage.cache_read.input_tokens"] = count(usage.get(cache_read))
        cache_write = "cache_creation_input_tokens" if claude else (
            "cache_write_input_tokens" if "cache_write_input_tokens" in usage else "cache_creation_input_tokens")
        if cache_write in usage:
            attrs["gen_ai.usage.cache_write.input_tokens"] = count(usage.get(cache_write))
        reasoning = "reasoning_output_tokens" if "reasoning_output_tokens" in usage else "reasoning_tokens"
        if reasoning in usage:
            attrs["gen_ai.usage.reasoning_tokens"] = count(usage.get(reasoning))
        if self.model:
            attrs["gen_ai.request.model"] = str(self.model)[:200]
        if claude:
            attrs["observatory.native_usage"] = {field: count(usage.get(field)) for field in CLAUDE_USAGE_FIELDS}
        return self.span(key, self.runtime + " · " + str(self.model or "model call")[:200], stamp, attrs)

    def pi(self, row, stamp):
        """Compatibility entry point; Pi format logic lives in its adapter."""
        return self.adapter.parse(self, row, stamp, {})

    def kiro(self, row, stamp):
        """Compatibility entry point; Kiro format logic lives in its adapter."""
        return self.adapter.parse(self, row, stamp, {})

    def kiro_usage_span(self, row, payload, stamp):
        return self.adapter.usage_span(self, row, payload, stamp)

    def kiro_apply_parent_outcome(self):
        return self.adapter.apply_parent_outcome(self)

    def adapter_status_metadata(self):
        hook = getattr(self.adapter, "status_metadata", None)
        return hook(self) if hook is not None else {}

    def codex(self, row, stamp):
        """Compatibility entry point; Codex format logic lives in its adapter."""
        return self.adapter.parse(self, row, stamp, {})

    def claude(self, row, stamp, prior_models=None):
        """Compatibility entry point; Claude format logic lives in its adapter."""
        return self.adapter.parse(self, row, stamp, prior_models or {})

    def read_initial_task(self, stream):
        """Read a bounded prefix for identity only; never replay old work or state."""
        if not self.capture_details or self.internal:
            return
        from .local_activity import safe
        from .session_text import task_description
        prefix = stream.read(min(MAX_TASK_PREFIX_BYTES, MAX_READ))
        self.bytes_read_total += len(prefix)
        # The last fragment may be an incomplete record at the byte boundary.
        for line in prefix.split(b'\n')[:-1][:MAX_TASK_PREFIX_RECORDS]:
            try:
                row = loads(line)
                if not isinstance(row, dict):
                    continue
                content = None
                inspect_prefix = getattr(self.adapter, "inspect_prefix", None)
                if inspect_prefix is not None and inspect_prefix(self, row, timestamp(row.get("timestamp"))):
                    return
                content = self.adapter.initial_task(row)
                if isinstance(content, list):
                    content = [block for block in content if isinstance(block, dict)
                               and block.get('type') in {'text','input_text','output_text'}]
                title = task_description(content)
                if title and not self.task_description:
                    self.task_description = safe(title, 500)
                    if not getattr(self.adapter, "continue_prefix_after_task", False):
                        return
            except (ValueError, TypeError, AttributeError, OverflowError, RecursionError):
                continue

    def poll(self):
        # Parsing advances bounded reader state. Retry the same idempotent
        # batch before reading more bytes if either storage transaction failed.
        if self._pending_ingest is not None:
            self.flush_pending_ingest()
        prepare_poll = getattr(self.adapter, "prepare_poll", None)
        if prepare_poll is not None:
            prepare_poll(self)
        stat = self.path.stat()
        self.source_size_bytes = stat.st_size
        self.source_mtime_ns = stat.st_mtime_ns
        self.records_last_poll = self.spans_last_poll = self.logs_last_poll = 0
        file_id = stat.st_dev, stat.st_ino
        if self.file_id != file_id or stat.st_size < self.offset:
            self.offset, self.pending = 0, b""
            self.discarding_line = False
            self.file_id = file_id
            self.last_total = None
            self.modern_usage = False
            self.turn = 'session'
            self.started.clear()
            self.calls.clear()
            self.usage.clear()
            self.tasks.clear()
            self.task_description = ''
            self.tool_inputs.clear()
            self.pending_tools.clear()
            self.pending_request_times.clear()
            self.spawn_calls.clear()
            self.announced_agents.clear()
            self.run_state, self.waiting_for, self.state_since = "unknown", "", 0
            self.request_id, self.request_time_ns = '', 0
            self.outcome_id, self.outcome_time_ns = '', 0
            self.last_record_ns = 0
            reset = getattr(self.adapter, "reset_after_rotation", None)
            if reset is not None:
                reset(self)
        with self.path.open("rb") as stream:
            if self.offset == 0 and stat.st_size > MAX_READ:
                # Recover session identity from the header, then import a bounded tail.
                header = stream.readline(MAX_LINE)
                try:
                    row = loads(header)
                    bootstrap = getattr(self.adapter, "bootstrap_header", None)
                    if bootstrap is not None:
                        bootstrap(self, row, timestamp(row.get("timestamp")))
                except (ValueError, TypeError, AttributeError, OverflowError, RecursionError):
                    self.skipped += 1
                    self.schema_errors += 1
                stream.seek(0)
                self.read_initial_task(stream)
                bounded_tail = getattr(self.adapter, "bounded_tail", None)
                if bounded_tail is not None:
                    bounded_tail(self)
                stream.seek(stat.st_size - MAX_READ)
                # Skip only the partial first record, without allocating an
                # arbitrarily large line from a malformed session file.
                while True:
                    fragment = stream.readline(MAX_LINE)
                    if not fragment or fragment.endswith(b"\n"):
                        break
                self.offset = stream.tell()
            stream.seek(self.offset)
            data = stream.read(MAX_READ)
            self.offset = stream.tell()
        self.bytes_read_total += len(data)
        self.backlog_bytes = max(0, stat.st_size - self.offset)
        if self.discarding_line:
            _, separator, data = data.partition(b"\n")
            self.discarding_line = not bool(separator)
        line_offset=self.offset-len(data)-len(self.pending)
        chunks = (self.pending + data).split(b"\n")
        self.pending = chunks.pop()
        if len(self.pending) > MAX_LINE:
            self.pending = b""
            self.discarding_line = True
            self.skipped += 1
            self.skipped_oversized += 1
        records = [];record_offsets=[]
        for line in chunks:
            line_offset+=len(line)+1
            if len(line) > MAX_LINE:
                self.skipped += 1
                self.skipped_oversized += 1
                continue
            try:
                row = loads(line)
                if isinstance(row, dict):
                    records.append(row)
                    record_offsets.append(line_offset)
                else:
                    self.skipped += 1
                    self.schema_errors += 1
            except (ValueError, UnicodeDecodeError, OverflowError, RecursionError):
                self.skipped += 1
                self.schema_errors += 1
        if getattr(self.adapter, "modern_usage_records", False):
            if any(row.get("type") == "token_usage_record" for row in records):
                self.modern_usage = True
        self.records_last_poll = len(records)
        self.records_total += len(records)
        spans = []
        # Poll-local references only (bounded by MAX_READ), discarded on return.
        # Avoid scanning the entire batch for late streaming usage updates.
        prior_models = {}
        logs = []
        for row,source_offset in zip(records,record_offsets):
            self._current_record_offset=source_offset
            current_turn = self.turn
            historical_outcome = False
            try:
                payload = row.get('payload') or {}
                historical_check = getattr(self.adapter, "historical_outcome", None)
                historical_outcome = bool(historical_check(row, current_turn)) if historical_check else False
                stamp = timestamp(row.get("timestamp"))
                self.last_record_ns = max(self.last_record_ns, stamp)
                new_spans = self.adapter.parse(self, row, stamp, prior_models)
                for span in new_spans:
                    if span:
                        spans.append(span)
                        if "observatory.native_usage" in span["attributes"]:
                            prior_models[(span["trace_id"], span["span_id"])] = span
                activity = self.adapter.activity(self, row, stamp)
                for event in activity:event['attributes'].update(self.native_position())
                logs.extend(activity)
                if historical_outcome:
                    # Keep the historical event and trace, without closing a
                    # newer turn or clearing its pending requests and tools.
                    continue
                self.observe_state(row, stamp)
                if self.capture_details and self.turn in self.tasks and self.turn in self.started:
                    continuation = getattr(self.adapter, "continuation_root", None)
                    if continuation is not None and continuation(row):
                        spans.append(self.root(stamp, "running"))
                if self.run_state in {"completed", "interrupted"}:
                    self.finish_turn()
            except (ValueError, TypeError, KeyError, AttributeError, OverflowError, RecursionError):
                self.skipped += 1
                self.schema_errors += 1
            finally:
                if historical_outcome:
                    self.turn = current_turn
        finish = getattr(self.adapter, "finish_poll", None)
        if finish is not None:
            spans.extend(span for span in finish(self) if span)
        if self.internal:
            from .native_policy import internal_review_attrs, internal_review_events
            for span in spans:
                span['service'] = self.service
                span['resource_attributes']['service.name'] = self.service
                span['attributes']=internal_review_attrs(span['attributes'])
                span['events']=internal_review_events(span.get('events') or [])
                span['links']=[]
                if span['name'] == 'agent.run':
                    span['attributes']['agent.task'] = 'Approval review'
                else:span['name']='Approval review'
            for log in logs:
                log['service'] = self.service
                log['body'] = ''
                log['attributes']=internal_review_attrs(log['attributes'])
        self._new_storage_batch(spans, logs)
        self.flush_pending_ingest()
        state = "running" if time.time() - stat.st_mtime < 35 else "stopped"
        if time.monotonic() - self.last_heartbeat > 10 or state != self.last_state:
            self.store.register_connection({"source_id": self.source_id, "service": self.service,
                "runtime": self.runtime + " local sessions", "integrations": ["local-session-log"],
                "capture_content": self.capture_details, "state": state})
            self.last_heartbeat, self.last_state = time.monotonic(), state
        return len(spans)

    def flush_pending_ingest(self):
        with self._storage_lock:
            spans,logs=self._pending_ingest
            # Directly restored/legacy buffered batches have no current state
            # publication until a real delivery attempt starts here.
            if self._pending_ingest is not self._storage_buffer:
                now = time.monotonic()
                self._storage_counter += 1
                self._storage_batch_id = self._storage_epoch + ':' + str(self._storage_counter)
                self._storage_buffer = self._pending_ingest
                self._storage_started = self._storage_progress_at = now
                self._storage_progress_sequence = 0
            self._storage_attempt_in_flight = True
            self._storage_stage = 'capture_policy'
        try:
            try:
                if spans or logs or self._redaction_pending:
                    self.refresh_capture_policy()
                    self._storage_step('capture_policy', progressed=True)
                if self.internal:
                    from .native_policy import scrub_record
                    spans=[scrub_record(row,'span') for row in spans]
                    logs=[scrub_record(row,'log') for row in logs]
                    with self._storage_lock:
                        self._pending_ingest=(spans,logs)
                        self._storage_buffer=self._pending_ingest
                if self._redaction_pending:
                    self._storage_step('redaction')
                    self.store.redact_internal_source(self.source_id)
                    self._redaction_pending=False
                    self._storage_step('redaction', progressed=True)
                if spans and not self._pending_spans_done:
                    self._storage_step('spans')
                    self.store.ingest(spans,native_retry=self._pending_retry,native_source_path=self.path)
                    self._storage_step('spans', progressed=True, spans_done=True)
                if logs:
                    self._storage_step('logs')
                    self.store.ingest_logs(logs,native_retry=self._pending_retry,native_source_path=self.path)
                    self._storage_step('logs', progressed=True)
                if spans or logs:
                    self._storage_step('capture_policy')
                    self.refresh_capture_policy()
                    self._storage_step('capture_policy', progressed=True)
            except Exception:
                with self._storage_lock:self._pending_retry=True
                raise
            self._storage_step('collector_metadata')
            if logs:
                last = logs[-1]
                self.last_activity = {"time_unix_nano": last["time_unix_nano"],
                                      "domain": last["attributes"].get("agent.activity.domain"),
                                      "action": last["attributes"].get("agent.activity.action"),
                                      "subject": last["attributes"].get("agent.activity.subject"),
                                      "result": last["attributes"].get("agent.activity.result")}
            self.spans_last_poll = len(spans)
            self.logs_last_poll = len(logs)
            self.spans_total += len(spans)
            self.logs_total += len(logs)
            self.last_success_ns = time.time_ns()
            with self._storage_lock:
                self._pending_ingest=None
                self._pending_retry=False
                self._storage_buffer=None
                self._storage_batch_id=None
                self._storage_started=self._storage_progress_at=None
                self._storage_progress_sequence=0
                self._storage_stage='idle'
                self._storage_attempt_in_flight=False
        finally:
            with self._storage_lock:self._storage_attempt_in_flight=False


SPAWN_TOOLS = {"spawn_agent", "Task", "Agent"}
KIRO_METADATA_BYTES = 64 * 1024
KIRO_STOP_STATES = {"end_turn": "completed", "cancelled": "interrupted", "error": "failed"}
KIRO_CREDITS_KEY = "kiro.usage.credits"
KIRO_PERMISSION_WINDOW = 256
KIRO_PERMISSION_RECENT = 10
KIRO_RULES_REFRESH_SECONDS = 10
# Older records omit the option list; these are Kiro's built-in option IDs.
KIRO_OPTION_KINDS = {"accept": "allow_once", "always-accept": "allow_always",
                     "reject": "reject_once", "always-reject": "reject_always",
                     # Some records store the option kind itself as the selection.
                     **{kind: kind for kind in ("allow_once", "allow_always", "reject_once", "reject_always")}}


def kiro_credits(payload):
    """Sum a usage_summary's credit entries; None when none is reported."""
    values = [item.get("usage") for item in payload.get("promptTurnSummaries") or []
              if isinstance(item, dict) and item.get("unit") == "credit"]
    values = [float(value) for value in values
              if type(value) in {int, float} and math.isfinite(value) and value >= 0]
    return round(sum(values), 8) if values else None
def _kiro_layout(path):
    """Return (session directory, worker id) for a Kiro messages or sub-execution file.

    <KIRO_HOME>/sessions/<workspace>/sess_<id>/messages.jsonl is a session;
    <session>/sub-executions/<subSessionId>.jsonl is a worker it delegated to.
    """
    path = Path(path)
    if path.parent.name == "sub-executions":
        return path.parent.parent, _safe_identifier(path.stem)
    return path.parent, None


def _kiro_interaction_key(payload, row):
    call_id = _safe_identifier(payload.get("toolCallId"))
    if not call_id:
        return None
    round_ = payload.get("consentRound")
    return call_id + ":" + str(round_ if type(round_) is int else 0)


def _kiro_mcp_attributes(payload):
    """Kiro records the configured MCP server beside its flattened tool name."""
    name = str(payload.get("toolName") or "")
    meta = ((payload.get("_meta") or {}).get("kiro") or {}) if isinstance(payload.get("_meta"), dict) else {}
    server = meta.get("serverName") if isinstance(meta, dict) else None
    if not isinstance(server, str) or not server.strip() or not name:
        return mcp_attributes(name)
    server = server.strip()[:128]
    prefix = "mcp_" + "_".join(filter(None, "".join(
        char if char.isalnum() else " " for char in server.casefold()).split())) + "_"
    tool = name[len(prefix):] if name.casefold().startswith(prefix) and len(name) > len(prefix) else name
    return {"mcp.server.name": server, "mcp.tool.name": tool[:128], "mcp.method.name": "tools/call"}


def _spawned_agent_id(value):
    """Extract only an exact child identity from a successful spawn result."""
    if isinstance(value, str):
        try:
            value = loads(value)
        except (ValueError, TypeError, OverflowError, RecursionError):
            return None
    if not isinstance(value, dict):
        return None
    if value.get("is_error") or value.get("isError") or value.get("error"):
        return None
    for key in ("agent_id", "agentId", "thread_id"):
        candidate = value.get(key)
        if isinstance(candidate, str) and candidate.strip() and len(candidate) <= 256:
            return candidate.strip()
    return None


def _safe_identifier(value, limit=256):
    if not isinstance(value, str):
        return None
    value = value.strip()
    return value if value and len(value) <= limit else None


_HOST_STATUS_FIELDS = frozenset({
    'source_id', 'runtime', 'service', 'session_id', 'project', 'model',
    'working_directory', 'branch', 'run_state', 'state_since_ns', 'trace_id',
    'run_start_ns', 'request_id', 'request_time_ns', 'outcome_id',
    'outcome_time_ns', 'waiting_for', 'task', 'initial_task', 'capture_content',
    'last_activity', 'last_record_ns', 'process', 'diagnostics', 'bytes_read',
    'agent_id', 'source_namespace', 'native_session_id', 'native_worker_id',
    'parent_agent_id', 'relationship_kind', 'relationship_state',
    'relationship_provenance', 'discovery_state', 'coverage', 'observed_ns',
    'review',
})
_HOST_STATUS_TOP_LEVEL = frozenset({
    'enabled', 'initial_poll_complete', 'sources', 'discovered_sources',
    'omitted_sources', 'read_errors', 'ambiguous_sources', 'last_read_error',
    'skipped_records', 'backlog_bytes', 'schema_errors',
    'skipped_oversized_records', 'capture_content', 'lookback_hours',
    'max_sources', 'opencode_discovered_sessions', 'opencode_omitted_sessions',
    'opencode_diagnostics', 'reader_plugins', 'runtime', 'mcp_configurations',
    'mcp_config_errors', 'mcp_config_updated', 'hierarchy', 'sessions',
    'opencode_read_errors', 'source_coverage', 'source_coverage_overflow',
    'collection_read_errors',
})


class LocalAgents:
    def __init__(self, store, capture_details=True, source_specs: tuple[NativeSourceSpec, ...] | None = None,
                 reader_specs=None):
        self.store = store
        self.evidence = NativeEvidenceFacade(store)
        self.capture_details = capture_details
        self.source_specs = builtin_sources() if source_specs is None else tuple(source_specs)
        if len({spec.id for spec in self.source_specs}) != len(self.source_specs):
            raise ValueError("duplicate native source ID")
        self.stop = threading.Event()
        self.sessions = {}
        self.discovered_sources = 0
        self.omitted_sources = 0
        self.ambiguous_sources = 0
        self.errors = 0
        self.last_error = None
        self.runtime_snapshot = {}
        self.runtime_updated = 0
        self.process_samples = {}
        self.mcp_configurations = []
        self.mcp_config_errors = 0
        self.mcp_config_updated = 0
        self.hierarchy_revision = ""
        self.hierarchy_facts_revision = ""
        self.hierarchy_write_errors = 0
        from .native_readers import builtin_readers
        self.reader_specs = builtin_readers() if reader_specs is None else tuple(reader_specs)
        if len({spec.id for spec in self.reader_specs}) != len(self.reader_specs):
            raise ValueError("duplicate native reader ID")
        if {spec.id for spec in self.reader_specs} & {spec.id for spec in self.source_specs}:
            raise ValueError("native reader and source IDs must be unique")
        self.reader_plugins = {spec.id: spec.build(self.evidence, capture_details) for spec in self.reader_specs}
        # Preserve the existing OpenCode diagnostic surface during migration.
        self.opencode = self.reader_plugins.get("opencode")
        self.initial_poll_complete = False
        self.thread = threading.Thread(target=self.loop, name="observatory-local-agents", daemon=True)

    def record_error(self, error, operation):
        from .findings import _safe_text
        self.errors+=1
        self.last_error={'kind':type(error).__name__,'operation':operation,
            'message':_safe_text(str(error))[:300],'at_ns':time.time_ns()}

    def discover(self):
        cutoff = time.time() - 86400
        self.discovered_sources = 0
        owners = {}
        ambiguous = set()
        pending_sources={path for path,session in self.sessions.items() if session._pending_ingest is not None}
        discovered_pending=set()
        def recent(*, known_owners_only=False):
            for spec in self.source_specs:
                try:
                    root = Path(spec.root()).resolve()
                except (OSError, TypeError, ValueError) as exc:
                    if not known_owners_only:
                        self.record_error(exc, f"Resolving native source {spec.id}")
                    continue
                if not root.is_dir():
                    continue
                # One directory pass; DirEntry reuses directory type metadata.
                # Path.rglob traverses directories separately from matching files
                # and constructs Paths for the entire (mostly old) history.
                pending = [root]
                while pending:
                    if self.stop.is_set():
                        return
                    try:
                        with os.scandir(pending.pop()) as entries:
                            for entry in entries:
                                if self.stop.is_set():
                                    return
                                try:
                                    if entry.is_dir(follow_symlinks=False):
                                        if not spec.accepts_directory(entry.name):
                                            continue
                                        pending.append(entry.path)
                                    elif entry.name.endswith('.jsonl') and entry.is_file():
                                        if not spec.accepts_file(entry.path):
                                            continue
                                        modified = entry.stat().st_mtime
                                        if modified >= cutoff:
                                            path = Path(entry.path)
                                            if known_owners_only:
                                                if path in ambiguous or owners.get(path) != spec.id:
                                                    continue
                                            else:
                                                self.discovered_sources += 1
                                                previous = owners.get(path)
                                                if previous is not None and previous != spec.id:
                                                    if path not in ambiguous:
                                                        self.record_error(ValueError("multiple adapters claim one source"),
                                                                          "Resolving native source ownership")
                                                    ambiguous.add(path)
                                                    continue
                                                owners[path] = spec.id
                                            if path in pending_sources:discovered_pending.add(path)
                                            yield modified, path, spec
                                except OSError as exc:
                                    self.record_error(exc,'Inspecting native source entry')
                    except OSError as exc:
                        self.record_error(exc,'Discovering native source directory')
        selected = heapq.nlargest(MAX_SOURCES, recent(), key=lambda row: row[0])
        if ambiguous:
            # Filtering after a fixed-size heap can hide older valid sources.
            # Pay for a second scan only when ownership conflicts actually exist.
            selected = heapq.nlargest(MAX_SOURCES, recent(known_owners_only=True),
                                      key=lambda row: row[0])
        self.ambiguous_sources = len(ambiguous)
        selected=[row for row in selected if row[1] not in pending_sources and row[1] not in ambiguous][:max(0,MAX_SOURCES-len(pending_sources))]
        keys={path for _,path,_ in selected}|pending_sources
        keys.difference_update(ambiguous - pending_sources)
        self.omitted_sources=max(0,self.discovered_sources-len(selected)-len(discovered_pending))
        self.sessions = {path: session for path, session in self.sessions.items() if path in keys}
        for _, path, spec in selected:
            if path not in self.sessions:
                try:
                    adapter = spec.build_adapter()
                    self.sessions[path] = Session(self.evidence, path, spec.runtime,
                                                  capture_details=self.capture_details, adapter=adapter)
                except (TypeError, ValueError) as exc:
                    self.record_error(exc, f"Starting native source {spec.id}")

    def poll(self):
        accepted = 0
        self.link_adapter_sessions()
        for session in list(self.sessions.values()):
            if self.stop.is_set():
                break
            try:
                accepted += session.poll()
            except OSError as exc:
                self.record_error(exc,'Reading native source')
        self.link_adapter_sessions()
        return accepted

    def link_adapter_sessions(self):
        """Run each registered adapter's peer reducer once per poll."""
        groups = {}
        for session in self.sessions.values():
            groups.setdefault(type(session.adapter), []).append(session)
        for peers in groups.values():
            try:
                link = getattr(peers[0].adapter, "link_sessions", None)
                if link is not None:
                    link(peers)
            except Exception as exc:
                self.record_error(exc, f"Linking native adapter {type(peers[0].adapter).__name__}")

    def adapter_status_metadata(self, session):
        """A broken adapter's optional status fields cannot hide other sessions."""
        try:
            fields = session.adapter_status_metadata()
            if not isinstance(fields, dict):
                raise TypeError("native adapter status metadata must be a mapping")
            if (fields.keys() & _HOST_STATUS_FIELDS or any(
                    type(key) is not str or not key or len(key) > 64 for key in fields)):
                raise ValueError("native adapter status metadata conflicts with host fields")
            return fields
        except Exception as exc:
            self.record_error(exc, f"Inspecting native adapter {type(session.adapter).__name__}")
            return {}

    def loop(self):
        discovered = 0
        while not self.stop.is_set():
            try:
                if time.monotonic() - discovered >= 10:
                    self.discover()
                    project_directories = {session.working_directory for session in self.sessions.values()
                                           if session.working_directory}
                    from .local_runtime import project_directories as runtime_project_directories
                    project_directories.update(runtime_project_directories({"gemini", "cursor", "opencode"}))
                    config_errors = [0]
                    def note_config_error():
                        config_errors[0] += 1
                    self.mcp_configurations = discover_mcp_configurations(
                        project_directories=project_directories,
                        on_error=note_config_error)
                    for spec in self.reader_specs:
                        if spec.mcp_client:
                            self.reader_plugins[spec.id].set_mcp_servers(
                                item["server"] for item in self.mcp_configurations
                                if item["client"] == spec.mcp_client)
                    self.mcp_config_errors = config_errors[0]
                    self.mcp_config_updated = time.monotonic()
                    discovered = time.monotonic()
                self.poll()
                for spec in self.reader_specs:
                    if self.stop.is_set():
                        break
                    try:
                        self.reader_plugins[spec.id].poll()
                    except Exception as exc:
                        self.record_error(exc, f"Polling native reader {spec.id}")
                    self.initial_poll_complete = True
            except Exception as exc:
                self.record_error(exc,'Polling native sources')
            self.stop.wait(1)

    def start(self):
        self.thread.start()
        return self

    def hierarchy_nodes(self, sessions=None):
        """Collect observed and announced nodes, deduplicated by canonical identity."""
        sessions = list(self.sessions.values()) if sessions is None else list(sessions)
        observed = {session.agent_id: session.hierarchy() for session in sessions}
        announced = {}
        for session in sessions:
            for child_id, item in session.announced_agents.items():
                if child_id not in observed:
                    announced[child_id] = dict(item)
        nodes = [*observed.values(), *announced.values()]
        return nodes, announced

    def adapter_status_extensions(self, sessions):
        """Collect active adapter status fields without letting one hide another."""
        groups = {}
        for session in sessions:
            groups.setdefault(type(session.adapter), []).append(session)
        extensions = {}
        for peers in groups.values():
            try:
                hook = getattr(peers[0].adapter, 'status_extension', None)
                if hook is None:
                    continue
                fields = hook(self.capture_details, self.record_error)
                if (not isinstance(fields, dict) or fields.keys() & _HOST_STATUS_TOP_LEVEL
                        or fields.keys() & extensions.keys() or any(
                            type(key) is not str or not key or len(key) > 64 for key in fields)):
                    raise ValueError('native adapter status extension conflicts with host fields')
                if len(json.dumps(fields, allow_nan=False).encode('utf-8')) > 64 * 1024:
                    raise ValueError('native adapter status extension exceeds 64 KiB')
                extensions.update(fields)
            except Exception as exc:
                self.record_error(exc, f'Inspecting native adapter {type(peers[0].adapter).__name__}')
        # Compatibility: existing health clients expect the field even when
        # the Kiro adapter has no active source.
        extensions.setdefault('kiro_folders', [])
        return extensions

    def status(self):
        sessions = list(self.sessions.values())
        internal=(self.store.internal_source_ids([session.source_id for session in sessions])
                  if hasattr(self.store,'internal_source_ids') else set())
        for session in sessions:
            if session.source_id in internal:session.adopt_internal_policy()
        if time.monotonic() - self.runtime_updated >= 5:
            from .local_runtime import snapshot
            self.runtime_snapshot = snapshot({session.path: session.source_id for session in sessions})
            now_ns = time.time_ns()
            samples = self.runtime_snapshot.get('sessions', {})
            for source, sample in samples.items():
                previous = self.process_samples.get(source, {})
                same_process = previous.get('identity') == sample['identity']
                progressed = (sample.get("progress_attributed", True) and same_process and
                              (sample['cpu_seconds'] > previous.get('cpu_seconds', 0) or
                               sample['io_chars'] > previous.get('io_chars', 0)))
                sample['observed_since_ns'] = previous.get('observed_since_ns', now_ns) if same_process else now_ns
                sample['last_progress_ns'] = (now_ns if progressed else previous.get('last_progress_ns', 0) if same_process else 0) if sample.get('progress_attributed', True) else 0
                sample['observed_at_ns'] = now_ns
            self.process_samples = samples
            self.runtime_updated = time.monotonic()
        observed = [{"source_id": session.source_id, "runtime": session.runtime,
                     "service": session.service, "session_id": session.session_id,
                     "project": session.project, "model": session.model,
                     "working_directory": session.working_directory, "branch": session.branch,
                     "run_state": session.run_state, "state_since_ns": session.state_since,
                     "trace_id": identity(session.source_id, session.turn) if session.turn != 'session' else '',
                     "run_start_ns": session.started.get(session.turn) or None,
                     "request_id": session.request_id, "request_time_ns": session.request_time_ns,
                     "outcome_id": session.outcome_id, "outcome_time_ns": session.outcome_time_ns,
                     "waiting_for": session.waiting_for,
                     "task": "Approval review" if session.internal else (session.task_description or session.tasks.get(session.turn) or "")[:500],
                     "initial_task": session.task_description if session.capture_details and not session.internal else "",
                     "capture_content": session.capture_details,
                     "last_activity": session.last_activity,
                     "last_record_ns": session.last_record_ns,
                     "process": self.process_samples.get(session.source_id, {}),
                     "diagnostics": session.diagnostics(),
                     **self.adapter_status_metadata(session),
                     **session.hierarchy(),
                     "bytes_read": session.offset} for session in sessions]
        reader_observed = []
        reader_diagnostics = {}
        reader_counts = {}
        reader_failed = set()
        for spec in self.reader_specs:
            reader = self.reader_plugins[spec.id]
            try:
                rows = reader.status_sessions()
                if not isinstance(rows, list) or not all(isinstance(row, dict) for row in rows):
                    raise TypeError("reader status_sessions must return a list of records")
                # A reader may publish one display row per session while all
                # rows come from a different, shared evidence source (OpenCode
                # is one example). Keep that source distinct for scoped reads.
                evidence_source = getattr(reader, 'source_id', None)
                if isinstance(evidence_source, str) and evidence_source:
                    rows = [dict(row, evidence_source_id=evidence_source) for row in rows]
                reader_observed.extend(rows)
                reader_counts[spec.id] = len(rows)
                reader_diagnostics[spec.id] = reader.diagnostics()
            except Exception as exc:
                reader_failed.add(spec.id)
                self.record_error(exc, f"Inspecting native reader {spec.id}")
        observed.extend(reader_observed)
        source_coverage = [{"label": "Native JSONL", "unit": "files",
                            "active": len(sessions), "discovered": self.discovered_sources,
                            "omitted": self.omitted_sources, "limit": MAX_SOURCES,
                            "state": "partial" if self.omitted_sources or self.ambiguous_sources else "ready"}]
        # Reserve one of the 24 display slots for a process-adapter group
        # appended by the HTTP host after native collection.
        for spec in self.reader_specs[:22]:
            details = {}
            if spec.coverage and spec.id not in reader_failed:
                try:
                    details = spec.coverage(self.reader_plugins[spec.id])
                    if (not isinstance(details, dict) or
                            not details.keys() <= {"discovered", "omitted", "limit"} or any(
                            type(value) is not int or value < 0 or value > 1_000_000_000
                            for value in details.values())):
                        raise ValueError("native reader coverage is invalid")
                except Exception as exc:
                    reader_failed.add(spec.id)
                    self.record_error(exc, f"Inspecting native reader coverage {spec.id}")
                    details = {}
            source_coverage.append({"label": spec.label or spec.id, "unit": "sessions",
                                    "active": reader_counts.get(spec.id),
                                    "omitted": details.get("omitted"),
                                    "limit": details.get("limit"),
                                    "state": "unavailable" if spec.id in reader_failed else
                                             "partial" if details.get("omitted") is None or details.get("omitted") else "ready"})
        collection_read_errors = self.errors + sum(
            value for diagnostics in reader_diagnostics.values()
            if isinstance(diagnostics, dict)
            for value in (diagnostics.get("read_errors"),)
            if type(value) is int and 0 <= value <= 1_000_000_000)
        nodes, announced = self.hierarchy_nodes(sessions)
        observed_ids = {item.get("agent_id") for item in observed}
        nodes.extend(item for item in reader_observed
                     if item.get("agent_id") and item.get("agent_id") not in
                     {node.get("agent_id") for node in nodes})
        facts = tuple(sorted(
            (node.get("agent_id"), node.get("parent_agent_id"), node.get("relationship_kind"),
             node.get("relationship_state"), node.get("relationship_provenance"),
             node.get("discovery_state"), node.get("coverage"), bool(node.get("review")))
            for node in nodes))
        facts_revision = identity("hierarchy", *facts) if facts else ""
        if facts_revision != self.hierarchy_facts_revision:
            upsert = getattr(self.store, "upsert_agent_nodes", None)
            if upsert:
                try:
                    upsert(nodes)
                except (ValueError, TypeError, OSError):
                    self.hierarchy_write_errors += 1
                else:
                    self.hierarchy_facts_revision = facts_revision
            else:
                self.hierarchy_facts_revision = facts_revision
        persisted = getattr(self.store, "hierarchy_revision", None)
        if persisted:
            self.hierarchy_revision = persisted()
        elif not self.hierarchy_revision:
            self.hierarchy_revision = facts_revision
        for child_id, node in announced.items():
            if child_id in observed_ids:
                continue
            observed.append({
                **node, "source_id": node.get("source_id") or "announced:" + child_id,
                "id": node.get("source_id") or "announced:" + child_id,
                "session_id": node.get("native_worker_id") or node.get("native_session_id"),
                "service": node.get("runtime", "").casefold().replace(" ", "-"),
                "project": "", "model": "", "working_directory": "", "branch": "",
                "run_state": "announced", "state_since_ns": node.get("observed_ns", 0),
                "waiting_for": "", "task": "Spawn recorded", "capture_content": self.capture_details,
                "last_activity": {}, "last_record_ns": node.get("observed_ns", 0),
                "diagnostics": {}, "bytes_read": 0,
            })
        node_ids = {item.get("agent_id") for item in nodes}
        missing_parents = {item.get("parent_agent_id") for item in observed
                           if item.get("parent_agent_id") and
                           item.get("parent_agent_id") not in node_ids}
        context_lookup = getattr(self.store, "agent_node_contexts", None)
        if missing_parents and context_lookup:
            try:
                contexts = context_lookup(missing_parents)
            except (ValueError, TypeError, OSError):
                contexts = {}
                self.hierarchy_write_errors += 1
            for item in observed:
                context = contexts.get(item.get("parent_agent_id"))
                if context:
                    item["parent_context"] = context
        extensions = self.adapter_status_extensions(sessions)
        return {"enabled": True, "initial_poll_complete": self.initial_poll_complete, "sources": len(sessions), "discovered_sources": self.discovered_sources,
                "omitted_sources": self.omitted_sources, "read_errors": self.errors,
                "collection_read_errors": collection_read_errors,
                "ambiguous_sources": self.ambiguous_sources,
                "last_read_error": self.last_error,
                "skipped_records": sum(session.skipped for session in sessions),
                "backlog_bytes": sum(session.backlog_bytes for session in sessions),
                "schema_errors": sum(session.schema_errors for session in sessions),
                "skipped_oversized_records": sum(session.skipped_oversized for session in sessions),
                "capture_content": self.capture_details, "lookback_hours": 24, "max_sources": MAX_SOURCES,
                "opencode_discovered_sessions": getattr(self.opencode, "discovered_sessions", 0),
                "opencode_omitted_sessions": getattr(self.opencode, "omitted_sessions", 0),
                "opencode_diagnostics": reader_diagnostics.get("opencode", {}),
                "reader_plugins": reader_diagnostics,
                "source_coverage": source_coverage,
                "source_coverage_overflow": max(0, len(self.reader_specs) - 22),
                **extensions,
                "runtime": self.runtime_snapshot,
                "mcp_configurations": list(self.mcp_configurations),
                "mcp_config_errors": self.mcp_config_errors,
                "mcp_config_updated": self.mcp_config_updated,
                "hierarchy": {"revision": self.hierarchy_revision or facts_revision,
                              "nodes": len(nodes), "announced": len(announced),
                              "coverage": ("partial" if any(
                                  node.get("discovery_state") == "announced" or
                                  node.get("coverage") in {"partial", "unknown"}
                                  for node in nodes) else "complete"),
                              "write_errors": self.hierarchy_write_errors},
                "sessions": observed,
                "opencode_read_errors": getattr(self.opencode, "errors", 0)}

    def close(self):
        self.stop.set()
        # Ctrl-C can arrive while the main thread is joining this worker.
        # The stop flag is already set, so consume interrupts during the
        # bounded shutdown wait rather than leaking a traceback on exit.
        deadline = time.monotonic() + 15
        while self.thread.is_alive() and time.monotonic() < deadline:
            try:
                self.thread.join(timeout=min(1, max(0, deadline - time.monotonic())))
            except KeyboardInterrupt:
                continue
