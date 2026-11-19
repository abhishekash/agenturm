"""Atomic, metadata-only admission for an opt-in source plugin.

This is a durable inbox, not a projection into Agenturm's live views. The
receiver owns the source, cursor, lease and dirty sequence. A worker supplies
only bounded records and never writes the database itself.
"""
from __future__ import annotations

import hashlib
import re
import secrets
import time
from dataclasses import dataclass
from typing import TYPE_CHECKING

from .plugin_wire import PollBatch, ProtocolError, encode_message, validate_batch

if TYPE_CHECKING:
    from sqlite3 import Connection
    from .core import Store


_ID = re.compile(r"[a-zA-Z0-9][a-zA-Z0-9._:-]{0,255}\Z")
_HASH = re.compile(r"[0-9a-f]{64}\Z")
_CODE = re.compile(r"[a-z][a-z0-9_./-]{0,63}\Z")
_USAGE = frozenset({"input_tokens", "output_tokens", "cache_read_tokens",
                    "cache_write_tokens", "total_tokens"})
_STATES = frozenset({"started", "running", "paused", "completed", "failed",
                     "interrupted", "cancelled"})
_SEVERITIES = frozenset({"info", "warning", "error"})
MAX_SOURCE_RECORDS = 50_000


@dataclass(frozen=True)
class SourceLease:
    receiver_id: str
    plugin_id: str
    source_id: str
    generation: str
    epoch: int
    cursor: str
    policy_epoch: int


def initialize(db: Connection) -> None:
    db.executescript("""
        CREATE TABLE IF NOT EXISTS plugin_source_cursor_v1 (
            source_id TEXT PRIMARY KEY,
            receiver_id TEXT NOT NULL,
            plugin_id TEXT NOT NULL,
            generation TEXT NOT NULL,
            lease_epoch INTEGER NOT NULL,
            cursor TEXT NOT NULL DEFAULT '',
            prior_cursor TEXT NOT NULL DEFAULT '',
            last_batch_digest TEXT NOT NULL DEFAULT '',
            dirty_seq INTEGER NOT NULL DEFAULT 0,
            policy_epoch INTEGER NOT NULL DEFAULT 1,
            capture_enabled INTEGER NOT NULL DEFAULT 1,
            updated_ns INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS plugin_evidence_v1 (
            source_id TEXT NOT NULL,
            event_id_hash TEXT NOT NULL,
            revision INTEGER NOT NULL,
            kind TEXT NOT NULL,
            time_ns INTEGER NOT NULL,
            payload_json TEXT NOT NULL,
            record_digest TEXT NOT NULL,
            native_session_id TEXT NOT NULL DEFAULT '',
            native_worker_id TEXT NOT NULL DEFAULT '',
            native_turn_id TEXT NOT NULL DEFAULT '',
            policy_epoch INTEGER NOT NULL DEFAULT 1,
            admitted_ns INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(source_id,event_id_hash,revision),
            FOREIGN KEY(source_id) REFERENCES plugin_source_cursor_v1(source_id));
        CREATE TABLE IF NOT EXISTS plugin_evidence_dirty_v1 (
            source_id TEXT PRIMARY KEY,
            dirty_seq INTEGER NOT NULL,
            FOREIGN KEY(source_id) REFERENCES plugin_source_cursor_v1(source_id));
        CREATE TABLE IF NOT EXISTS plugin_session_projection_v1 (
            source_id TEXT NOT NULL,
            native_session_id TEXT NOT NULL,
            session_key TEXT NOT NULL,
            state TEXT NOT NULL,
            reason_code TEXT NOT NULL,
            state_time_ns INTEGER NOT NULL,
            last_record_ns INTEGER NOT NULL,
            policy_epoch INTEGER NOT NULL,
            source_dirty_seq INTEGER NOT NULL,
            usage_json TEXT NOT NULL DEFAULT '{}',
            PRIMARY KEY(source_id,native_session_id),
            FOREIGN KEY(source_id) REFERENCES plugin_source_cursor_v1(source_id));
        CREATE INDEX IF NOT EXISTS plugin_session_projection_recent_v1
            ON plugin_session_projection_v1(last_record_ns DESC,session_key);
        CREATE TABLE IF NOT EXISTS plugin_projection_cursor_v1 (
            source_id TEXT PRIMARY KEY,
            projected_seq INTEGER NOT NULL,
            policy_epoch INTEGER NOT NULL,
            FOREIGN KEY(source_id) REFERENCES plugin_source_cursor_v1(source_id));
    """)
    columns = {row[1] for row in db.execute("PRAGMA table_info(plugin_evidence_v1)")}
    for name, declaration in (("native_session_id", "TEXT NOT NULL DEFAULT ''"),
                              ("native_worker_id", "TEXT NOT NULL DEFAULT ''"),
                              ("native_turn_id", "TEXT NOT NULL DEFAULT ''"),
                              ("policy_epoch", "INTEGER NOT NULL DEFAULT 1"),
                              ("admitted_ns", "INTEGER NOT NULL DEFAULT 0")):
        if name not in columns:
            db.execute(f"ALTER TABLE plugin_evidence_v1 ADD COLUMN {name} {declaration}")
    cursor_columns = {row[1] for row in db.execute("PRAGMA table_info(plugin_source_cursor_v1)")}
    for name, declaration in (("policy_epoch", "INTEGER NOT NULL DEFAULT 1"),
                              ("capture_enabled", "INTEGER NOT NULL DEFAULT 1")):
        if name not in cursor_columns:
            db.execute(f"ALTER TABLE plugin_source_cursor_v1 ADD COLUMN {name} {declaration}")
    projection_columns = {row[1] for row in db.execute("PRAGMA table_info(plugin_session_projection_v1)")}
    if "usage_json" not in projection_columns:
        db.execute("ALTER TABLE plugin_session_projection_v1 ADD COLUMN usage_json TEXT NOT NULL DEFAULT '{}'")


