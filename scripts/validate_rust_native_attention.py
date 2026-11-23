#!/usr/bin/env python3
"""Qualify the Rust bridge against the actual tagged Python 0.2.8 receiver."""

import argparse
from http.server import ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import sqlite3
import stat
import subprocess
import sys
import tempfile
import threading
import time
from zipfile import ZipFile


TRACE = "a" * 32
SOURCE = "fixture-source"
SESSION = "fixture-session"
SERVICE = "codex"
PRIVATE = "private-fixture-marker"
IDENTITY = (
    "sender_id", "receiver_id", "service", "agent_id", "session_id",
    "trace_id", "turn_id", "source_id", "parent_agent_id",
)


def pending(path):
    with sqlite3.connect(f"{path.as_uri()}?mode=ro", uri=True) as outbox:
        return outbox.execute(
            "SELECT sequence, payload FROM receipts WHERE payload IS NOT NULL ORDER BY sequence"
        ).fetchall()


def run(app, work, *args, successful=True):
    result = subprocess.run(
        [str(app), "attention", "native", *map(str, args)],
        cwd=work,
        env={"HOME": str(work), "XDG_STATE_HOME": str(work / "state"), "PATH": os.defpath},
        text=True,
        capture_output=True,
        timeout=25,
    )
    assert (result.returncode == 0) == successful, result.stderr
    assert not result.stdout, "native sender printed a response into the hook stream"
    assert PRIVATE not in result.stderr
    return result.stderr


def verify(args, work):
    installed = work / "legacy"
    with ZipFile(args.wheel) as archive:
        archive.extractall(installed)
    sys.path.insert(0, str(installed))
    from observatory.agent_hierarchy import canonical_agent_id
    from observatory.core import Store, normalize_native
    from observatory.server import handler_for

    actor = canonical_agent_id("SDK", "fixture", "worker")
    initial = Store(work / "receiver.sqlite")
    replacement = Store(work / "replacement.sqlite")
    try:
        for store in (initial, replacement):
            now = time.time_ns()
            store.ingest([normalize_native(dict(
                trace_id=TRACE,
                span_id="1" * 16,
                name="Bash",
                start_unix_nano=now - 1000,
                end_unix_nano=now,
                status="OK",
                events=[],
                attributes={
                    "agent.id": actor, "session.id": SESSION, "tool.name": "Bash",
                    "gen_ai.tool.type": "shell", "process.exit.code": 0,
                },
                resource_attributes={"observatory.source_id": SOURCE},
            ), SERVICE)])

        token = "local_test_token_" + "ab" * 22
        token_file = work / "token"
        token_file.write_text(token + "\n", encoding="ascii")
        token_file.chmod(0o600)
        control = {"receiver": "initial", "fault": None, "writes": 0}
        original = handler_for(initial, token, quiet=True)
        other = handler_for(replacement, token, quiet=True)

        class FaultHandler(original):
            def _json(self, code, value):
                write = self.command == "POST" and code == 200 and value.get("accepted") == 1
                if write:
                    control["writes"] += 1
                    fault, control["fault"] = control["fault"], None
                    if fault == "lost":
                        self.close_connection = True
                        self.connection.shutdown(socket.SHUT_RDWR)
                        return
                    if fault == "replaced":
                        control["receiver"] = "replacement"
                return super()._json(code, value)

        def dispatch(*args):
            handler = FaultHandler if control["receiver"] == "initial" else other
            handler(*args)

        with ThreadingHTTPServer(("127.0.0.1", 0), dispatch) as http:
            thread = threading.Thread(target=http.serve_forever, daemon=True)
            thread.start()
            try:
                qualify_send(args.app, work, actor, initial, replacement, http, token_file, control)
            finally:
                http.shutdown()
                thread.join(timeout=5)
    finally:
        initial.close()
        replacement.close()


