"""Pi session format: identity, evidence, lifecycle and activity callbacks."""
from __future__ import annotations

from pathlib import Path

from ..agent_hierarchy import canonical_agent_id, provenance
from ..local_activity import emit, plaintext
from ..session_text import remember_task


class PiAdapter:
    def initial_task(self, row: dict) -> object:
        if row.get("type") != "message":
            return None
        message = row.get("message") or {}
        return message.get("content") if isinstance(message, dict) and message.get("role") == "user" else None

    def parse(self, session, row: dict, stamp: int, prior_models: dict) -> list[dict]:
        from ..local_agents import _safe_identifier, timestamp

        kind = row.get("type")
        if kind == "session":
            native_id = (_safe_identifier(row.get("agentId")) or
                         _safe_identifier(row.get("id")) or session.session_id)
            session.session_id = native_id
            session.native_session_id = _safe_identifier(row.get("sessionId")) or native_id
            session.native_worker_id = native_id if native_id != session.native_session_id else None
            session.agent_id = canonical_agent_id(session.runtime, session.agent_namespace, native_id)
            parent = (_safe_identifier(row.get("parentAgentId")) or
                      _safe_identifier(row.get("parentSessionId")) or
                      _safe_identifier(row.get("parent_id")))
            if parent:
                session.parent_agent_id = canonical_agent_id(
                    session.runtime, session.agent_namespace, parent)
                session.relationship_kind = "delegated"
                session.relationship_state = "confirmed"
                session.relationship_provenance = provenance(
                    "pi-session", "explicit parentAgentId/parentSessionId", timestamp(row.get("timestamp")))
            else:
                session.relationship_kind = "unknown"
                session.relationship_state = "unresolved"
                session.relationship_provenance = provenance(
                    "pi-session", "no verified parent metadata", timestamp(row.get("timestamp")))
            cwd = row.get("cwd")
            if cwd:
                session.project = Path(cwd).name
                session.working_directory = str(cwd) if session.capture_details else ""
            return []
        if kind == "model_change":
            session.model = row.get("modelId") or session.model
            session.provider = row.get("provider") or session.provider
            return []
        if kind != "message":
            return []
        message = row.get("message") or {}
        if not isinstance(message, dict):
            return []
        role = message.get("role")
        if role == "user":
            session.turn = str(row.get("id") or stamp or session.turn)
            session.started[session.turn] = stamp
            text = plaintext(message.get("content")) if session.capture_details else ""
            if text:
                remember_task(session, message.get("content"))
            return [session.root(stamp, "running")]
        if role == "assistant":
            session.model = message.get("model") or session.model
            session.provider = message.get("provider") or session.provider
            result = []
            usage = message.get("usage")
            if isinstance(usage, dict):
                usage_fields = {
                    "input": "input_tokens", "output": "output_tokens",
                    "cacheRead": "cache_read_input_tokens", "cacheWrite": "cache_write_input_tokens",
                    "reasoning": "reasoning_tokens",
                }
                normalized = {target: usage[source] for source, target in usage_fields.items()
                              if source in usage}
                span = session.model_span(("pi-model", row.get("id") or stamp), normalized, stamp)
                if span:
                    span["attributes"]["gen_ai.provider.name"] = str(session.provider or "unknown")[:128]
                    result.append(span)
            for block in message.get("content") or []:
                if not isinstance(block, dict) or block.get("type") != "toolCall":
                    continue
                call_id = str(block.get("id") or "")
                name = str(block.get("name") or "Tool")
                if call_id:
                    session.calls[call_id] = (name, stamp, session.turn)
            if message.get("stopReason") == "stop":
                result.append(session.root(stamp, "completed"))
            elif message.get("stopReason") == "error":
                result.append(session.root(stamp, "interrupted"))
            return result
        if role == "toolResult":
            call_id = str(message.get("toolCallId") or "")
            pending = session.calls.pop(call_id, None)
            name = str(message.get("toolName") or (pending[0] if pending else "Tool"))
            start = pending[1] if pending else stamp
            turn = pending[2] if pending else session.turn
            previous, session.turn = session.turn, turn
            span = session.span(("tool", call_id), name, stamp,
                {"gen_ai.operation.name": "execute_tool", "tool.name": name},
                start=start, error=bool(message.get("isError")))
            session.turn = previous
            return [span] if span else []
        return []

    def observe_state(self, session, row: dict, stamp: int) -> tuple[str, object] | None:
        message = row.get("message") or {}
        if not isinstance(message, dict):
            return None
        state = session.run_state
        role = message.get("role")
        if role == "user":
            state = "running"
            session.pending_tools.clear()
        elif role == "assistant":
            for block in message.get("content") or []:
                if isinstance(block, dict) and block.get("type") == "toolCall":
                    key = str(block.get("id") or "")
                    name = str(block.get("name") or "Tool")
                    session.pending_tools[key] = name
                    if name.rsplit(".", 1)[-1] in {"request_user_input", "AskUserQuestion"}:
                        session.pending_request_times[key] = stamp
            if message.get("stopReason") == "stop":
                state = "completed"
                session.pending_tools.clear()
            else:
                state = "running"
        elif role == "toolResult":
            session.pending_tools.pop(str(message.get("toolCallId") or ""), None)
            state = "running"
        return state, None

    def activity(self, session, row: dict, stamp: int) -> list[dict]:
        message = row.get("message") or {}
        if row.get("type") != "message" or not isinstance(message, dict):
            return []
        role = message.get("role")
        out = []
        if role == "user":
            text = plaintext(message.get("content")) if session.capture_details else ""
            if text:
                remember_task(session, message.get("content"))
            out.append(emit(session, row, stamp, "message", "user",
                            text.splitlines()[0][:300] if text else "user message", text,
                            "observed", {"message.role": "user"}, turn=session.turn))
        elif role == "assistant":
            for block in message.get("content") or []:
                if not isinstance(block, dict) or block.get("type") != "toolCall":
                    continue
                call_id = str(block.get("id") or "")
                name = str(block.get("name") or "Tool")
                if not call_id:
                    continue
                session.calls[call_id] = (name, stamp, session.turn)
                session.tool_inputs[call_id] = (name, "", session.turn)
                out.append(emit(session, row, stamp, "tool", "call", name,
                                block.get("arguments") if session.capture_details else "",
                                "started", {"tool.name": name}, call_id=call_id, turn=session.turn))
        elif role == "toolResult":
            call_id = str(message.get("toolCallId") or "")
            name = str(message.get("toolName") or "Tool")
            pending = session.calls.get(call_id)
            turn = pending[2] if pending else session.turn
            failed = bool(message.get("isError"))
            detail = plaintext(message.get("content")) if session.capture_details else ""
            out.append(emit(session, row, stamp, "tool", "result", name, detail,
                            "error" if failed else "success", {"tool.name": name},
                            call_id=call_id or None, turn=turn))
        if role == "assistant" and isinstance(message.get("usage"), dict):
            out.append(emit(session, row, stamp, "model", "usage", session.model or "Model not reported",
                            message["usage"], "reported", {"gen_ai.request.model": session.model or "unknown"}))
        return [entry for entry in out if entry]