def _identity(value: str, name: str) -> str:
    if not isinstance(value, str) or not _ID.fullmatch(value):
        raise ValueError(f"invalid {name}")
    return value


def activate_source(store: Store, *, plugin_id: str, source_id: str,
                    generation: str) -> SourceLease:
    """Issue a new fence for this exact artifact and return its durable cursor."""
    _identity(plugin_id, "plugin ID")
    _identity(source_id, "source ID")
    if not isinstance(generation, str) or not _HASH.fullmatch(generation):
        raise ValueError("generation must be the artifact SHA-256")
    epoch = secrets.randbelow(2**63 - 1) + 1
    receiver_id = store.receiver_namespace()
    with store._source_write(), store._connect() as db:
        db.execute("BEGIN IMMEDIATE")
        actual = db.execute("SELECT value FROM schema_meta WHERE key='receiver_id'").fetchone()
        if actual is None or actual[0] != receiver_id:
            raise ProtocolError("receiver identity changed during activation")
        row = db.execute("SELECT * FROM plugin_source_cursor_v1 WHERE source_id=?",
                         (source_id,)).fetchone()
        if row is not None and (row["receiver_id"], row["plugin_id"], row["generation"]) != (
                receiver_id, plugin_id, generation):
            raise ProtocolError("source ownership or artifact generation changed")
        if row is not None and not row["capture_enabled"]:
            raise ProtocolError("plugin source capture revoked")
        now = time.time_ns()
        if row is None:
            db.execute("INSERT INTO plugin_source_cursor_v1 "
                       "(source_id,receiver_id,plugin_id,generation,lease_epoch,updated_ns) "
                       "VALUES(?,?,?,?,?,?)",
                       (source_id, receiver_id, plugin_id, generation, epoch, now))
            cursor = ""
            policy_epoch = 1
        else:
            db.execute("UPDATE plugin_source_cursor_v1 SET lease_epoch=?,updated_ns=? "
                       "WHERE source_id=?", (epoch, now, source_id))
            cursor = row["cursor"]
            policy_epoch = row["policy_epoch"]
    return SourceLease(receiver_id, plugin_id, source_id, generation, epoch, cursor,
                       policy_epoch)


