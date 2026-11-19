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
MAX_CURSOR_CHARS = 512
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


def validate_batch(message: dict, *, source_id: str) -> PollBatch:
    if set(message) != {"type", "protocol", "source_id", "cursor", "records"}:
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
        cursor = _text(cursor, "cursor", MAX_CURSOR_CHARS)
    seen = set()
    for record in records:
        required = {"event_id", "revision", "kind", "time_ns", "payload"}
        if not isinstance(record, dict) or set(record) not in (required, required | {"identity"}):
            raise ProtocolError("invalid record fields")
        if "identity" in record:
            identity = record["identity"]
            if (not isinstance(identity, dict)
                    or not {"session_id"} <= set(identity) <= {"session_id", "worker_id", "turn_id"}):
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
    return PollBatch(source_id, cursor, tuple(records))
