"""Internal, fail-closed read of one requested Pi bash command.

This module has no HTTP, UI, export, or copy caller. A later delivery path
must recheck its point-in-time proof immediately before returning text.
"""
from __future__ import annotations

import hashlib
import json
import os
import sqlite3
import stat
from dataclasses import dataclass

from .local_activity import safe
from .plugin_activity import _EVENT_KEY, index_rows
from .plugin_pi_command import (
    MAX_COMMAND_BYTES, MAX_COMMAND_ROWS, MAX_COMMAND_STORED_BYTES,
    PiCommandGrant, _source_turns,
)
from .plugin_pi_content import (
    MAX_PREFIX_BYTES, MAX_ROW_BYTES, _cursor, _json_row, _open_no_follow,
    _open_root_no_follow, _path, _prefix, _time_ns,
)
from .plugin_pi_content_read import _Closed, _member_path, _one, _require
from .plugin_supervisor import PluginSupervisor
from .plugin_wire import ProtocolError
from .rust_shadow_snapshot import _store_file_identity


@dataclass(frozen=True)
class CommandProof:
    """One point-in-time binding, never a durable read authorization."""

    receiver_id: str
    source_id: str
    activity_id: str
    event_id_hash: str
    policy_epoch: int
    lease_epoch: int
    source_dirty_seq: int
    command_admitted_seq: int
    member_id: str
    root_dev: int
    root_ino: int
    file_dev: int
    file_ino: int
    row_sha256: str


@dataclass(frozen=True)
class Available:
    redacted_command: str
    proof: CommandProof


@dataclass(frozen=True)
class Unavailable:
    reason: str


def _snapshot(store, activity_id: str, supervisor: PluginSupervisor):
    """Capture all selected DB state in one short, coherent read transaction."""
    with store._read() as db:
        db.execute("BEGIN")
        activity = _one(db, "SELECT * FROM activity_index_v1 WHERE event_key=? AND kind='plugin'",
                        (activity_id,))
        _require(activity is not None, "activity_unavailable")
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
                 and type(current["dirty_seq"]) is int and current["dirty_seq"] > 0
                 and projection[1:] == (current["dirty_seq"], current["policy_epoch"]),
                 "source_not_current")
        grant = PiCommandGrant.binding_for_source(source)
        _require(grant[0] == current["content_fingerprint"], "configuration_changed")

        size = _one(db, "SELECT COUNT(*),COALESCE(SUM(length(CAST(redacted_command AS BLOB))),0) "
                    "FROM plugin_command_content_v1 WHERE source_id=?", (owner,))
        _require(size is not None and size[0] <= MAX_COMMAND_ROWS and
                 size[1] <= MAX_COMMAND_STORED_BYTES, "command_budget")
        command_rows = db.execute("SELECT * FROM plugin_command_content_v1 WHERE source_id=? "
                                  "ORDER BY event_id_hash,revision LIMIT ?",
                                  (owner, MAX_COMMAND_ROWS + 1)).fetchall()
        _require(len(command_rows) <= MAX_COMMAND_ROWS, "command_budget")
        matching = [tuple(row) for row in command_rows if
                    "plugin-event:" + hashlib.sha256((receiver[0] + "\0" + owner + "\0" +
                    row[1]).encode()).hexdigest() == activity_id]
        _require(len(matching) == 1, "command_unavailable")
        command = matching[0]
        event_hash, revision, kind, session_id, turn_id, tool_id = command[1:7]
        _require(revision == 0 and kind == "pi.shell_command_text"
                 and type(command[8]) is int and 0 <= command[8] < 128
                 and command[16] == current["policy_epoch"]
                 and type(command[17]) is int and 0 < command[17] < 2**63
                 and type(command[18]) is int and 0 < command[18] <= current["dirty_seq"]
                 and isinstance(command[15], str)
                 and len(command[15].encode("utf-8")) <= MAX_COMMAND_BYTES,
                 "command_not_current")
        evidence = _one(db, "SELECT * FROM plugin_evidence_v1 WHERE source_id=? AND "
                        "event_id_hash=? AND revision=0", (owner, event_hash))
        latest = _one(db, "SELECT MAX(revision) FROM plugin_evidence_v1 WHERE source_id=? "
                      "AND event_id_hash=?", (owner, event_hash))
        _require(evidence is not None and latest == (0,), "evidence_changed")
        evidence_names = [row[1] for row in db.execute("PRAGMA table_info(plugin_evidence_v1)")]
        fact = dict(zip(evidence_names, evidence))
        _require(isinstance(fact["payload_json"], str)
                 and len(fact["payload_json"].encode("utf-8")) <= 1024,
                 "evidence_changed")
        _require(fact["kind"] == "tool" and fact["native_session_id"] == session_id
                 and fact["native_turn_id"] == turn_id
                 and fact["native_operation_id"] == tool_id
                 and fact["member_id"] == command[9]
                 and fact["policy_epoch"] == current["policy_epoch"]
                 and json.loads(fact["payload_json"]) ==
                 {"category": "shell", "result": "started"}, "evidence_changed")
        member = _one(db, "SELECT member_id FROM plugin_session_member_v1 WHERE source_id=? "
                      "AND native_session_id=?", (owner, session_id))
        session = _one(db, "SELECT session_key,reason_code,policy_epoch,source_dirty_seq "
                       "FROM plugin_session_projection_v1 WHERE source_id=? "
                       "AND native_session_id=?", (owner, session_id))
        checkpoint = _one(db, "SELECT cursor FROM plugin_source_member_cursor_v1 "
                          "WHERE source_id=? AND member_id=?", (owner, command[9]))
        gap = _one(db, "SELECT 1 FROM plugin_source_gap_invalidated_v1 WHERE source_id=? "
                   "AND native_session_id IN ('*',?) LIMIT 1", (owner, session_id))
        retention = _one(db, "SELECT 1 FROM plugin_retention_invalidated_v1 WHERE source_id=? "
                         "AND native_session_id=?", (owner, session_id))
        _require(member == (command[9],) and session is not None
                 and session[1] not in ("source_gap", "retention_gap")
                 and session[2:] == (current["policy_epoch"], current["dirty_seq"])
                 and activity[5] == session[0] and checkpoint is not None
                 and gap is None and retention is None, "origin_unavailable")
        _require(isinstance(activity[-1], str)
                 and len(activity[-1].encode("utf-8")) <= 8192,
                 "activity_changed")
        # A started tool's Activity is completely determined by this one
        # retained fact; comparing the entire row also binds opaque turn and
        # operation IDs without a second mutable mapping.
        expected = index_rows([fact], receiver_id=receiver[0], source_id=owner,
                              plugin_id=source.manifest.id, invalidated=set())
        _require(len(expected) == 1 and activity == expected[0], "activity_changed")
        return (source, receiver, cursor, projection, activity, command, evidence,
                member, checkpoint, grant, fact, session)


