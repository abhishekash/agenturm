"""Internal, fail-closed Pi user-text read. No HTTP or UI caller is wired here.

Only the receiver's active supervisor supplies the configured root. Short
Store snapshots fence policy and row changes around source-file I/O.
"""
from __future__ import annotations

import hashlib
import json
import os
import sqlite3
import stat
from dataclasses import dataclass

from .local_activity import plaintext
from .plugin_activity import _EVENT_KEY
from .plugin_pi_content import (MAX_CONTENT_BYTES, MAX_CONTENT_ROWS, MAX_PREFIX_BYTES, MAX_ROW_BYTES,
                                PiContentGrant, _cursor, _json_row, _open_no_follow,
                                _open_root_no_follow, _path, _prefix, _time_ns)
from .plugin_supervisor import PluginSupervisor
from .plugin_wire import ProtocolError
from .rust_shadow_snapshot import _store_file_identity

MAX_DIRS = 256
MAX_ENTRIES = 4096
MAX_FILES = 48
MAX_DEPTH = 6


@dataclass(frozen=True)
class ContentProof:
    """Point-in-time binding; a later consumer must check again at delivery."""

    receiver_id: str
    source_id: str
    activity_id: str
    event_id_hash: str
    policy_epoch: int
    lease_epoch: int
    source_dirty_seq: int
    content_admitted_seq: int
    member_id: str
    root_dev: int
    root_ino: int
    file_dev: int
    file_ino: int
    row_sha256: str


@dataclass(frozen=True)
class Available:
    redacted_text: str
    proof: ContentProof


@dataclass(frozen=True)
class Unavailable:
    reason: str


class _Closed(Exception):
    pass


def _require(condition: bool, reason: str) -> None:
    if not condition:
        raise _Closed(reason)


def _one(db, sql: str, args: tuple = ()) -> tuple | None:
    row = db.execute(sql, args).fetchone()
    return tuple(row) if row is not None else None


def _snapshot(store, activity_id: str, supervisor: PluginSupervisor):
    """Materialize exact selected rows in one short read transaction."""
    with store._read() as db:
        db.execute("BEGIN")
        activity = _one(db, "SELECT * FROM activity_index_v1 WHERE event_key=? AND kind='plugin'",
                        (activity_id,))
        _require(activity is not None, "activity_unavailable")
        # event_key,kind,owner_key,...,payload are stable in the index schema.
        owner = activity[2]
        configured = [item for item in supervisor.sources if item.source_id == owner]
        _require(len(configured) == 1, "source_unavailable")
        source = configured[0]
        cursor = _one(db, "SELECT * FROM plugin_source_cursor_v1 WHERE source_id=?", (owner,))
        projection = _one(db, "SELECT * FROM plugin_projection_cursor_v1 WHERE source_id=?", (owner,))
        receiver = _one(db, "SELECT value FROM schema_meta WHERE key='receiver_id'")
        _require(cursor is not None and projection is not None and receiver is not None,
                 "source_unavailable")
        names = [row[1] for row in db.execute("PRAGMA table_info(plugin_source_cursor_v1)")]
        current = dict(zip(names, cursor))
        _require(current["capture_enabled"] == 1 and current["receiver_id"] == receiver[0]
                 and current["plugin_id"] == source.manifest.id
                 and current["generation"] == source.manifest.artifact_sha256
                 and current["content_fingerprint"]
                 and current["dirty_seq"] > 0
                 and projection[1:] == (current["dirty_seq"], current["policy_epoch"]),
                 "source_not_current")
        grant = PiContentGrant.binding_for_source(source)
        _require(grant[0] == current["content_fingerprint"], "configuration_changed")
        # A source has at most 256 retained content rows. Ambiguous/over-budget
        # selection closes rather than trusting a caller-supplied event hash.
        content_size = _one(db, "SELECT COUNT(*),COALESCE(SUM(length(CAST(redacted_text AS BLOB))),0) "
                            "FROM plugin_content_v1 WHERE source_id=?", (owner,))
        _require(content_size is not None and content_size[0] <= MAX_CONTENT_ROWS and
                 content_size[1] <= MAX_CONTENT_BYTES, "content_budget")
        content_rows = db.execute("SELECT * FROM plugin_content_v1 WHERE source_id=? "
                                  "ORDER BY event_id_hash,revision LIMIT ?",
                                  (owner, MAX_CONTENT_ROWS + 1)).fetchall()
        _require(len(content_rows) <= MAX_CONTENT_ROWS, "content_budget")
        matching = [tuple(row) for row in content_rows if
                    "plugin-event:" + hashlib.sha256((receiver[0] + "\0" + owner + "\0" +
                    row[1]).encode()).hexdigest() == activity_id]
        _require(len(matching) == 1, "content_unavailable")
        content = matching[0]
        event_hash, revision, kind, session_id, turn_id, member_id = content[1:7]
        _require(revision == 0 and kind == "pi.user_text" and
                 content[13] == current["policy_epoch"] and
                 type(content[14]) is int and 0 < content[14] < 2**63 and
                 type(content[15]) is int and 0 < content[15] <= current["dirty_seq"] and
                 isinstance(content[12], str), "content_not_current")
        evidence = _one(db, "SELECT * FROM plugin_evidence_v1 WHERE source_id=? AND "
                        "event_id_hash=? AND revision=0", (owner, event_hash))
        latest = _one(db, "SELECT MAX(revision) FROM plugin_evidence_v1 WHERE source_id=? "
                      "AND event_id_hash=?", (owner, event_hash))
        _require(evidence is not None and latest == (0,), "evidence_changed")
        evidence_names = [row[1] for row in db.execute("PRAGMA table_info(plugin_evidence_v1)")]
        fact = dict(zip(evidence_names, evidence))
        _require(fact["kind"] == "message" and fact["native_session_id"] == session_id
                 and fact["native_turn_id"] == turn_id and fact["member_id"] == member_id
                 and fact["policy_epoch"] == current["policy_epoch"]
                 and json.loads(fact["payload_json"]) == {"role": "user"},
                 "evidence_changed")
        member = _one(db, "SELECT member_id FROM plugin_session_member_v1 WHERE source_id=? "
                      "AND native_session_id=?", (owner, session_id))
        session = _one(db, "SELECT session_key,reason_code,policy_epoch,source_dirty_seq "
                       "FROM plugin_session_projection_v1 WHERE source_id=? "
                       "AND native_session_id=?", (owner, session_id))
        checkpoint = _one(db, "SELECT cursor FROM plugin_source_member_cursor_v1 "
                          "WHERE source_id=? AND member_id=?", (owner, member_id))
        gap = _one(db, "SELECT 1 FROM plugin_source_gap_invalidated_v1 WHERE source_id=? "
                   "AND native_session_id IN ('*',?) LIMIT 1", (owner, session_id))
        retention = _one(db, "SELECT 1 FROM plugin_retention_invalidated_v1 WHERE source_id=? "
                         "AND native_session_id=?", (owner, session_id))
        _require(member == (member_id,) and session is not None and
                 session[1] not in ("source_gap", "retention_gap") and
                 session[2:] == (current["policy_epoch"], current["dirty_seq"]) and
                 activity[5] == session[0] and checkpoint is not None and
                 gap is None and retention is None, "origin_unavailable")
        try:
            projected = json.loads(activity[-1])
        except (TypeError, ValueError):
            raise _Closed("activity_changed") from None
        _require(isinstance(projected, dict) and projected.get("id") == activity_id
                 and projected.get("source") == "plugin"
                 and projected.get("source_id") == activity_id
                 and projected.get("session_id") == session[0]
                 and projected.get("domain") == "message"
                 and projected.get("action") == "user"
                 and projected.get("result") == "observed"
                 and projected.get("actor") == source.manifest.id
                 and projected.get("service") == source.manifest.id
                 and projected.get("capture_content") is False
                 and projected.get("time_unix_nano") == fact["time_ns"]
                 and projected.get("metadata", {}).get("plugin_source_id") == owner,
                 "activity_changed")
        return (source, receiver, cursor, projection, activity, content, evidence,
                member, checkpoint, grant, fact, session)


