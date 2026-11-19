"""Kiro session format callbacks for the trusted in-process collector."""
from __future__ import annotations

import math
import re
import time
from pathlib import Path

from ..agent_hierarchy import canonical_agent_id, provenance
from ..local_activity import emit, mcp_attributes, plaintext, safe
from ..local_state import RecentMap
from ..native_json import loads
from ..native_sources import kiro_home
from ..session_text import remember_task


SOURCE_PRIORITY = 3


def source_spec():
    from ..native_sources import (KIRO_SKIPPED_DIRECTORIES, NativeSourceSpec,
                                  kiro_source)

    return NativeSourceSpec(
        "kiro", "Kiro", lambda: kiro_home() / "sessions", kiro_source,
        lambda name: name not in KIRO_SKIPPED_DIRECTORIES,
        adapter_factory=KiroAdapter)


class KiroAdapter:
    agent_namespace = "kiro-sessions"

    def __init__(self):
        self._workspace_folders = []
        self._workspace_cache = {}
        self._workspace_checked = 0.0

    def status_extension(self, capture_details, on_error):
        """Publish bounded workspace permission metadata for active Kiro sources."""
        from ..local_agents import KIRO_RULES_REFRESH_SECONDS
        if time.monotonic() - self._workspace_checked >= KIRO_RULES_REFRESH_SECONDS:
            self._workspace_checked = time.monotonic()
            from ..kiro_permissions import workspace_permissions
            try:
                self._workspace_folders = workspace_permissions(kiro_home(), self._workspace_cache)
            except OSError as exc:
                on_error(exc, "Reading Kiro workspace permissions")
        folders = (list(self._workspace_folders) if capture_details else
                   [{**item, "folder": Path(item["folder"]).name if item.get("folder") else None}
                    for item in self._workspace_folders])
        return {"kiro_folders": folders}

    def link_sessions(self, sessions):
        """Resolve workers from exact spawn evidence in this adapter's peers."""
        spawners = {}
        for session in sessions:
            for item in list(session.announced_agents.values()):
                child = item.get("native_worker_id")
                if child:
                    spawners[(session.kiro_session_dir, child)] = session
        for session in sessions:
            if not session.native_worker_id:
                continue
            spawner = spawners.get((session.kiro_session_dir, session.native_worker_id))
            if spawner is None or spawner is session:
                continue
            if session.parent_agent_id != spawner.agent_id:
                session.parent_agent_id = spawner.agent_id
                session.relationship_state = "confirmed"
                session.relationship_provenance = provenance(
                    "kiro-session", "sub_agent_start.subSessionId in the "
                    + ("worker" if spawner.native_worker_id else "session") + " transcript")
            outcome = spawner.kiro_child_outcomes.get(session.native_worker_id)
            if outcome and session.run_state not in {"completed", "interrupted", "failed", "incomplete"}:
                session.kiro_parent_outcome = outcome
    def reset_after_rotation(self, session):
        session.kiro_interactions.clear()
        session.kiro_turn_from_prompt = session.kiro_turn_open = False
        session.kiro_queued_prompt = session.kiro_released_prompt = None
        self.reset_permissions(session)

    def bounded_tail(self, session):
        # Earlier approval records were not read from the bounded tail.
        session.kiro_history_partial = True

    def finish_poll(self, session):
        spans = self.apply_parent_outcome(session) if session.native_worker_id and session.kiro_parent_outcome else []
        if (session.kiro_title and not session.task_description and not session.native_worker_id
                and session.capture_details and not session.internal):
            # A bounded tail can skip the prompt; the session title is a fallback.
            session.task_description = safe(session.kiro_title, 500)
        return spans

    def status_metadata(self, session):
        return {"permissions": self.permission_summary(session)}

    def initialize(self, session):
        """Identity comes from the session directory; a worker from sub-executions/."""
        from ..local_agents import _kiro_layout, MAX_RECENT_CALLS, _safe_identifier
        session.kiro_session_dir, worker = _kiro_layout(session.path)
        session.kiro_metadata_mtime_ns = None
        session.kiro_title = ""
        session.kiro_turn_from_prompt = False
        session.kiro_turn_open = False
        session.kiro_queued_prompt = None
        session.kiro_released_prompt = None
        session.kiro_interactions = RecentMap(MAX_RECENT_CALLS)
        session.kiro_child_outcomes = RecentMap(MAX_RECENT_CALLS)
        session.kiro_parent_outcome = None
        self.reset_permissions(session)
        session.kiro_autopilot = None
        session.kiro_agent = ""
        session.kiro_workspace = ""
        session.kiro_rules = []
        session.kiro_rules_cache = {}
        session.kiro_rules_checked = 0.0
        native = _safe_identifier(session.kiro_session_dir.name) or session.session_id
        session.native_session_id = native
        if worker:
            session.session_id = worker
            session.native_worker_id = worker
            session.agent_id = canonical_agent_id(session.runtime, session.agent_namespace, worker)
            # Nested workers share this directory, so it names the session tree,
            # not the direct parent. Only the spawning sub_agent_start does.
            session.relationship_kind = "delegated"
            session.relationship_state = "unresolved"
            session.relationship_provenance = provenance(
                "kiro-session", "sub-executions worker; spawning sub_agent_start not yet read")
        else:
            session.session_id = native
            session.agent_id = canonical_agent_id(session.runtime, session.agent_namespace, native)
            session.relationship_provenance = provenance(
                "kiro-session", "top-level session directory; no parent metadata")
        self.refresh_metadata(session)

    def reset_permissions(self, session):
        from ..local_agents import KIRO_PERMISSION_WINDOW
        session.kiro_permissions = RecentMap(KIRO_PERMISSION_WINDOW)
        session.kiro_permission_counts = {}
        session.kiro_history_partial = False

    def permission(self, session, row, payload, stamp):
        """Record each tool-approval request and the answer Kiro wrote for it."""
        from ..local_agents import _kiro_interaction_key, _safe_identifier, KIRO_OPTION_KINDS
        event = payload.get("type")
        key = _kiro_interaction_key(payload, row)
        if event == "tool_call":
            call_id = _safe_identifier(payload.get("toolCallId"))
            for entry in session.kiro_permissions.values():
                if entry["call_id"] == call_id and not entry["tool"]:
                    entry["tool"] = str(payload.get("toolName") or "")[:128]
            meta = (payload.get("_meta") or {}).get("kiro") if isinstance(payload.get("_meta"), dict) else None
            denial = meta.get("policyDenial") if isinstance(meta, dict) else None
            if isinstance(denial, dict) and call_id:
                from ..local_activity import safe
                resource = denial.get("resource") if isinstance(denial.get("resource"), str) else ""
                rule = denial.get("matchedRule") if isinstance(denial.get("matchedRule"), dict) else {}
                session.kiro_permissions["policy:" + call_id] = {
                    "call_id": call_id, "requested_ns": stamp, "resolved_ns": stamp, "kind": "policy",
                    "tool": str(payload.get("toolName") or "")[:128],
                    "capability": str(denial.get("capability") or rule.get("capability") or "")[:64],
                    "subject": safe(resource, 300) if session.capture_details and not session.internal else "",
                    "rule_source": str(denial.get("source") or denial.get("scope") or "")[:64],
                    "options": {}, "decision": "policy_denied"}
                session.kiro_permission_counts["policy_denied"] = session.kiro_permission_counts.get("policy_denied", 0) + 1
            return
        if not key:
            return
        if event == "pending_interaction":
            meta = (payload.get("_meta") or {}).get("kiro") if isinstance(payload.get("_meta"), dict) else None
            meta = meta if isinstance(meta, dict) else {}
            consent = meta.get("consent") if isinstance(meta.get("consent"), dict) else {}
            options = {str(option.get("optionId")): str(option.get("kind") or "")
                       for option in payload.get("options") or [] if isinstance(option, dict)}
            subject = payload.get("question") if isinstance(payload.get("question"), str) else ""
            from ..local_activity import safe
            session.kiro_permissions[key] = {
                "call_id": _safe_identifier(payload.get("toolCallId")),
                "requested_ns": stamp, "resolved_ns": 0,
                "kind": str(payload.get("interactionType") or "")[:64],
                "tool": str(meta.get("toolId") or "")[:128],
                "capability": str(consent.get("capability") or "")[:64],
                "subject": safe(subject, 300) if session.capture_details and not session.internal else "",
                "options": options, "decision": "pending"}
            session.kiro_permission_counts["requested"] = session.kiro_permission_counts.get("requested", 0) + 1
        elif event == "interaction_resolved":
            entry = session.kiro_permissions.get(key)
            if payload.get("outcome") == "cancelled":
                decision = "cancelled"
            else:
                option = str(payload.get("selectedOption") or "")
                decision = ((entry or {}).get("options") or {}).get(option) or KIRO_OPTION_KINDS.get(option, "answered")
            if entry is not None:
                if entry["decision"] != "pending":
                    return
                entry.update(decision=decision, resolved_ns=stamp)
            session.kiro_permission_counts[decision] = session.kiro_permission_counts.get(decision, 0) + 1

    def permission_summary(self, session):
        from ..local_agents import KIRO_PERMISSION_RECENT
        recent = sorted(session.kiro_permissions.values(), key=lambda item: item["requested_ns"], reverse=True)
        return {"autopilot": session.kiro_autopilot, "counts": dict(session.kiro_permission_counts),
                "complete": not session.kiro_history_partial and not session.kiro_permissions.evictions,
                "recent": [{key: item[key] for key in ("requested_ns", "resolved_ns", "kind", "tool",
                                                         "capability", "subject", "decision", "rule_source")
                            if key in item}
                           for item in recent[:KIRO_PERMISSION_RECENT]],
                "agent": session.kiro_agent, "rules": session.kiro_rules}

    def refresh_metadata(self, session):
        """Read bounded session.json fields: title, workspace and model only."""
        from ..local_agents import KIRO_METADATA_BYTES
        path = session.kiro_session_dir / "session.json"
        try:
            stat = path.stat()
            if stat.st_mtime_ns == session.kiro_metadata_mtime_ns:
                return
            session.kiro_metadata_mtime_ns = stat.st_mtime_ns
            if stat.st_size > KIRO_METADATA_BYTES:
                session.schema_errors += 1
                return
            data = loads(path.read_bytes())
        except FileNotFoundError:
            return
        except (OSError, ValueError, TypeError, OverflowError, RecursionError):
            session.schema_errors += 1
            return
        if not isinstance(data, dict):
            session.schema_errors += 1
            return
        if isinstance(data.get("title"), str):
            session.kiro_title = data["title"].strip()[:500]
        paths = data.get("workspacePaths") or data.get("rootPaths")
        cwd = next((item for item in paths if isinstance(item, str) and item.strip()), None) \
            if isinstance(paths, list) else None
        if cwd:
            session.project = Path(cwd).name
            session.working_directory = str(cwd) if session.capture_details else ""
        if isinstance(data.get("modelId"), str) and data["modelId"].strip():
            session.model = data["modelId"].strip()[:200]
        if isinstance(data.get("autopilot"), bool):
            session.kiro_autopilot = data["autopilot"]
        if isinstance(data.get("agentMode"), str):
            session.kiro_agent = data["agentMode"].strip()[:128]
        if cwd:
            session.kiro_workspace = str(cwd)

    def refresh_rules(self, session, force=False):
        """Re-read standing rule files at most every 10 s, and only when they change."""
        from ..local_agents import KIRO_RULES_REFRESH_SECONDS
        if not force and time.monotonic() - session.kiro_rules_checked < KIRO_RULES_REFRESH_SECONDS:
            return
        session.kiro_rules_checked = time.monotonic()
        from ..kiro_permissions import display_path, read_rules, standing_sources
        home = kiro_home()
        workspace_key = session.kiro_session_dir.parent.name if session.kiro_session_dir.parent.parent.name == "sessions" else ""
        result = []
        for label, path in standing_sources(home, workspace_key, session.kiro_agent, session.kiro_workspace or None):
            try:
                stat = path.stat()
                signature = (stat.st_mtime_ns, stat.st_size)
            except OSError:
                signature = None
            cached = session.kiro_rules_cache.get(path)
            if cached is None or cached[0] != signature:
                cached = (signature, read_rules(path))
                session.kiro_rules_cache[path] = cached
            entry = {"source": label, "path": display_path(path, Path.home()), **cached[1]}
            if label == "Workspace":
                entry["folder"] = session.kiro_workspace if session.capture_details else Path(session.kiro_workspace).name
            result.append(entry)
        session.kiro_rules = result

    def prepare_poll(self, session) -> None:
        self.refresh_metadata(session)
        self.refresh_rules(session)

    def initial_task(self, row: dict) -> object:
        payload = row.get("payload") or {}
        if (isinstance(payload, dict) and payload.get("type") == "user"
                and payload.get("source") != "steer"):
            return payload.get("content")
        return None

    def observe_state(self, session, row: dict, stamp: int) -> tuple[str, object] | None:
        from ..local_agents import KIRO_STOP_STATES, _kiro_interaction_key
        payload = row.get("payload") or {}
        if not isinstance(payload, dict):
            return None
        event = payload.get("type")
        state = session.run_state
        request_key = None
        if event == "turn_start" or (event == "user" and payload.get("source") != "steer"
                                     and session.kiro_queued_prompt is not row):
            state = "running"
            session.waiting_for = ""
            session.kiro_interactions.clear()
        elif event == "pending_interaction":
            key = _kiro_interaction_key(payload, row)
            if key:
                label = "Approval" if payload.get("interactionType") == "tool_approval" else "User input"
                session.kiro_interactions[key] = (label, stamp)
        elif event == "interaction_resolved":
            key = _kiro_interaction_key(payload, row)
            if key:
                session.kiro_interactions.pop(key, None)
            if state == "waiting" and not session.kiro_interactions:
                state = "running"
                session.waiting_for = ""
        elif event == "turn_end":
            state = KIRO_STOP_STATES.get(str(payload.get("stopReason") or ""), "incomplete")
            session.waiting_for = ""
            session.kiro_interactions.clear()
        elif event in {"assistant", "tool_call", "tool_result", "sub_agent_start"} and state == "unknown":
            state = "running"
        if session.kiro_interactions and state not in {"completed", "interrupted", "failed", "incomplete"}:
            key, (label, since) = next(iter(session.kiro_interactions.items()))
            state, session.waiting_for, request_key = "waiting", label, key
            session.pending_request_times[key] = since
        return state, request_key

    def parse(self, session, row, stamp, prior_models):
        from ..local_agents import KIRO_STOP_STATES, _safe_identifier, _kiro_mcp_attributes
        payload = row.get("payload") or {}
        if not isinstance(payload, dict):
            return []
        event = payload.get("type")
        result = []
        if event in {"pending_interaction", "interaction_resolved", "tool_call"}:
            session.kiro_permission(row, payload, stamp)
        if session.native_worker_id and session.turn == "session" and stamp:
            # Worker transcripts have no prompt or turn records; the file is one run.
            session.turn = session.native_worker_id
            session.started[session.turn] = stamp
            result.append(session.root(stamp, "running"))
        if event == "user" and payload.get("source") != "steer":
            if session.kiro_turn_open:
                # Kiro queues a prompt sent mid-turn; the current turn keeps
                # writing its tools, usage and turn_end until it finishes.
                session.kiro_queued_prompt = row
                return result
            # The prompt opens the turn so its text belongs to the new trace;
            # Kiro writes the matching turn_start immediately afterwards.
            session.turn = str(row.get("id") or stamp or session.turn)
            session.started[session.turn] = stamp
            session.kiro_turn_from_prompt = True
            session.kiro_turn_open = True
            if isinstance(payload.get("content"), str):
                remember_task(session, payload["content"])
            return [session.root(stamp, "running")]
        if event == "turn_start":
            if not session.kiro_turn_from_prompt:
                queued, session.kiro_queued_prompt = session.kiro_queued_prompt, None
                if queued is not None:
                    session.turn = str(queued.get("id") or stamp)
                    session.kiro_released_prompt = queued
                    content = (queued.get("payload") or {}).get("content")
                    if isinstance(content, str):
                        remember_task(session, content)
                else:
                    session.turn = str(payload.get("turnId") or payload.get("executionId") or row.get("id") or stamp or session.turn)
                session.started[session.turn] = stamp
                result.append(session.root(stamp, "running"))
            session.kiro_turn_from_prompt = False
            session.kiro_turn_open = True
            return result
        session.kiro_turn_from_prompt = False
        if event == "turn_end":
            session.kiro_turn_open = False
            state = KIRO_STOP_STATES.get(str(payload.get("stopReason") or ""), "incomplete")
            result.append(session.root(stamp, state))
        elif event == "tool_call":
            call_id = _safe_identifier(payload.get("toolCallId"))
            if call_id:
                session.calls[call_id] = (str(payload.get("toolName") or "Tool")[:256], stamp, session.turn,
                                       _kiro_mcp_attributes(payload), str(payload.get("status") or ""))
        elif event == "tool_result":
            call_id = _safe_identifier(payload.get("toolCallId"))
            pending = session.calls.pop(call_id, None) if call_id else None
            name, start, turn, mcp, status = pending or ("Tool", stamp, session.turn, {}, "")
            attrs = {"gen_ai.operation.name": "execute_tool", "tool.name": name, **mcp}
            elapsed = payload.get("durationMs")
            if type(elapsed) in {int, float} and math.isfinite(elapsed) and elapsed >= 0:
                start = max(0, stamp - int(elapsed * 1e6))
            else:
                # Kiro writes the call and its result together after execution.
                attrs["observatory.duration_unknown"] = True
            previous, session.turn = session.turn, turn
            span = session.span(("tool", call_id or row.get("id") or stamp),
                             (mcp["mcp.server.name"] + " / " + mcp["mcp.tool.name"]) if mcp else name,
                             stamp, attrs, start=start,
                             error=payload.get("success") is False or status in {"failed", "denied"})
            session.turn = previous
            if span:
                result.append(span)
        elif event == "sub_agent_start":
            session.announce(payload.get("subSessionId"), stamp, "sub_agent_start.subSessionId")
        elif event == "usage_summary":
            span = self.usage_span(session, row, payload, stamp)
            if span:
                result.append(span)
        elif event == "sub_agent_complete":
            child = _safe_identifier(payload.get("subSessionId"))
            if child and stamp:
                session.kiro_child_outcomes[child] = ("end_turn" if payload.get("status") == "success" else "error", stamp)
        return result

    def usage_span(self, session, row, payload, stamp):
        """One model-usage span per turn: Kiro reports credits, never tokens."""
        from ..local_agents import KIRO_CREDITS_KEY, kiro_credits
        credits = kiro_credits(payload)
        if credits is None:
            return None
        attrs = {"gen_ai.operation.name": "chat", KIRO_CREDITS_KEY: credits,
                 "kiro.usage.status": str(payload.get("status") or "")[:32]}
        if session.model:
            attrs["gen_ai.request.model"] = str(session.model)[:200]
        elapsed = payload.get("elapsedTime")
        if type(elapsed) in {int, float} and math.isfinite(elapsed) and elapsed >= 0:
            start = max(0, stamp - int(elapsed * 1e6))
        else:
            start, attrs["observatory.duration_unknown"] = stamp, True
        return session.span(("kiro-usage", row.get("id") or session.turn), "Kiro · " + str(session.model or "model call")[:200],
                         stamp, attrs, start=start)

    def apply_parent_outcome(self, session):
        """A worker transcript has no end record; its parent records the outcome."""
        outcome, session.kiro_parent_outcome = session.kiro_parent_outcome, None
        if not outcome or session.run_state in {"completed", "interrupted", "failed", "incomplete"}:
            return []
        stop_reason, stamp = outcome
        if stamp < session.last_record_ns:
            return []
        row = {"payload": {"type": "turn_end", "stopReason": stop_reason}}
        spans = self.parse(session, row, stamp, {})
        session.observe_state(row, stamp)
        return spans

    def activity(self, session, row: dict, stamp: int) -> list[dict]:
        return _kiro_activity(session, row, stamp)


