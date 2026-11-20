"""Strict v1 messages for an isolated, opt-in source adapter process.

The wire contract is deliberately smaller than Agenturm's canonical evidence
schema. The opt-in Store inbox admits a narrow metadata subset; identity-bearing
lifecycle records can be projected into live session health by the host.
"""
from __future__ import annotations

import json
import math
import re
from dataclasses import dataclass

from .plugin_frames import MAX_FRAME_BYTES


PROTOCOL_MAJOR = 1
MAX_NESTING = 20
MAX_STRUCTURAL_TOKENS = 8192
MAX_BATCH_RECORDS = 256
MAX_CONTROL_FRAME_BYTES = 64 * 1024
MAX_RECORD_BYTES = 16 * 1024
# Opaque process-source cursors are bounded by UTF-8 bytes on every transport.
MAX_CURSOR_CHARS = 800
MAX_DISCOVERY_MEMBERS = 48
MEMBER_ID = re.compile(r"[0-9a-f]{64}\Z")
MAX_ID_CHARS = 256
INT64_MAX = 2**63 - 1
DECIMAL_NS = re.compile(r"(?:0|[1-9][0-9]{0,18})\Z")
KINDS = frozenset({"message", "tool", "check", "lifecycle", "usage", "relationship", "diagnostic"})


class ProtocolError(ValueError):
    """A peer violated the wire contract; close and replay the connection."""


def _preflight(raw: str) -> None:
    """Reject excessive JSON structure before constructing Python containers."""
    depth = tokens = 0
    quoted = escaped = False
    for char in raw:
        if quoted:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quoted = False
            continue
        if char == '"':
            quoted = True
            tokens += 1
        elif char in "{[":
            depth += 1
            tokens += 1
            if depth > MAX_NESTING:
                raise ProtocolError("JSON nesting limit exceeded")
        elif char in "}]":
            depth -= 1
            tokens += 1
            if depth < 0:
                raise ProtocolError("invalid JSON structure")
        elif char in ",:":
            tokens += 1
        if tokens > MAX_STRUCTURAL_TOKENS:
            raise ProtocolError("JSON structural token limit exceeded")
    if quoted or depth != 0:
        raise ProtocolError("incomplete JSON structure")


