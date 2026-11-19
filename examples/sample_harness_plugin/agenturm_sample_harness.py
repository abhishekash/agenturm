#!/usr/bin/env python3
"""Standalone metadata-only sample harness adapter for source-adapter/1.

This artifact uses only the Python standard library. It is intentionally
independent of Agenturm's Python package and is activated only by an explicit
manifest and source configuration.
"""
from __future__ import annotations

import json
import hashlib
import os
import re
import stat
import struct
import sys
from pathlib import Path


PLUGIN_ID = "example.sample-harness"
MAX_CONTROL = 64 * 1024
MAX_LINE = 16 * 1024
MAX_SCAN = 1024 * 1024
MAX_FRAME = 4 * 1024 * 1024
IDENT = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,199}\Z")
CODE = re.compile(r"[a-z][a-z0-9_./-]{0,63}\Z")
STATES = {"started", "running", "paused", "completed", "failed", "interrupted", "cancelled"}
USAGE = {"input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens", "total_tokens"}
RELATIONSHIPS = {"delegated", "review"}
MESSAGE_ROLES = {"user", "assistant"}
TOOL_CATEGORIES = {"shell", "file", "mcp", "other"}
TOOL_RESULTS = {"started", "succeeded", "failed", "denied"}
CHECK_CATEGORIES = {"test", "lint", "build", "review", "other"}
CHECK_RESULTS = {"passed", "failed", "skipped", "unknown"}


def pairs(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def read_frame():
    prefix = sys.stdin.buffer.read(4)
    if not prefix:
        return None
    if len(prefix) != 4:
        raise ValueError("truncated control frame")
    size = struct.unpack(">I", prefix)[0]
    if not 0 < size <= MAX_CONTROL:
        raise ValueError("invalid control frame size")
    raw = sys.stdin.buffer.read(size)
    if len(raw) != size:
        raise ValueError("truncated control frame")
    message = json.loads(raw, object_pairs_hook=pairs)
    if not isinstance(message, dict):
        raise ValueError("control frame must be an object")
    return message


def encode(value):
    return json.dumps(value, ensure_ascii=False, allow_nan=False,
                      sort_keys=True, separators=(",", ":")).encode("utf-8")


def write_frame(value, limit):
    raw = encode(value)
    if not 0 < len(raw) <= limit:
        raise ValueError("data frame exceeds budget")
    sys.stdout.buffer.write(struct.pack(">I", len(raw)))
    sys.stdout.buffer.write(raw)
    sys.stdout.buffer.flush()


def identity(value):
    if not isinstance(value, str) or not IDENT.fullmatch(value):
        raise ValueError("invalid source identity")
    return value


def record(row):
    if not isinstance(row, dict) or not {"id", "session", "at_ns", "kind"} <= row.keys():
        raise ValueError("invalid source record")
    if set(row) - {"id", "session", "at_ns", "kind", "revision", "state", "reason_code", "usage",
                   "parent_session", "relation_kind", "role", "category", "result"}:
        raise ValueError("unknown source record field")
    event_id = identity(row["id"])
    session_id = identity(row["session"])
    stamp = row["at_ns"]
    if (not isinstance(stamp, str) or not stamp.isascii() or not stamp.isdecimal()
            or int(stamp) > 2**63 - 1):
        raise ValueError("invalid source timestamp")
    revision = row.get("revision", 0)
    if type(revision) is not int or not 0 <= revision <= 2**31 - 1:
        raise ValueError("invalid source revision")
    if row["kind"] == "lifecycle" and set(row) <= {
            "id", "session", "at_ns", "kind", "revision", "state", "reason_code"}:
        state = row.get("state")
        if state not in STATES:
            raise ValueError("invalid source state")
        payload = {"state": state}
        if "reason_code" in row:
            code = row["reason_code"]
            if not isinstance(code, str) or not CODE.fullmatch(code):
                raise ValueError("invalid source reason code")
            payload["reason_code"] = code
    elif row["kind"] == "usage" and set(row) <= {
            "id", "session", "at_ns", "kind", "revision", "usage"}:
        payload = row.get("usage")
        if (not isinstance(payload, dict) or not payload or not set(payload) <= USAGE
                or any(type(value) is not int or not 0 <= value <= 2**63 - 1
                       for value in payload.values())):
            raise ValueError("invalid source usage")
    elif row["kind"] == "relationship" and set(row) <= {
            "id", "session", "at_ns", "kind", "revision", "parent_session", "relation_kind"}:
        parent = identity(row.get("parent_session"))
        relation_kind = row.get("relation_kind")
        if not isinstance(relation_kind, str) or relation_kind not in RELATIONSHIPS:
            raise ValueError("invalid source relationship kind")
        payload = {"parent_session_id": parent, "kind": relation_kind}
    elif row["kind"] == "message" and set(row) <= {
            "id", "session", "at_ns", "kind", "revision", "role"}:
        role = row.get("role")
        if type(role) is not str or role not in MESSAGE_ROLES:
            raise ValueError("invalid source message role")
        payload = {"role": role}
    elif row["kind"] == "tool" and set(row) <= {
            "id", "session", "at_ns", "kind", "revision", "category", "result"}:
        category, result = row.get("category"), row.get("result")
        if (type(category) is not str or category not in TOOL_CATEGORIES
                or type(result) is not str or result not in TOOL_RESULTS):
            raise ValueError("invalid source tool metadata")
        payload = {"category": category, "result": result}
    elif row["kind"] == "check" and set(row) <= {
            "id", "session", "at_ns", "kind", "revision", "category", "result"}:
        category, result = row.get("category"), row.get("result")
        if (type(category) is not str or category not in CHECK_CATEGORIES
                or type(result) is not str or result not in CHECK_RESULTS):
            raise ValueError("invalid source check metadata")
        payload = {"category": category, "result": result}
    else:
        raise ValueError("unsupported source record kind")
    return {"event_id": "sample:" + event_id, "revision": revision,
            "kind": row["kind"], "time_ns": stamp,
            "identity": {"session_id": session_id}, "payload": payload}


def cursor_parts(cursor):
    if cursor == "":
        return None, 0, ""
    if not isinstance(cursor, str) or not re.fullmatch(r"[0-9]+:[0-9]+:[0-9]+:[0-9a-f]{64}", cursor):
        raise ValueError("invalid resume cursor")
    parts = cursor.split(":")
    device, inode, offset = map(int, parts[:3])
    if max(device, inode, offset) > 2**63 - 1:
        raise ValueError("resume cursor overflow")
    return (device, inode), offset, parts[3]


def tail_digest(stream, offset):
    stream.seek(max(0, offset - 256))
    return hashlib.sha256(stream.read(min(offset, 256))).hexdigest()


def poll(root, source_id, cursor, max_records, max_bytes):
    if type(max_records) is not int or not 1 <= max_records <= 256:
        raise ValueError("invalid record budget")
    if type(max_bytes) is not int or not 1 <= max_bytes <= MAX_FRAME:
        raise ValueError("invalid byte budget")
    prior_file, offset, prior_digest = cursor_parts(cursor)
    records = []
    try:
        stream = root.open("rb")
    except FileNotFoundError:
        if cursor:
            raise ValueError("previously observed source file is unavailable")
        return {"type": "batch", "protocol": 1, "source_id": source_id,
                "cursor": cursor, "records": []}
    with stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode):
            raise ValueError("configured source is not a regular file")
        file_id = (info.st_dev, info.st_ino)
        changed = (prior_file is not None and
                   (prior_file != file_id or offset > info.st_size
                    or prior_digest != tail_digest(stream, offset)))
        if changed:
            offset = 0
            gap_id = hashlib.sha256((cursor + "|" + str(file_id)).encode()).hexdigest()
            records.append({"event_id": "sample-gap:" + gap_id, "revision": 0,
                            "kind": "diagnostic", "time_ns": str(min(max(0, info.st_mtime_ns), 2**63 - 1)),
                            "payload": {"code": "possible_source_gap", "severity": "warning"}})
        position = offset
        stream.seek(offset)
        while len(records) < max_records and stream.tell() - offset < MAX_SCAN:
            start = stream.tell()
            line = stream.readline(MAX_LINE + 1)
            if not line:
                break
            if len(line) > MAX_LINE:
                raise ValueError("source line exceeds byte limit")
            if not line.endswith(b"\n"):
                break  # Resume this partial line when its newline arrives.
            row = json.loads(line, object_pairs_hook=pairs)
            candidate = record(row)
            next_cursor = f"{file_id[0]}:{file_id[1]}:{stream.tell()}:{tail_digest(stream, stream.tell())}"
            response = {"type": "batch", "protocol": 1, "source_id": source_id,
                        "cursor": next_cursor, "records": [*records, candidate]}
            if len(encode(response)) > max_bytes:
                if not records:
                    raise ValueError("source record exceeds host byte budget")
                stream.seek(start)
                break
            records.append(candidate)
            position = stream.tell()
        digest = tail_digest(stream, position)
    next_cursor = f"{file_id[0]}:{file_id[1]}:{position}:{digest}"
    response = {"type": "batch", "protocol": 1, "source_id": source_id,
                "cursor": next_cursor, "records": records}
    if len(encode(response)) > max_bytes:
        raise ValueError("source diagnostic exceeds host byte budget")
    return response


