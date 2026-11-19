"""Host-owned, metadata-only projection of admitted plugin session evidence.

No child code runs during projection. A source snapshot is reduced outside the
writer transaction, then published only if its policy and dirty sequence still
match. A revoked source's projection is erased by the policy transaction.
"""
from __future__ import annotations

import hashlib
import json
from typing import TYPE_CHECKING

from .agent_hierarchy import canonical_agent_id
from .plugin_wire import ProtocolError

if TYPE_CHECKING:
    from .core import Store

MAX_PROJECTION_EVIDENCE = 50_000
MAX_PROJECTION_SESSIONS = 2_000
MAX_JSON_SAFE_INTEGER = 2**53 - 1


def _reduce(rows: list, receiver_id: str, source_id: str, policy_epoch: int,
            dirty_seq: int) -> list[tuple]:
    latest = {}
    for row in rows:
        key = row["event_id_hash"]
        if key not in latest:
            latest[key] = row
    sessions: dict[str, dict] = {}
    for row in latest.values():
        native_id = row["native_session_id"]
        if not native_id:
            continue
        entry = sessions.setdefault(native_id, {"last_record_ns": 0, "lifecycle": [],
                                                "usage": {}, "relationships": []})
        entry["last_record_ns"] = max(entry["last_record_ns"], row["admitted_ns"])
        if row["kind"] == "lifecycle":
            payload = json.loads(row["payload_json"])
            entry["lifecycle"].append((row["time_ns"], payload["state"],
                                       payload.get("reason_code", "")))
        elif row["kind"] == "usage":
            # Each distinct event is an additive observation; a higher
            # revision replaces it before this reduction. Preserve absent
            # counters as unknown rather than manufacturing zeroes.
            for field, value in json.loads(row["payload_json"]).items():
                entry["usage"][field] = entry["usage"].get(field, 0) + value
        elif row["kind"] == "relationship":
            payload = json.loads(row["payload_json"])
            entry["relationships"].append((row["time_ns"], payload["parent_session_id"],
                                           payload["kind"]))
    if len(sessions) > MAX_PROJECTION_SESSIONS:
        raise ProtocolError("plugin session projection capacity reached")
    # A relationship is confirmed only when the referenced parent is present
    # in this policy-current source snapshot. Cycles and self-links are invalid.
    chosen: dict[str, tuple[str, str, str]] = {}
    for native_id, entry in sessions.items():
        relations = entry["relationships"]
        if not relations:
            chosen[native_id] = ("", "unknown", "unresolved")
            continue
        latest_time = max(item[0] for item in relations)
        latest = {(parent, kind) for stamp, parent, kind in relations if stamp == latest_time}
        if len(latest) != 1:
            chosen[native_id] = ("", "unknown", "conflicted")
        else:
            parent, kind = next(iter(latest))
            chosen[native_id] = (parent, kind, "unresolved")
    for native_id, (parent, kind, state) in tuple(chosen.items()):
        if not parent or state == "conflicted":
            continue
        seen = {native_id}
        cursor = parent
        while cursor in sessions and cursor not in seen:
            seen.add(cursor)
            cursor = chosen[cursor][0]
        state = ("invalid" if cursor in seen else
                 "confirmed" if parent in sessions else "unresolved")
        chosen[native_id] = (parent, kind, state)
    result = []
    for native_id, entry in sessions.items():
        lifecycle = entry["lifecycle"]
        if lifecycle:
            state_time = max(item[0] for item in lifecycle)
            last = [item for item in lifecycle if item[0] == state_time]
            if len({(item[1], item[2]) for item in last}) == 1:
                state, reason = last[0][1:]
            else:
                state, reason = "unknown", "ambiguous_lifecycle"
        else:
            state_time, state, reason = 0, "unknown", ""
        session_key = "plugin:" + hashlib.sha256(
            (receiver_id + "\0" + source_id + "\0" + native_id).encode()).hexdigest()
        parent, kind, relation_state = chosen[native_id]
        result.append((source_id, native_id, session_key, state, reason, state_time,
                       entry["last_record_ns"], policy_epoch, dirty_seq,
                       json.dumps(entry["usage"], sort_keys=True, separators=(",", ":")),
                       parent, kind, relation_state))
    return result


