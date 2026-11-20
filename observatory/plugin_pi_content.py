"""Feature-off, receiver-owned Pi directory content admission primitive.

The process may nominate a file range, but never supplies content. The host
checks the directory member and JSONL checkpoint against its own open file,
then extracts and redacts one user message. No read API consumes this table.
"""
from __future__ import annotations

import hashlib
import json
import math
import os
import re
import stat
from dataclasses import dataclass
from datetime import datetime, timezone

from .local_activity import plaintext
from .plugin_wire import ProtocolError

MAX_REFS = 48
MAX_ROW_BYTES = 8 * 1024
MAX_BATCH_BYTES = 64 * 1024
MAX_PREFIX_BYTES = 8 * 1024 * 1024
MAX_CONTENT_ROWS = 256
MAX_CONTENT_BYTES = 1024 * 1024
_HASH = re.compile(r"[0-9a-f]{64}\Z")
_ID = re.compile(r"[a-zA-Z0-9][a-zA-Z0-9._:-]{0,159}\Z")
_STAMP = re.compile(r"([0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2})(?:\.([0-9]{1,9}))?(Z|[+-][0-9]{2}:[0-9]{2})\Z")
_FIELDS = frozenset({"schema", "source_id", "member_id", "relative_member_path",
                     "row_offset", "row_length", "row_sha256", "event_id", "revision",
                     "native_session_id", "native_turn_id", "kind"})


class UnanchoredOrigin(ProtocolError):
    """Metadata may advance, but no source-derived Pi origin is proven."""


@dataclass(frozen=True)
class PiContentGrant:
    """Minted by trusted host configuration, never from a child message."""

    source_id: str
    plugin_id: str
    generation: str
    lease_epoch: int
    policy_epoch: int
    root: str
    fingerprint: str
    root_dev: int
    root_ino: int

    @staticmethod
    def binding_for_source(source):
        from .plugin_supervisor import PluginSource

        required = {"harness.pi/1", "source-discovery/1", "configured-root/1",
                    "pi-content-ref/1"}
        if (not isinstance(source, PluginSource)
                or source.capture_mode != "pi-review-text-v1"
                or not source.source_root
                or not required <= set(source.manifest.provides)
                or not required <= set(source.runtime_features)
                or "configured-source-read" not in source.manifest.grants
                or "configured-source-read" not in source.allowed_grants):
            raise ProtocolError("Pi content grant requires trusted directory configuration")
        try:
            fd = _open_root_no_follow(source.source_root)
        except OSError as exc:
            raise ProtocolError("Pi content root unavailable") from exc
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

    @classmethod
    def fingerprint_for_source(cls, source):
        return cls.binding_for_source(source)[0]

    @classmethod
    def from_source(cls, source, lease):
        from .plugin_supervisor import PluginSource

        if (not isinstance(source, PluginSource)
                or (source.source_id, source.manifest.id,
                    source.manifest.artifact_sha256) !=
                (lease.source_id, lease.plugin_id, lease.generation)):
            raise ProtocolError("Pi content grant requires trusted directory configuration")
        fingerprint, root_dev, root_ino = cls.binding_for_source(source)
        if lease.content_fingerprint != fingerprint:
            raise ProtocolError("Pi content configuration changed")
        return cls(source.source_id, source.manifest.id, source.manifest.artifact_sha256,
                   lease.epoch, lease.policy_epoch, source.source_root, fingerprint,
                   root_dev, root_ino)


