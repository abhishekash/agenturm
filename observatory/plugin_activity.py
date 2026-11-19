"""Strict content-free plugin activity facts and policy-owned index rows."""
from __future__ import annotations

import hashlib
import json
import re

from .agent_hierarchy import canonical_agent_id
from .plugin_wire import ProtocolError

_MESSAGE_ROLES = frozenset({"user", "assistant"})
_TOOL_CATEGORIES = frozenset({"shell", "file", "mcp", "other"})
_TOOL_RESULTS = frozenset({"started", "succeeded", "failed", "denied"})
_CHECK_CATEGORIES = frozenset({"test", "lint", "build", "review", "other"})
_CHECK_RESULTS = frozenset({"passed", "failed", "skipped", "unknown"})
_ACTIVITY_KINDS = frozenset({"message", "tool", "check"})
_OPERATION_ID = re.compile(r"[a-zA-Z0-9][a-zA-Z0-9._:-]{0,255}\Z")


def validate_metadata(record: dict) -> dict:
    """Reject free text, command names, paths and malformed identity before SQL."""
    kind, payload = record["kind"], record["payload"]
    if not record.get("identity", {}).get("session_id"):
        raise ProtocolError("plugin activity requires native session identity")
    operation_id = record.get("identity", {}).get("operation_id")
    if operation_id and (kind != "tool" or not _OPERATION_ID.fullmatch(operation_id)):
        raise ProtocolError("invalid plugin tool operation identity")
    if kind == "message":
        valid = set(payload) == {"role"} and type(payload.get("role")) is str \
            and payload["role"] in _MESSAGE_ROLES
    elif kind == "tool":
        valid = (set(payload) == {"category", "result"}
                 and type(payload.get("category")) is str
                 and payload["category"] in _TOOL_CATEGORIES
                 and type(payload.get("result")) is str
                 and payload["result"] in _TOOL_RESULTS)
    elif kind == "check":
        valid = (set(payload) == {"category", "result"}
                 and type(payload.get("category")) is str
                 and payload["category"] in _CHECK_CATEGORIES
                 and type(payload.get("result")) is str
                 and payload["result"] in _CHECK_RESULTS)
    else:
        raise ProtocolError("unsupported plugin activity kind")
    if not valid:
        raise ProtocolError("invalid content-free plugin activity metadata")
    return payload


def _opaque(*parts: str) -> str:
    return hashlib.sha256("\0".join(parts).encode("utf-8")).hexdigest()


def index_rows(rows: list, *, receiver_id: str, source_id: str, plugin_id: str,
               invalidated: set[str]) -> list[tuple]:
    """Build bounded Activity rows from the latest retained source snapshot."""
    latest: dict[str, object] = {}
    for row in rows:
        latest.setdefault(row["event_id_hash"], row)
    result = []
    for row in latest.values():
        kind = row["kind"]
        native_session = row["native_session_id"]
        if kind not in _ACTIVITY_KINDS or not native_session or native_session in invalidated:
            continue
        payload = json.loads(row["payload_json"])
        # Stored metadata was validated at admission; fail closed on corrupt or
        # legacy rows before publishing any revision of this source.
        validate_metadata({"kind": kind, "payload": payload,
                           "identity": {"session_id": native_session,
                                        "operation_id": row["native_operation_id"]}})
        session_key = "plugin:" + _opaque(receiver_id, source_id, native_session)
        event_key = "plugin-event:" + _opaque(receiver_id, source_id, row["event_id_hash"])
        agent_id = canonical_agent_id(plugin_id, "plugin:" + source_id, native_session)
        turn_id = (_opaque("plugin-turn", receiver_id, source_id, native_session,
                           row["native_turn_id"])[:32]
                   if row["native_turn_id"] else "")
        if kind == "message":
            action, result_name = payload["role"], "observed"
            subject = payload["role"].capitalize() + " message recorded"
        else:
            action, result_name = payload["category"], payload["result"]
            subject = (payload["category"].capitalize() +
                       (" check recorded" if kind == "check" else " operation recorded"))
        operation_id = row["native_operation_id"]
        operation_key = ("plugin-operation:" + _opaque(receiver_id, source_id,
                                                        native_session, operation_id)
                         if kind == "tool" and operation_id else "")
        fact = {"activity_scope_version": 1, "id": event_key,
                "time_unix_nano": row["time_ns"], "domain": kind,
                "action": action, "subject": subject, "result": result_name,
                "actor": plugin_id, "agent_id": agent_id, "session_id": session_key,
                "service": plugin_id, "source": "plugin", "source_id": event_key,
                "span_id": None, "trace_id": None, "turn_id": turn_id,
                "operation_id": operation_key or None,
                "detail": "", "mcp_server": "", "mcp_tool": "",
                "duration_ms": None,
                "metadata": {"plugin_source_id": source_id,
                             "native_worker_observed": bool(row["native_worker_id"]),
                             "native_turn_observed": bool(row["native_turn_id"]),
                             "native_operation_observed": bool(operation_id),
                             "capture": "content-free", "coverage": "partial"},
                "capture_content": False, "detail_present": False,
                "operation_present": False}
        result.append((event_key, "plugin", source_id, row["time_ns"], None,
                       session_key, agent_id, plugin_id, kind, action, result_name,
                       turn_id or None, json.dumps(fact, sort_keys=True,
                                                   separators=(",", ":"))))
    return result
