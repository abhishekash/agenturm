"""Atomic, metadata-only admission for an opt-in source plugin.

This is a durable inbox, not a projection into Agenturm's live views. The
receiver owns the source, cursor, lease and dirty sequence. A worker supplies
only bounded records and never writes the database itself.
"""
from __future__ import annotations

import hashlib
import json
import re
import secrets
import time
from dataclasses import dataclass
from typing import TYPE_CHECKING

from .plugin_wire import (MAX_CURSOR_CHARS, MAX_DISCOVERY_MEMBERS, MEMBER_ID, PollBatch,
                          ProtocolError, encode_message, validate_batch)
from . import plugin_retention, plugin_source_gap
from .plugin_activity import validate_metadata as validate_activity_metadata

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
_RELATIONSHIPS = frozenset({"delegated", "review"})
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
        CREATE TABLE IF NOT EXISTS plugin_source_member_cursor_v1 (
            source_id TEXT NOT NULL,
            member_id TEXT NOT NULL,
            cursor TEXT NOT NULL,
            PRIMARY KEY(source_id,member_id),
            FOREIGN KEY(source_id) REFERENCES plugin_source_cursor_v1(source_id));
        CREATE TABLE IF NOT EXISTS plugin_session_member_v1 (
            source_id TEXT NOT NULL,
            native_session_id TEXT NOT NULL,
            member_id TEXT NOT NULL,
            PRIMARY KEY(source_id,native_session_id),
            FOREIGN KEY(source_id) REFERENCES plugin_source_cursor_v1(source_id));
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
            native_operation_id TEXT NOT NULL DEFAULT '',
            member_id TEXT NOT NULL DEFAULT '',
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
            parent_native_session_id TEXT NOT NULL DEFAULT '',
            relationship_kind TEXT NOT NULL DEFAULT 'unknown',
            relationship_state TEXT NOT NULL DEFAULT 'unresolved',
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
                              ("native_operation_id", "TEXT NOT NULL DEFAULT ''"),
                              ("member_id", "TEXT NOT NULL DEFAULT ''"),
                              ("policy_epoch", "INTEGER NOT NULL DEFAULT 1"),
                              ("admitted_ns", "INTEGER NOT NULL DEFAULT 0")):
        if name not in columns:
            db.execute(f"ALTER TABLE plugin_evidence_v1 ADD COLUMN {name} {declaration}")
    db.execute("CREATE INDEX IF NOT EXISTS plugin_evidence_diagnostic_recent_v1 "
               "ON plugin_evidence_v1(source_id,policy_epoch,admitted_ns DESC) "
               "WHERE kind='diagnostic'")
    cursor_columns = {row[1] for row in db.execute("PRAGMA table_info(plugin_source_cursor_v1)")}
    for name, declaration in (("policy_epoch", "INTEGER NOT NULL DEFAULT 1"),
                              ("capture_enabled", "INTEGER NOT NULL DEFAULT 1")):
        if name not in cursor_columns:
            db.execute(f"ALTER TABLE plugin_source_cursor_v1 ADD COLUMN {name} {declaration}")
    projection_columns = {row[1] for row in db.execute("PRAGMA table_info(plugin_session_projection_v1)")}
    if "usage_json" not in projection_columns:
        db.execute("ALTER TABLE plugin_session_projection_v1 ADD COLUMN usage_json TEXT NOT NULL DEFAULT '{}'")
    for name, declaration in (("parent_native_session_id", "TEXT NOT NULL DEFAULT ''"),
                              ("relationship_kind", "TEXT NOT NULL DEFAULT 'unknown'"),
                              ("relationship_state", "TEXT NOT NULL DEFAULT 'unresolved'")):
        if name not in projection_columns:
            db.execute(f"ALTER TABLE plugin_session_projection_v1 ADD COLUMN {name} {declaration}")
    plugin_retention.initialize(db)
    plugin_source_gap.initialize(db)
    # A pre-activity plugin projection cannot certify the new Activity rows.
    # The worker's startup projection pass republishes both from one snapshot.
    if (db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_meta'").fetchone()
            and db.execute("SELECT 1 FROM schema_meta WHERE key='plugin_activity_projection_v1'")
            .fetchone() is None):
        db.execute("DELETE FROM plugin_projection_cursor_v1")
        db.execute("INSERT INTO schema_meta(key,value) VALUES('plugin_activity_projection_v1','1')")
    # Earlier Activity projections did not carry an operation correlation key.
    # Hide them until the policy-current source snapshot republishes both views.
    if (db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_meta'").fetchone()
            and db.execute("SELECT 1 FROM schema_meta WHERE key='plugin_activity_projection_v2'")
            .fetchone() is None):
        db.execute("DELETE FROM plugin_projection_cursor_v1")
        db.execute("INSERT INTO schema_meta(key,value) VALUES('plugin_activity_projection_v2','1')")


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
            db.execute("DELETE FROM activity_index_v1 WHERE kind='plugin' AND owner_key=?",
                       (source_id,))
            db.execute("DELETE FROM investigation_dirty_v1 WHERE kind='plugin' AND owner_key=?",
                       (source_id,))
            plugin_retention.revoke(db, source_id)
            plugin_source_gap.revoke(db, source_id)
            db.execute("DELETE FROM plugin_session_member_v1 WHERE source_id=?", (source_id,))
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