def initialize(db):
    db.execute("""CREATE TABLE IF NOT EXISTS plugin_content_v1 (
        source_id TEXT NOT NULL,
        event_id_hash TEXT NOT NULL,
        revision INTEGER NOT NULL,
        kind TEXT NOT NULL,
        native_session_id TEXT NOT NULL,
        native_turn_id TEXT NOT NULL,
        member_id TEXT NOT NULL,
        row_offset INTEGER NOT NULL,
        row_length INTEGER NOT NULL,
        row_sha256 TEXT NOT NULL,
        file_dev INTEGER NOT NULL,
        file_ino INTEGER NOT NULL,
        redacted_text TEXT NOT NULL,
        policy_epoch INTEGER NOT NULL,
        lease_epoch INTEGER NOT NULL,
        admitted_seq INTEGER NOT NULL,
        PRIMARY KEY(source_id,event_id_hash,revision),
        FOREIGN KEY(source_id) REFERENCES plugin_source_cursor_v1(source_id))""")
    db.execute("""CREATE TABLE IF NOT EXISTS plugin_content_batch_v1 (
        source_id TEXT PRIMARY KEY,
        cursor TEXT NOT NULL,
        refs_digest TEXT NOT NULL,
        FOREIGN KEY(source_id) REFERENCES plugin_source_cursor_v1(source_id))""")
    db.execute("""CREATE TABLE IF NOT EXISTS plugin_content_status_v1 (
        source_id TEXT PRIMARY KEY,
        cursor TEXT NOT NULL,
        reason TEXT NOT NULL,
        refs_count INTEGER NOT NULL,
        policy_epoch INTEGER NOT NULL,
        FOREIGN KEY(source_id) REFERENCES plugin_source_cursor_v1(source_id))""")


