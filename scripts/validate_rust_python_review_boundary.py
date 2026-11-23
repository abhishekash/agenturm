"""Verify exact Rust Review authority against controlled Python receiver responses."""

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
import tempfile
import termios
import time
from urllib.parse import urlsplit

from validate_rust_private_eval_details import TerminalCells
from validate_rust_python_receiver_client import serve, stop


IDENTITY = "1" * 32
ACTOR = "ag1_" + "2" * 32
TRACE = "a" * 32
MEMBER = "b" * 32
TURN = "c" * 32
FOREIGN_TRACE = "d" * 32
IN_SCOPE = "CAPTURED_MEMBER_PROMPT_ONLY"
OUTSIDE = "UNRELATED_TURN_INPUT"
OWNER = dict(producer="opencode", agent_id=ACTOR, session_id="fixture-session")


def event(trace, stamp, text):
    return dict(id=f"event-{stamp}", source_id=f"source-{stamp}", service=OWNER["producer"],
                agent_id=ACTOR, session_id=OWNER["session_id"], receiver_id=IDENTITY,
                trace_id=trace, span_id=None, turn_id=TURN, time_unix_nano=stamp,
                domain="message", action="user", result="unknown", detail=text)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    parser.add_argument("--include-pty", action="store_true")
    args = parser.parse_args()
    app = args.bin_dir.resolve() / "agenturm"
    token = secrets.token_urlsafe(32)
    state = dict(mode="steady", activity=0, turn=0, run=0)
    roster = dict(service=OWNER["producer"], session_id=OWNER["session_id"], agent_id=ACTOR)

    class Receiver(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            assert self.headers.get("Authorization") == f"Bearer {token}"
            path = urlsplit(self.path).path
            mode = state["mode"]
            body = dict(receiver_id=IDENTITY)
            if path == "/api/health":
                body.update(ok=True, indexing={"ready": True}, review_trace_filter=True,
                            review_turn_filter=True, review_run_summary=True,
                            observed_time_unix_nano=time.time_ns(),
                            security={"authentication_required": True, "host_validation": True},
                            local_agents={"enabled": False, "capture_content": mode != "policy-denied"})
            elif path == "/api/review-turn":
                state["turn"] += 1
                changed = ((mode == "turn-drift" and state["activity"] > 0)
                           or (mode == "turn-prepublish" and state["turn"] >= 3)
                           or mode == "turn-redact")
                members = [TRACE] if changed else [TRACE, MEMBER]
                if mode == "reordered-steps" and state["turn"] % 2 == 0:
                    members.reverse()
                if mode == "duplicate-steps":
                    members.append(TRACE)
                body["turn"] = dict(trace_id=TRACE, turn_id=TURN, scope_owner=OWNER, step_trace_ids=members)
            elif path == "/api/review-run":
                state["run"] += 1
                owner = OWNER
                if mode == "run-loss" and state["activity"] > 0:
                    owner = None
                elif mode == "wrong-run-owner":
                    owner = dict(OWNER, agent_id="ag1_" + "3" * 32)
                body["run"] = dict(trace_id=TRACE, review_owner=owner)
            elif path == "/api/activity":
                if "review=1" in self.path:
                    state["activity"] += 1
                    body.update(activity=[event(TRACE, 10, IN_SCOPE), event(MEMBER, 20, IN_SCOPE)],
                                review_context=[event(FOREIGN_TRACE, 99, OUTSIDE)], next_cursor=None)
                else:
                    body.update(activity=[], next_cursor=None)
            elif path == "/api/sessions":
                body["sessions"] = [dict(roster, run_state="working", last_unix_nano=20)]
            elif path == "/api/connections":
                body["connections"] = []
            elif path == "/api/traces":
                body.update(traces=[dict(roster, trace_id=TRACE, turn_id=TURN, status="running")], has_more=False)
            else:
                raise AssertionError(path)
            data = json.dumps(body).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("X-Agenturm-Receiver-ID", IDENTITY)
            self.end_headers()
            self.wfile.write(data)

    server = HTTPServer(("127.0.0.1", 0), Receiver)
    thread = serve(server)
    try:
        with tempfile.TemporaryDirectory(prefix="agenturm-review-boundary-") as temp:
            root = Path(temp)
            token_file = root / "token"
            token_file.write_text(token + "\n")
            token_file.chmod(0o600)
            flags = ["--receiver-protocol", "python-v0.2.8", "--token-file", token_file,
                     "--endpoint", f"http://127.0.0.1:{server.server_port}"]
            env = {**os.environ, "TERM": "xterm-256color", "HOME": str(root), "XDG_STATE_HOME": str(root / "state")}

            def mode(name):
                state.update(mode=name, activity=0, turn=0, run=0)

            def run(*args, failure=None):
                result = subprocess.run([app, *args], env=env, capture_output=True, text=True, timeout=18)
                assert OUTSIDE not in result.stdout + result.stderr and token not in result.stdout + result.stderr
                if failure:
                    assert result.returncode != 0 and failure in result.stderr and not result.stdout, result.stderr
                else:
                    assert result.returncode == 0, result.stderr
                return result.stdout

            def review(selector):
                return ["review", selector, TRACE, "--service", OWNER["producer"], "--session-id", OWNER["session_id"],
                        "--agent-id", ACTOR, "--format", "json", *flags]

            mode("steady")
            allowed = json.loads(run(*review("--turn-of-trace")))
            assert allowed["request"]["source"]["trace_id"] == MEMBER and allowed["request"]["text"] == IN_SCOPE
            assert allowed["window"]["count"] == 2 and "turn_steps" not in allowed["scope"]
            mode("reordered-steps")
            assert json.loads(run(*review("--turn-of-trace")))["request"]["text"] == IN_SCOPE
            for drift in ("turn-drift", "turn-prepublish"):
                mode(drift)
                run(*review("--turn-of-trace"), failure="exact turn membership changed")
            mode("duplicate-steps")
            run(*review("--turn-of-trace"), failure="execution identity")
            assert state["activity"] == 0
            mode("wrong-run-owner")
            run(*review("--trace-id"), failure="execution identity")
            assert state["activity"] == 0
            mode("run-loss")
            run(*review("--trace-id"), failure="owner or exact turn membership changed")
            mode("policy-denied")
            hidden = json.loads(run(*review("--trace-id")))
            assert hidden["request"] is None and (state["run"], state["activity"]) == (0, 0)

            mode("steady")
            receipt = root / "review.json"
            receipt.write_text(run(*review("--trace-id")))
            span = root / "seed.jsonl"
            span.write_text(json.dumps(dict(trace_id="e" * 32, span_id="e" * 16,
                                            start_unix_nano="1", end_unix_nano="2", status="OK")) + "\n")
            db = root / "history.sqlite"
            run("import-jsonl", "--db", db, "--file", span, "--source-id", "otlp.fixture", "--session-id", "fixture")
            draft = root / "draft.json"
            run("task", "draft", "--db", db, "--review-file", receipt, "--output", draft, *flags)
            mode("run-loss")
            run("task", "approve-draft", draft, "--db", db, *flags, failure="owner or exact turn membership changed")
            mode("steady")
            receipt.write_text(json.dumps(allowed))
            turn_draft = root / "turn-draft.json"
            run("task", "draft", "--db", db, "--review-file", receipt, "--output", turn_draft, *flags)
            mode("turn-drift")
            run("task", "approve-draft", turn_draft, "--db", db, *flags, failure="exact turn membership changed")
            assert not json.loads(run("task", "list", "--db", db))["items"]
            if args.include_pty:
                for size in ((120, 32), (60, 18)):
                    mode("steady")
                    verify_pty(app, flags, env, size, mode)
    finally:
        stop(server, thread)
    print("PASS Python Review member, owner, policy and continuity boundaries")


def verify_pty(app, flags, env, size, update_mode):
    width, height = size
    master, slave = pty.openpty()
    termios_before = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
    child = subprocess.Popen([app, "cockpit", *flags], env=env, stdin=slave, stdout=slave, stderr=slave)
    terminal = TerminalCells(width, height)

    def wait_for(markers, absent=()):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if select.select([master], [], [], .05)[0]:
                terminal.feed(os.read(master, 16384))
            text = terminal.contents()
            assert OUTSIDE not in text
            if all(marker in text for marker in markers) and all(marker not in text for marker in absent):
                return
            assert child.poll() is None, text
        raise AssertionError((markers, terminal.contents()))

    try:
        wait_for(("Agenturm / Agents", "opencode / fixture-session"))
        os.write(master, b"\r4")
        wait_for(("Agenturm / Traces", TRACE))
        os.write(master, b"\x14")
        wait_for(("verified Review", IN_SCOPE))
        update_mode("turn-redact")
        wait_for(("Agenturm / Traces",), absent=(IN_SCOPE, "verified Review"))
        os.write(master, b"q")
        assert child.wait(timeout=8) == 0 and termios.tcgetattr(slave) == termios_before
    finally:
        if child.poll() is None:
            child.kill()
            child.wait(timeout=8)
        os.close(master)
        os.close(slave)


if __name__ == "__main__":
    main()
