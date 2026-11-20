"""Durable, source-local invalidation after an admitted source continuity gap.

A gap is an observation of missing or rewritten input, not proof that the
remaining terminal facts still describe the session. The receiver captures
affected native session IDs in the same transaction as the cursor advance.
"""
from __future__ import annotations

from sqlite3 import Connection

from .plugin_retention import MAX_INVALIDATED_SESSIONS
from .plugin_wire import ProtocolError

_LEGACY_ALL = "*"


def initialize(db: Connection) -> None:
    db.execute("CREATE TABLE IF NOT EXISTS plugin_source_gap_invalidated_v1 ("
               "source_id TEXT NOT NULL,native_session_id TEXT NOT NULL,"
               "last_record_ns INTEGER NOT NULL,"
               "PRIMARY KEY(source_id,native_session_id),"
               "FOREIGN KEY(source_id) REFERENCES plugin_source_cursor_v1(source_id))")
    # Old receivers retained a diagnostic but continued projecting Completed.
    # Their exact pre-gap membership was not durably recorded. Quarantine the
    # whole source until explicit capture reset rather than guessing a subset.
    if (db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_meta'").fetchone()
            and db.execute("SELECT 1 FROM schema_meta WHERE key='plugin_source_gap_v1'").fetchone() is None):
        db.execute("INSERT OR IGNORE INTO plugin_source_gap_invalidated_v1 "
                   "(source_id,native_session_id,last_record_ns) "
                   "SELECT source_id,?,0 FROM plugin_evidence_v1 "
                   "WHERE kind='diagnostic' AND "
                   "json_extract(payload_json,'$.code')='possible_source_gap' "
                   "GROUP BY source_id", (_LEGACY_ALL,))
        db.execute("DELETE FROM plugin_projection_cursor_v1 WHERE source_id IN "
                   "(SELECT source_id FROM plugin_source_gap_invalidated_v1)")
        db.execute("INSERT INTO schema_meta(key,value) VALUES('plugin_source_gap_v1','1')")


def invalidate_existing(db: Connection, source_id: str,
                        incoming_session_ids: set[str]) -> None:
    """Snapshot already admitted sessions before the new gap record is written.

    The caller owns BEGIN IMMEDIATE and the cursor transaction. Evidence
    inserted before this gap in the same batch is included; evidence inserted
    afterward is not. Any failure rolls back both invalidation and cursor.
    """
    existing = {row[0] for row in db.execute(
        "SELECT native_session_id FROM plugin_source_gap_invalidated_v1 WHERE source_id=?",
        (source_id,))}
    if _LEGACY_ALL in existing:
        return
    rows = db.execute(
        "SELECT native_session_id,MAX(admitted_ns) FROM plugin_evidence_v1 "
        "WHERE source_id=? AND native_session_id!='' "
        "GROUP BY native_session_id LIMIT ?",
        (source_id, MAX_INVALIDATED_SESSIONS + 1)).fetchall()
    retained = {row[0] for row in db.execute(
        "SELECT native_session_id FROM plugin_retention_invalidated_v1 WHERE source_id=?",
        (source_id,))}
    if len(existing | retained | {row[0] for row in rows} | incoming_session_ids) > MAX_INVALIDATED_SESSIONS:
        raise ProtocolError("plugin source gap invalidation capacity reached")
    db.executemany("INSERT INTO plugin_source_gap_invalidated_v1 "
                   "(source_id,native_session_id,last_record_ns) VALUES(?,?,?) "
                   "ON CONFLICT(source_id,native_session_id) DO UPDATE SET "
                   "last_record_ns=MAX(last_record_ns,excluded.last_record_ns)",
                   ((source_id, row[0], row[1]) for row in rows))


def revoke(db: Connection, source_id: str) -> None:
    db.execute("DELETE FROM plugin_source_gap_invalidated_v1 WHERE source_id=?", (source_id,))