def _duplicate_free(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ProtocolError("duplicate Pi JSON key")
        result[key] = value
    return result


def _json_row(raw):
    try:
        def bounded_int(value):
            result = int(value)
            if not -2**63 <= result <= 2**64 - 1:
                raise ProtocolError("invalid Pi source number")
            return result
        def finite_float(value):
            result = float(value)
            if not math.isfinite(result):
                raise ProtocolError("invalid Pi source number")
            return result
        value = json.loads(raw.decode("utf-8", "strict"), object_pairs_hook=_duplicate_free,
                           parse_int=bounded_int, parse_float=finite_float,
                           parse_constant=lambda _value: (_ for _ in ()).throw(
                               ProtocolError("invalid Pi source number")))
    except (ValueError, UnicodeError, RecursionError) as exc:
        raise ProtocolError("invalid Pi content source row") from exc
    if not isinstance(value, dict):
        raise ProtocolError("invalid Pi content source row")
    return value


def _time_ns(raw):
    match = _STAMP.fullmatch(raw) if isinstance(raw, str) else None
    if match is None:
        raise ProtocolError("invalid Pi content row timestamp")
    try:
        parsed = datetime.fromisoformat(match[1] + match[3]).astimezone(timezone.utc)
    except ValueError as exc:
        raise ProtocolError("invalid Pi content row timestamp") from exc
    delta = parsed - datetime(1970, 1, 1, tzinfo=timezone.utc)
    seconds = delta.days * 86400 + delta.seconds
    value = seconds * 1_000_000_000 + int((match[2] or "").ljust(9, "0"))
    if not 0 <= value <= 2**63 - 1:
        raise ProtocolError("Pi content row timestamp exceeds range")
    return str(value)


def _cursor(raw, *, allow_overflow=False):
    if not isinstance(raw, str) or not raw.startswith("v2:"):
        raise ProtocolError("unproven Pi member cursor")
    fields = raw[3:].split(":", 6)
    if (len(fields) != 7 or any(not field or len(field) > 20 or not field.isascii()
            or not field.isdecimal() for field in fields[:3])
            or any(not _HASH.fullmatch(field) for field in fields[3:6])
            or (fields[6] and not _ID.fullmatch(fields[6]))):
        raise ProtocolError("unproven Pi member cursor")
    dev, ino, offset = (int(field) for field in fields[:3])
    if offset > MAX_PREFIX_BYTES and not allow_overflow:
        raise ProtocolError("Pi content prefix exceeds limit")
    return dev, ino, offset, *fields[3:]


def withheld_reason(batch):
    if any(record["kind"] == "diagnostic" and
           record["payload"].get("code") == "possible_source_gap"
           for record in batch.records):
        return "source_gap"
    if batch.member_cursor and _cursor(batch.member_cursor, allow_overflow=True)[2] > MAX_PREFIX_BYTES:
        return "proof_budget"
    return ""


def _path(value):
    try:
        length = len(value.encode("utf-8")) if isinstance(value, str) else 1025
    except UnicodeError as exc:
        raise ProtocolError("invalid Pi member path") from exc
    if (not isinstance(value, str) or not value or length > 1024
            or value.startswith("/") or "\\" in value or "\x00" in value):
        raise ProtocolError("invalid Pi member path")
    parts = value.split("/")
    if (len(parts) > 7 or any(part in ("", ".", "..") for part in parts)
            or not parts[-1].endswith(".jsonl")):
        raise ProtocolError("invalid Pi member path")
    return parts


def validate_refs(refs, batch):
    """Closed reference shape and exact same-batch v1 occurrence binding."""
    if not isinstance(refs, tuple) or len(refs) > MAX_REFS:
        raise ProtocolError("invalid Pi content reference count")
    if refs and (not batch.member_id or not batch.member_cursor or batch.root_gap):
        raise ProtocolError("Pi content requires one directory member batch")
    events = {record["event_id"]: record for record in batch.records}
    checked = []
    seen = set()
    size = 0
    for ref in refs:
        if not isinstance(ref, dict) or set(ref) != _FIELDS or ref["schema"] != 1:
            raise ProtocolError("invalid Pi content reference fields")
        if (ref["source_id"] != batch.source_id or ref["member_id"] != batch.member_id
                or ref["kind"] != "pi.user_text"
                or not isinstance(ref["row_sha256"], str)
                or not _HASH.fullmatch(ref["row_sha256"])
                or not isinstance(ref["member_id"], str)
                or not _HASH.fullmatch(ref["member_id"])):
            raise ProtocolError("invalid Pi content reference identity")
        parts = _path(ref["relative_member_path"])
        if hashlib.sha256(ref["relative_member_path"].encode()).hexdigest() != ref["member_id"]:
            raise ProtocolError("Pi member path does not match member identity")
        if (type(ref["row_offset"]) is not int or ref["row_offset"] < 0
                or type(ref["row_length"]) is not int
                or not 1 <= ref["row_length"] <= MAX_ROW_BYTES
                or type(ref["revision"]) is not int or not 0 <= ref["revision"] <= 2**31 - 1
                or not isinstance(ref["event_id"], str)
                or not isinstance(ref["native_session_id"], str)
                or not isinstance(ref["native_turn_id"], str)):
            raise ProtocolError("invalid Pi content reference range")
        size += ref["row_length"]
        if size > MAX_BATCH_BYTES:
            raise ProtocolError("Pi content batch exceeds limit")
        event = events.get(ref["event_id"])
        if (event is None or event["revision"] != ref["revision"]
                or event["kind"] != "message" or event["payload"] != {"role": "user"}
                or event.get("identity", {}).get("session_id") != ref["native_session_id"]
                or event.get("identity", {}).get("turn_id") != ref["native_turn_id"]
                or ref["event_id"] in seen):
            raise ProtocolError("Pi content reference lacks exact admitted message")
        seen.add(ref["event_id"])
        checked.append((ref, parts, event))
    return checked


def _open_root_no_follow(root):
    if not isinstance(root, str) or not root.startswith("/"):
        raise ProtocolError("Pi content root must be absolute")
    components = root.split("/")[1:]
    if any(part in ("", ".", "..") for part in components):
        raise ProtocolError("invalid Pi content root")
    flags = os.O_RDONLY | os.O_CLOEXEC | os.O_DIRECTORY | os.O_NOFOLLOW
    fd = os.open("/", flags)
    try:
        for part in components:
            next_fd = os.open(part, flags, dir_fd=fd)
            os.close(fd)
            fd = next_fd
        result = fd
        fd = -1
        return result
    finally:
        if fd >= 0:
            os.close(fd)


def _open_no_follow(root, parts, *, expected_root):
    flags = os.O_RDONLY | os.O_CLOEXEC | os.O_DIRECTORY | os.O_NOFOLLOW
    fd = _open_root_no_follow(root)
    try:
        info = os.fstat(fd)
        if (info.st_dev, info.st_ino) != expected_root:
            raise ProtocolError("Pi content root identity changed")
        for part in parts[:-1]:
            next_fd = os.open(part, flags, dir_fd=fd)
            os.close(fd)
            fd = next_fd
        file_fd = os.open(parts[-1], os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK,
                          dir_fd=fd)
        return file_fd
    finally:
        os.close(fd)


def _prefix(fd, length):
    if length > MAX_PREFIX_BYTES:
        raise ProtocolError("Pi content prefix exceeds limit")
    chunks = []
    for offset in range(0, length, 64 * 1024):
        chunk = os.pread(fd, min(64 * 1024, length - offset), offset)
        if len(chunk) != min(64 * 1024, length - offset):
            raise ProtocolError("Pi content source changed")
        chunks.append(chunk)
    return hashlib.sha256(b"".join(chunks)).hexdigest()


def verify_refs(checked, grant, batch, prior_cursor):
    """Independently prove current source generation, line and Pi identity."""
    if not checked:
        return ()
    # Gap and deterministic prefix-budget limits withhold content while the
    # coarse v1 cursor keeps advancing. Any other verification failure aborts.
    if withheld_reason(batch):
        return ()
    current = _cursor(batch.member_cursor)
    prior = _cursor(prior_cursor) if prior_cursor else None
    if any(parts != checked[0][1] for _, parts, _ in checked):
        raise ProtocolError("Pi content batch spans multiple member paths")
    try:
        fd = _open_no_follow(grant.root, checked[0][1],
                             expected_root=(grant.root_dev, grant.root_ino))
    except OSError as exc:
        raise ProtocolError("Pi content source unavailable") from exc
    try:
        before = os.fstat(fd)
        if (not stat.S_ISREG(before.st_mode) or before.st_dev < 0 or before.st_ino < 0
                or before.st_dev > 2**63 - 1 or before.st_ino > 2**63 - 1
                or (before.st_dev, before.st_ino) != current[:2]
                or current[2] > before.st_size):
            raise ProtocolError("Pi content source identity changed")
        header_end = os.pread(fd, MAX_ROW_BYTES, 0).find(b"\n")
        if header_end < 0:
            raise ProtocolError("Pi content header unavailable")
        header = os.pread(fd, header_end + 1, 0)
        header_hash = hashlib.sha256(header).hexdigest()
        if header_hash != current[4]:
            raise ProtocolError("Pi content header changed")
        header_row = _json_row(header)
        if (header_row.get("type") != "session"
                or not isinstance(header_row.get("id"), str)
                or not _ID.fullmatch(header_row["id"])):
            raise ProtocolError("Pi content header identity unavailable")
        session_id = header_row.get("sessionId", header_row.get("id"))
        worker_id = header_row.get("agentId", "")
        if (not isinstance(session_id, str) or not _ID.fullmatch(session_id)
                or not isinstance(worker_id, str)
                or worker_id and not _ID.fullmatch(worker_id)):
            raise ProtocolError("Pi content header identity mismatch")
        canonical_origin = hashlib.sha256(
            f"{before.st_dev}:{before.st_ino}:{header_hash}".encode()).hexdigest()
        if prior:
            if (prior[:2] != current[:2] or prior[4] != current[4]
                    or prior[2] > current[2]
                    or _prefix(fd, prior[2]) != prior[3]):
                raise ProtocolError("Pi content continuity changed")
        if _prefix(fd, current[2]) != current[3]:
            raise ProtocolError("unproven Pi content generation")
        # Metadata-only v1 previously accepted opaque member checkpoints. A
        # prior generation is not an attestation. This first primitive admits
        # content only for the independently derived initial generation; a
        # host-owned post-gap generation ledger is a later gate.
        if current[5] != canonical_origin or prior and prior[5] != canonical_origin:
            raise UnanchoredOrigin("Pi content origin has no host-owned anchor")
        origin = canonical_origin
        verified = []
        for ref, _, event in checked:
            if (session_id != ref["native_session_id"]
                    or event.get("identity", {}).get("worker_id", "") != worker_id):
                raise ProtocolError("Pi content header identity mismatch")
            offset, length = ref["row_offset"], ref["row_length"]
            if (offset < len(header) or offset < (prior[2] if prior else 0)
                    or offset + length > current[2]
                    or (offset and os.pread(fd, 1, offset - 1) != b"\n")):
                raise ProtocolError("Pi content row is outside checkpoint")
            raw = os.pread(fd, length, offset)
            if (len(raw) != length or not raw.endswith(b"\n")
                    or b"\n" in raw[:-1]
                    or hashlib.sha256(raw).hexdigest() != ref["row_sha256"]):
                raise ProtocolError("Pi content row changed")
            row = _json_row(raw)
            message = row.get("message")
            row_id = row.get("id")
            if (row.get("type") != "message" or not isinstance(row_id, str)
                    or not _ID.fullmatch(row_id) or not isinstance(message, dict)
                    or message.get("role") != "user"
                    or row_id != ref["native_turn_id"]
                    or ref["revision"] != 0
                    or event["time_ns"] != _time_ns(row.get("timestamp"))
                    or ref["event_id"] != f"pi:{origin}:{row_id}:message"):
                raise ProtocolError("Pi content row identity mismatch")
            text = plaintext(message.get("content"))
            verified.append((ref, before.st_dev, before.st_ino, text))
        after = os.fstat(fd)
        if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns,
                before.st_ctime_ns) != (after.st_dev, after.st_ino, after.st_size,
                                        after.st_mtime_ns, after.st_ctime_ns):
            raise ProtocolError("Pi content source changed during verification")
        return tuple(verified)
    finally:
        os.close(fd)