def last_source_diagnostic(store: Store, lease: SourceLease) -> dict:
    """Load the latest policy-current coarse diagnostic after a worker restart."""
    with store._connect() as db:
        row = db.execute("SELECT e.payload_json,e.time_ns FROM plugin_evidence_v1 e "
                         "JOIN plugin_source_cursor_v1 c USING(source_id) "
                         "WHERE e.source_id=? AND e.kind='diagnostic' "
                         "AND e.policy_epoch=? AND c.policy_epoch=? "
                         "AND c.receiver_id=? AND c.capture_enabled=1 "
                         "ORDER BY e.admitted_ns DESC,e.rowid DESC LIMIT 1",
                         (lease.source_id, lease.policy_epoch, lease.policy_epoch,
                          lease.receiver_id)).fetchone()
    return ({**json.loads(row["payload_json"]), "time_ns": str(row["time_ns"])}
            if row is not None else {})


def member_cursors(store: Store, lease: SourceLease) -> dict[str, str]:
    """Return a bounded snapshot of opaque per-file checkpoints for one lease."""
    with store._connect() as db:
        row = db.execute("SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,"
                         "capture_enabled FROM plugin_source_cursor_v1 WHERE source_id=?",
                         (lease.source_id,)).fetchone()
        if (row is None or not row["capture_enabled"] or tuple(row[:5]) !=
                (lease.receiver_id, lease.plugin_id, lease.generation,
                 lease.epoch, lease.policy_epoch)):
            raise ProtocolError("stale plugin source lease")
        invalid = db.execute(
            "SELECT 1 FROM plugin_source_member_cursor_v1 WHERE source_id=? AND "
            "(length(CAST(member_id AS BLOB))>64 OR "
            "length(CAST(cursor AS BLOB))>?) LIMIT 1",
            (lease.source_id, MAX_CURSOR_CHARS)).fetchone()
        if invalid is not None:
            raise ProtocolError("discovery checkpoint limit exceeded")
        rows = db.execute(
            "SELECT member_id,cursor FROM plugin_source_member_cursor_v1 "
            "WHERE source_id=? ORDER BY member_id LIMIT ?",
            (lease.source_id, MAX_DISCOVERY_MEMBERS + 1)).fetchall()
    if (len(rows) > MAX_DISCOVERY_MEMBERS or any(
            not isinstance(member, str) or not MEMBER_ID.fullmatch(member)
            or not isinstance(cursor, str) or len(cursor) > MAX_CURSOR_CHARS
            or any(ord(char) < 32 or ord(char) == 127 for char in cursor)
            for member, cursor in rows)):
        raise ProtocolError("discovery checkpoint limit exceeded")
    return dict(rows)


def _metadata(record: dict) -> dict:
    """Allow only typed, coarse facts; no prompts, output, paths or free text."""
    kind = record["kind"]
    payload = record["payload"]
    operation_id = record.get("identity", {}).get("operation_id")
    if operation_id and (kind != "tool" or not _ID.fullmatch(operation_id)):
        raise ProtocolError("invalid plugin tool operation identity")
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
        if payload["code"] == "possible_source_gap" and (
                record["revision"] != 0 or record.get("identity")):
            raise ProtocolError("source gap must be a new source-wide diagnostic")
    elif kind == "relationship":
        if (set(payload) != {"parent_session_id", "kind"}
                or not isinstance(payload.get("parent_session_id"), str)
                or not _ID.fullmatch(payload["parent_session_id"])
                or not isinstance(payload.get("kind"), str)
                or payload["kind"] not in _RELATIONSHIPS
                or not record.get("identity", {}).get("session_id")):
            raise ProtocolError("invalid relationship metadata")
    elif kind in {"message", "tool", "check"}:
        return validate_activity_metadata(record)
    else:
        raise ProtocolError("record kind has no metadata admission policy")
    return payload