def set_source_capture(store: Store, *, source_id: str, enabled: bool) -> int:
    """Change host-owned capture policy and fence active workers atomically.

    Revocation erases the source's metadata evidence and invalidates downstream
    projections. The durable cursor remains, so re-enabling starts from the
    last acknowledged source position rather than silently replaying history.
    """
    _identity(source_id, "source ID")
    if type(enabled) is not bool:
        raise TypeError("enabled must be a boolean")
    receiver_id = store.receiver_namespace()
    with store._source_write(), store._connect() as db:
        db.execute("BEGIN IMMEDIATE")
        actual = db.execute("SELECT value FROM schema_meta WHERE key='receiver_id'").fetchone()
        if actual is None or actual[0] != receiver_id:
            raise ProtocolError("receiver identity changed during policy update")
        row = db.execute("SELECT * FROM plugin_source_cursor_v1 WHERE source_id=?",
                         (source_id,)).fetchone()
        if row is None or row["receiver_id"] != receiver_id:
            raise ProtocolError("unknown plugin source")
        if bool(row["capture_enabled"]) == enabled:
            return row["policy_epoch"]
        epoch = row["policy_epoch"] + 1
        lease_epoch = secrets.randbelow(2**63 - 1) + 1
        if not enabled:
            db.execute("DELETE FROM plugin_evidence_v1 WHERE source_id=?", (source_id,))
            db.execute("DELETE FROM plugin_session_projection_v1 WHERE source_id=?", (source_id,))
        dirty_seq = row["dirty_seq"] + 1
        db.execute("UPDATE plugin_source_cursor_v1 SET capture_enabled=?,policy_epoch=?,"
                   "lease_epoch=?,dirty_seq=?,updated_ns=? WHERE source_id=?",
                   (int(enabled), epoch, lease_epoch, dirty_seq, time.time_ns(), source_id))
        db.execute("INSERT INTO plugin_evidence_dirty_v1(source_id,dirty_seq) VALUES(?,?) "
                   "ON CONFLICT(source_id) DO UPDATE SET dirty_seq=excluded.dirty_seq",
                   (source_id, dirty_seq))
        db.execute("INSERT INTO plugin_projection_cursor_v1(source_id,projected_seq,policy_epoch) "
                   "VALUES(?,?,?) ON CONFLICT(source_id) DO UPDATE SET "
                   "projected_seq=excluded.projected_seq,policy_epoch=excluded.policy_epoch",
                   (source_id, dirty_seq, epoch))
    return epoch


def _metadata(record: dict) -> dict:
    """Allow only typed, coarse facts; no prompts, output, paths or free text."""
    kind = record["kind"]
    payload = record["payload"]
    if kind == "lifecycle":
        if (set(payload) not in ({"state"}, {"state", "reason_code"})
                or not isinstance(payload.get("state"), str)
                or payload["state"] not in _STATES):
            raise ProtocolError("invalid lifecycle metadata")
        if "reason_code" in payload and (not isinstance(payload["reason_code"], str)
                                         or not _CODE.fullmatch(payload["reason_code"])):
            raise ProtocolError("invalid lifecycle reason code")
    elif kind == "usage":
        if not payload or not set(payload) <= _USAGE or any(
                type(value) is not int or not 0 <= value <= 2**63 - 1
                for value in payload.values()):
            raise ProtocolError("invalid usage metadata")
    elif kind == "diagnostic":
        if (set(payload) != {"code", "severity"}
                or not isinstance(payload.get("code"), str)
                or not _CODE.fullmatch(payload["code"])
                or not isinstance(payload.get("severity"), str)
                or payload["severity"] not in _SEVERITIES):
            raise ProtocolError("invalid diagnostic metadata")
    else:
        raise ProtocolError("record kind has no metadata admission policy")
    return payload


def _validated(batch: PollBatch, source_id: str) -> tuple[str, tuple[tuple, ...]]:
    if not isinstance(batch, PollBatch):
        raise TypeError("validated plugin batch required")
    # Revalidate because PollBatch contains mutable dictionaries and callers can
    # construct it directly. This also bounds the whole batch before SQL writes.
    validated = validate_batch({"type": "batch", "protocol": 1,
                                "source_id": batch.source_id, "cursor": batch.cursor,
                                "records": list(batch.records)}, source_id=source_id)
    rows = []
    for record in validated.records:
        payload = _metadata(record)
        canonical = encode_message(record)
        identity = record.get("identity") or {}
        rows.append((source_id,
                     hashlib.sha256((source_id + "\0" + record["event_id"]).encode()).hexdigest(),
                     record["revision"], record["kind"], int(record["time_ns"]),
                     encode_message(payload), hashlib.sha256(canonical.encode()).hexdigest(),
                     identity.get("session_id", ""), identity.get("worker_id", ""),
                     identity.get("turn_id", "")))
    batch_digest = hashlib.sha256(encode_message({"cursor": validated.cursor,
                                                  "records": list(validated.records)}).encode()).hexdigest()
    return batch_digest, tuple(rows)