def timestamp_of(row):
    from ..local_agents import timestamp
    return timestamp(row.get("timestamp"))


KIRO_EXIT_CODE = re.compile(r"\n\s*Exit Code: (-?\d{1,10})\s*$")
KIRO_FILE_ACTIONS = {"fs_write": "write", "write_file": "write", "str_replace": "edit",
                     "fs_append": "edit", "delete_file": "delete"}


def _kiro_call(payload, turn):
    """Retain only the bounded call fields that classify its later result."""
    from ..local_agents import _kiro_mcp_attributes
    args = payload.get("args") if isinstance(payload.get("args"), dict) else {}
    keep = {key: args[key] for key in ("command", "cwd", "run_in_background", "path", "targetFile", "url", "query")
            if isinstance(args.get(key), (str, bool))}
    for key in ("command", "cwd", "path", "targetFile", "url", "query"):
        if isinstance(keep.get(key), str):
            keep[key] = keep[key][:8000]
    path = payload.get("filePath") if isinstance(payload.get("filePath"), str) else keep.get("path") or keep.get("targetFile")
    mcp = _kiro_mcp_attributes(payload)
    return {"name": str(payload.get("toolName") or "Tool")[:256], "kind": str(payload.get("kind") or ""),
            "status": str(payload.get("status") or ""), "args": keep, "path": path, "mcp": mcp,
            "mcp_args": safe(args, 2000) if mcp else "", "turn": turn}


