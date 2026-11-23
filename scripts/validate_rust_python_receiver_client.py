"""Exercise Rust attach against an unchanged Python v0.2.8 receiver on loopback."""

import argparse
import fcntl
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import pty
import secrets
import select
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time

from validate_rust_private_eval_details import TerminalCells


PRIVATE = "PRIVATE_LEGACY_RECEIVER_BODY"
SESSION = "same-session"
ACTOR = "ag1_" + "2" * 32
OTHER_ACTOR = "ag1_" + "3" * 32
TRACE = "a" * 32
OTHER_TRACE = "b" * 32
TURN = "d" * 32


def wait_for_line(process):
    assert select.select([process.stdout], [], [], 12)[0], "Missing attention change"
    return json.loads(process.stdout.readline())


def run_cockpit(command, environment, columns, rows):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
    original = termios.tcgetattr(slave)
    process = subprocess.Popen(command, env=environment, stdin=slave, stdout=slave, stderr=slave)
    terminal = TerminalCells(columns, rows)

    def frame(*markers):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if select.select([master], [], [], .05)[0]:
                terminal.feed(os.read(master, 8192))
            content = terminal.contents()
            assert PRIVATE not in content
            if all(marker in content for marker in markers):
                return content
            assert process.poll() is None, (markers, content)
        raise AssertionError((markers, terminal.contents()))

    try:
        initial = frame("Agenturm / Agents", f"codex / {SESSION}", f"sdk-other / {SESSION}")
        if f"› codex / {SESSION}" not in initial:
            os.write(master, b"j")
        frame(f"› codex / {SESSION}")
        os.write(master, b"\r")
        selected = frame("Agenturm / Activity", f"codex / {SESSION}", "approval requested")
        assert "sdk-other" not in selected
        os.write(master, b"8")
        frame("Agenturm / Traces", f"codex / {SESSION}")
        os.write(master, b"w")
        selected = frame("Agenturm / Pending", "Awaiting approval")
        assert "sdk-other" not in selected
        os.write(master, b"\x1b")
        frame("Agenturm / Agents", f"› codex / {SESSION}")
        os.write(master, b"q" if columns > 60 else b"\x03")
        assert process.wait(timeout=8) == 0
        assert termios.tcgetattr(slave) == original
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=8)
        os.close(master)
        os.close(slave)


def serve(server):
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return thread