def qualify_send(app, work, actor, initial, replacement, http, token_file, control):
    binding_file = work / "binding.json"
    state = work / "outbox.sqlite"
    endpoint = f"http://127.0.0.1:{http.server_port}"
    binding_args = [
        "bind", "--receiver-protocol", "python-v0.2.8", "--endpoint", endpoint,
        "--token-file", token_file, "--output", binding_file, "--receiver-id", initial.receiver_id,
        "--service", SERVICE, "--agent-id", actor, "--session-id", SESSION,
        "--trace-id", TRACE, "--source-id", SOURCE,
    ]
    run(app, work, *binding_args)
    before = binding_file.read_bytes()
    assert stat.S_IMODE(binding_file.stat().st_mode) == 0o600
    assert "local_test_token" not in before.decode()
    run(app, work, *binding_args, successful=False)
    assert binding_file.read_bytes() == before
    binding = json.loads(before)

    def event(sequence, kind, **changes):
        return {
            **{key: binding[key] for key in IDENTITY},
            "schema": "agenturm-native-event-v1",
            "event_id": f"fixture-event-{sequence}",
            "sequence": sequence,
            "kind": kind,
            "request_id": "fixture-request",
            "time_unix_nano": 0,
            **changes,
        }

    def invoke(verb, payload=None, successful=True):
        command = [verb, "--binding", binding_file, "--token-file", token_file, "--state", state]
        if payload is not None:
            command.append(json.dumps(payload))
        return run(app, work, *command, successful=successful)

    control["fault"] = "lost"
    request = event(1, "approval_requested")
    invoke("send", request, successful=False)
    first = pending(state)
    assert len(first) == 1 and first[0][0] == 1
    assert len(initial.logs()) == 1
    assert control["writes"] == 1

    bad = event(2, "request_resolved", request_id="unrelated")
    invoke("send", bad, successful=False)
    assert pending(state) == first and len(initial.logs()) == 1

    invoke("retry")
    assert not pending(state)
    logs = initial.logs()
    assert len(logs) == 1, "lost reply replay changed the canonical record"
    assert control["writes"] == 2
    metadata = logs[0]["attributes"]
    assert logs[0]["body"] == ""
    assert metadata["observatory.native.kind"] == "approval_requested"
    assert metadata["observatory.native.source_time_ns"] == 0
    assert metadata["observatory.native.time_basis"] == "bridge-receipt"

    control["fault"] = "replaced"
    resolution = event(2, "request_resolved")
    invoke("send", resolution, successful=False)
    second = pending(state)
    assert len(second) == 1 and second[0][0] == 2
    assert len(initial.logs()) == 2 and len(replacement.logs()) == 0
    invoke("retry", successful=False)
    assert pending(state) == second and len(replacement.logs()) == 0

    control["receiver"] = "initial"
    invoke("retry")
    assert not pending(state)
    assert len(initial.logs()) == 2
    assert control["writes"] == 4
    invoke("send", resolution)
    assert control["writes"] == 4

    before = initial.native_lifecycle_inventory()
    assert before["receiver_id"] == initial.receiver_id and before["complete"]
    assert len(before["observations"]) == 1 and before["observations"][0]["resolved"]
    assert before["observations"][0]["request_kind"] == "approval_requested"

    assert "uncertain" in invoke("hook", event(3, "approval_requested", prompt=PRIVATE))
    assert "uncertain" in invoke("hook", event(3, "approval_requested", session_id="another-session"))
    assert not pending(state) and control["writes"] == 4
    assert initial.native_lifecycle_inventory() == before
    print("Verified private binding, durable lost reply, replaced receiver denial, ordered replay and metadata-only resolution")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", required=True, type=Path)
    parser.add_argument(
        "--wheel", type=Path,
        default=Path(__file__).resolve().parents[1] / "tests/fixtures/legacy/agenturm-0.2.8-py3-none-any.whl",
    )
    args = parser.parse_args()
    args.app = args.app.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="agenturm-native-bridge-") as path:
        verify(args, Path(path))


if __name__ == "__main__":
    main()
