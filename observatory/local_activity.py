"""Bounded, redacted activity extracted from native agent session records."""
from __future__ import annotations

import json
import re
import shlex
from itertools import islice
from pathlib import Path

from .findings import _safe_text, _SENSITIVE_FIELD, _TOKEN_FIELD
from .native_json import loads
from .session_text import remember_task

CONTENT_LIMIT = 8000


def safe(value, limit=CONTENT_LIMIT):
    def clean(item, depth=0):
        if depth>=12:return '[nested payload omitted]'
        if isinstance(item,dict):
            result={str(key): '[REDACTED]' if _SENSITIVE_FIELD.search(str(key)) or _TOKEN_FIELD.search(str(key))
                else '[opaque payload omitted]' if str(key) in {'encrypted_content','signature','raw_content'} or str(key)=='data' and isinstance(child,str) and len(child)>1000
                else clean(child,depth+1) for key,child in islice(item.items(),60)}
            if len(item)>60:result['[truncated]']=f'{len(item)-60} fields omitted'
            return result
        if isinstance(item,list):
            result=[clean(child,depth+1) for child in item[:60]]
            if len(item)>60:result.append(f'[truncated: {len(item)-60} items omitted]')
            return result
        if item is None or isinstance(item,(int,float,bool)):return item
        text=_safe_text(str(item))
        return text[:2000]+('[truncated]' if len(text)>2000 else '')
    if isinstance(value,(dict,list)):
        value=json.dumps(clean(value),ensure_ascii=False,indent=2)
    text = _safe_text(value)
    text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)
    text = "".join(char for char in text if char in "\n\t" or ord(char) >= 32 and ord(char) != 127)
    return text[:limit] + ("\n[truncated]" if len(text) > limit else "")


def plaintext(content):
    if isinstance(content, str):
        return safe(content)
    if not isinstance(content, list):
        return ""
    return safe("\n".join(str(block.get("text") or "") for block in content
                         if isinstance(block, dict) and str(block.get("type") or "").casefold() in {"text", "input_text", "output_text"}))


def mcp_attributes(name):
    """Recognize qualified MCP names without guessing from ordinary tool names."""
    parts = str(name).split("__", 2)
    if len(parts) == 3 and parts[0] == "mcp" and parts[1] and parts[2]:
        return {"mcp.server.name": parts[1], "mcp.tool.name": parts[2],
                "mcp.method.name": "tools/call"}
    return {}


def mcp_failed(value):
    if isinstance(value, str):
        try:
            value = loads(value)
        except (ValueError, OverflowError, RecursionError):
            return False
    return isinstance(value,dict) and (value.get('isError') is True or value.get('is_error') is True)


def emit(session, row, stamp, domain, action, subject, detail="", result="observed", extra=None,
         *, call_id=None, turn=None):
    if not stamp:
        return None
    from .local_agents import identity
    identity_subject=subject  # Preserve existing native occurrence/seen IDs.
    if domain=='shell':
        from .operation_facts import safe_value
        subject=safe_value(subject)[0]
    attrs = {"session.id": session.session_id, "observatory.project": session.project,
             "agent.id": session.agent_id, "agent.runtime": session.runtime,
             "agent.discovery_state": session.discovery_state,
             "agent.relationship.kind": session.relationship_kind,
             "agent.relationship.state": session.relationship_state,
             "observatory.data_source": "local-session-log", "agent.activity.domain": domain,
             "observatory.operation.capture_version": 1,
             "agent.activity.action": action, "agent.activity.subject": safe(subject, 500),
             "agent.activity.result": result, "agent.activity.actor": session.runtime,
             "observatory.record_id": identity(session.source_id, row.get("ordinal") or row.get("uuid") or stamp,
                                                domain, action, str(identity_subject)[:500], *([call_id] if call_id else [])), **(extra or {})}
    if session.capture_details and not session.internal and session.task_description:
        attrs['agent.session.task'] = session.task_description
    if session.parent_agent_id:
        attrs["agent.parent_id"] = session.parent_agent_id
    payload = row.get("payload") or {}
    item = payload.get("item") or {} if isinstance(payload, dict) else {}
    span_id = None
    if isinstance(item, dict) and item.get("type") in {"CommandExecution", "FileChange", "McpToolCall", "Extension"}:
        span_id = identity(session.source_id, ("tool", item.get("id") or row.get("ordinal") or stamp), size=16)
    if call_id:
        span_id = identity(session.source_id, ("tool", call_id), size=16)
    return {"trace_id": identity(session.source_id, turn or session.turn), "span_id": span_id,
            "time_unix_nano": stamp, "service": session.service,
            "severity_number": 17 if result in {"error", "failed", "denied"} else 9,
            "severity_text": "ERROR" if result in {"error", "failed", "denied"} else "INFO",
            "event_name": "agent.activity", "body": safe(detail) if session.capture_details else "",
            "attributes": attrs, "resource_attributes": {"observatory.source_id": session.source_id,
                "observatory.capture": "local-session-log"}, "scope_name": "observatory.local_agents"}


def duration(item):
    value = item.get("duration")
    if isinstance(value, dict):
        try:
            return float(value.get("secs", 0)) * 1000 + float(value.get("nanos", 0))/1e6
        except (ValueError, TypeError):
            pass
    return None


def codex_activity(session, row, stamp):
    """Compatibility entry point for the bundled Codex format adapter."""
    from .adapters.codex import CodexAdapter
    return CodexAdapter().activity(session, row, stamp)


def claude_activity(session, row, stamp):
    """Compatibility entry point for the bundled Claude format adapter."""
    from .adapters.claude import ClaudeAdapter
    return ClaudeAdapter().activity(session, row, stamp)


def pi_activity(session, row, stamp):
    """Compatibility entry point for the bundled Pi format adapter."""
    from .adapters.pi import PiAdapter
    return PiAdapter().activity(session, row, stamp)


def timestamp_of(row):
    from .local_agents import timestamp
    return timestamp(row.get("timestamp"))


KIRO_EXIT_CODE = re.compile(r"\n\s*Exit Code: (-?\d{1,10})\s*$")
KIRO_FILE_ACTIONS = {"fs_write": "write", "write_file": "write", "str_replace": "edit",
                     "fs_append": "edit", "delete_file": "delete"}


def _kiro_call(payload, turn):
    """Retain only the bounded call fields that classify its later result."""
    from .local_agents import _kiro_mcp_attributes
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


def kiro_activity(session, row, stamp):
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
        out.extend(kiro_activity(session, released, timestamp_of(released) or stamp))
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
        from .local_agents import KIRO_STOP_STATES
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
        from .operation_facts import evidence_detail
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