def _kiro_activity(session, row, stamp):
    """Extract bounded activity from Kiro's append-only messages.jsonl records."""
    payload = row.get("payload") or {}
    if not isinstance(payload, dict):
        return []
    queued = session.kiro_queued_prompt is row
    released, session.kiro_released_prompt = session.kiro_released_prompt, None
    if isinstance(row.get("id"), str):
        # Kiro record IDs are stable per record; use them as occurrence identity.
        row = {**row, "uuid": row["id"]}
    event = payload.get("type")
    capture = session.capture_details and not session.internal
    out = []
    if released is not None and event == "turn_start":
        # A prompt queued during the previous turn is recorded with its own turn.
        out.extend(_kiro_activity(session, released, timestamp_of(released) or stamp))
    if event == "user" and queued:
        return []
    if event == "user":
        text = plaintext(payload.get("content")) if capture else ""
        steer = payload.get("source") == "steer"
        out.append(emit(session, row, stamp, "message", "user",
                        text.splitlines()[0][:300] if text else "user message", text, "observed",
                        {"message.role": "user", **({"message.source": "steer"} if steer else {})}))
    elif event == "assistant":
        operation = payload.get("operationType")
        if operation == "Say":
            text = plaintext(payload.get("content")) if capture else ""
            out.append(emit(session, row, stamp, "message", "assistant",
                            text.splitlines()[0][:300] if text else "assistant message", text,
                            "observed", {"message.role": "assistant"}))
        elif operation == "Reasoning":
            out.append(emit(session, row, stamp, "agent", "processing", "Model processing"))
        elif operation == "Summary":
            out.append(emit(session, row, stamp, "agent", "compaction", "Context compacted"))
    elif event == "tool_call":
        call_id = str(payload.get("toolCallId") or "")
        if call_id:
            session.tool_inputs[call_id] = _kiro_call(payload, session.turn)
    elif event == "tool_result":
        call_id = str(payload.get("toolCallId") or "")
        call = session.tool_inputs.pop(call_id, None) if call_id else None
        if not isinstance(call, dict):
            call = {"name": "Tool", "kind": "", "status": "", "args": {}, "path": None, "mcp": {},
                    "mcp_args": "", "turn": session.turn}
        out.extend(_kiro_result(session, row, stamp, payload, call, call_id))
    elif event == "pending_interaction":
        meta = ((payload.get("_meta") or {}).get("kiro") or {}) if isinstance(payload.get("_meta"), dict) else {}
        tool = meta.get("toolId") if isinstance(meta, dict) and isinstance(meta.get("toolId"), str) else ""
        approval = payload.get("interactionType") == "tool_approval"
        out.append(emit(session, row, stamp, "agent", "approval_request" if approval else "request_user_input",
                        "Approval requested" if approval else "Reply requested",
                        payload.get("question") if capture else "", "waiting",
                        {"tool.name": tool[:128]} if tool else None))
    elif event == "interaction_resolved":
        option = str(payload.get("selectedOption") or payload.get("outcome") or "")
        out.append(emit(session, row, stamp, "agent", "interaction_resolved",
                        "Approval cancelled" if payload.get("outcome") == "cancelled" else "Approval answered",
                        "", "observed", {"kiro.selected_option": option[:64]} if option else None))
    elif event in {"turn_start", "turn_end"}:
        from ..local_agents import KIRO_STOP_STATES
        action = "started" if event == "turn_start" else KIRO_STOP_STATES.get(
            str(payload.get("stopReason") or ""), "incomplete")
        out.append(emit(session, row, stamp, "agent", action, session.project or session.runtime,
                        {"stop_reason": payload["stopReason"]} if isinstance(payload.get("stopReason"), str) else "",
                        "error" if action == "failed" else action))
    elif event == "usage_summary":
        # Kiro reports credits and elapsed time, not token counts.
        usage = {"elapsed_ms": payload.get("elapsedTime"), "status": payload.get("status"),
                 "credits": [item.get("usage") for item in payload.get("promptTurnSummaries") or []
                             if isinstance(item, dict) and item.get("unit") == "credit"]}
        out.append(emit(session, row, stamp, "model", "usage", session.model or "Model not reported",
                        usage, "reported", {"gen_ai.request.model": str(session.model or "unknown")}))
    elif event == "session_metadata" and payload.get("key") == "displayError":
        value = payload.get("value") if isinstance(payload.get("value"), dict) else {}
        kind = str(value.get("errorType") or "Error")[:128]
        out.append(emit(session, row, stamp, "agent", "error", kind,
                        value.get("message") if capture else "", "error"))
    elif event == "sub_agent_start":
        name = str(payload.get("subAgentName") or "Subagent")[:128]
        out.append(emit(session, row, stamp, "agent", "delegated", name,
                        payload.get("prompt") if capture else "", "started",
                        {"agent.child.native_id": str(payload.get("subSessionId") or "")[:256]}))
    elif event == "sub_agent_complete":
        failed = payload.get("status") not in {"success", None}
        out.append(emit(session, row, stamp, "agent", "delegation_complete", "Subagent finished",
                        (payload.get("errorMessage") or payload.get("response")) if capture else "",
                        "error" if failed else "success",
                        {"agent.child.native_id": str(payload.get("subSessionId") or "")[:256]}))
    elif event == "tombstone" and payload.get("kind") == "summarization":
        out.append(emit(session, row, stamp, "agent", "compaction", "Context compacted"))
    return [entry for entry in out if entry]


