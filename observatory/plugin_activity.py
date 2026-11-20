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
_LEGACY_PI_PROCESS_ID = "example.rust-pi-harness"
_OPERATION_ID = re.compile(r"[a-zA-Z0-9][a-zA-Z0-9._:-]{0,255}\Z")
_EVENT_KEY = re.compile(r"plugin-event:[0-9a-f]{64}\Z")


def export_proof(store, event_id: str) -> dict | None:
    """Read one policy-current projected event and its exact source revision."""
    if type(event_id) is not str or not _EVENT_KEY.fullmatch(event_id):
        raise ValueError('invalid plugin Activity event identity')
    with store._connect() as db:
        db.execute('BEGIN')
        row = db.execute('''SELECT a.payload, a.owner_key, c.receiver_id,
                              c.lease_epoch, c.policy_epoch, c.dirty_seq
            FROM activity_index_v1 a
            JOIN plugin_source_cursor_v1 c ON c.source_id=a.owner_key
            JOIN plugin_projection_cursor_v1 p ON p.source_id=c.source_id
            WHERE a.event_key=? AND a.kind='plugin' AND c.capture_enabled=1
              AND c.dirty_seq=p.projected_seq AND c.policy_epoch=p.policy_epoch''',
            (event_id,)).fetchone()
        if row is None:
            return None
        try:
            record = json.loads(row['payload'])
        except (TypeError, ValueError):
            return None
        if not isinstance(record, dict) or not isinstance(record.get('metadata'), dict):
            return None
        if (record.get('id') != event_id or record.get('source') != 'plugin'
                or record['metadata'].get('plugin_source_id') != row['owner_key']):
            return None
        record['receiver_id'] = row['receiver_id']
        return {'receiver_id': row['receiver_id'],
                'revision': [row['lease_epoch'], row['policy_epoch'], row['dirty_seq']],
                'record': record}


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
    visible = []
    calls: dict[tuple[str, str], list[tuple[str, str, int, int]]] = {}
    for row in latest.values():
        kind, native_session = row["kind"], row["native_session_id"]
        if kind not in _ACTIVITY_KINDS or not native_session or native_session in invalidated:
            continue
        payload = json.loads(row["payload_json"])
        # Validate before correlating, including call rows which may later be
        # used to assign a result's turn. No unadmitted state is consulted.
        validate_metadata({"kind": kind, "payload": payload,
                           "identity": {"session_id": native_session,
                                        "operation_id": row["native_operation_id"]}})
        visible.append((row, payload))
        if kind == "tool" and payload["result"] == "started" and row["native_operation_id"]:
            key = (native_session, row["native_operation_id"])
            calls.setdefault(key, []).append((row["native_turn_id"], payload["category"],
                                              row["time_ns"], row["admitted_ns"]))
    result = []
    for row, payload in visible:
        kind = row["kind"]
        native_session = row["native_session_id"]
        session_key = "plugin:" + _opaque(receiver_id, source_id, native_session)
        event_key = "plugin-event:" + _opaque(receiver_id, source_id, row["event_id_hash"])
        agent_id = canonical_agent_id(plugin_id, "plugin:" + source_id, native_session)
        native_turn = row["native_turn_id"]
        category = payload.get("category", "")
        relation = ""
        category_conflict = False
        operation_id = row["native_operation_id"]
        if kind == "tool" and payload["result"] != "started" and operation_id:
            matches = calls.get((native_session, operation_id), [])
            match = matches[0] if len(matches) == 1 else None
            if (match and match[0] and match[2] < row["time_ns"]
                    and match[3] <= row["admitted_ns"]
                    and (not native_turn or native_turn == match[0])):
                native_turn = match[0]
                relation = "matched_call"
                # A result's missing name can be recovered from its call. A
                # disagreement is left visible as unknown coarse category.
                if category == "other":
                    category = match[1]
                elif category != match[1] and match[1] != "other":
                    category = "other"
                    category_conflict = True
            else:
                if len(matches) > 1:
                    native_turn = ""
                    relation = "ambiguous_call"
                elif match is None:
                    # Other adapters can report a turn directly even when a
                    # terminal tool fact has no separate started fact. Older
                    # bundled Pi results borrowed the current user turn, so
                    # their raw turn cannot be trusted without a call.
                    if native_turn and plugin_id != _LEGACY_PI_PROCESS_ID:
                        relation = "unverified_native_turn"
                    else:
                        native_turn = ""
                        relation = "missing_call"
                elif match[2] > row["time_ns"] or match[3] > row["admitted_ns"]:
                    native_turn = ""
                    relation = "future_call"
                elif match[2] == row["time_ns"]:
                    native_turn = ""
                    relation = "unproven_order"
                elif not match[0]:
                    native_turn = ""
                    relation = "call_turn_unavailable"
                else:
                    native_turn = ""
                    relation = "conflicting_turn"
        turn_id = (_opaque("plugin-turn", receiver_id, source_id, native_session,
                           native_turn)[:32]
                   if native_turn else "")
        if kind == "message":
            action, result_name = payload["role"], "observed"
            subject = payload["role"].capitalize() + " message recorded"
        else:
            action, result_name = category, payload["result"]
            subject = (category.capitalize() +
                       (" check recorded" if kind == "check" else " operation recorded"))
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
                             "capture": "content-free", "coverage": "partial",
                             **({"tool_turn_relation": relation} if relation else {}),
                             **({"tool_category_conflict": True} if category_conflict else {})},
                "capture_content": False, "detail_present": False,
                "operation_present": False}
        result.append((event_key, "plugin", source_id, row["time_ns"], None,
                       session_key, agent_id, plugin_id, kind, action, result_name,
                       turn_id or None, json.dumps(fact, sort_keys=True,
                                                   separators=(",", ":"))))
    return result
