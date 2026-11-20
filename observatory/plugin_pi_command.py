"""Inactive, receiver-owned proof for one requested Pi bash command.

No installed source negotiates this reference type yet. The child may nominate
a source range; only the host opens that file and extracts bounded text.
"""
from __future__ import annotations

import hashlib
import os
import stat
from dataclasses import dataclass

from .local_activity import safe
from .plugin_pi_content import (
    MAX_BATCH_BYTES, MAX_PREFIX_BYTES, MAX_REFS, MAX_ROW_BYTES, PiContentGrant,
    UnanchoredOrigin, _ID, _cursor, _json_row, _open_no_follow, _open_root_no_follow,
    _path, _prefix, _time_ns, withheld_reason,
)
from .plugin_wire import ProtocolError

MAX_COMMAND_BYTES = 4096
MAX_COMMAND_ROWS = 256
MAX_COMMAND_STORED_BYTES = 1024 * 1024


@dataclass(frozen=True)
class PiCommandRef:
    schema: int
    source_id: str
    member_id: str
    relative_member_path: str
    row_offset: int
    row_length: int
    row_sha256: str
    event_id: str
    revision: int
    native_session_id: str
    native_turn_id: str
    assistant_row_id: str
    block_index: int
    tool_call_id: str
    kind: str


@dataclass(frozen=True)
class PiCommandGrant(PiContentGrant):
    """A separate trusted grant; the installed config cannot mint it yet."""

    @staticmethod
    def binding_for_source(source):
        from .plugin_supervisor import PluginSource

        required = {"harness.pi/1", "source-discovery/1",
                    "source-discovery-root-gap/1", "configured-root/1",
                    "pi-command-ref/1"}
        if (not isinstance(source, PluginSource)
                or source.capture_mode != "pi-review-text-command-v1"
                or source.replaces != ("pi",)
                or not source.source_root
                or not required <= set(source.manifest.provides)
                or not required <= set(source.runtime_features)
                or "configured-source-read" not in source.manifest.grants
                or "configured-source-read" not in source.allowed_grants):
            raise ProtocolError("Pi command grant requires trusted directory configuration")
        try:
            fd = _open_root_no_follow(source.source_root)
        except OSError as exc:
            raise ProtocolError("Pi command root unavailable") from exc
        try:
            root_stat = os.fstat(fd)
        finally:
            os.close(fd)
        binding = "\0".join((source.source_id, source.manifest.id,
                             source.manifest.artifact_sha256, source.capture_mode,
                             source.source_root, str(root_stat.st_dev),
                             str(root_stat.st_ino),
                             ",".join(sorted(source.allowed_grants)),
                             ",".join(sorted(source.runtime_features))))
        return hashlib.sha256(binding.encode()).hexdigest(), root_stat.st_dev, root_stat.st_ino


def initialize(db):
    db.execute("""CREATE TABLE IF NOT EXISTS plugin_command_content_v1 (
        source_id TEXT NOT NULL,
        event_id_hash TEXT NOT NULL,
        revision INTEGER NOT NULL,
        kind TEXT NOT NULL,
        native_session_id TEXT NOT NULL,
        native_turn_id TEXT NOT NULL,
        tool_call_id TEXT NOT NULL,
        assistant_row_id TEXT NOT NULL,
        block_index INTEGER NOT NULL,
        member_id TEXT NOT NULL,
        row_offset INTEGER NOT NULL,
        row_length INTEGER NOT NULL,
        row_sha256 TEXT NOT NULL,
        file_dev INTEGER NOT NULL,
        file_ino INTEGER NOT NULL,
        redacted_command TEXT NOT NULL,
        policy_epoch INTEGER NOT NULL,
        lease_epoch INTEGER NOT NULL,
        admitted_seq INTEGER NOT NULL,
        PRIMARY KEY(source_id,event_id_hash,revision),
        FOREIGN KEY(source_id) REFERENCES plugin_source_cursor_v1(source_id))""")
    db.execute("""CREATE TABLE IF NOT EXISTS plugin_command_batch_v1 (
        source_id TEXT PRIMARY KEY,
        cursor TEXT NOT NULL,
        refs_digest TEXT NOT NULL,
        FOREIGN KEY(source_id) REFERENCES plugin_source_cursor_v1(source_id))""")


