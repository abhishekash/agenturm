#!/usr/bin/env python3
"""Read a bounded, local HTTP fixture through Agenturm's source-adapter/1 pipe."""
from __future__ import annotations

import hashlib
import json
import os
import re
import stat
import struct
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


PLUGIN_ID = "example.fake-inbound"
FEATURES = ["source-adapter/1", "configured-root/1", "connector.inbound/1"]
MAX_CONTROL = 64 * 1024
MAX_HTTP = 1024 * 1024
MAX_FRAME = 4 * 1024 * 1024
IDENT = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,199}\Z")
TOKEN = re.compile(r"[A-Za-z0-9._:-]{0,200}\Z")
CODE = re.compile(r"[a-z][a-z0-9_./-]{0,63}\Z")
STAMP = re.compile(r"(?:0|[1-9][0-9]{0,18})\Z")
STATES = {"started", "running", "paused", "completed", "failed", "interrupted", "cancelled"}
USAGE = {"input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens", "total_tokens"}


def pairs(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def parse_json(raw):
    return json.loads(raw, object_pairs_hook=pairs,
                      parse_constant=lambda _value: (_ for _ in ()).throw(ValueError("non-finite JSON")))


def encode(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True,
                      separators=(",", ":"), allow_nan=False).encode("utf-8")


def exact(stream, count):
    chunks = []
    while count:
        piece = stream.read(count)
        if not piece:
            raise EOFError("truncated control frame")
        chunks.append(piece)
        count -= len(piece)
    return b"".join(chunks)


def read_frame():
    prefix = sys.stdin.buffer.read(4)
    if not prefix:
        return None
    if len(prefix) != 4:
        raise ValueError("truncated frame prefix")
    size = struct.unpack(">I", prefix)[0]
    if not 0 < size <= MAX_CONTROL:
        raise ValueError("invalid control frame length")
    message = parse_json(exact(sys.stdin.buffer, size))
    if not isinstance(message, dict):
        raise ValueError("control message must be an object")
    return message


def send(value, limit):
    raw = encode(value)
    if not 0 < len(raw) <= limit:
        raise ValueError("data frame exceeds host budget")
    sys.stdout.buffer.write(struct.pack(">I", len(raw)))
    sys.stdout.buffer.write(raw)
    sys.stdout.buffer.flush()


def identity(value):
    if not isinstance(value, str) or not IDENT.fullmatch(value):
        raise ValueError("invalid source identity")
    return value


def endpoint(value):
    if not isinstance(value, str) or len(value) > 300:
        raise ValueError("invalid endpoint")
    parsed = urllib.parse.urlsplit(value)
    if (parsed.scheme != "http" or parsed.hostname != "127.0.0.1"
            or not parsed.port or parsed.username or parsed.password
            or parsed.path != "/events" or parsed.query or parsed.fragment
            or parsed.netloc != f"127.0.0.1:{parsed.port}"):
        raise ValueError("only an exact loopback /events endpoint is permitted")
    return value


def source_record(row):
    if not isinstance(row, dict) or not {"id", "session", "at_ns", "kind"} <= set(row):
        raise ValueError("invalid source record")
    if set(row) - {"id", "session", "at_ns", "kind", "revision", "state", "reason_code", "usage"}:
        raise ValueError("unsupported source record field")
    event_id, session_id = identity(row["id"]), identity(row["session"])
    stamp = row["at_ns"]
    if not isinstance(stamp, str) or not STAMP.fullmatch(stamp) or int(stamp) > 2**63 - 1:
        raise ValueError("invalid source timestamp")
    revision = row.get("revision", 0)
    if type(revision) is not int or not 0 <= revision <= 2**31 - 1:
        raise ValueError("invalid source revision")
    if row["kind"] == "lifecycle" and set(row) <= {
            "id", "session", "at_ns", "kind", "revision", "state", "reason_code"}:
        state = row.get("state")
        if not isinstance(state, str) or state not in STATES:
            raise ValueError("invalid source lifecycle")
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
    else:
        raise ValueError("unsupported source record kind")
    return {"event_id": "inbound:" + event_id, "revision": revision,
            "kind": row["kind"], "time_ns": stamp,
            "identity": {"session_id": session_id}, "payload": payload}


def cursor_parts(cursor):
    if cursor == "" or re.fullmatch(r"gap:[0-9a-f]{64}", cursor):
        return "", "0"
    match = re.fullmatch(r"v1:([A-Za-z0-9._:-]{1,200}|~):((?:0|[1-9][0-9]{0,18}))", cursor)
    if match is None:
        raise ValueError("invalid resume cursor")
    token = match[1]
    if token == "~":
        token = ""
    if not TOKEN.fullmatch(token) or int(match[2]) > 2**63 - 1:
        raise ValueError("invalid resume cursor")
    return token, match[2]


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        raise ValueError("connector redirects are not permitted")


def fetch(opener, url, token):
    request = urllib.request.Request(url + "?" + urllib.parse.urlencode({"after": token}),
                                     headers={"Accept": "application/json"})
    try:
        with opener.open(request, timeout=2.0) as response:
            if response.status != 200:
                raise ValueError("unexpected fixture response")
            raw = response.read(MAX_HTTP + 1)
            if len(raw) > MAX_HTTP:
                raise ValueError("fixture response exceeds byte budget")
            return 200, parse_json(raw), {}
    except urllib.error.HTTPError as exc:
        if exc.code in (410, 429):
            return exc.code, None, {"retry_after": exc.headers.get("Retry-After", "1")}
        raise ValueError("fixture service failed") from exc


def poll(opener, url, source_id, cursor, max_records, max_bytes, retry_at):
    if type(max_records) is not int or not 1 <= max_records <= 256:
        raise ValueError("invalid host record budget")
    if type(max_bytes) is not int or not 1 <= max_bytes <= MAX_FRAME:
        raise ValueError("invalid host byte budget")
    token, last_ns = cursor_parts(cursor)
    if time.monotonic() < retry_at:
        return {"type": "batch", "protocol": 1, "source_id": source_id,
                "cursor": cursor, "records": []}, retry_at
    status, data, headers = fetch(opener, url, token)
    if status == 429:
        raw = headers["retry_after"]
        seconds = min(int(raw), 60) if re.fullmatch(r"[0-9]{1,3}", raw) else 1
        return {"type": "batch", "protocol": 1, "source_id": source_id,
                "cursor": cursor, "records": []}, time.monotonic() + max(1, seconds)
    if status == 410:
        if cursor == "" or cursor.startswith("gap:"):
            raise ValueError("fixture cannot supply a starting cursor")
        digest = hashlib.sha256(cursor.encode()).hexdigest()
        batch = {"type": "batch", "protocol": 1, "source_id": source_id,
                 "cursor": "gap:" + digest,
                 "records": [{"event_id": "inbound-gap:" + digest, "revision": 0,
                              "kind": "diagnostic", "time_ns": last_ns,
                              "payload": {"code": "cursor_expired", "severity": "warning"}}]}
        if len(encode(batch)) > max_bytes:
            raise ValueError("gap diagnostic exceeds host byte budget")
        return batch, 0.0
    if not isinstance(data, dict) or set(data) != {"records", "next"}:
        raise ValueError("invalid fixture page")
    records, next_token = data["records"], data["next"]
    if (not isinstance(records, list) or len(records) > max_records
            or not isinstance(next_token, str) or not TOKEN.fullmatch(next_token)):
        raise ValueError("invalid fixture page budget or cursor")
    if records and next_token == token:
        raise ValueError("fixture did not advance after records")
    projected = [source_record(item) for item in records]
    if projected:
        last_ns = max((last_ns, *(item["time_ns"] for item in projected)), key=int)
    next_cursor = "v1:" + (next_token or "~") + ":" + last_ns
    if not projected and next_token == token:
        next_cursor = cursor
    batch = {"type": "batch", "protocol": 1, "source_id": source_id,
             "cursor": next_cursor, "records": projected}
    if len(encode(batch)) > max_bytes:
        raise ValueError("fixture page exceeds host byte budget")
    return batch, 0.0


def run():
    root = os.environ.get("AGENTURM_PLUGIN_SOURCE_ROOT")
    if not root or not Path(root).is_absolute():
        raise ValueError("configured source root required")
    source = Path(root)
    if source.is_symlink() or not stat.S_ISREG(source.stat().st_mode) or source.stat().st_size > 4096:
        raise ValueError("invalid connector configuration file")
    config = parse_json(source.read_bytes())
    if not isinstance(config, dict) or set(config) != {"endpoint"}:
        raise ValueError("invalid connector configuration")
    url = endpoint(config["endpoint"])
    opener = urllib.request.build_opener(NoRedirect(), urllib.request.ProxyHandler({}))
    source_id = None
    pending = None
    retry_at = 0.0
    while True:
        message = read_frame()
        if message is None:
            return
        kind = message.get("type")
        if type(message.get("protocol")) is not int or message["protocol"] != 1:
            raise ValueError("protocol mismatch")
        if source_id is None:
            if (set(message) != {"type", "protocol", "plugin_id", "source_id", "max_frame_bytes"}
                    or kind != "hello" or message["plugin_id"] != PLUGIN_ID):
                raise ValueError("hello mismatch")
            source_id = identity(message["source_id"])
            if type(message["max_frame_bytes"]) is not int or not 1 <= message["max_frame_bytes"] <= MAX_FRAME:
                raise ValueError("invalid host frame budget")
            send({"type": "hello_ack", "protocol": 1, "plugin_id": PLUGIN_ID,
                  "features": FEATURES}, message["max_frame_bytes"])
            continue
        if kind == "stop" and set(message) == {"type", "protocol"}:
            return
        if kind == "poll":
            if (set(message) != {"type", "protocol", "source_id", "cursor", "max_records", "max_bytes"}
                    or message["source_id"] != source_id or pending is not None):
                raise ValueError("invalid poll")
            batch, retry_at = poll(opener, url, source_id, message["cursor"],
                                   message["max_records"], message["max_bytes"], retry_at)
            send(batch, message["max_bytes"])
            pending = batch["cursor"]
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
        # This fixture never exposes an endpoint, token or source body on stderr.
        print("fake inbound connector stopped", file=sys.stderr)
        raise SystemExit(1)
