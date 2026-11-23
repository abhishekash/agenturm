"""Exercise opt-in Rust desktop attention using the tagged Python receiver."""

import argparse
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
import time

from validate_rust_python_receiver_client import serve, stop


PRIVATE = "PRIVATE_DESKTOP_FIXTURE_BODY"
RECEIVER = "a" * 32
SCOPE = dict(source_id="local-codex", service="codex", agent_id="ag1_" + "c" * 32,
             session_id="desktop-session", trace_id="d" * 32, turn_id="e" * 32)


def request(receiver, sender, stamp, *, sequence=1, resolved=False, clock="receiver"):
    return dict(schema=1, receiver_id=receiver, sender_id=sender,
                event_id=f"{sender}-event-{sequence}", request_id=f"{sender}-request",
                sequence=sequence, source_order="sender-sequence", source_clock=clock,
                time_unix_nano=stamp, received_time_unix_nano=time.time_ns(),
                kind="request_resolved" if resolved else "approval_requested",
                request_kind="approval_requested", resolved=resolved, **SCOPE)


def check_real(poll, token, notifier_log, store_type, event_log, server_type, handler):
    store = store_type(notifier_log.parent / "python-history.sqlite")
    server = server_type(("127.0.0.1", 0), handler(store, token, quiet=True))
    thread = serve(server)
    port = server.server_port

    def publish(sender, stamp, clock="receiver"):
        row = event_log(request(store.receiver_id, sender, stamp, clock=clock))
        row["body"] = PRIVATE
        store.ingest_logs([row])

    try:
        publish("initial", time.time_ns())
        assert poll(port, desktop=False)["ready"] and not notifier_log.exists()
        first = poll(port)
        assert first["ready"] and len(first["pending"]) == 1 and "desktop_delivery" not in first, first
        assert not notifier_log.exists()
        publish("backfilled", time.time_ns() - 25_000_000_000)
        publish("unknown-clock", time.time_ns(), "unknown")
        publish("future-clock", time.time_ns() + 25_000_000_000)
        withheld = poll(port)
        assert len(withheld["pending"]) == 4 and withheld["desktop_delivery"] == "deferred_source_clock", withheld
        assert not notifier_log.exists()
        publish("fresh", time.time_ns())
        delivered = poll(port)
        assert len(delivered["pending"]) == 5 and delivered["desktop_delivery"] == "delivered", delivered
        assert poll(port)["desktop_delivery"] == "deferred_source_clock"
        assert len(notifier_log.read_text().splitlines()) == 1

        publish("notify-error", time.time_ns())
        assert poll(port, exit_code="1")["desktop_delivery"] == "failed"
        assert poll(port)["desktop_delivery"] == "deferred_source_clock"
        assert len(notifier_log.read_text().splitlines()) == 2
    finally:
        stop(server, thread)
        store.close()


