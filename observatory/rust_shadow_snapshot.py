"""Read-only, fenced Store snapshot for the opt-in Rust domain parity check.

This module never starts a child or writes to Store. Callers may pass its
content-free fixture to a separately installed reducer, then use ``current``
to discard a result if the source or policy changed while it ran.
"""
from __future__ import annotations

import json
import os
import sqlite3
import stat
from dataclasses import dataclass

from .plugin_activity import index_rows as activity_index_rows
from .plugin_admission import _ID, _metadata
from .plugin_projection import MAX_JSON_SAFE_INTEGER
from .plugin_wire import INT64_MAX, ProtocolError, decode_message


MAX_EVIDENCE = 50_000
MAX_SESSIONS = 2_000
MAX_FIXTURE_BYTES = 16 * 1024 * 1024
MAX_TABLE_PAYLOAD_BYTES = 8 * 1024 * 1024


@dataclass(frozen=True)
class ShadowSnapshot:
    source_id: str
    revision: tuple[str, int, int, int, int, int, int]
    fixture: dict
    expected: dict
    # Optional for old callers that construct snapshots directly. Such a
    # snapshot cannot pass current(): only capture() can bind a Store inode.
    file_identity: tuple[int, int] | None = None


def _store_file_identity(store) -> tuple[int, int]:
    """Identify the regular Store target, including an explicit --db symlink."""
    try:
        info = os.stat(store.path)
    except (AttributeError, OSError) as exc:
        raise ProtocolError("shadow Store file is unavailable") from exc
    if not stat.S_ISREG(info.st_mode):
        raise ProtocolError("shadow Store path is not a regular file")
    return info.st_dev, info.st_ino


def _revision(db, source_id: str):
    row = db.execute(
        "SELECT c.receiver_id,c.lease_epoch,c.policy_epoch,c.dirty_seq,"
        "c.capture_enabled,p.projected_seq,p.policy_epoch AS projected_policy "
        "FROM plugin_source_cursor_v1 c LEFT JOIN plugin_projection_cursor_v1 p "
        "USING(source_id) WHERE c.source_id=?", (source_id,)).fetchone()
    if row is None:
        raise ProtocolError("unknown plugin source")
    return tuple(row[key] for key in ("receiver_id", "lease_epoch", "policy_epoch",
                                      "dirty_seq", "capture_enabled", "projected_seq",
                                      "projected_policy"))


def _ready(revision) -> bool:
    return (revision[4] == 1 and revision[3] > 0
            and revision[5] == revision[3] and revision[6] == revision[2])