def _member_path(root: str, root_identity: tuple[int, int], member_id: str) -> str:
    """Discover a hash-addressed member without following any directory entry."""
    root_fd = _open_root_no_follow(root)
    pending = [(root_fd, "", 0)]
    dirs = entries = files = 0
    match = None
    try:
        _require(tuple((os.fstat(root_fd).st_dev, os.fstat(root_fd).st_ino)) == root_identity,
                 "root_changed")
        while pending:
            fd, prefix, depth = pending.pop()
            try:
                dirs += 1
                _require(dirs <= MAX_DIRS, "discovery_budget")
                with os.scandir(fd) as scanner:
                    for item in scanner:
                        entries += 1
                        _require(entries <= MAX_ENTRIES, "discovery_budget")
                        name = item.name
                        if (name in ("", ".", "..") or "/" in name or "\\" in name
                                or any(ord(ch) < 32 or ord(ch) == 127 for ch in name)):
                            continue
                        relative = prefix + name
                        try:
                            encoded = relative.encode("utf-8")
                        except UnicodeError:
                            continue
                        if len(encoded) > 1024:
                            continue
                        if item.is_dir(follow_symlinks=False):
                            if depth < MAX_DEPTH:
                                child = os.open(name, os.O_RDONLY | os.O_CLOEXEC |
                                                os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                                pending.append((child, relative + "/", depth + 1))
                        elif name.endswith(".jsonl") and item.is_file(follow_symlinks=False):
                            files += 1
                            _require(files <= MAX_FILES, "discovery_budget")
                            if hashlib.sha256(encoded).hexdigest() == member_id:
                                _path(relative)
                                _require(match is None, "origin_ambiguous")
                                match = relative
            finally:
                os.close(fd)
        _require(match is not None, "member_unavailable")
        return match
    finally:
        for fd, _, _ in pending:
            os.close(fd)


def _prove_file(source, content: tuple, fact: dict, checkpoint: tuple,
                grant: tuple) -> str:
    root_identity = grant[1:3]
    relative = _member_path(source.source_root, root_identity, content[6])
    cursor = _cursor(checkpoint[0])
    _require(cursor[2] <= MAX_PREFIX_BYTES and cursor[2] > 0, "proof_budget")
    fd = _open_no_follow(source.source_root, _path(relative), expected_root=root_identity)
    try:
        before = os.fstat(fd)
        _require(stat.S_ISREG(before.st_mode) and
                 (before.st_dev, before.st_ino) == (content[10], content[11]) == cursor[:2]
                 and cursor[2] <= before.st_size, "file_changed")
        head = os.pread(fd, MAX_ROW_BYTES, 0)
        end = head.find(b"\n")
        _require(end >= 0, "header_unavailable")
        header = head[:end + 1]
        header_hash = hashlib.sha256(header).hexdigest()
        _require(header_hash == cursor[4] and _prefix(fd, cursor[2]) == cursor[3],
                 "generation_changed")
        origin = hashlib.sha256(f"{before.st_dev}:{before.st_ino}:{header_hash}".encode()).hexdigest()
        _require(cursor[5] == origin, "generation_unanchored")
        parsed_header = _json_row(header)
        session_id = parsed_header.get("sessionId", parsed_header.get("id"))
        worker_id = parsed_header.get("agentId", "")
        _require(parsed_header.get("type") == "session" and
                 session_id == content[4], "header_changed")
        _require(fact["native_worker_id"] == worker_id and
                 fact["native_session_id"] == session_id and
                 fact["native_turn_id"] == content[5], "identity_changed")
        offset, length = content[7:9]
        _require(type(offset) is int and type(length) is int and
                 len(header) <= offset and 1 <= length <= MAX_ROW_BYTES and
                 offset + length <= cursor[2] and
                 os.pread(fd, 1, offset - 1) == b"\n", "row_unavailable")
        raw = os.pread(fd, length, offset)
        _require(len(raw) == length and raw.endswith(b"\n") and
                 b"\n" not in raw[:-1] and
                 hashlib.sha256(raw).hexdigest() == content[9], "row_changed")
        row = _json_row(raw)
        message = row.get("message")
        _require(row.get("type") == "message" and isinstance(message, dict) and
                 message.get("role") == "user" and row.get("id") == content[5] and
                 content[2] == 0 and
                 fact["time_ns"] == int(_time_ns(row.get("timestamp"))) and
                 hashlib.sha256((source.source_id + "\0" +
                                 f"pi:{origin}:{content[5]}:message").encode()).hexdigest()
                 == content[1] and plaintext(message.get("content")) == content[12],
                 "row_changed")
        after = os.fstat(fd)
        _require((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns,
                  before.st_ctime_ns) ==
                 (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns,
                  after.st_ctime_ns), "file_changed")
    finally:
        os.close(fd)
    again = _open_root_no_follow(source.source_root)
    try:
        info = os.fstat(again)
        _require((info.st_dev, info.st_ino) == root_identity, "root_changed")
    finally:
        os.close(again)
    return relative


def read_exact_pi_user_text(store, activity_id: str) -> Available | Unavailable:
    """Return one redacted Pi user message only while all provenance is current."""
    if type(activity_id) is not str or not _EVENT_KEY.fullmatch(activity_id):
        return Unavailable("invalid_activity")
    supervisor = getattr(store, "plugin_supervisor", None)
    if not isinstance(supervisor, PluginSupervisor) or supervisor.store is not store or supervisor.stop.is_set():
        return Unavailable("source_unavailable")
    try:
        store_identity = _store_file_identity(store)
        first = _snapshot(store, activity_id, supervisor)
        _require(_store_file_identity(store) == store_identity, "store_changed")
        relative = _prove_file(first[0], first[5], first[10], first[8], first[9])
        _require(_store_file_identity(store) == store_identity, "store_changed")
        second = _snapshot(store, activity_id, supervisor)
        _require(first == second and _store_file_identity(store) == store_identity,
                 "state_changed")
        final_relative = _prove_file(second[0], second[5], second[10], second[8], second[9])
        _require(final_relative == relative, "member_changed")
        final = _snapshot(store, activity_id, supervisor)
        _require(final == second, "state_changed")
        _require(_store_file_identity(store) == store_identity, "store_changed")
        _require(getattr(store, "plugin_supervisor", None) is supervisor and
                 supervisor.store is store and not supervisor.stop.is_set(),
                 "source_unavailable")
        source, receiver, cursor, _, _, content, _, _, _, grant, _, _ = second
        # The lease and revision are read from the same checked cursor tuple.
        return Available(content[12], ContentProof(receiver[0], source.source_id, activity_id,
                         content[1], cursor[9], cursor[4], cursor[8], content[15],
                         content[6], grant[1], grant[2], content[10], content[11],
                         content[9]))
    except (_Closed, ProtocolError, OSError, sqlite3.Error, ValueError, TypeError,
            AttributeError, KeyError, IndexError, OverflowError) as exc:
        # Never expose a source path, JSONL text, or lower-level exception.
        return Unavailable(exc.args[0] if isinstance(exc, _Closed) else "verification_failed")
