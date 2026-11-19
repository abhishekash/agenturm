"""Bounded plugin evidence horizon and fail-closed correction guard.

The retained window is a source-local checkpoint, not a claim of complete
history. Eviction invalidates every affected session until the source is
explicitly revoked/reset. Unknown corrections and old observations are refused
after the first checkpoint so evicted facts cannot silently reappear.
"""
from __future__ import annotations

from sqlite3 import Connection

from .plugin_wire import ProtocolError

MAX_INVALIDATED_SESSIONS = 2_000


def initialize(db: Connection) -> None:
    db.executescript("""
        CREATE TABLE IF NOT EXISTS plugin_retention_v1 (
            source_id TEXT PRIMARY KEY,
            policy_epoch INTEGER NOT NULL,
            watermark_ns INTEGER NOT NULL,
            dropped_events INTEGER NOT NULL,
            FOREIGN KEY(source_id) REFERENCES plugin_source_cursor_v1(source_id));
        CREATE TABLE IF NOT EXISTS plugin_retention_invalidated_v1 (
            source_id TEXT NOT NULL,
            native_session_id TEXT NOT NULL,
            last_record_ns INTEGER NOT NULL,
            PRIMARY KEY(source_id,native_session_id),
            FOREIGN KEY(source_id) REFERENCES plugin_source_cursor_v1(source_id));
    """)


def reject_unseen(db: Connection, source_id: str, *, revision: int, time_ns: int) -> None:
    """Enforce the immutable occurrence-time horizon on an unknown event ID."""
    row = db.execute("SELECT watermark_ns FROM plugin_retention_v1 WHERE source_id=?",
                     (source_id,)).fetchone()
    if row is not None and (revision != 0 or time_ns <= row[0]):
        raise ProtocolError("plugin event is outside retained evidence horizon")


def prune(db: Connection, source_id: str, *, policy_epoch: int, protected_hashes: set[str],
          incoming_session_ids: set[str], max_records: int) -> int:
    """Free a bounded chunk inside the caller's cursor transaction.

    Source IDs in the incoming batch are protected so a correction remains
    monotonic even when its existing occurrence is old. If all candidates are
    protected or invalidation would exceed its own cap, refuse the batch.
    """
    if max_records < 4:
        raise ProtocolError("plugin source evidence capacity reached")
    target = max_records // 2
    candidates = db.execute(
        "SELECT event_id_hash,native_session_id,time_ns,admitted_ns FROM plugin_evidence_v1 "
        "WHERE source_id=? ORDER BY admitted_ns,rowid LIMIT ?",
        (source_id, max_records + len(protected_hashes))).fetchall()
    victims = [row for row in candidates if row[0] not in protected_hashes][:target]
    if not victims:
        raise ProtocolError("plugin source evidence capacity reached")
    existing = {row[0] for row in db.execute(
        "SELECT native_session_id FROM plugin_retention_invalidated_v1 WHERE source_id=?",
        (source_id,))}
    invalidated = {row[1] for row in victims if row[1]}
    victim_hashes = {row[0] for row in victims}
    remaining = {row[1] for row in candidates if row[0] not in victim_hashes and row[1]}
    if (len(existing | invalidated) > MAX_INVALIDATED_SESSIONS
            or len(existing | invalidated | remaining | incoming_session_ids)
            > MAX_INVALIDATED_SESSIONS):
        raise ProtocolError("plugin source invalidation capacity reached")
    watermark = max(row[2] for row in victims)
    db.executemany("DELETE FROM plugin_evidence_v1 WHERE source_id=? AND event_id_hash=?",
                   ((source_id, row[0]) for row in victims))
    db.executemany("INSERT INTO plugin_retention_invalidated_v1 "
                   "(source_id,native_session_id,last_record_ns) VALUES(?,?,?) "
                   "ON CONFLICT(source_id,native_session_id) DO UPDATE SET "
                   "last_record_ns=MAX(last_record_ns,excluded.last_record_ns)",
                   ((source_id, row[1], row[3]) for row in victims if row[1]))
    db.execute("INSERT INTO plugin_retention_v1 "
               "(source_id,policy_epoch,watermark_ns,dropped_events) VALUES(?,?,?,?) "
               "ON CONFLICT(source_id) DO UPDATE SET "
               "watermark_ns=MAX(watermark_ns,excluded.watermark_ns),"
               "dropped_events=dropped_events+excluded.dropped_events,"
               "policy_epoch=excluded.policy_epoch",
               (source_id, policy_epoch, watermark, len(victims)))
    return len(victims)


def revoke(db: Connection, source_id: str) -> None:
    db.execute("DELETE FROM plugin_retention_invalidated_v1 WHERE source_id=?", (source_id,))
    db.execute("DELETE FROM plugin_retention_v1 WHERE source_id=?", (source_id,))