def _pairs(items: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in items:
        if key in result:
            raise ProtocolError("duplicate JSON key")
        result[key] = value
    return result


def _integer(raw: str) -> int:
    if len(raw) > 20:
        raise ProtocolError("JSON integer exceeds signed 64-bit range")
    value = int(raw)
    if not -INT64_MAX - 1 <= value <= INT64_MAX:
        raise ProtocolError("JSON integer exceeds signed 64-bit range")
    return value


def _float(raw: str) -> float:
    value = float(raw)
    if not math.isfinite(value):
        raise ProtocolError("non-finite JSON number")
    return value


def decode_message(raw: str) -> dict:
    if not isinstance(raw, str):
        raise ProtocolError("message must be UTF-8 text")
    try:
        if len(raw.encode("utf-8", "strict")) > MAX_FRAME_BYTES:
            raise ProtocolError("plugin message exceeds frame limit")
    except UnicodeEncodeError as exc:
        raise ProtocolError("plugin message contains invalid Unicode") from exc
    _preflight(raw)
    try:
        result = json.loads(raw, object_pairs_hook=_pairs, parse_int=_integer,
                            parse_float=_float,
                            parse_constant=lambda _value: (_ for _ in ()).throw(
                                ProtocolError("non-finite JSON number")))
    except ProtocolError:
        raise
    except (ValueError, RecursionError) as exc:
        raise ProtocolError("invalid plugin JSON") from exc
    if not isinstance(result, dict):
        raise ProtocolError("plugin message must be an object")
    return result


def encode_message(value: dict) -> str:
    try:
        raw = json.dumps(value, ensure_ascii=False, allow_nan=False,
                         sort_keys=True, separators=(",", ":"))
    except (TypeError, ValueError, OverflowError, RecursionError) as exc:
        raise ProtocolError("cannot encode plugin message") from exc
    decode_message(raw)
    return raw


def _text(value: object, name: str, limit: int) -> str:
    if not isinstance(value, str) or not value or len(value) > limit or any(
            ord(char) < 32 or ord(char) == 127 for char in value):
        raise ProtocolError(f"invalid {name}")
    return value


def _cursor_valid(value: object, *, allow_empty: bool = False) -> bool:
    if (not isinstance(value, str) or (not value and not allow_empty)
            or any(ord(char) < 32 or 127 <= ord(char) <= 159 for char in value)):
        return False
    try:
        size = len(value.encode("utf-8"))
    except UnicodeEncodeError:
        return False
    return size <= MAX_CURSOR_CHARS


def _cursor_text(value: object, name: str, *, allow_empty: bool = False) -> str:
    if not _cursor_valid(value, allow_empty=allow_empty):
        raise ProtocolError(f"invalid {name}")
    return value


def validate_hello_ack(message: dict, *, plugin_id: str) -> tuple[str, ...]:
    if set(message) != {"type", "protocol", "plugin_id", "features"}:
        raise ProtocolError("invalid hello acknowledgment fields")
    if (message["type"] != "hello_ack" or type(message["protocol"]) is not int
            or message["protocol"] != PROTOCOL_MAJOR or message["plugin_id"] != plugin_id):
        raise ProtocolError("plugin identity or protocol mismatch")
    features = message["features"]
    if (not isinstance(features, list) or len(features) > 32
            or any(not isinstance(item, str) or len(item) > 64
                   or not re.fullmatch(r"[a-z][a-z0-9.-]*/[1-9][0-9]*", item)
                   for item in features)
            or len(set(features)) != len(features)
            or "source-adapter/1" not in features):
        raise ProtocolError("source adapter feature unavailable")
    return tuple(features)


@dataclass(frozen=True)
class PollBatch:
    source_id: str
    cursor: str
    records: tuple[dict, ...]
    member_id: str = ""
    member_cursor: str = ""
    root_gap: bool = False


@dataclass(frozen=True)
class ContentPollBatch(PollBatch):
    """A negotiated Pi reference envelope over an otherwise strict v1 batch."""

    content_refs: tuple[dict, ...] = ()


@dataclass(frozen=True)
class CommandPollBatch(PollBatch):
    """A separately negotiated, reference-only Pi command envelope."""

    command_refs: tuple[object, ...] = ()


@dataclass(frozen=True)
class ReferencePollBatch(PollBatch):
    """Host-selected, bounded reference family; the child cannot name a provider."""

    reference_feature: str = ""
    reference_refs: tuple[object, ...] = ()


def validate_reference_batch(message: dict, *, source_id: str, feature: str,
                             validate_refs) -> ReferencePollBatch:
    """Decode a common reference envelope before a trusted family validator."""
    if not isinstance(message, dict) or "reference_refs" not in message:
        raise ProtocolError("reference envelope is required")
    envelope = message["reference_refs"]
    if (not isinstance(envelope, dict) or set(envelope) != {"schema_version", "refs"}
            or type(envelope["schema_version"]) is not int
            or envelope["schema_version"] != 1
            or not isinstance(envelope["refs"], list)
            or len(envelope["refs"]) > 48
            or len(encode_message(envelope).encode("utf-8")) > MAX_CONTROL_FRAME_BYTES):
        raise ProtocolError("invalid reference envelope")
    ordinary = {key: value for key, value in message.items() if key != "reference_refs"}
    batch = validate_batch(ordinary, source_id=source_id)
    checked = validate_refs(tuple(envelope["refs"]), batch)
    if type(checked) is not tuple or len(checked) > 48:
        raise ProtocolError("invalid trusted reference validator result")
    return ReferencePollBatch(batch.source_id, batch.cursor, batch.records,
                              batch.member_id, batch.member_cursor, batch.root_gap,
                              feature, checked)


def validate_command_batch(message: dict, *, source_id: str) -> CommandPollBatch:
    """Decode only a bounded command-reference envelope, never command text."""
    from dataclasses import fields
    from .plugin_pi_command import PiCommandRef

    if not isinstance(message, dict) or "command_refs" not in message:
        raise ProtocolError("Pi command reference envelope is required")
    envelope = message["command_refs"]
    if (not isinstance(envelope, dict) or set(envelope) != {"schema_version", "refs"}
            or type(envelope["schema_version"]) is not int
            or envelope["schema_version"] != 1
            or not isinstance(envelope["refs"], list)
            or len(envelope["refs"]) > 48
            or len(encode_message(envelope).encode("utf-8")) > MAX_CONTROL_FRAME_BYTES):
        raise ProtocolError("invalid Pi command reference envelope")
    expected = {field.name for field in fields(PiCommandRef)}
    refs = []
    for item in envelope["refs"]:
        if not isinstance(item, dict) or set(item) != expected:
            raise ProtocolError("invalid Pi command reference shape")
        refs.append(PiCommandRef(**item))
    ordinary = {key: value for key, value in message.items() if key != "command_refs"}
    batch = validate_batch(ordinary, source_id=source_id)
    return CommandPollBatch(batch.source_id, batch.cursor, batch.records,
                            batch.member_id, batch.member_cursor, batch.root_gap,
                            tuple(refs))


def validate_content_batch(message: dict, *, source_id: str) -> ContentPollBatch:
    """Require a separately versioned, bounded reference-only envelope.

    The ordinary v1 validator never accepts the extra field. A caller must
    negotiate ``pi-content-ref/1`` before selecting this path.
    """
    if not isinstance(message, dict) or "content_refs" not in message:
        raise ProtocolError("Pi content reference envelope is required")
    envelope = message["content_refs"]
    if (not isinstance(envelope, dict) or set(envelope) != {"schema_version", "refs"}
            or type(envelope["schema_version"]) is not int
            or envelope["schema_version"] != 1
            or not isinstance(envelope["refs"], list)
            or len(envelope["refs"]) > 48
            or len(encode_message(envelope).encode("utf-8")) > MAX_CONTROL_FRAME_BYTES):
        raise ProtocolError("invalid Pi content reference envelope")
    ordinary = {key: value for key, value in message.items() if key != "content_refs"}
    batch = validate_batch(ordinary, source_id=source_id)
    return ContentPollBatch(batch.source_id, batch.cursor, batch.records,
                            batch.member_id, batch.member_cursor, batch.root_gap,
                            tuple(envelope["refs"]))


def validate_batch(message: dict, *, source_id: str) -> PollBatch:
    fields = {"type", "protocol", "source_id", "cursor", "records"}
    if set(message) not in (fields, fields | {"member_id", "member_cursor"},
                            fields | {"root_gap"}):
        raise ProtocolError("invalid batch fields")
    if (message["type"] != "batch" or type(message["protocol"]) is not int
            or message["protocol"] != PROTOCOL_MAJOR or message["source_id"] != source_id):
        raise ProtocolError("batch source or protocol mismatch")
    records = message["records"]
    if not isinstance(records, list) or len(records) > MAX_BATCH_RECORDS:
        raise ProtocolError("invalid batch record count")
    # The initial durable cursor is empty. An idle source may acknowledge no
    # changes without inventing a new cursor, including on its first poll.
    cursor = message["cursor"]
    if cursor != "" or records:
        cursor = _cursor_text(cursor, "cursor")
    seen = set()
    for record in records:
        required = {"event_id", "revision", "kind", "time_ns", "payload"}
        if not isinstance(record, dict) or set(record) not in (required, required | {"identity"}):
            raise ProtocolError("invalid record fields")
        if "identity" in record:
            identity = record["identity"]
            if (not isinstance(identity, dict)
                    or not {"session_id"} <= set(identity) <=
                    {"session_id", "worker_id", "turn_id", "operation_id"}):
                raise ProtocolError("invalid record identity fields")
            for field, value in identity.items():
                _text(value, "record " + field, MAX_ID_CHARS)
        event_id = _text(record["event_id"], "event ID", MAX_ID_CHARS)
        if event_id in seen:
            raise ProtocolError("duplicate event ID within batch")
        seen.add(event_id)
        if type(record["revision"]) is not int or not 0 <= record["revision"] <= 2**31 - 1:
            raise ProtocolError("invalid record revision")
        if not isinstance(record["kind"], str) or record["kind"] not in KINDS:
            raise ProtocolError("unsupported record kind")
        stamp = record["time_ns"]
        if (not isinstance(stamp, str) or not DECIMAL_NS.fullmatch(stamp)
                or int(stamp) > INT64_MAX):
            raise ProtocolError("invalid nanosecond timestamp")
        if not isinstance(record["payload"], dict):
            raise ProtocolError("record payload must be an object")
        if len(encode_message(record).encode("utf-8")) > MAX_RECORD_BYTES:
            raise ProtocolError("record exceeds byte limit")
    member_id = message.get("member_id", "")
    member_cursor = message.get("member_cursor", "")
    if member_id:
        if not isinstance(member_id, str) or not MEMBER_ID.fullmatch(member_id):
            raise ProtocolError("invalid discovery member ID")
        member_cursor = _cursor_text(member_cursor, "discovery member cursor",
                                     allow_empty=True)
    elif "member_id" in message:
        raise ProtocolError("invalid discovery member ID")
    root_gap = message.get("root_gap", False)
    if "root_gap" in message and (root_gap is not True or len(records) != 1
                                   or records[0]["kind"] != "diagnostic"
                                   or records[0]["revision"] != 0
                                   or records[0].get("identity")
                                   or records[0]["payload"] != {
                                       "code": "possible_source_gap", "severity": "warning"}):
        raise ProtocolError("invalid discovery root gap")
    return PollBatch(source_id, cursor, tuple(records), member_id, member_cursor, root_gap)