def commit_batch(store: Store, lease: SourceLease, expected_cursor: str,
                 batch: PollBatch) -> str:
    """Commit evidence, cursor and dirty marker together, then permit ack.

    Exact replay after a crash is accepted. Any different batch at the same
    cursor, stale lease, source conflict or changed event revision is rejected.
    """
    if not isinstance(lease, SourceLease):
        raise TypeError("source lease required")
    if not isinstance(expected_cursor, str):
        raise ValueError("expected cursor must be text")
    digest, rows = _validated(batch, lease.source_id)
    with store._source_write(), store._connect() as db:
        db.execute("BEGIN IMMEDIATE")
        receiver = db.execute("SELECT value FROM schema_meta WHERE key='receiver_id'").fetchone()
        row = db.execute("SELECT * FROM plugin_source_cursor_v1 WHERE source_id=?",
                         (lease.source_id,)).fetchone()
        if (receiver is None or receiver[0] != lease.receiver_id or row is None
                or not row["capture_enabled"]
                or (row["receiver_id"], row["plugin_id"], row["generation"],
                    row["lease_epoch"], row["policy_epoch"]) !=
                (lease.receiver_id, lease.plugin_id, lease.generation, lease.epoch,
                 lease.policy_epoch)):
            raise ProtocolError("stale plugin source lease")
        if not rows and batch.cursor == row["cursor"] and expected_cursor == row["cursor"]:
            # An idle poll has no durable effect. It still follows lease and
            # source checks before the host may acknowledge it.
            return row["cursor"]
        if batch.cursor == row["cursor"] and digest == row["last_batch_digest"] and expected_cursor in (
                row["prior_cursor"], row["cursor"]):
            return row["cursor"]
        if expected_cursor != row["cursor"]:
            raise ProtocolError("stale plugin cursor")
        if batch.cursor == expected_cursor:
            raise ProtocolError("plugin cursor did not advance")
        count = db.execute("SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id=?",
                           (lease.source_id,)).fetchone()[0]
        admitted_ns = time.time_ns()
        inserted = False
        for item in rows:
            latest = db.execute("SELECT revision,record_digest,kind,native_session_id,"
                                "native_worker_id,native_turn_id FROM plugin_evidence_v1 "
                                "WHERE source_id=? AND event_id_hash=? "
                                "ORDER BY revision DESC LIMIT 1", item[:2]).fetchone()
            if latest is not None:
                if tuple(latest[2:]) != (item[3], item[7], item[8], item[9]):
                    raise ProtocolError("plugin event identity or kind changed")
                if item[2] < latest[0]:
                    raise ProtocolError("stale plugin event revision")
                if item[2] == latest[0]:
                    if latest[1] != item[6]:
                        raise ProtocolError("conflicting plugin event revision")
                    continue
                # Only the latest revision participates in projection and
                # future monotonic admission. Replace obsolete rows inside
                # this same cursor transaction, freeing capacity for a
                # correction even when the source is otherwise at its cap.
                deleted = db.execute("DELETE FROM plugin_evidence_v1 WHERE source_id=? "
                                     "AND event_id_hash=? AND revision<?",
                                     (*item[:2], item[2])).rowcount
                count -= deleted
            if count >= MAX_SOURCE_RECORDS:
                raise ProtocolError("plugin source evidence capacity reached")
            db.execute("INSERT INTO plugin_evidence_v1 "
                       "(source_id,event_id_hash,revision,kind,time_ns,payload_json,record_digest,"
                       "native_session_id,native_worker_id,native_turn_id,policy_epoch,admitted_ns) "
                       "VALUES(?,?,?,?,?,?,?,?,?,?,?,?)", (*item, lease.policy_epoch, admitted_ns))
            count += 1
            inserted = True
        seq = row["dirty_seq"] + int(inserted)
        db.execute("UPDATE plugin_source_cursor_v1 SET cursor=?,prior_cursor=?,last_batch_digest=?,"
                   "dirty_seq=?,updated_ns=? WHERE source_id=?",
                   (batch.cursor, expected_cursor, digest, seq, time.time_ns(), lease.source_id))
        if inserted:
            db.execute("INSERT INTO plugin_evidence_dirty_v1(source_id,dirty_seq) VALUES(?,?) "
                       "ON CONFLICT(source_id) DO UPDATE SET dirty_seq=excluded.dirty_seq",
                       (lease.source_id, seq))
    return batch.cursor
