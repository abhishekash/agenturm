"""Claude Code format callbacks for the trusted in-process collector."""
from __future__ import annotations

from pathlib import Path

from ..agent_hierarchy import canonical_agent_id, provenance
from ..local_activity import emit, mcp_attributes, plaintext, safe
from ..session_text import remember_task


class ClaudeAdapter:
    def initial_task(self, row: dict) -> object:
        if row.get("type") != "user":
            return None
        message = row.get("message") or {}
        return message.get("content") if isinstance(message, dict) else None

    def activity(self, session, row: dict, stamp: int) -> list[dict]:
        message = row.get("message") or {}
        if not isinstance(message, dict):
            return []
        content = message.get("content") or []
        blocks = content if isinstance(content, list) else []
        out = []
        if row.get("type") == "assistant" and isinstance(message.get("usage"), dict):
            out.append(emit(session, row, stamp, "model", "usage", session.model or "Model not reported", message["usage"], "reported"))
        text = plaintext(content)
        if text and row.get("type") in {"assistant", "user"}:
            role = row["type"]
            if role == "user" and session.capture_details:
                remember_task(session, content)
            out.append(emit(session, row, stamp, "message", role, text.splitlines()[0][:300] if session.capture_details else role+" message", text))
        for block in blocks:
            if not isinstance(block, dict):
                continue
            if block.get("type") == "tool_use":
                name, args = str(block.get("name") or "Tool"), block.get("input") or {}
                domain = "shell" if name.lower() in {"bash", "shell", "terminal"} else "file" if name.lower() in {"read", "write", "edit", "glob", "grep", "multiedit"} else "browser" if name.lower() in {"webfetch", "websearch"} else "tool"
                subject = args.get("command") or args.get("file_path") or args.get("path") or args.get("url") or args.get("query") or name if isinstance(args, dict) else name
                mcp = mcp_attributes(name)
                if mcp:
                    domain, subject = "mcp", mcp["mcp.server.name"]
                session.tool_inputs[str(block.get("id"))] = (name, safe(subject, 500) if session.capture_details else "", session.turn)
                out.append(emit(session, row, stamp, domain, name, subject if session.capture_details else name,
                    {"arguments": args} if mcp else args, "started", {"tool.name": name, **mcp},
                    call_id=block.get("id") if mcp else None))
            elif block.get("type") == "tool_result":
                name, subject, call_turn = session.tool_inputs.pop(str(block.get("tool_use_id")), ("Tool", "Tool result", session.turn))
                mcp = mcp_attributes(name)
                pending = session.calls.pop(block.get("tool_use_id"), None)
                output = block.get("content") or ""
                out.append(emit(session, row, stamp, "mcp" if mcp else "tool", name+" result", subject if session.capture_details else name,
                    {"result": output} if mcp else output, "error" if block.get("is_error") else "success", mcp,
                    call_id=block.get("tool_use_id") if mcp else None, turn=pending[2] if pending else call_turn))
            elif block.get("type") == "thinking":
                out.append(emit(session, row, stamp, "agent", "processing", "Model processing"))
        if row.get("type") == "system":
            out.append(emit(session, row, stamp, "context", str(row.get("subtype") or "event"), "Session event",
                            {key:row[key] for key in ("durationMs", "messageCount", "isMeta") if key in row}))
        return [entry for entry in out if entry]

    def observe_state(self, session, row: dict, stamp: int) -> tuple[str, object] | None:
        from ..local_agents import claude_local_command
        message = row.get("message") or {}
        if not isinstance(message, dict):
            return None
        if row.get("type") == "user" and claude_local_command(row):
            return None
        state = session.run_state
        blocks = message.get("content")
        blocks = blocks if isinstance(blocks, list) else []
        if row.get("type") == "user" and not any(
                isinstance(block, dict) and block.get("type") == "tool_result" for block in blocks):
            state = "running"
            session.pending_tools.clear()
        for block in blocks:
            if not isinstance(block, dict):
                continue
            if block.get("type") == "tool_use":
                key = str(block.get("id"))
                name = str(block.get("name") or "Tool")
                session.pending_tools[key] = name
                if name.rsplit(".", 1)[-1] in {"request_user_input", "AskUserQuestion"}:
                    session.pending_request_times[key] = stamp
                state = "running"
            elif block.get("type") == "tool_result":
                session.pending_tools.pop(str(block.get("tool_use_id")), None)
                state = "running"
                session.waiting_for = ""
        if message.get("stop_reason") == "end_turn":
            state = "completed"
            session.pending_tools.clear()
        return state, None

    def parse(self, session, row, stamp, prior_models):
        from ..local_agents import (CLAUDE_USAGE_FIELDS, SPAWN_TOOLS, _safe_identifier,
                                    claude_local_command, _spawned_agent_id, identity, count)
        row_session = _safe_identifier(row.get("sessionId"))
        row_agent = _safe_identifier(row.get("agentId"))
        if session.claude_agent_id:
            # Shared-session workers keep their own evidence identity even when
            # every record repeats the root sessionId.
            session.session_id = row_agent or session.claude_agent_id
            session.native_session_id = row_session or session.claude_parent_session_id or session.session_id
            session.native_worker_id = session.claude_agent_id
        elif row_session:
            session.session_id = row_session
            session.native_session_id = row_session
            if not session.native_root_session_id:
                session.native_root_session_id = row_session
            session.agent_id = canonical_agent_id(session.runtime, session.agent_namespace, row_session)
            if not row_agent and not session.parent_agent_id:
                session.relationship_kind = "unknown"
                session.relationship_state = "unresolved"
                session.relationship_provenance = provenance(
                    "claude-code", "main transcript; no parent metadata")
        session.project = Path(row["cwd"]).name if row.get("cwd") else session.project
        message = row.get("message") or {}
        if not isinstance(message, dict):
            return []
        if row.get('type')=='user' and claude_local_command(row):
            return []
        content = message.get("content") or []
        blocks = content if isinstance(content, list) else []
        result = []
        if row.get("type") == "user":
            tools = [block for block in blocks if isinstance(block, dict) and block.get("type") == "tool_result"]
            if not tools:
                session.turn = str(row.get("uuid") or stamp)
                session.started[session.turn] = stamp
                result.append(session.root(stamp, "running"))
            for block in tools:
                key = block.get("tool_use_id")
                if key in session.spawn_calls:
                    session.spawn_calls.pop(key, None)
                    if not block.get("is_error"):
                        child = _spawned_agent_id(row.get("toolUseResult"))
                        if child:
                            session.announce(child, stamp,
                                          "tool_result.toolUseResult.agentId (Claude Task/Agent)")
                name, start, turn = session.calls.get(key, ("Claude tool", stamp, session.turn))
                previous, session.turn = session.turn, turn
                result.append(session.span(("tool", key), name, stamp,
                    {"gen_ai.operation.name": "execute_tool", "tool.name": name, **mcp_attributes(name)},
                    start=start, error=bool(block.get("is_error"))))
                session.turn = previous
        elif row.get("type") == "assistant":
            session.model = message.get("model") or session.model
            key = message.get("id") or row.get("uuid") or stamp
            usage = message.get("usage")
            if isinstance(usage, dict):
                previous = session.usage.get(key)
                if previous is None:
                    previous = {}
                    # Evicting maxima must not lower a persisted model span if
                    # an old streaming update arrives after the cache window.
                    if session.usage.evictions or session.usage.dropped_oversized:
                        span_id = identity(session.source_id, ("model", key), size=16)
                        span = (prior_models or {}).get((identity(session.source_id, session.turn), span_id))
                        if span:
                            previous = span["attributes"].get("observatory.native_usage", previous)
                        elif hasattr(session.store, "native_model_usage"):
                            previous = session.store.native_model_usage(
                                session.source_id, identity(session.source_id, session.turn), span_id)
                for field in CLAUDE_USAGE_FIELDS:
                    previous[field] = max(count(previous.get(field)), count(usage.get(field)))
                session.usage[key] = previous
                result.append(session.model_span(("model", key), previous, stamp, claude=True))
            for block in blocks:
                if isinstance(block, dict) and block.get("type") == "tool_use":
                    name = str(block.get("name") or "Claude tool")
                    session.calls[block.get("id")] = (name, stamp, session.turn)
                    if name in SPAWN_TOOLS:
                        session.spawn_calls[block.get("id")] = (name, stamp, session.turn)
            if message.get("stop_reason") == "end_turn":
                result.append(session.root(stamp, "completed"))
        return result