def _prove_file(source, command: tuple, fact: dict, checkpoint: tuple,
                grant: tuple) -> str:
    """Re-open the configured member without symlinks and rederive its command."""
    root_identity = grant[1:3]
    relative = _member_path(source.source_root, root_identity, command[9])
    cursor = _cursor(checkpoint[0])
    _require(0 < cursor[2] <= MAX_PREFIX_BYTES, "proof_budget")
    fd = _open_no_follow(source.source_root, _path(relative), expected_root=root_identity)
    try:
        before = os.fstat(fd)
        _require(stat.S_ISREG(before.st_mode) and
                 (before.st_dev, before.st_ino) == (command[13], command[14]) == cursor[:2]
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
        _require(parsed_header.get("type") == "session" and session_id == command[4]
                 and fact["native_worker_id"] == worker_id
                 and fact["native_session_id"] == session_id
                 and fact["native_turn_id"] == command[5]
                 and fact["native_operation_id"] == command[6], "identity_changed")
        offset, length = command[10:12]
        _require(type(offset) is int and type(length) is int
                 and len(header) <= offset and 1 <= length <= MAX_ROW_BYTES
                 and offset + length <= cursor[2]
                 and os.pread(fd, 1, offset - 1) == b"\n", "row_unavailable")
        raw = os.pread(fd, length, offset)
        _require(len(raw) == length and raw.endswith(b"\n") and
                 b"\n" not in raw[:-1] and
                 hashlib.sha256(raw).hexdigest() == command[12], "row_changed")
        row = _json_row(raw)
        message = row.get("message")
        event_id = f"pi:{origin}:{command[7]}:tool-{command[8]}"
        _require(row.get("type") == "message" and row.get("id") == command[7]
                 and isinstance(message, dict) and message.get("role") == "assistant"
                 and fact["time_ns"] == int(_time_ns(row.get("timestamp")))
                 and hashlib.sha256((source.source_id + "\0" + event_id).encode()).hexdigest()
                 == command[1], "row_changed")
        turns = _source_turns(fd, len(header), {offset})
        _require(turns.get(offset) == command[5], "turn_changed")
        blocks = message.get("content")
        _require(isinstance(blocks, list) and 0 <= command[8] < len(blocks),
                 "block_unavailable")
        block = blocks[command[8]]
        _require(isinstance(block, dict) and block.get("type") == "toolCall"
                 and block.get("id") == command[6] and block.get("name") == "bash"
                 and isinstance(block.get("arguments"), dict)
                 and type(block["arguments"].get("command")) is str,
                 "block_changed")
        requested = block["arguments"]["command"]
        _require(requested and len(requested.encode("utf-8")) <= MAX_COMMAND_BYTES,
                 "command_budget")
        redacted = safe(requested, MAX_COMMAND_BYTES)
        if len(redacted.encode("utf-8")) > MAX_COMMAND_BYTES:
            redacted = redacted.encode("utf-8")[:MAX_COMMAND_BYTES].decode("utf-8", "ignore")
        _require(redacted == command[15], "command_changed")
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


def read_exact_pi_requested_command(store, activity_id: str) -> Available | Unavailable:
    """Return redacted requested text only while exact provenance is current."""
    if type(activity_id) is not str or not _EVENT_KEY.fullmatch(activity_id):
        return Unavailable("invalid_activity")
    supervisor = getattr(store, "plugin_supervisor", None)
    if (not isinstance(supervisor, PluginSupervisor) or supervisor.store is not store
            or supervisor.stop.is_set()):
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
        _require(getattr(store, "plugin_supervisor", None) is supervisor
                 and supervisor.store is store and not supervisor.stop.is_set(),
                 "source_unavailable")
        source, receiver, cursor, _, _, command, _, _, _, grant, _, _ = second
        return Available(command[15], CommandProof(
            receiver[0], source.source_id, activity_id, command[1], cursor[9],
            cursor[4], cursor[8], command[18], command[9], grant[1], grant[2],
            command[13], command[14], command[12]))
    except (_Closed, ProtocolError, OSError, sqlite3.Error, ValueError, TypeError,
            AttributeError, KeyError, IndexError, OverflowError, UnicodeError,
            RecursionError) as exc:
        return Unavailable(exc.args[0] if isinstance(exc, _Closed)
                           else "verification_failed")
