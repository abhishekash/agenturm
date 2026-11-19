#!/usr/bin/env python3
"""Standalone, fixture-only action adapter for Agenturm's outbound contract proof."""
from __future__ import annotations

import json
import re
import struct
import sys
import urllib.error
import urllib.parse
import urllib.request


PLUGIN_ID = "example.fake-outbound"
MAX_FRAME = 64 * 1024
IDENT = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,127}\Z")
KEY = re.compile(r"[0-9a-f]{48}\Z")
RECEIPT = re.compile(r"[A-Za-z0-9._:-]{1,128}\Z")
ACTION = "write_fixture_metric"


def pairs(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def exact(count):
    chunks = []
    while count:
        part = sys.stdin.buffer.read(count)
        if not part:
            raise EOFError
        chunks.append(part)
        count -= len(part)
    return b"".join(chunks)


def receive():
    prefix = sys.stdin.buffer.read(4)
    if not prefix:
        return None
    if len(prefix) != 4:
        raise ValueError("partial frame prefix")
    size = struct.unpack(">I", prefix)[0]
    if not 0 < size <= MAX_FRAME:
        raise ValueError("invalid frame size")
    message = json.loads(exact(size), object_pairs_hook=pairs)
    if not isinstance(message, dict):
        raise ValueError("frame must contain an object")
    return message


def send(value):
    raw = json.dumps(value, ensure_ascii=False, allow_nan=False,
                     sort_keys=True, separators=(",", ":")).encode("utf-8")
    if not 0 < len(raw) <= MAX_FRAME:
        raise ValueError("reply exceeds frame limit")
    sys.stdout.buffer.write(struct.pack(">I", len(raw)))
    sys.stdout.buffer.write(raw)
    sys.stdout.buffer.flush()


def endpoint(value):
    if not isinstance(value, str) or len(value) > 300:
        raise ValueError("invalid endpoint")
    parsed = urllib.parse.urlsplit(value)
    if (parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or not parsed.port
            or parsed.netloc != f"127.0.0.1:{parsed.port}" or parsed.path != "/deliver"
            or parsed.query or parsed.fragment or parsed.username or parsed.password):
        raise ValueError("only exact loopback /deliver endpoint allowed")
    return value


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        raise ValueError("redirected delivery refused")


def response_json(response):
    raw = response.read(4097)
    if len(raw) > 4096:
        raise ValueError("sink reply exceeds budget")
    data = json.loads(raw, object_pairs_hook=pairs)
    if not isinstance(data, dict):
        raise ValueError("invalid sink reply")
    return data


def dispatch(opener, url, message):
    if (set(message) != {"type", "protocol", "delivery_id", "idempotency_key", "action", "value"}
            or message["type"] != "dispatch" or type(message["protocol"]) is not int
            or message["protocol"] != 1 or message["action"] != ACTION
            or not isinstance(message["delivery_id"], str) or not IDENT.fullmatch(message["delivery_id"])
            or not isinstance(message["idempotency_key"], str) or not KEY.fullmatch(message["idempotency_key"])
            or type(message["value"]) is not int or not 0 <= message["value"] <= 1_000_000):
        raise ValueError("invalid dispatch request")
    request = urllib.request.Request(
        url, data=json.dumps({"action": ACTION, "value": message["value"]},
                             separators=(",", ":")).encode(), method="POST",
        headers={"Content-Type": "application/json", "Idempotency-Key": message["idempotency_key"]})
    try:
        with opener.open(request, timeout=2.0) as response:
            if response.status not in (200, 201):
                raise ValueError("unexpected sink status")
            data = response_json(response)
            receipt = data.get("receipt")
            if set(data) != {"receipt"} or not isinstance(receipt, str) or not RECEIPT.fullmatch(receipt):
                raise ValueError("invalid sink receipt")
            status, code = "accepted", "accepted"
    except urllib.error.HTTPError as exc:
        receipt = ""
        if exc.code == 503:
            status, code = "retryable", "temporarily_unavailable"
        elif exc.code in (400, 409, 422):
            status, code = "terminal", "rejected"
        else:
            # A response failure after POST is not proof that the sink rejected
            # the action. Let the host retain an uncertain outcome.
            raise ValueError("unknown sink outcome") from exc
    return {"type": "dispatch_result", "protocol": 1,
            "delivery_id": message["delivery_id"], "status": status,
            "code": code, "receipt": receipt}


def lookup(opener, url, message):
    if (set(message) != {"type", "protocol", "idempotency_key"}
            or message["type"] != "lookup" or type(message["protocol"]) is not int
            or message["protocol"] != 1 or not isinstance(message["idempotency_key"], str)
            or not KEY.fullmatch(message["idempotency_key"])):
        raise ValueError("invalid lookup request")
    status_url = url.replace("/deliver", "/status") + "?" + urllib.parse.urlencode(
        {"key": message["idempotency_key"]})
    request = urllib.request.Request(status_url, headers={"Accept": "application/json"})
    with opener.open(request, timeout=2.0) as response:
        if response.status != 200:
            raise ValueError("status lookup failed")
        data = response_json(response)
    if data == {"status": "missing"}:
        status, receipt = "missing", ""
    elif (set(data) == {"status", "receipt"} and data["status"] == "accepted"
          and isinstance(data["receipt"], str) and RECEIPT.fullmatch(data["receipt"])):
        status, receipt = "accepted", data["receipt"]
    else:
        raise ValueError("invalid status result")
    return {"type": "lookup_result", "protocol": 1,
            "idempotency_key": message["idempotency_key"],
            "status": status, "receipt": receipt}


def run():
    hello = receive()
    if (not isinstance(hello, dict)
            or set(hello) != {"type", "protocol", "plugin_id", "destination_id", "action", "endpoint"}
            or hello["type"] != "hello" or type(hello["protocol"]) is not int
            or hello["protocol"] != 1 or hello["plugin_id"] != PLUGIN_ID
            or not isinstance(hello["destination_id"], str)
            or not IDENT.fullmatch(hello["destination_id"]) or hello["action"] != ACTION):
        raise ValueError("invalid action handshake")
    url = endpoint(hello["endpoint"])
    opener = urllib.request.build_opener(NoRedirect(), urllib.request.ProxyHandler({}))
    send({"type": "hello_ack", "protocol": 1, "plugin_id": PLUGIN_ID,
          "features": ["action-adapter/1"]})
    while True:
        message = receive()
        if message is None:
            return
        kind = message.get("type")
        if kind == "dispatch":
            send(dispatch(opener, url, message))
        elif kind == "lookup":
            send(lookup(opener, url, message))
        else:
            raise ValueError("unsupported action request")


if __name__ == "__main__":
    try:
        run()
    except Exception:
        # Never echo an endpoint, payload or idempotency key to stderr.
        print("fake outbound adapter stopped", file=sys.stderr)
        raise SystemExit(1)