def _sha(raw):
    return hashlib.sha256(raw).hexdigest()


def _source_turns(fd, header_length, offsets):
    """Derive requested turns in one bounded scan, independent of child facts."""
    last_offset = max(offsets)
    if not header_length <= last_offset <= MAX_PREFIX_BYTES:
        raise ProtocolError("Pi command turn proof exceeds limit")
    preceding = os.pread(fd, last_offset - header_length, header_length)
    if len(preceding) != last_offset - header_length:
        raise ProtocolError("Pi command preceding rows changed")
    last_user = ""
    position = header_length
    turns = {}
    for raw in preceding.splitlines(keepends=True):
        if not raw.endswith(b"\n") or len(raw) > MAX_ROW_BYTES:
            raise ProtocolError("Pi command preceding row unavailable")
        if position in offsets:
            turns[position] = last_user
        row = _json_row(raw)
        if row.get("type") != "message":
            position += len(raw)
            continue
        message = row.get("message")
        if isinstance(message, dict) and message.get("role") == "user":
            user_id = row.get("id")
            if not isinstance(user_id, str) or not _ID.fullmatch(user_id):
                raise ProtocolError("Pi command preceding user identity invalid")
            last_user = user_id
        position += len(raw)
    if last_offset in offsets:
        turns[last_offset] = last_user
    return turns


def validate_refs(refs, batch):
    """Accept only typed, exact same-batch started shell call references."""
    if type(refs) is not tuple or len(refs) > MAX_REFS:
        raise ProtocolError("invalid Pi command reference count")
    if refs and (not batch.member_id or not batch.member_cursor or batch.root_gap):
        raise ProtocolError("Pi command requires one directory member batch")
    events = {record["event_id"]: record for record in batch.records}
    checked, seen, total = [], set(), 0
    for ref in refs:
        if type(ref) is not PiCommandRef or type(ref.schema) is not int or ref.schema != 1:
            raise ProtocolError("invalid Pi command reference shape")
        if (ref.source_id != batch.source_id or ref.member_id != batch.member_id
                or ref.kind != "pi.shell_command_text"
                or type(ref.revision) is not int or ref.revision != 0
                or type(ref.row_offset) is not int or ref.row_offset < 0
                or type(ref.row_length) is not int or not 1 <= ref.row_length <= MAX_ROW_BYTES
                or type(ref.block_index) is not int or not 0 <= ref.block_index < 128
                or not isinstance(ref.row_sha256, str)
                or not isinstance(ref.member_id, str)
                or not isinstance(ref.event_id, str)
                or not isinstance(ref.native_session_id, str)
                or not isinstance(ref.native_turn_id, str)
                or not isinstance(ref.assistant_row_id, str)
                or not isinstance(ref.tool_call_id, str)
                or not all(_ID.fullmatch(value) for value in (
                    ref.native_session_id, ref.native_turn_id,
                    ref.assistant_row_id, ref.tool_call_id))
                or len(ref.row_sha256) != 64
                or any(char not in "0123456789abcdef" for char in ref.row_sha256)
                or len(ref.member_id) != 64
                or any(char not in "0123456789abcdef" for char in ref.member_id)):
            raise ProtocolError("invalid Pi command reference identity")
        parts = _path(ref.relative_member_path)
        if _sha(ref.relative_member_path.encode()) != ref.member_id:
            raise ProtocolError("Pi command member identity mismatch")
        total += ref.row_length
        if total > MAX_BATCH_BYTES:
            raise ProtocolError("Pi command reference bytes exceed limit")
        event = events.get(ref.event_id)
        identity = event.get("identity", {}) if event is not None else {}
        if (event is None or event["revision"] != 0 or event["kind"] != "tool"
                or event["payload"] != {"category": "shell", "result": "started"}
                or not isinstance(identity, dict)
                or identity.get("session_id") != ref.native_session_id
                or identity.get("turn_id") != ref.native_turn_id
                or identity.get("operation_id") != ref.tool_call_id
                or set(identity) - {"session_id", "turn_id", "operation_id", "worker_id"}
                or ref.event_id in seen):
            raise ProtocolError("Pi command lacks exact started shell fact")
        seen.add(ref.event_id)
        checked.append((ref, parts, event))
    return tuple(checked)