def project_source(store: Store, source_id: str) -> int:
    """Publish one coherent source revision; caller retries on a stale snapshot."""
    receiver_id = store.receiver_namespace()
    with store._connect() as db:
        db.execute("BEGIN")
        source = db.execute("SELECT * FROM plugin_source_cursor_v1 WHERE source_id=?",
                            (source_id,)).fetchone()
        if source is None or source["receiver_id"] != receiver_id:
            raise ProtocolError("unknown plugin source")
        if not source["capture_enabled"]:
            return 0
        published = db.execute("SELECT projected_seq,policy_epoch FROM plugin_projection_cursor_v1 "
                               "WHERE source_id=?", (source_id,)).fetchone()
        if (source["dirty_seq"] == 0 or published is not None and
                (published["projected_seq"], published["policy_epoch"]) ==
                (source["dirty_seq"], source["policy_epoch"])):
            return 0
        rows = db.execute("SELECT event_id_hash,revision,kind,time_ns,payload_json,"
                          "native_session_id,admitted_ns FROM plugin_evidence_v1 "
                          "WHERE source_id=? ORDER BY event_id_hash,revision DESC LIMIT ?",
                          (source_id, MAX_PROJECTION_EVIDENCE + 1)).fetchall()
        if len(rows) > MAX_PROJECTION_EVIDENCE:
            raise ProtocolError("plugin evidence projection capacity reached")
        invalidated = {row[0]: row[1] for row in db.execute(
            "SELECT native_session_id,last_record_ns FROM plugin_retention_invalidated_v1 "
            "WHERE source_id=?", (source_id,))}
        policy_epoch, dirty_seq = source["policy_epoch"], source["dirty_seq"]
        revision = (source["lease_epoch"], policy_epoch, dirty_seq)
    # Release the read snapshot before CPU work so a large source cannot pin WAL.
    projected = _reduce(rows, receiver_id, source_id, policy_epoch, dirty_seq)
    projected = [((*row[:3], "unknown", "retention_gap", *row[5:9], "{}",
                   "", "unknown", "unresolved")
                  if row[1] in invalidated else row) for row in projected]
    present = {row[1] for row in projected}
    for native_id, last_record_ns in invalidated.items():
        if native_id in present:
            continue
        session_key = "plugin:" + hashlib.sha256(
            (receiver_id + "\0" + source_id + "\0" + native_id).encode()).hexdigest()
        projected.append((source_id, native_id, session_key, "unknown", "retention_gap",
                          0, last_record_ns, policy_epoch, dirty_seq, "{}",
                          "", "unknown", "unresolved"))
    if len(projected) > MAX_PROJECTION_SESSIONS:
        raise ProtocolError("plugin session projection capacity reached")
    with store._source_write(), store._connect() as db:
        db.execute("BEGIN IMMEDIATE")
        current = db.execute("SELECT receiver_id,lease_epoch,policy_epoch,dirty_seq,"
                             "capture_enabled FROM plugin_source_cursor_v1 WHERE source_id=?",
                             (source_id,)).fetchone()
        if (current is None or current["receiver_id"] != receiver_id
                or not current["capture_enabled"]
                or tuple(current[key] for key in ("lease_epoch", "policy_epoch", "dirty_seq"))
                != revision):
            raise ProtocolError("plugin projection source changed; retry")
        db.execute("DELETE FROM plugin_session_projection_v1 WHERE source_id=?", (source_id,))
        db.executemany("INSERT INTO plugin_session_projection_v1(source_id,native_session_id,session_key,"
                       "state,reason_code,state_time_ns,last_record_ns,policy_epoch,source_dirty_seq,"
                       "usage_json,parent_native_session_id,relationship_kind,relationship_state) "
                       "VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                       projected)
        db.execute("INSERT INTO plugin_projection_cursor_v1(source_id,projected_seq,policy_epoch) "
                   "VALUES(?,?,?) ON CONFLICT(source_id) DO UPDATE SET "
                   "projected_seq=excluded.projected_seq,policy_epoch=excluded.policy_epoch",
                   (source_id, dirty_seq, policy_epoch))
    return len(projected)


def sessions(store: Store, *, limit: int = 200) -> dict:
    """Return only policy-current, fully projected rows and coverage state."""
    if type(limit) is not int or not 1 <= limit <= 1_000:
        raise ValueError("invalid plugin session limit")
    receiver_id = store.receiver_namespace()
    with store._connect() as db:
        db.execute("BEGIN")
        if db.execute("SELECT 1 FROM plugin_source_cursor_v1 LIMIT 1").fetchone() is None:
            return {"sessions": [], "pending_sources": 0, "omitted_sessions": False,
                    "retention_gaps": []}
        gaps = [{"source_id": row["source_id"], "watermark_ns": str(row["watermark_ns"]),
                 "dropped_events": (row["dropped_events"] if row["dropped_events"]
                                    <= MAX_JSON_SAFE_INTEGER else str(row["dropped_events"]))}
                for row in db.execute(
            "SELECT r.source_id,r.watermark_ns,r.dropped_events "
            "FROM plugin_retention_v1 r JOIN plugin_source_cursor_v1 c USING(source_id) "
            "WHERE c.receiver_id=? AND c.capture_enabled=1 AND r.policy_epoch=c.policy_epoch "
            "ORDER BY r.source_id LIMIT 32", (receiver_id,))]
        pending = db.execute("SELECT COUNT(*) FROM plugin_source_cursor_v1 c "
                             "LEFT JOIN plugin_projection_cursor_v1 p USING(source_id) "
                             "WHERE c.receiver_id=? AND c.capture_enabled=1 AND c.dirty_seq>0 "
                             "AND (p.projected_seq IS NULL OR p.projected_seq!=c.dirty_seq "
                             "OR p.policy_epoch!=c.policy_epoch)", (receiver_id,)).fetchone()[0]
        rows = db.execute("SELECT p.*,c.plugin_id FROM plugin_session_projection_v1 p "
                          "JOIN plugin_source_cursor_v1 c USING(source_id) "
                          "JOIN plugin_projection_cursor_v1 pc USING(source_id) "
                          "WHERE c.receiver_id=? AND c.capture_enabled=1 "
                          "AND p.policy_epoch=c.policy_epoch AND p.source_dirty_seq=c.dirty_seq "
                          "AND pc.policy_epoch=c.policy_epoch AND pc.projected_seq=c.dirty_seq "
                          "ORDER BY p.last_record_ns DESC,p.session_key LIMIT ?",
                          (receiver_id, limit + 1)).fetchall()
    result = []
    for row in rows[:limit]:
        reported = {"started": "running", "paused": "waiting",
                    "cancelled": "interrupted"}.get(row["state"], row["state"])
        usage = {key: value if value <= MAX_JSON_SAFE_INTEGER else str(value)
                 for key, value in json.loads(row["usage_json"]).items()}
        parent = row["parent_native_session_id"]
        parent_agent_id = (canonical_agent_id(row["plugin_id"],
                            "plugin:" + row["source_id"], parent)
                           if parent and row["relationship_state"] != "invalid" else None)
        result.append({"id": row["session_key"], "source_id": row["session_key"],
                       "plugin_source_id": row["source_id"],
                       "session_id": row["session_key"], "native_session_id": row["native_session_id"],
                       "agent_native_id": row["native_session_id"],
                       "runtime": row["plugin_id"], "service": row["plugin_id"],
                       "source_namespace": "plugin:" + row["source_id"],
                       "run_state": reported, "reported_state": row["state"], "task": "",
                       "capture_content": False, "stamp": row["last_record_ns"],
                       "last_record_ns": row["last_record_ns"],
                       "task_outcome_ns": row["state_time_ns"] if reported in
                       {"completed", "failed", "interrupted", "cancelled"} else 0,
                       "reason_code": row["reason_code"], "coverage": "partial",
                       "metadata_usage": usage,
                       "discovery_state": "observed", "relationship_kind": row["relationship_kind"],
                       "relationship_state": row["relationship_state"],
                       "parent_agent_id": parent_agent_id,
                       "parent_agent_native_id": parent or None,
                       "relationship_provenance": "plugin explicit relationship" if parent else
                       "No parent relationship was reported"})
    return {"sessions": result, "pending_sources": pending,
            "omitted_sessions": len(rows) > limit, "retention_gaps": gaps}