def _kiro_result(session, row, stamp, payload, call, call_id):
    name, args, mcp, turn = call["name"], call["args"], call["mcp"], call["turn"]
    content = payload.get("content") if isinstance(payload.get("content"), str) else ""
    denied = call["status"] == "denied"
    failed = payload.get("success") is False or call["status"] == "failed"
    result = "denied" if denied else "error" if failed else "success"
    capture = session.capture_details and not session.internal
    extra = {"tool.name": name, "observatory.item_type": call["kind"] or "tool"}
    if type(payload.get("durationMs")) in {int, float}:
        extra["duration_ms"] = payload["durationMs"]
    if mcp:
        return [emit(session, row, stamp, "mcp", mcp["mcp.tool.name"], mcp["mcp.server.name"],
                     {"arguments": call["mcp_args"], "result": content} if capture else "",
                     result, {**extra, **mcp}, call_id=call_id or None, turn=turn)]
    command = args.get("command")
    if isinstance(command, str) and (call["kind"] == "execute" or name in {"execute_bash", "run_bash"}):
        from ..operation_facts import evidence_detail
        background = args.get("run_in_background") is True
        match = None if denied or background else KIRO_EXIT_CODE.search(content)
        code = int(match.group(1)) if match else None
        if match:
            extra["process.exit.code"] = code
            if code and result == "success":
                result = "error"
        elif background and result == "success":
            # A background launch is not a completed command.
            result = "unknown"
        cwd = args.get("cwd") or session.working_directory or None
        output = safe(content, 2000)
        evidence = dict(version=1, inputs=[("native.input", {"command": command, "cwd": cwd})],
                        commands=[("native.command", command)], cwd=cwd, tool=name, server=None,
                        exits=[("kiro.result_exit_code", code)] if match else [],
                        outputs=[("native.result", "output", output)],
                        output_clipped=len(content) > 2000, is_error=failed or denied)
        extra["observatory.operation.schema"] = 1
        return [emit(session, row, stamp, "shell", "execute", command if capture else "Shell command",
                     evidence_detail(evidence) if capture else "", result, extra,
                     call_id=call_id or None, turn=turn)]
    path = call["path"]
    if isinstance(path, str) and path and call["kind"] in {"edit", "delete", "read"}:
        action = KIRO_FILE_ACTIONS.get(name) or {"delete": "delete", "read": "read"}.get(call["kind"], "edit")
        return [emit(session, row, stamp, "file", action, path if capture else Path(path).name,
                     content, result, extra, call_id=call_id or None, turn=turn)]
    url = args.get("url")
    if isinstance(url, str) and url and call["kind"] == "fetch":
        return [emit(session, row, stamp, "browser", "fetch", url if capture else name,
                     content, result, extra, call_id=call_id or None, turn=turn)]
    return [emit(session, row, stamp, "tool", "call", name, content, result, extra,
                 call_id=call_id or None, turn=turn)]