def verify_refs(checked, grant, batch, prior_cursor):
    """Independently prove source, Pi row, block, and command before Store write."""
    if not checked or withheld_reason(batch):
        return ()
    current = _cursor(batch.member_cursor)
    prior = _cursor(prior_cursor) if prior_cursor else None
    if current[2] > MAX_PREFIX_BYTES or any(parts != checked[0][1] for _, parts, _ in checked):
        raise ProtocolError("Pi command source budget or member mismatch")
    fd = _open_no_follow(grant.root, checked[0][1],
                         expected_root=(grant.root_dev, grant.root_ino))
    try:
        before = os.fstat(fd)
        if (not stat.S_ISREG(before.st_mode) or before.st_dev > 2**63 - 1
                or before.st_ino > 2**63 - 1 or (before.st_dev, before.st_ino) != current[:2]
                or current[2] > before.st_size):
            raise ProtocolError("Pi command source identity changed")
        header_end = os.pread(fd, MAX_ROW_BYTES, 0).find(b"\n")
        if header_end < 0:
            raise ProtocolError("Pi command header unavailable")
        header = os.pread(fd, header_end + 1, 0)
        header_hash = _sha(header)
        header_row = _json_row(header)
        session_id = header_row.get("sessionId", header_row.get("id"))
        worker_id = header_row.get("agentId", "")
        if (header_row.get("type") != "session" or header_hash != current[4]
                or not isinstance(session_id, str) or not _ID.fullmatch(session_id)
                or not isinstance(worker_id, str)
                or worker_id and not _ID.fullmatch(worker_id)):
            raise ProtocolError("Pi command header identity changed")
        origin = _sha(f"{before.st_dev}:{before.st_ino}:{header_hash}".encode())
        if (current[5] != origin or prior and (
                prior[:2] != current[:2] or prior[4] != current[4]
                or prior[5] != origin or prior[2] > current[2]
                or _prefix(fd, prior[2]) != prior[3])):
            raise UnanchoredOrigin("Pi command source origin unanchored")
        if _prefix(fd, current[2]) != current[3]:
            raise ProtocolError("Pi command prefix changed")
        turns = _source_turns(fd, len(header),
                              {ref.row_offset for ref, _, _ in checked})
        verified = []
        for ref, _, event in checked:
            if (ref.native_session_id != session_id
                    or event["identity"].get("worker_id", "") != worker_id):
                raise ProtocolError("Pi command session identity changed")
            offset, length = ref.row_offset, ref.row_length
            if (offset < len(header) or offset < (prior[2] if prior else 0)
                    or offset + length > current[2]
                    or os.pread(fd, 1, offset - 1) != b"\n"):
                raise ProtocolError("Pi command row outside checkpoint")
            raw = os.pread(fd, length, offset)
            if (len(raw) != length or not raw.endswith(b"\n")
                    or b"\n" in raw[:-1] or _sha(raw) != ref.row_sha256):
                raise ProtocolError("Pi command row changed")
            row = _json_row(raw)
            message = row.get("message")
            if (row.get("type") != "message" or row.get("id") != ref.assistant_row_id
                    or not isinstance(message, dict) or message.get("role") != "assistant"
                    or event["time_ns"] != _time_ns(row.get("timestamp"))
                    or ref.event_id !=
                    f"pi:{origin}:{ref.assistant_row_id}:tool-{ref.block_index}"):
                raise ProtocolError("Pi command row identity changed")
            if turns.get(offset) != ref.native_turn_id:
                raise ProtocolError("Pi command source turn changed")
            blocks = message.get("content")
            if not isinstance(blocks, list) or not 0 <= ref.block_index < len(blocks):
                raise ProtocolError("Pi command block unavailable")
            block = blocks[ref.block_index]
            if (not isinstance(block, dict) or block.get("type") != "toolCall"
                    or block.get("id") != ref.tool_call_id or block.get("name") != "bash"
                    or not isinstance(block.get("arguments"), dict)
                    or type(block["arguments"].get("command")) is not str):
                raise ProtocolError("Pi command block identity changed")
            command = block["arguments"]["command"]
            if not command or len(command.encode("utf-8")) > MAX_COMMAND_BYTES:
                raise ProtocolError("Pi command source text exceeds limit")
            redacted = safe(command, MAX_COMMAND_BYTES)
            if len(redacted.encode("utf-8")) > MAX_COMMAND_BYTES:
                redacted = redacted.encode("utf-8")[:MAX_COMMAND_BYTES].decode("utf-8", "ignore")
            verified.append((ref, before.st_dev, before.st_ino, redacted))
        after = os.fstat(fd)
        if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns,
                before.st_ctime_ns) != (after.st_dev, after.st_ino, after.st_size,
                                        after.st_mtime_ns, after.st_ctime_ns):
            raise ProtocolError("Pi command source changed during proof")
        return tuple(verified)
    finally:
        os.close(fd)