def run():
    configured = os.environ.get("AGENTURM_PLUGIN_SOURCE_ROOT")
    if not configured or not Path(configured).is_absolute():
        raise ValueError("configured source root required")
    root = Path(configured)
    source_id = None
    pending = None
    while True:
        message = read_frame()
        if message is None:
            return
        kind = message.get("type")
        if message.get("protocol") != 1 or type(message.get("protocol")) is not int:
            raise ValueError("protocol mismatch")
        if source_id is None:
            if (set(message) != {"type", "protocol", "plugin_id", "source_id", "max_frame_bytes"}
                    or kind != "hello" or message["plugin_id"] != PLUGIN_ID):
                raise ValueError("hello mismatch")
            source_id = identity(message["source_id"])
            if type(message["max_frame_bytes"]) is not int or not 1 <= message["max_frame_bytes"] <= MAX_FRAME:
                raise ValueError("invalid host frame limit")
            write_frame({"type": "hello_ack", "protocol": 1, "plugin_id": PLUGIN_ID,
                         "features": ["source-adapter/1", "configured-root/1"]},
                        message["max_frame_bytes"])
            continue
        if kind == "stop" and set(message) == {"type", "protocol"}:
            return
        if kind == "poll":
            if (set(message) != {"type", "protocol", "source_id", "cursor", "max_records", "max_bytes"}
                    or message["source_id"] != source_id or pending is not None):
                raise ValueError("invalid poll")
            response = poll(root, source_id, message["cursor"],
                            message["max_records"], message["max_bytes"])
            write_frame(response, message["max_bytes"])
            pending = response["cursor"]
        elif kind == "ack":
            if (set(message) != {"type", "protocol", "source_id", "cursor"}
                    or message["source_id"] != source_id or message["cursor"] != pending):
                raise ValueError("invalid ack")
            pending = None
        else:
            raise ValueError("unknown control message")


if __name__ == "__main__":
    try:
        run()
    except Exception:
        # Never print source paths or record contents to the host's stderr.
        sys.exit(2)
