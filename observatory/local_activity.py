"""Bounded, redacted activity extracted from native agent session records."""
from __future__ import annotations

import json
import re
from itertools import islice

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


def kiro_activity(session, row, stamp):
    """Compatibility entry point for the bundled Kiro format adapter."""
    from .adapters.kiro import KiroAdapter
    return KiroAdapter().activity(session, row, stamp)