def capture(store, source_id: str) -> ShadowSnapshot | None:
    """Copy one policy-current projection and admitted evidence in one read txn.

    ``None`` means capture is disabled or projection is pending. The returned
    fixture contains only typed metadata admitted by the Store. A caller must
    check ``current`` after its independent reduction before using a result.
    """
    file_identity = _store_file_identity(store)
    with store._read() as db:
        db.execute("BEGIN")
        revision = _revision(db, source_id)
        if not _ready(revision):
            return None
        receiver = db.execute("SELECT value FROM schema_meta WHERE key='receiver_id'").fetchone()
        if receiver is None or receiver[0] != revision[0]:
            raise ProtocolError("receiver identity changed during shadow snapshot")
        plugin = db.execute("SELECT plugin_id FROM plugin_source_cursor_v1 WHERE source_id=?",
                            (source_id,)).fetchone()[0]
        evidence_size = db.execute(
            "SELECT COUNT(*),COALESCE(SUM(length(CAST(payload_json AS BLOB))),0) "
            "FROM plugin_evidence_v1 WHERE source_id=?", (source_id,)).fetchone()
        activity_size = db.execute(
            "SELECT COUNT(*),COALESCE(SUM(length(CAST(payload AS BLOB))),0) "
            "FROM activity_index_v1 WHERE kind='plugin' AND owner_key=?",
            (source_id,)).fetchone()
        if (evidence_size[0] > MAX_EVIDENCE or activity_size[0] > MAX_EVIDENCE
                or evidence_size[1] > MAX_TABLE_PAYLOAD_BYTES
                or activity_size[1] > MAX_TABLE_PAYLOAD_BYTES):
            raise ProtocolError("shadow source exceeds bounded read budget")
        evidence = db.execute(
            "SELECT event_id_hash,revision,kind,time_ns,payload_json,native_session_id,"
            "native_worker_id,native_turn_id,native_operation_id,admitted_ns "
            "FROM plugin_evidence_v1 WHERE source_id=? "
            "ORDER BY event_id_hash,revision DESC LIMIT ?",
            (source_id, MAX_EVIDENCE + 1)).fetchall()
        invalidated = db.execute(
            "SELECT native_session_id,last_record_ns FROM plugin_retention_invalidated_v1 "
            "WHERE source_id=? ORDER BY native_session_id LIMIT ?",
            (source_id, MAX_SESSIONS + 1)).fetchall()
        gap_size = db.execute(
            "SELECT COUNT(*),COALESCE(SUM(length(CAST(native_session_id AS BLOB))),0) "
            "FROM plugin_source_gap_invalidated_v1 WHERE source_id=?", (source_id,)).fetchone()
        if gap_size[0] > MAX_SESSIONS or gap_size[1] > MAX_TABLE_PAYLOAD_BYTES:
            raise ProtocolError("shadow source gaps exceed bounded read budget")
        source_gaps = db.execute(
            "SELECT native_session_id,last_record_ns FROM plugin_source_gap_invalidated_v1 "
            "WHERE source_id=? ORDER BY native_session_id LIMIT ?",
            (source_id, MAX_SESSIONS + 1)).fetchall()
        projected = db.execute(
            "SELECT native_session_id,session_key,state,reason_code,state_time_ns,"
            "last_record_ns,usage_json,parent_native_session_id,relationship_kind,"
            "relationship_state,policy_epoch,source_dirty_seq "
            "FROM plugin_session_projection_v1 WHERE source_id=? "
            "ORDER BY native_session_id LIMIT ?", (source_id, MAX_SESSIONS + 1)).fetchall()
        activity = db.execute(
            "SELECT payload FROM activity_index_v1 WHERE kind='plugin' AND owner_key=? "
            "ORDER BY event_key LIMIT ?", (source_id, MAX_EVIDENCE + 1)).fetchall()
    if (len(evidence) > MAX_EVIDENCE or len(invalidated) > MAX_SESSIONS
            or len(source_gaps) > MAX_SESSIONS
            or len(projected) > MAX_SESSIONS or len(activity) > MAX_EVIDENCE):
        raise ProtocolError("shadow snapshot capacity reached")
    if (any((row["native_session_id"] != "*" and
             (not isinstance(row["native_session_id"], str)
              or not _ID.fullmatch(row["native_session_id"])))
            or type(row["last_record_ns"]) is not int
            or not 0 <= row["last_record_ns"] <= INT64_MAX
            or (row["native_session_id"] == "*" and row["last_record_ns"] != 0)
            for row in source_gaps)
            or any(row["native_session_id"] == "*" for row in source_gaps)
            and len(source_gaps) != 1):
        raise ProtocolError("invalid stored source-gap invalidation")
    if any(row["policy_epoch"] != revision[2] or row["source_dirty_seq"] != revision[3]
           for row in projected):
        raise ProtocolError("shadow projection is not policy-current")
    rows = []
    for row in evidence:
        identity = {key: row[column] for key, column in (
            ("session_id", "native_session_id"), ("worker_id", "native_worker_id"),
            ("turn_id", "native_turn_id"), ("operation_id", "native_operation_id"))
            if row[column]}
        payload = decode_message(row["payload_json"])
        _metadata({"kind": row["kind"], "revision": row["revision"],
                   "payload": payload, "identity": identity})
        rows.append({"event_id_hash": row["event_id_hash"],
                     "revision": row["revision"], "kind": row["kind"],
                     "time_ns": str(row["time_ns"]),
                     "admitted_ns": str(row["admitted_ns"]),
                     "native_session_id": row["native_session_id"] or None,
                     "native_worker_id": row["native_worker_id"] or None,
                     "native_turn_id": row["native_turn_id"] or None,
                     "native_operation_id": row["native_operation_id"] or None,
                     "payload": payload})
    fixture = {"schema_version": 3, "receiver_id": revision[0],
               "source_id": source_id, "plugin_id": plugin,
               "policy_epoch": revision[2], "dirty_seq": revision[3],
               "rows": rows, "invalidated": [
                   {"native_session_id": row["native_session_id"],
                    "last_record_ns": str(row["last_record_ns"])}
                   for row in invalidated],
               "source_gaps": [
                   {"native_session_id": row["native_session_id"],
                    "last_record_ns": str(row["last_record_ns"])}
                   for row in source_gaps]}
    try:
        size = len(json.dumps(fixture, separators=(",", ":"),
                              ensure_ascii=False).encode("utf-8", "strict"))
    except (TypeError, ValueError, UnicodeError) as exc:
        raise ProtocolError("invalid shadow fixture encoding") from exc
    if size > MAX_FIXTURE_BYTES:
        raise ProtocolError("shadow fixture exceeds Rust input budget")
    sessions = []
    for row in projected:
        state = row["state"]
        try:
            usage = json.loads(row["usage_json"])
        except (TypeError, ValueError) as exc:
            raise ProtocolError("invalid stored plugin usage") from exc
        if (not isinstance(usage, dict)
                or any(not isinstance(key, str) or type(value) is not int or value < 0
                       for key, value in usage.items())):
            raise ProtocolError("invalid stored plugin usage")
        sessions.append({"native_session_id": row["native_session_id"],
                         "session_key": row["session_key"],
                         "reported_state": state,
                         "run_state": {"started": "running", "paused": "waiting",
                                       "cancelled": "interrupted"}.get(state, state),
                         "reason_code": row["reason_code"],
                         "state_time_ns": row["state_time_ns"],
                         "last_record_ns": row["last_record_ns"],
                         "metadata_usage": {
                             key: value if value <= MAX_JSON_SAFE_INTEGER else str(value)
                             for key, value in usage.items()},
                         "parent_native_session_id": row["parent_native_session_id"],
                         "relationship_kind": row["relationship_kind"],
                         "relationship_state": row["relationship_state"]})
    facts = [decode_message(row["payload"]) for row in activity]
    gap_ids = {row["native_session_id"] for row in source_gaps}
    if "*" in gap_ids:
        gap_ids.update(row["native_session_id"] for row in evidence
                       if row["native_session_id"])
    computed = [decode_message(row[-1]) for row in activity_index_rows(
        evidence, receiver_id=revision[0], source_id=source_id,
        plugin_id=plugin,
        invalidated={row["native_session_id"] for row in invalidated} | gap_ids)]
    if sorted(facts, key=lambda fact: fact["id"]) != sorted(
            computed, key=lambda fact: fact["id"]):
        raise ProtocolError("shadow Activity differs from admitted metadata")
    expected = {"schema_version": 3, "sessions": sessions,
                "activities": sorted(facts, key=lambda fact: fact["id"])}
    if _store_file_identity(store) != file_identity:
        raise ProtocolError("shadow Store file changed during capture")
    return ShadowSnapshot(source_id, revision, fixture, expected, file_identity)


def current(store, snapshot: ShadowSnapshot) -> bool:
    """Fence a shadow result against Store file, receiver, lease and projection."""
    if not isinstance(snapshot, ShadowSnapshot):
        raise TypeError("shadow snapshot required")
    if snapshot.file_identity is None:
        return False
    try:
        if _store_file_identity(store) != snapshot.file_identity:
            return False
    except ProtocolError:
        return False
    try:
        with store._read() as db:
            db.execute("BEGIN")
            try:
                revision = _revision(db, snapshot.source_id)
            except ProtocolError:
                return False
            receiver = db.execute("SELECT value FROM schema_meta WHERE key='receiver_id'").fetchone()
    except (sqlite3.Error, OSError):
        return False
    try:
        same_file = _store_file_identity(store) == snapshot.file_identity
    except ProtocolError:
        return False
    return bool(same_file and receiver and receiver[0] == revision[0]
                and _ready(revision) and revision == snapshot.revision)
