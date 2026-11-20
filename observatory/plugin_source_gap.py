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
                        incoming_session_ids: set[str], *, member_id: str = "") -> None:
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
    # A member-scoped gap is safe only when current evidence has durable,
    # matching ownership. Earlier unowned rows and a still-authoritative
    # projection without evidence force the old whole-source disposition.
    # Retention tombstones are already Unknown even after their evidence is
    # pruned; their member bindings are retained if they were admitted here.
    scoped = bool(member_id) and db.execute(
        "SELECT 1 FROM plugin_evidence_v1 e LEFT JOIN plugin_session_member_v1 m "
        "ON m.source_id=e.source_id AND m.native_session_id=e.native_session_id "
        "WHERE e.source_id=? AND e.native_session_id!='' "
        "AND (e.member_id='' OR m.member_id IS NULL OR m.member_id='' "
        "OR m.member_id!=e.member_id) LIMIT 1",
        (source_id,)).fetchone() is None and db.execute(
        "SELECT 1 FROM plugin_session_projection_v1 p LEFT JOIN plugin_session_member_v1 m "
        "ON m.source_id=p.source_id AND m.native_session_id=p.native_session_id "
        "WHERE p.source_id=? AND p.reason_code NOT IN ('retention_gap','source_gap') "
        "AND (m.member_id IS NULL OR m.member_id='') LIMIT 1",
        (source_id,)).fetchone() is None and db.execute(
        "SELECT 1 FROM plugin_session_member_v1 WHERE source_id=? "
        "AND member_id='' LIMIT 1",
        (source_id,)).fetchone() is None
    if scoped:
        rows = db.execute(
            "SELECT native_session_id,MAX(record_ns) FROM ("
            "SELECT native_session_id,admitted_ns AS record_ns FROM plugin_evidence_v1 "
            "WHERE source_id=? AND member_id=? AND native_session_id!='' "
            "UNION ALL SELECT p.native_session_id,p.last_record_ns AS record_ns "
            "FROM plugin_session_projection_v1 p JOIN plugin_session_member_v1 m "
            "ON m.source_id=p.source_id AND m.native_session_id=p.native_session_id "
            "WHERE p.source_id=? AND m.member_id=? "
            "AND p.reason_code NOT IN ('retention_gap','source_gap')) "
            "GROUP BY native_session_id LIMIT ?",
            (source_id, member_id, source_id, member_id,
             MAX_INVALIDATED_SESSIONS + 1)).fetchall()
    else:
        rows = db.execute(
            "SELECT native_session_id,MAX(record_ns) FROM ("
            "SELECT native_session_id,admitted_ns AS record_ns FROM plugin_evidence_v1 "
            "WHERE source_id=? AND native_session_id!='' "
            "UNION ALL SELECT native_session_id,last_record_ns AS record_ns "
            "FROM plugin_session_projection_v1 WHERE source_id=? "
            "AND reason_code NOT IN ('retention_gap','source_gap')) "
            "GROUP BY native_session_id LIMIT ?",
            (source_id, source_id, MAX_INVALIDATED_SESSIONS + 1)).fetchall()
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