def prune_orphans(db, source_id):
    return db.execute("""DELETE FROM plugin_content_v1 WHERE source_id=? AND (
        NOT EXISTS (SELECT 1 FROM plugin_evidence_v1 e WHERE e.source_id=plugin_content_v1.source_id
          AND e.event_id_hash=plugin_content_v1.event_id_hash
          AND e.revision=plugin_content_v1.revision)
        OR EXISTS (SELECT 1 FROM plugin_source_gap_invalidated_v1 g
          WHERE g.source_id=plugin_content_v1.source_id
          AND g.native_session_id IN (plugin_content_v1.native_session_id,'*'))
        OR EXISTS (SELECT 1 FROM plugin_retention_invalidated_v1 r
          WHERE r.source_id=plugin_content_v1.source_id
          AND r.native_session_id=plugin_content_v1.native_session_id))""", (source_id,)).rowcount


def prune_capacity(db, source_id):
    """Keep metadata advancing when the best-effort content horizon fills."""
    rows = db.execute(
        "SELECT event_id_hash,revision,length(CAST(redacted_text AS BLOB)) AS bytes "
        "FROM plugin_content_v1 WHERE source_id=? "
        "ORDER BY admitted_seq,event_id_hash,revision", (source_id,)).fetchall()
    size = sum(row["bytes"] for row in rows)
    excess = max(0, len(rows) - MAX_CONTENT_ROWS)
    victims = []
    for row in rows:
        if len(victims) >= excess and size <= MAX_CONTENT_BYTES:
            break
        victims.append((source_id, row["event_id_hash"], row["revision"]))
        size -= row["bytes"]
    db.executemany("DELETE FROM plugin_content_v1 WHERE source_id=? "
                   "AND event_id_hash=? AND revision=?", victims)
    return len(victims)
