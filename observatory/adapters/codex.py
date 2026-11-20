"""Codex rollout callbacks for the trusted in-process collector."""
from __future__ import annotations

import json
import os
import shlex
from pathlib import Path

from ..agent_hierarchy import canonical_agent_id, provenance
from ..local_activity import duration, emit, mcp_attributes, mcp_failed, plaintext, safe
from ..native_json import loads
from ..session_text import remember_task


SOURCE_PRIORITY = 0


def mcp_config_contributions():
    from ..mcp_config_registry import McpConfigContribution

    return (McpConfigContribution(
        "codex", "Codex", (("mcp_servers",),),
        lambda: (Path(os.environ.get("CODEX_HOME", Path.home() / ".codex")) / "config.toml",),
        lambda _root: (), priority=0),)


def source_spec():
    from ..native_sources import NativeSourceSpec

    return NativeSourceSpec(
        "codex", "Codex",
        lambda: Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex"))) / "sessions",
        adapter_factory=CodexAdapter)


class CodexAdapter:
    agent_namespace = "codex-rollout"
    continue_prefix_after_task = True
    modern_usage_records = True

    def continuation_root(self, row):
        payload = row.get("payload") or {}
        return (isinstance(payload, dict) and isinstance(payload.get("item"), dict)
                and payload["item"].get("type") == "UserMessage")

    def historical_outcome(self, row: dict, current_turn: str) -> bool:
        payload = row.get("payload") or {}
        if not isinstance(payload, dict) or row.get("type") != "event_msg":
            return False
        terminal = (payload.get("type") in {"task_complete", "task_aborted", "turn_aborted"}
                    or (payload.get("type") == "item_completed"
                        and isinstance(payload.get("item"), dict)
                        and payload["item"].get("type") == "AgentMessage"
                        and payload["item"].get("phase") == "final_answer"))
        return bool(terminal and payload.get("turn_id") and current_turn != "session"
                    and str(payload["turn_id"]) != current_turn)

    def initial_task(self, row: dict) -> object:
        payload = row.get("payload") or {}
        if not isinstance(payload, dict):
            return None
        item = payload.get("item") or {}
        if row.get("type") == "event_msg" and isinstance(item, dict) and item.get("type") == "UserMessage":
            return item.get("content")
        if row.get("type") == "event_msg" and payload.get("type") == "user_message":
            return payload.get("message")
        if row.get("type") == "response_item" and payload.get("type") == "message" and payload.get("role") == "user":
            return payload.get("content")
        return None

    def inspect_prefix(self, session, row: dict, stamp: int) -> bool:
        payload = row.get("payload") or {}
        if (row.get("type") == "turn_context" and isinstance(payload, dict)
                and payload.get("model") == "codex-auto-review"):
            self.parse(session, row, stamp, {})
            return True
        return False

    def bootstrap_header(self, session, row: dict, stamp: int) -> None:
        self.parse(session, row, stamp, {})

    def observe_state(self, session, row: dict, stamp: int) -> tuple[str, object] | None:
        payload = row.get("payload") or {}
        if not isinstance(payload, dict):
            return None
        state = session.run_state
        event = payload.get("type")
        request_key = None
        if row.get("type") == "event_msg":
            if event in {"task_started", "task_complete", "task_aborted", "turn_aborted"}:
                state = {"task_started": "running", "task_complete": "completed",
                         "task_aborted": "interrupted", "turn_aborted": "interrupted"}[event]
                session.pending_tools.clear()
            elif event in {"approval_request", "request_user_input"}:
                state = "waiting"
                session.waiting_for = "Approval" if event == "approval_request" else "User input"
                request_key = (payload.get("request_id") or payload.get("call_id") or payload.get("id")
                               or row.get("ordinal") or row.get("uuid") or row.get("timestamp"))
            elif event == "item_completed":
                item = payload.get("item") or {}
                if isinstance(item, dict):
                    if item.get("type") == "UserMessage":
                        state = "running"
                        session.pending_tools.clear()
                    elif item.get("type") == "AgentMessage" and item.get("phase") == "final_answer":
                        state = "completed"
                        session.pending_tools.clear()
        if row.get("type") == "response_item":
            key = str(payload.get("call_id") or payload.get("id"))
            if event in {"function_call", "custom_tool_call"}:
                name = str(payload.get("name") or "Tool")
                session.pending_tools[key] = name
                if name.rsplit(".", 1)[-1] in {"request_user_input", "AskUserQuestion"}:
                    session.pending_request_times[key] = stamp
                state = "running"
            elif event in {"function_call_output", "custom_tool_call_output"}:
                session.pending_tools.pop(key, None)
                if state not in {"completed", "interrupted", "failed", "error", "incomplete"}:
                    state = "running"
                    session.waiting_for = ""
        return state, request_key

    def parse(self, session, row, stamp, prior_models):
        from ..local_agents import _safe_identifier, _spawned_agent_id, timestamp
        kind, payload = row.get("type"), row.get("payload") or {}
        if not isinstance(payload, dict):
            return []
        if kind == "session_meta":
            thread_id = _safe_identifier(payload.get("id")) or session.session_id
            session.session_id = thread_id
            session.native_session_id = _safe_identifier(payload.get("session_id")) or thread_id
            session.native_root_session_id = session.native_session_id
            session.native_worker_id = thread_id if thread_id != session.native_session_id else None
            session.agent_id = canonical_agent_id(session.runtime, session.agent_namespace, thread_id)
            source = payload.get("source")
            spawn = {}
            if isinstance(source, dict):
                nested = source.get("subagent")
                if isinstance(nested, dict):
                    spawn = nested.get("thread_spawn") if isinstance(nested.get("thread_spawn"), dict) else {}
            parent_native = (_safe_identifier(payload.get("parent_thread_id")) or
                             _safe_identifier(spawn.get("parent_thread_id")))
            if parent_native:
                session.parent_agent_id = canonical_agent_id(
                    session.runtime, session.agent_namespace, parent_native)
                session.relationship_kind = "delegated"
                session.relationship_state = "confirmed"
                session.relationship_provenance = provenance(
                    "codex-session-meta", "payload.parent_thread_id/source.subagent.thread_spawn",
                    timestamp(payload.get("timestamp")))
            elif _safe_identifier(payload.get("forked_from_id")):
                fork = _safe_identifier(payload.get("forked_from_id"))
                session.parent_agent_id = canonical_agent_id(
                    session.runtime, session.agent_namespace, fork)
                session.relationship_kind = "forked"
                session.relationship_state = "confirmed"
                session.relationship_provenance = provenance(
                    "codex-session-meta", "payload.forked_from_id is a fork, not delegation",
                    timestamp(payload.get("timestamp")))
            elif (isinstance(source, dict) and "subagent" in source) or \
                    str(payload.get("thread_source") or "").casefold() == "subagent":
                session.relationship_kind = "delegated"
                session.relationship_state = "unresolved"
                session.relationship_provenance = provenance(
                    "codex-session-meta", "subagent source without parent_thread_id",
                    timestamp(payload.get("timestamp")))
            elif isinstance(source, str) and source.casefold() in {"cli", "vscode", "exec", "mcp"}:
                session.relationship_kind = "unknown"
                session.relationship_state = "confirmed"
                session.relationship_provenance = provenance(
                    "codex-session-meta", "top-level source=" + source.casefold(),
                    timestamp(payload.get("timestamp")))
            else:
                session.relationship_kind = "unknown"
                session.relationship_state = "unresolved"
                session.relationship_provenance = provenance(
                    "codex-session-meta", "no parent or root marker",
                    timestamp(payload.get("timestamp")))
            session.project = Path(payload["cwd"]).name if payload.get("cwd") else session.project
        elif kind == "turn_context":
            session.turn = str(payload.get("turn_id") or session.turn)
            session.model = payload.get("model") or session.model
            if session.model == "codex-auto-review" and not session.internal:
                session.adopt_internal_policy()
                session._redaction_pending = True
            session.project = Path(payload["cwd"]).name if payload.get("cwd") else session.project
        elif kind == "token_usage_record":
            session.modern_usage = True
            session.turn = str(payload.get("turn_id") or session.turn)
            key = payload.get("response_id") or row.get("ordinal") or row.get("timestamp")
            return [session.model_span(("model", key), payload.get("usage"), stamp)]
        elif kind == "response_item":
            event = payload.get("type")
            key = str(payload.get("call_id") or payload.get("id"))
            if event in {"function_call", "custom_tool_call"}:
                name = str(payload.get("name") or "Tool")
                if name == "spawn_agent":
                    session.spawn_calls[key] = (name, stamp, session.turn)
                if mcp_attributes(name):
                    session.calls[key] = (name, stamp, session.turn)
            elif event in {"function_call_output", "custom_tool_call_output"} and key in session.spawn_calls:
                session.spawn_calls.pop(key, None)
                child = _spawned_agent_id(payload.get("output"))
                if child:
                    session.announce(child, stamp, "function_call_output spawn_agent agent_id")
                if key not in session.calls:
                    return []
            if event in {"function_call_output", "custom_tool_call_output"} and key in session.calls:
                name, start, turn = session.calls[key]
                previous, session.turn = session.turn, turn
                span = session.span(("tool", key), name, stamp,
                    {"gen_ai.operation.name": "execute_tool", "tool.name": name, **mcp_attributes(name)},
                    start=start, error=mcp_failed(payload.get("output")))
                session.turn = previous
                return [span]
        elif kind == "event_msg":
            event = payload.get("type")
            session.turn = str(payload.get("turn_id") or session.turn)
            if event == "task_started":
                session.started[session.turn] = timestamp(payload.get("started_at")) or stamp
                return [session.root(stamp, "running")]
            if event in {"task_complete", "task_aborted", "turn_aborted"}:
                return [session.root(stamp, "completed" if event == "task_complete" else "interrupted")]
            if event == "token_count" and not session.modern_usage:
                info = payload.get("info") or {}
                total = info.get("total_token_usage")
                if not isinstance(total, dict) or total == session.last_total:
                    return []
                usage = info.get("last_token_usage")
                session.last_total = total
                return [session.model_span(("usage", session.turn, json.dumps(total, sort_keys=True)), usage, stamp)]
            if event == "item_completed":
                item = payload.get("item") or {}
                tool_type = item.get("type")
                if tool_type == "AgentMessage" and item.get("phase") == "final_answer":
                    return [session.root(stamp, "completed")]
                names = {"CommandExecution": "Shell command", "FileChange": "File changes",
                         "McpToolCall": "MCP tool", "Extension": "Extension tool"}
                if tool_type in names:
                    attrs = {"gen_ai.operation.name": "execute_tool", "tool.name": names[tool_type]}
                    if tool_type == "McpToolCall":
                        attrs["tool.name"] = str(item.get("tool") or "MCP tool")[:128]
                        attrs["mcp.tool.name"] = attrs["tool.name"]
                        attrs["mcp.server.name"] = str(item.get("server") or "")[:128]
                        attrs["mcp.method.name"] = "tools/call"
                    code = item.get("exit_code")
                    if isinstance(code, int):
                        attrs["process.exit.code"] = code
                    error = item.get("status") in {"failed", "error"} or isinstance(code, int) and code != 0 or (tool_type == "McpToolCall" and mcp_failed(item.get("result")))
                    end = timestamp(payload.get("completed_at_ms")) or stamp
                    start = timestamp(payload.get("started_at_ms"))
                    elapsed = duration(item)
                    if not start and elapsed is not None:
                        start = max(0, end-int(elapsed*1e6))
                    if not start:
                        attrs["observatory.duration_unknown"] = True
                    name = (attrs["mcp.server.name"] + " / " + attrs["tool.name"]) if tool_type == "McpToolCall" else str(attrs["tool.name"])
                    return [session.span(("tool", item.get("id") or row.get("ordinal") or stamp),
                        name, end, attrs, start=start, error=error)]
        return []

    def activity(self, session, row: dict, stamp: int) -> list[dict]:
        payload = row.get("payload") or {}
        if not isinstance(payload, dict):
            return []
        kind = row.get("type")
        event = payload.get("type")
        out = []
        if kind == "token_usage_record":
            usage = payload.get("usage") or {}
            out.append(emit(session, row, stamp, "model", "usage", session.model or "Model not reported", usage,
                            "reported", {"gen_ai.request.model": str(session.model or "unknown")}))
        elif kind == "response_item" and event in {"function_call", "custom_tool_call"}:
            name = str(payload.get("name") or "Tool")
            args = payload.get("arguments") or payload.get("input") or ""
            try:
                parsed = loads(args) if isinstance(args, str) else args
            except (ValueError, TypeError, OverflowError, RecursionError):
                parsed = {}
            # Results only need the tool name, not another retained argument payload.
            session.tool_inputs[str(payload.get("call_id") or payload.get("id"))] = (name, "", session.turn)
            subject = name
            domain = "tool"
            mcp = mcp_attributes(name)
            if mcp:
                domain, subject = "mcp", mcp["mcp.server.name"]
            elif isinstance(parsed, dict):
                command = parsed.get("cmd") or parsed.get("command")
                if command:
                    domain, subject = "shell", str(command)
                elif parsed.get("url"):
                    domain, subject = "browser", str(parsed["url"])
                elif parsed.get("path"):
                    domain, subject = "file", str(parsed["path"])
            out.append(emit(session, row, stamp, domain, "call", subject if session.capture_details else name,
                            {"arguments": parsed or args} if mcp else args, "started", {"tool.name": name, **mcp},
                            call_id=payload.get("call_id") or payload.get("id") if mcp else None))
        elif kind == "response_item" and event in {"function_call_output", "custom_tool_call_output"}:
            key = str(payload.get("call_id") or payload.get("id"))
            pending = session.calls.get(key)
            fallback = (pending[0], "", pending[2]) if pending else ("Tool", "", session.turn)
            name, _, call_turn = session.tool_inputs.pop(key, fallback)
            mcp = mcp_attributes(name)
            output = payload.get("output") or ""
            pending = session.calls.pop(key, None) if mcp else None
            out.append(emit(session, row, stamp, "mcp" if mcp else "tool", "result", name,
                            {"result": output} if mcp else output,
                            "error" if mcp_failed(output) else "success" if mcp else "observed", mcp,
                            call_id=key if mcp else None, turn=pending[2] if pending else call_turn))
        elif kind == 'event_msg' and event in {'approval_request', 'request_user_input'}:
            # Request identities and generic labels remain available in metadata-only mode.
            out.append(emit(session, row, stamp, 'agent', event,
                            'Approval requested' if event == 'approval_request' else 'Reply requested',
                            result='waiting'))
        elif kind == "event_msg" and event in {"task_started", "task_complete", "task_aborted", "turn_aborted"}:
            action = "started" if event == "task_started" else "completed" if event == "task_complete" else "interrupted"
            detail = {"event": event, **{key: payload[key] for key in
                      ("duration_ms", "time_to_first_token_ms") if key in payload}}
            if isinstance(payload.get("reason"), str):
                # Bound the field before encoding so a long reason cannot clip the
                # surrounding JSON and make otherwise valid evidence unreadable.
                detail["reason"] = safe(payload["reason"], 1000)
            out.append(emit(session, row, stamp, "agent", action, session.project or session.runtime, detail, action))
        elif kind == "turn_context":
            # Only selected settings, never the system instructions or environment.
            settings = {key: payload[key] for key in ("model", "effort", "approval_policy", "sandbox_policy") if key in payload}
            out.append(emit(session, row, stamp, "context", "settings", session.model or "Agent settings", settings))
        elif kind == "event_msg" and event == "item_completed":
            item = payload.get("item") or {}
            if not isinstance(item, dict):
                return []
            item_type = item.get("type")
            extra = {"observatory.item_type": item_type}
            elapsed = duration(item)
            if elapsed is not None:
                extra["duration_ms"] = elapsed
            failed = str(item.get("status", "")).lower() in {"failed", "error", "declined", "denied"}
            result = "error" if failed else "success"
            if item_type == "CommandExecution":
                command = item.get("command") or []
                command = shlex.join(map(str, command)) if isinstance(command, list) else str(command)
                code = item.get("exit_code")
                if 'exit_code' in item:extra['process.exit.code']=code
                if type(code) is int:
                    extra["process.exit.code"] = code
                    if code:
                        result = "error"
                from ..operation_facts import evidence_detail
                output=item.get('aggregated_output','')
                captured_output=safe(output,2000) if isinstance(output,str) else output
                evidence=dict(version=1,inputs=[('native.input',{'command':command,'cwd':item.get('cwd')})],
                    commands=[('native.command',command)],cwd=item.get('cwd'),tool='CommandExecution',server=None,
                    exits=[('native.exit_code',code)] if 'exit_code' in item else [],
                    outputs=[('native.aggregated_output','output',captured_output)],
                    output_clipped=isinstance(output,str) and len(output)>2000)
                extra['observatory.operation.schema']=1
                out.append(emit(session,row,stamp,'shell','execute',command if session.capture_details else 'Shell command',
                    evidence_detail(evidence),result,extra))
            elif item_type == "FileChange":
                if not failed and str(item.get("status", "")).lower() not in {"completed","success","ok"}:result="unknown"
                changes = item.get("changes") or {}
                if isinstance(changes, dict):
                    for path, change in list(changes.items())[:60]:
                        operation = change.get("type") or change.get("kind") or "change" if isinstance(change, dict) else "change"
                        if isinstance(operation, dict):
                            operation = next(iter(operation), "change")
                        operation = str(operation)
                        out.append(emit(session, row, stamp, "file", operation[:40], path if session.capture_details else Path(path).name,
                                        change, result, extra))
            elif item_type == "Extension":
                action = item.get("action") or {}
                name = str(action.get("type") or item.get("kind") or "activity") if isinstance(action, dict) else str(item.get("kind") or "activity")
                subject = action.get("url") or item.get("query") or name if isinstance(action, dict) else name
                out.append(emit(session, row, stamp, "browser", name, subject if session.capture_details else name,
                    {"action": action, "query": item.get("query"), "results": item.get("results")}, result, extra))
            elif item_type == "McpToolCall":
                name = str(item.get("tool") or "MCP tool")
                extra["mcp.server.name"] = str(item.get("server") or "")[:128]
                extra["mcp.tool.name"] = name
                extra["mcp.method.name"] = "tools/call"
                details = {"arguments": item.get("arguments"), "result": item.get("result")}
                if isinstance(item.get("result"),dict) and item['result'].get('isError') is True:
                    result = "error"
                out.append(emit(session, row, stamp, "mcp", name, str(item.get("server") or "MCP"), details, result, extra))
            elif item_type in {"UserMessage", "AgentMessage"}:
                text = "" if session.internal else plaintext(item.get("content"))
                role = "user" if item_type == "UserMessage" else "assistant"
                if role == "user" and text and session.capture_details:
                    remember_task(session, item.get("content"))
                out.append(emit(session, row, stamp, "message", role,
                    "Approval review" if session.internal else text.splitlines()[0][:300] if text and session.capture_details else role + " message", text,
                    "observed", {"message.role": role, "message.phase": item.get("phase") or ""}))
            elif item_type in {"Reasoning", "ContextCompaction"}:
                out.append(emit(session, row, stamp, "agent", "processing" if item_type == "Reasoning" else "compaction",
                                "Model processing" if item_type == "Reasoning" else "Context compacted"))
        return [entry for entry in out if entry]