def prune_orphans(db, source_id):
    db.execute("""DELETE FROM plugin_command_content_v1 WHERE source_id=? AND (
        NOT EXISTS (SELECT 1 FROM plugin_evidence_v1 e WHERE e.source_id=plugin_command_content_v1.source_id
          AND e.event_id_hash=plugin_command_content_v1.event_id_hash
          AND e.revision=plugin_command_content_v1.revision)
        OR NOT EXISTS (SELECT 1 FROM plugin_session_member_v1 m
          WHERE m.source_id=plugin_command_content_v1.source_id
          AND m.native_session_id=plugin_command_content_v1.native_session_id
          AND m.member_id=plugin_command_content_v1.member_id)
        OR EXISTS (SELECT 1 FROM plugin_source_gap_invalidated_v1 g
          WHERE g.source_id=plugin_command_content_v1.source_id
          AND g.native_session_id IN (plugin_command_content_v1.native_session_id,'*'))
        OR EXISTS (SELECT 1 FROM plugin_retention_invalidated_v1 r
          WHERE r.source_id=plugin_command_content_v1.source_id
          AND r.native_session_id=plugin_command_content_v1.native_session_id))""", (source_id,))


def prune_capacity(db, source_id):
    rows = db.execute(
        "SELECT event_id_hash,revision,length(CAST(redacted_command AS BLOB)) AS bytes "
        "FROM plugin_command_content_v1 WHERE source_id=? "
        "ORDER BY admitted_seq,event_id_hash,revision", (source_id,)).fetchall()
    size = sum(row["bytes"] for row in rows)
    excess = max(0, len(rows) - MAX_COMMAND_ROWS)
    victims = []
    for row in rows:
        if len(victims) >= excess and size <= MAX_COMMAND_STORED_BYTES:
            break
        victims.append((source_id, row["event_id_hash"], row["revision"]))
        size -= row["bytes"]
    db.executemany("DELETE FROM plugin_command_content_v1 WHERE source_id=? "
                   "AND event_id_hash=? AND revision=?", victims)
    return len(victims)