def _validated(batch: PollBatch, source_id: str) -> tuple[str, tuple[tuple, ...]]:
    if not isinstance(batch, PollBatch):
        raise TypeError("validated plugin batch required")
    # Revalidate because PollBatch contains mutable dictionaries and callers can
    # construct it directly. This also bounds the whole batch before SQL writes.
    wire = {"type": "batch", "protocol": 1,
            "source_id": batch.source_id, "cursor": batch.cursor,
            "records": list(batch.records)}
    if batch.member_cursor and not batch.member_id:
        raise ProtocolError("discovery cursor lacks member identity")
    if batch.member_id:
        wire.update(member_id=batch.member_id, member_cursor=batch.member_cursor)
    validated = validate_batch(wire, source_id=source_id)
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
                     identity.get("turn_id", ""), identity.get("operation_id", ""),
                     validated.member_id))
    digest_input = {"cursor": validated.cursor, "records": list(validated.records)}
    if validated.member_id:
        digest_input.update(member_id=validated.member_id,
                            member_cursor=validated.member_cursor)
    batch_digest = hashlib.sha256(encode_message(digest_input).encode()).hexdigest()
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
    protected_hashes = {item[1] for item in rows}
    incoming_session_ids = {item[7] for item in rows if item[7]}
    gap_hashes = {item[1] for item in rows if item[3] == "diagnostic"
                  and json.loads(item[5])["code"] == "possible_source_gap"}
    if batch.member_id and not batch.member_cursor and not gap_hashes:
        raise ProtocolError("removed discovery member requires source-gap evidence")
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
        if (not rows and not batch.member_id and batch.cursor == row["cursor"]
                and expected_cursor == row["cursor"]):
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
        ambiguous_ids: set[str] = set()
        if batch.member_id:
            old_member = db.execute("SELECT cursor FROM plugin_source_member_cursor_v1 "
                                    "WHERE source_id=? AND member_id=?",
                                    (lease.source_id, batch.member_id)).fetchone()
            if not batch.member_cursor and old_member is None:
                raise ProtocolError("unknown discovery member removal")
            if old_member is None and batch.member_cursor:
                count_members = db.execute("SELECT COUNT(*) FROM plugin_source_member_cursor_v1 "
                                           "WHERE source_id=?", (lease.source_id,)).fetchone()[0]
                if count_members >= MAX_DISCOVERY_MEMBERS:
                    raise ProtocolError("discovery member limit exceeded")
        if batch.member_id:
            mapped_count = db.execute("SELECT COUNT(*) FROM plugin_session_member_v1 "
                                      "WHERE source_id=?", (lease.source_id,)).fetchone()[0]
            for native_id in sorted(incoming_session_ids):
                mapped = db.execute("SELECT member_id FROM plugin_session_member_v1 "
                                    "WHERE source_id=? AND native_session_id=?",
                                    (lease.source_id, native_id)).fetchone()
                if mapped is not None:
                    if mapped[0] == "":
                        ambiguous_ids.add(native_id)
                    elif mapped[0] != batch.member_id:
                        raise ProtocolError("plugin session changed discovery member")
                    continue
                # A historic identity cannot silently acquire a new member
                # after its evidence was pruned or its old source reconfigured.
                legacy = db.execute(
                    "SELECT 1 FROM plugin_evidence_v1 WHERE source_id=? "
                    "AND native_session_id=? LIMIT 1",
                    (lease.source_id, native_id)).fetchone()
                retired = db.execute(
                    "SELECT 1 FROM plugin_retention_invalidated_v1 WHERE source_id=? "
                    "AND native_session_id=? LIMIT 1",
                    (lease.source_id, native_id)).fetchone()
                gapped = db.execute(
                    "SELECT 1 FROM plugin_source_gap_invalidated_v1 WHERE source_id=? "
                    "AND native_session_id=? LIMIT 1",
                    (lease.source_id, native_id)).fetchone()
                if mapped_count >= plugin_retention.MAX_INVALIDATED_SESSIONS:
                    raise ProtocolError("plugin session member capacity reached")
                owner = "" if legacy or retired or gapped else batch.member_id
                db.execute("INSERT INTO plugin_session_member_v1 "
                           "(source_id,native_session_id,member_id) VALUES(?,?,?)",
                           (lease.source_id, native_id, owner))
                if not owner:
                    ambiguous_ids.add(native_id)
                mapped_count += 1
        else:
            for native_id in incoming_session_ids:
                mapped = db.execute("SELECT member_id FROM plugin_session_member_v1 "
                                    "WHERE source_id=? AND native_session_id=?",
                                    (lease.source_id, native_id)).fetchone()
                if mapped is not None and mapped[0]:
                    raise ProtocolError("plugin session changed discovery member")
        count = db.execute("SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id=?",
                           (lease.source_id,)).fetchone()[0]
        admitted_ns = time.time_ns()
        inserted = False
        for raw_item in rows:
            # An upgraded directory source may keep reporting an older native
            # session. Preserve that session's unowned provenance on every
            # revision; assigning its new facts to this file would silently
            # turn a later member gap into a false exact-scope claim.
            item = (*raw_item[:11], "") if raw_item[7] in ambiguous_ids else raw_item
            latest = db.execute("SELECT revision,record_digest,kind,native_session_id,"
                                "native_worker_id,native_turn_id,native_operation_id,member_id "
                                "FROM plugin_evidence_v1 "
                                "WHERE source_id=? AND event_id_hash=? "
                                "ORDER BY revision DESC LIMIT 1", item[:2]).fetchone()
            if item[1] in gap_hashes and latest is not None:
                # A prior possible-gap record cannot invalidate sessions
                # admitted after that earlier gap. Exact replay returned above.
                raise ProtocolError("source-gap evidence must be new")
            if latest is not None:
                if tuple(latest[2:]) != (item[3], item[7], item[8], item[9], item[10], item[11]):
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
            else:
                if item[1] in gap_hashes:
                    plugin_source_gap.invalidate_existing(db, lease.source_id,
                                                          incoming_session_ids,
                                                          member_id=batch.member_id)
                else:
                    plugin_retention.reject_unseen(db, lease.source_id,
                                                   revision=item[2], time_ns=item[4])
            if count >= MAX_SOURCE_RECORDS:
                count -= plugin_retention.prune(db, lease.source_id,
                                                policy_epoch=lease.policy_epoch,
                                                protected_hashes=protected_hashes,
                                                incoming_session_ids=incoming_session_ids,
                                                max_records=MAX_SOURCE_RECORDS)
                # A newly established watermark may fence this very batch.
                if latest is None and item[1] not in gap_hashes:
                    plugin_retention.reject_unseen(db, lease.source_id,
                                                   revision=item[2], time_ns=item[4])
            db.execute("INSERT INTO plugin_evidence_v1 "
                       "(source_id,event_id_hash,revision,kind,time_ns,payload_json,record_digest,"
                       "native_session_id,native_worker_id,native_turn_id,native_operation_id,member_id,"
                       "policy_epoch,admitted_ns) "
                       "VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)", (*item, lease.policy_epoch, admitted_ns))
            count += 1
            inserted = True
        seq = row["dirty_seq"] + int(inserted)
        db.execute("UPDATE plugin_source_cursor_v1 SET cursor=?,prior_cursor=?,last_batch_digest=?,"
                   "dirty_seq=?,updated_ns=? WHERE source_id=?",
                   (batch.cursor, expected_cursor, digest, seq, time.time_ns(), lease.source_id))
        if batch.member_id:
            if batch.member_cursor:
                db.execute("INSERT INTO plugin_source_member_cursor_v1(source_id,member_id,cursor) "
                           "VALUES(?,?,?) ON CONFLICT(source_id,member_id) DO UPDATE SET "
                           "cursor=excluded.cursor",
                           (lease.source_id, batch.member_id, batch.member_cursor))
            else:
                db.execute("DELETE FROM plugin_source_member_cursor_v1 "
                           "WHERE source_id=? AND member_id=?",
                           (lease.source_id, batch.member_id))
        if inserted:
            db.execute("INSERT INTO plugin_evidence_dirty_v1(source_id,dirty_seq) VALUES(?,?) "
                       "ON CONFLICT(source_id) DO UPDATE SET dirty_seq=excluded.dirty_seq",
                       (lease.source_id, seq))
    return batch.cursor