def check_revalidation(poll, token, notifier_log):
    mode = {"reads": 0, "change": None, "row": None, "ready": True}

    class Receiver(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            assert self.headers.get("Authorization") == f"Bearer {token}"
            if self.path == "/api/health":
                body = dict(ok=True, receiver_id=RECEIVER, indexing={"ready": mode["ready"]},
                            observed_time_unix_nano=time.time_ns(), native_lifecycle_v1=True,
                            security={"authentication_required": True, "host_validation": True})
            else:
                assert self.path == "/api/native-lifecycle"
                mode["reads"] += 1
                row = mode["row"]
                if row and mode["reads"] == mode["change"]:
                    row = request(RECEIVER, row["sender_id"], time.time_ns(), sequence=2, resolved=True)
                body = dict(receiver_id=RECEIVER, complete=True, observations=[row] if row else [])
            payload = json.dumps(body).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.send_header("X-Agenturm-Receiver-Id", RECEIVER)
            self.end_headers()
            self.wfile.write(payload)

    server = HTTPServer(("127.0.0.1", 0), Receiver)
    thread = serve(server)
    port = server.server_port
    try:
        mode["ready"] = False
        incomplete = poll(port, succeeds=False)
        assert not incomplete["ready"] and incomplete["desktop_delivery"] == "deferred_incomplete", incomplete
        mode["ready"] = True
        assert poll(port)["ready"]

        mode.update(reads=0, change=2, row=request(RECEIVER, "change-before-claim", time.time_ns()))
        assert poll(port)["desktop_delivery"] == "deferred_history_changed"
        assert len(notifier_log.read_text().splitlines()) == 2
        mode.update(reads=0, change=None)
        assert poll(port)["desktop_delivery"] == "delivered"

        mode.update(reads=0, change=3, row=request(RECEIVER, "change-before-delivery", time.time_ns()))
        assert poll(port)["desktop_delivery"] == "deferred_history_changed"
        assert len(notifier_log.read_text().splitlines()) == 3
        mode.update(reads=0, change=None)
        assert "desktop_delivery" not in poll(port), "A discarded attempt was retried after reopen"
        assert len(notifier_log.read_text().splitlines()) == 3
    finally:
        stop(server, thread)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    parser.add_argument("--python-root", type=Path, required=True, help="Tagged Python v0.2.8 checkout")
    args = parser.parse_args()
    app = args.bin_dir.resolve(strict=True) / "agenturm"
    python_root = args.python_root.resolve(strict=True)
    sys.path.insert(0, str(python_root))
    from observatory.core import Store
    from observatory.native_lifecycle import event_log
    from observatory.server import BoundedHTTPServer, handler_for
    import observatory.server
    assert Path(observatory.server.__file__).resolve().is_relative_to(python_root)

    with tempfile.TemporaryDirectory(prefix="agenturm-python-desktop-") as temp:
        root = Path(temp)
        token = secrets.token_urlsafe(32)
        token_file = root / "receiver.token"
        token_file.write_text(token + "\n")
        token_file.chmod(0o600)
        notifier_log = root / "notify.log"
        notifier = root / "notify-send"
        notifier.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$AGENTURM_TEST_NOTIFY_LOG"\nexit "$AGENTURM_TEST_NOTIFY_STATUS"\n')
        notifier.chmod(0o700)
        state = root / "state"
        env = {**os.environ, "HOME": str(root), "XDG_STATE_HOME": str(state), "PATH": str(root),
               "AGENTURM_TEST_NOTIFY_LOG": str(notifier_log)}

        def poll(port, *, desktop=True, succeeds=True, exit_code="0"):
            command = [app, "attention", "watch", "--once", "--endpoint", f"http://127.0.0.1:{port}",
                       "--token-file", token_file, "--receiver-protocol", "python-v0.2.8"]
            if desktop:
                command.append("--desktop")
            result = subprocess.run(command, env={**env, "AGENTURM_TEST_NOTIFY_STATUS": exit_code},
                                    capture_output=True, text=True, timeout=16)
            assert (result.returncode == 0) == succeeds, result.stderr
            assert PRIVATE not in result.stdout + result.stderr and token not in result.stdout + result.stderr
            return json.loads(result.stdout)

        check_real(poll, token, notifier_log, Store, event_log, BoundedHTTPServer, handler_for)
        check_revalidation(poll, token, notifier_log)
        archives = list(state.rglob("*.desktop-attention.sqlite"))
        assert len(archives) == 2 and all(archive.stat().st_mode & 0o777 == 0o600 for archive in archives)
        for line in notifier_log.read_text().splitlines():
            assert line == "--app-name=Agenturm -- Agenturm attention 1 recorded attention request(s). Open Agenturm to inspect and respond in your agent terminal."
        assert PRIVATE not in notifier_log.read_text() and token not in notifier_log.read_text()
    print("PASS Python v0.2.8 desktop clock proofs, durable attempts, and pre/post-claim inventory revalidation")


if __name__ == "__main__":
    main()
