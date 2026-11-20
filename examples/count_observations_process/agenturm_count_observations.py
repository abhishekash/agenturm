#!/usr/bin/env python3
"""Fixture-only count/ratio pane over host-derived, content-free facts."""
import json
import struct
import sys

LIMIT = 64 * 1024


def receive():
    head = sys.stdin.buffer.read(4)
    if len(head) != 4:
        raise EOFError
    length = struct.unpack(">I", head)[0]
    if length > LIMIT:
        raise ValueError("oversized frame")
    raw = sys.stdin.buffer.read(length)
    if len(raw) != length:
        raise EOFError
    return json.loads(raw)


def send(value):
    raw = json.dumps(value, separators=(",", ":")).encode()
    if len(raw) > LIMIT:
        raise ValueError("oversized reply")
    sys.stdout.buffer.write(struct.pack(">I", len(raw)) + raw)
    sys.stdout.buffer.flush()


def main():
    hello = receive()
    if (set(hello) != {"type", "protocol", "plugin_id", "pane_id"}
            or hello["type"] != "hello" or hello["protocol"] != 1
            or hello["pane_id"] != "count-observations"):
        return 1
    send({"type": "hello_ack", "protocol": 1,
          "plugin_id": hello["plugin_id"], "features": ["capability-adapter/1"]})
    while True:
        try:
            request = receive()
        except EOFError:
            return 0
        if (set(request) != {"type", "protocol", "seq", "facts"}
                or request["type"] != "facts" or request["protocol"] != 1
                or type(request["seq"]) is not int or not isinstance(request["facts"], dict)):
            return 1
        facts = request["facts"]
        if (set(facts) != {"state", "observed_events", "counts", "window_complete"}
                or facts["state"] not in ("ready", "degraded")
                or type(facts["observed_events"]) is not int
                or not 0 <= facts["observed_events"] <= 200
                or type(facts["window_complete"]) is not bool
                or not isinstance(facts["counts"], dict)
                or set(facts["counts"]) != {"users", "shell", "checks"}
                or any(type(value) is not int or not 0 <= value <= 200
                       for value in facts["counts"].values())):
            return 1
        counts = facts["counts"]
        coverage = ("page complete; source may be partial" if facts["window_complete"]
                    else "recent page partial")
        if counts["users"]:
            scaled = (counts["shell"] * 100 + counts["users"] // 2) // counts["users"]
            ratio = f"{scaled // 100}.{scaled % 100:02d}"
        else:
            ratio = "unavailable"
        rows = [
            {"key": "users", "cells": ["User messages", str(counts["users"])], "detail": ""},
            {"key": "checks", "cells": ["Failed checks", str(counts["checks"])], "detail": ""},
            {"key": "shell-per-user",
             "cells": ["Shell actions / User messages", ratio], "detail": ""},
        ]
        send({"type": "model", "protocol": 1, "seq": request["seq"],
              "state": facts["state"],
              "summary": f'{facts["observed_events"]} scoped events · {coverage} · observations only',
              "rows": rows})


if __name__ == "__main__":
    raise SystemExit(main())