def stop(server, thread):
    server.shutdown()
    server.server_close()
    thread.join(timeout=8)
    assert not thread.is_alive()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    parser.add_argument("--python-root", type=Path, required=True, help="Python v0.2.8 checkout")
    args = parser.parse_args()
    app = args.bin_dir.resolve(strict=True) / "agenturm"
    python_root = args.python_root.resolve(strict=True)
    sys.path.insert(0, str(python_root))
    from observatory.core import Store, normalize_native
    from observatory.native_lifecycle import event_log
    from observatory.server import BoundedHTTPServer, handler_for
    import observatory.server
    assert Path(observatory.server.__file__).resolve().is_relative_to(python_root)

    with tempfile.TemporaryDirectory(prefix="agenturm-python-receiver-client-") as directory:
        root = Path(directory)
        token = secrets.token_urlsafe(32)
        token_file = root / "receiver.token"
        token_file.write_text(token + "\n")
        token_file.chmod(0o600)
        state = root / "rust-state"
        env = {**os.environ, "HOME": str(root), "XDG_STATE_HOME": str(state),
               "TERM": "xterm-256color", "HTTP_PROXY": "http://127.0.0.1:1",
               "HTTPS_PROXY": "http://127.0.0.1:1", "NO_PROXY": ""}
        store = Store(root / "legacy.sqlite")
        server = BoundedHTTPServer(("127.0.0.1", 0), handler_for(store, token, quiet=True))
        thread = serve(server)

        def flags(port=server.server_port, explicit=True):
            result = ["--endpoint", f"http://127.0.0.1:{port}", "--token-file", str(token_file)]
            if explicit:
                result.extend(["--receiver-protocol", "python-v0.2.8"])
            return result

        def run(argv, *, succeeds=True):
            result = subprocess.run([app, *argv], env=env, capture_output=True, text=True, timeout=15)
            assert (result.returncode == 0) == succeeds, (argv, result.stderr)
            assert all(marker not in result.stdout + result.stderr for marker in [PRIVATE, token])
            return result

        def publish(sequence, kind):
            stamp = time.time_ns()
            store.ingest_logs([event_log(dict(receiver_id=store.receiver_id, sender_id="native-sender",
                event_id=f"event-{sequence}", source_id="local-codex", service="codex", agent_id=ACTOR,
                session_id=SESSION, trace_id=TRACE, turn_id=TURN, source_order="sender-sequence",
                source_clock="receiver", time_unix_nano=stamp, received_time_unix_nano=stamp,
                sequence=sequence, kind=kind, request_id="request-one", request_kind="approval_requested"))])

        try:
            stamp = time.time_ns() - 5_000_000_000
            for source, trace, actor in [("codex", TRACE, ACTOR), ("sdk-other", OTHER_TRACE, OTHER_ACTOR)]:
                store.ingest([normalize_native(dict(trace_id=trace, span_id="4" * 16,
                    name=PRIVATE, start_unix_nano=stamp, end_unix_nano=stamp+1, status="OK",
                    attributes={"session.id": SESSION, "agent.id": actor,
                                "gen_ai.input.messages": PRIVATE}, events=[], links=[]), source)])
            store.ingest_logs([dict(trace_id=OTHER_TRACE, span_id=None, time_unix_nano=stamp+2,
                service="sdk-other", scope_name="fixture", severity_number=9, severity_text="INFO",
                body=PRIVATE, event_name="agent.activity", attributes={"session.id": SESSION,
                    "agent.id": OTHER_ACTOR, "agent.activity.domain": "shell",
                    "agent.activity.action": "execute", "agent.activity.subject": PRIVATE},
                resource_attributes={})])
            publish(1, "approval_requested")
            assert store.native_lifecycle_inventory()["observations"], "Tagged receiver failed to seed exact request"

            run(["cockpit", "--once", *flags(explicit=False)], succeeds=False)
            summary = json.loads(run(["cockpit", "--once", "--json", *flags()]).stdout)
            assert summary["format"] == "agenturm-python-receiver-live/1" and summary["ready"]
            assert "snapshot_id" not in summary and summary["receiver_id"] == store.receiver_id
            owners = {(row["service"], row["session_id"]) for row in summary["sessions"]}
            assert {("codex", SESSION), ("sdk-other", SESSION)} <= owners, summary
            assert {row["trace_id"] for row in summary["traces"]} >= {TRACE, OTHER_TRACE}, summary
            assert len(summary["attention"]["pending"]) == 1
            assert "bounded live metadata" in run(["cockpit", "--once", *flags()]).stdout
            for columns, rows in ((120, 32), (60, 18)):
                run_cockpit([app, "cockpit", *flags()], env, columns, rows)

            # Push the native request outside the latest Activity page.
            noise_time = time.time_ns()
            store.ingest_logs([dict(trace_id=OTHER_TRACE, span_id=None, time_unix_nano=noise_time+i,
                service="sdk-other", scope_name="fixture", severity_number=9, severity_text="INFO", body=PRIVATE,
                event_name="agent.activity", attributes={"session.id": SESSION, "agent.id": OTHER_ACTOR,
                    "agent.activity.domain": "message", "agent.activity.action": "assistant",
                    "observatory.record_id": str(i)}, resource_attributes={}) for i in range(210)])
            assert not any("native_lifecycle" in row for row in store.activity_page(limit=200)["activity"])
            watch = subprocess.Popen([app, "attention", "watch", *flags(), "--interval-ms", "100", "--max-polls", "30"],
                env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                first = wait_for_line(watch)
                assert first["ready"] and first["pending"][0]["turn_id"] == TURN
                assert first["pending"][0]["request_id"] == "request-one"
                publish(2, "request_resolved")
                change = wait_for_line(watch)
                if not change["ready"]:
                    assert change["partial"] and change["pending"] == first["pending"], change
                    change = wait_for_line(watch)
                assert change["type"] == "change" and change["ready"] and change["pending"] == [], change
                out, error = watch.communicate(timeout=8)
                assert watch.returncode == 0 and all(marker not in out+error for marker in [PRIVATE, token])
            finally:
                if watch.poll() is None:
                    watch.kill()
                    watch.wait(timeout=8)
            assert not state.exists(), "Live reader unexpectedly created durable work state"
        finally:
            stop(server, thread)
            store.close()

        trusted = "2" * 32
        foreign = "3" * 32
        health = dict(ok=True, receiver_id=trusted, indexing={"ready": True}, observed_time_unix_nano=123,
                      local_agents={"enabled": False}, native_lifecycle_v1=True,
                      security={"authentication_required": True, "host_validation": True})

        def synthetic(mode):
            requests = []

            class Handler(BaseHTTPRequestHandler):
                def log_message(self, *_):
                    pass

                def do_GET(self):
                    requests.append(self.path)
                    assert self.headers.get("Authorization") == f"Bearer {token}"
                    identity = trusted
                    status = 200
                    responses = {
                        "/api/health": health,
                        "/api/sessions?limit=64": {"sessions": []},
                        "/api/connections": {"connections": []},
                        "/api/activity?limit=200": {"activity": []},
                        "/api/traces?limit=100": {"traces": []},
                        "/api/native-lifecycle": {"observations": [], "receiver_id": trusted, "complete": False},
                    }
                    payload = responses[self.path]
                    if mode == "swap" and requests.count("/api/health") == 3:
                        identity = foreign
                        payload = {**health, "receiver_id": foreign}
                    elif mode == "mismatch":
                        payload = {**health, "receiver_id": foreign}
                    elif mode == "unready":
                        payload = {**health, "indexing": {"ready": False}}
                    elif mode == "redirect":
                        status = 302
                    elif mode == "unprotected":
                        payload = {**health, "security": {"authentication_required": False, "host_validation": True}}
                    elif mode == "unavailable" and self.path == "/api/native-lifecycle":
                        status = 503
                    body = json.dumps(payload).encode()
                    self.send_response(status)
                    if status == 302:
                        self.send_header("Location", "/api/connections")
                    if mode != "missing":
                        self.send_header("X-Agenturm-Receiver-ID", identity)
                    if mode == "duplicate":
                        self.send_header("X-Agenturm-Receiver-ID", identity)
                    self.send_header("Content-Type", "application/json; charset=utf-8")
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)

            receiver = HTTPServer(("127.0.0.1", 0), Handler)
            background = serve(receiver)
            try:
                args = flags(receiver.server_port)
                if mode in {"unready", "partial", "unavailable"}:
                    probe = run(["attention", "watch", "--once", *args], succeeds=False)
                    assert json.loads(probe.stdout)["partial"] and not json.loads(probe.stdout)["ready"]
                    if mode == "unready":
                        assert "/api/native-lifecycle" not in requests
                else:
                    result = run(["cockpit", "--once", "--json", *args], succeeds=False)
                    assert not result.stdout
                    if mode == "redirect":
                        assert requests == ["/api/health"], requests
            finally:
                stop(receiver, background)

        for mode in ("missing", "duplicate", "mismatch", "swap", "redirect", "unprotected", "unready", "partial", "unavailable"):
            synthetic(mode)
        token_file.chmod(0o644)
        assert "owner-only" in run(["cockpit", "--once", *flags()], succeeds=False).stderr
        print("PASS tagged Python receiver, isolated live metadata, exact requests, two PTYs and denied protocol/identity changes", flush=True)


if __name__ == "__main__":
    main()
