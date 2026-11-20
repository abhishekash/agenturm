#!/usr/bin/env python3
"""Fixture-only capability process. Its input contains aggregate counts only."""
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
            or hello["pane_id"] != "prompt-signals"):
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
        if (set(facts) != {"state", "observed_events", "user_messages", "shell_actions",
                           "first_shell_after_user_ms", "window_complete"}
                or facts["state"] not in ("ready", "degraded")
                or any(type(facts[key]) is not int or facts[key] < 0
                       for key in ("observed_events", "user_messages", "shell_actions"))
                or (facts["first_shell_after_user_ms"] is not None and
                    (type(facts["first_shell_after_user_ms"]) is not int or
                     facts["first_shell_after_user_ms"] < 0))
                or type(facts["window_complete"]) is not bool):
            return 1
        coverage = "complete recent window" if facts["window_complete"] else "partial recent window"
        rows = [
            {"key": "users", "cells": ["Recorded user messages", str(facts["user_messages"]), coverage], "detail": ""},
            {"key": "shell", "cells": ["Recorded shell actions", str(facts["shell_actions"]), coverage], "detail": ""},
            {"key": "first-shell", "cells": ["First shell action after a user message",
                str(facts["first_shell_after_user_ms"]) + " ms" if facts["first_shell_after_user_ms"] is not None else "unavailable", coverage], "detail": ""},
        ]
        send({"type": "model", "protocol": 1, "seq": request["seq"],
              "state": facts["state"],
              "summary": f'{facts["observed_events"]} scoped events · {coverage} · observational signals only',
              "rows": rows})


if __name__ == "__main__":
    raise SystemExit(main())
