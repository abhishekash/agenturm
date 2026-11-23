"""Exercise Rust investigations with the tagged Python 0.2.8 HTTP receiver."""

import argparse
import copy
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
import secrets
import shlex
import subprocess
import sys
import tempfile
from urllib.parse import parse_qs, urlsplit

from validate_rust_python_receiver_client import serve, stop


TRACE = "a" * 32
SECRET = "SERVER_COMMAND_MUST_NOT_BE_PRINTED"
REMEMBER = "Keep the public endpoint stable"


def seed(store, normalize):
    def span(index, extra):
        return normalize(dict(trace_id=TRACE, span_id=f"{index:016x}", name="tool", status="UNSET",
                              start_unix_nano=index, end_unix_nano=index + 1,
                              attributes={"session.id": "session", **extra}), "fixture")

    failed = {"tool.name": "Bash", "gen_ai.tool.type": "shell", "process.command_line": "pytest tests -q",
              "process.exit.code": 1, "stderr": "Fixture reported retry failure"}
    store.ingest([span(1, failed), span(2, failed), span(3, {
        "tool.name": "rename_file", "gen_ai.tool.type": "file", "tool.action": "rename",
        "agent.activity.subject": "src/new.py", "tool.arguments": {"old_path": "src/old.py", "new_path": "src/new.py"},
        "process.exit.code": 0,
    })])
    store.ingest_logs([dict(trace_id=None, span_id=None, time_unix_nano=10, service="fixture",
        scope_name="fixture", event_name="agent.activity", body=REMEMBER, severity_number=9,
        severity_text="INFO", resource_attributes={}, attributes={"session.id": "session",
        "agent.activity.domain": "message", "agent.activity.action": "user",
        "agent.activity.subject": REMEMBER})])


def real_receiver(app, token, token_path, server_type, handler_for, store, encode_reference):
    server = server_type(("127.0.0.1", 0), handler_for(store, token, quiet=True))
    thread = serve(server)
    endpoint = f"http://127.0.0.1:{server.server_port}"

    def run(kind, query, *args, success=True):
        return run_cli(app, token, token_path, endpoint, kind, query, *args, success=success)

    try:
        first = run("command", "pytest", "--failed", "--limit", "1", "--service", "fixture")
        assert first["limited"] and first["pages_read"] == 1 and first["next_cursor"], first
        second = run("command", "pytest", "--failed", "--limit", "1", "--service", "fixture", "--cursor", first["next_cursor"])
        assert second["next_cursor"] is None, second
        events = [row["event_id"] for row in first["matches"] + second["matches"]]
        assert len(set(events)) == 2, events
        together = run("command", "pytest", "--failed", "--limit", "1", "--service", "fixture", "--pages", "2")
        assert together["pages_read"] == 2 and [row["event_id"] for row in together["matches"]] == events
        run("command", "other", "--failed", "--limit", "1", "--service", "fixture", "--cursor", first["next_cursor"], success=False)
        assert not run("command", "pytest", "--service", "other")["matches"]

        row = first["matches"][0]
        argv = shlex.split(row["source_command"])
        assert argv[:4] == ["agenturm", "investigate", "source", row["event_id"]]
        assert argv[-6:] == ["--receiver-protocol", "python-v0.2.8", "--endpoint", endpoint, "--token-file", str(token_path)]
        evidence = run("source", row["event_id"], "--reference", argv[5])
        assert evidence["source_reference"] == row["source_reference"] and evidence["sources"][0]["record"]
        assert run("source", row["event_id"], "--reference", encode_reference(row["source_reference"])) == evidence
        bad_ref = {**row["source_reference"], "session_id": "other"}
        run("source", row["event_id"], "--reference", encode_reference(bad_ref), success=False)

        for term in ("src/old.py", "src/new.py"):
            path_result = run("path", term)
            assert [row["text"] for row in path_result["matches"]] == [term], path_result
        assert not run("path", "src/OLD.py")["matches"]
        instruction = run("instruction", "public endpoint")
        assert len(instruction["matches"]) == 1 and REMEMBER in instruction["matches"][0]["text"], instruction
        log_ref = instruction["matches"][0]["source_reference"]
        assert log_ref["trace_id"] is None and log_ref["span_id"] is None, log_ref
        actual_log = run("source", log_ref["event_id"], "--reference", encode_reference(log_ref))
        assert actual_log["sources"][0]["scope"]["span_id"] is None, actual_log
        return store.investigation_page("command", "pytest", limit=1), evidence
    finally:
        stop(server, thread)


def run_cli(app, token, token_path, endpoint, kind, query, *flags, success=True, json_output=True):
    command = [app, "investigate", kind, query, "--receiver-protocol", "python-v0.2.8",
               "--endpoint", endpoint, "--token-file", token_path, *flags]
    if json_output:
        command.extend(["--format", "json"])
    result = subprocess.run(command, text=True, capture_output=True, timeout=20)
    assert token not in result.stdout + result.stderr and SECRET not in result.stdout + result.stderr
    if not success:
        assert result.returncode == 2 and not result.stdout, (result.returncode, result.stderr)
        return result.stderr
    assert result.returncode == 0, (query, result.stderr)
    return json.loads(result.stdout) if json_output else result.stdout


def controlled_receiver(app, token, token_path, page, evidence, encode_reference):
    state = dict(mode="steady", health_reads=0)
    identity = page["receiver_id"]
    event = evidence["event_id"]
    reference = encode_reference(evidence["source_reference"])
    page = copy.deepcopy(page)
    page["matches"][0].update(source_command=SECRET, source_path=SECRET)

    class Receiver(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            assert self.headers.get("Authorization") == f"Bearer {token}"
            route = urlsplit(self.path)
            mode = state["mode"]
            body = None
            declared_length = None
            if route.path == "/api/health":
                state["health_reads"] += 1
                safety = mode == "revocation" and state["health_reads"] >= 3
                body = dict(ok=True, receiver_id=identity, investigation_index_v1=True,
                            indexing={"ready": True}, security={"authentication_required": True, "host_validation": True},
                            local_agents=dict(capture_content=True, capture_policy_schema=1,
                                              capture_global_revision=1 if safety else 0))
            elif route.path == "/api/investigations/source":
                body = copy.deepcopy(evidence)
                if mode == "missing-owner":
                    body["sources"][0]["scope"].pop("turn_id")
                if mode == "wrong-owner":
                    body["sources"][0]["scope"]["session_id"] = "other"
                if mode == "large-source":
                    declared_length = 64 * 1024 + 1
            else:
                assert route.path == "/api/investigations"
                body = copy.deepcopy(page)
                if mode == "wire-budget":
                    if parse_qs(route.query).get("cursor"):
                        declared_length = 1024 * 1024
                    else:
                        body = json.dumps(body).encode().ljust(1536 * 1024, b" ")
            payload = body if isinstance(body, bytes) else json.dumps(body).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("X-Agenturm-Receiver-ID", identity)
            self.send_header("Content-Length", str(declared_length or len(payload)))
            self.end_headers()
            if declared_length is None:
                self.wfile.write(payload)

    server = HTTPServer(("127.0.0.1", 0), Receiver)
    thread = serve(server)
    endpoint = f"http://127.0.0.1:{server.server_port}"

    def run(mode, *args, **kwargs):
        state.update(mode=mode, health_reads=0)
        return run_cli(app, token, token_path, endpoint, *args, **kwargs)

    try:
        text = run("steady", "command", "pytest", "--limit", "1", json_output=False)
        assert "Exact source: agenturm investigate source" in text
        limited = run("wire-budget", "command", "pytest", "--limit", "1", "--pages", "2")
        assert limited["pages_read"] == 1 and limited["next_cursor"] == page["next_cursor"], limited
        assert "capture policy" in run("revocation", "command", "pytest", "--limit", "1", success=False)
        assert "capture policy" in run("revocation", "source", event, "--reference", reference, success=False)
        for mode in ("missing-owner", "wrong-owner", "large-source"):
            run(mode, "source", event, "--reference", reference, success=False)
    finally:
        stop(server, thread)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", required=True, type=Path)
    parser.add_argument("--python-root", required=True, type=Path)
    args = parser.parse_args()
    app = args.bin_dir.resolve() / "agenturm"
    python_root = args.python_root.resolve(strict=True)
    sys.path.insert(0, str(python_root))
    from observatory.core import Store, normalize_native
    from observatory.investigation import encode_reference
    from observatory import server
    assert Path(server.__file__).resolve().is_relative_to(python_root)
    with tempfile.TemporaryDirectory(prefix="agenturm-python-investigations-") as directory:
        root = Path(directory)
        token = secrets.token_urlsafe(32)
        token_path = root / "receiver token"
        token_path.write_text(token + "\n")
        token_path.chmod(0o600)
        store = Store(root / "history.sqlite")
        try:
            seed(store, normalize_native)
            page, evidence = real_receiver(app, token, token_path, server.BoundedHTTPServer, server.handler_for, store, encode_reference)
            controlled_receiver(app, token, token_path, page, evidence, encode_reference)
        finally:
            store.close()
    print("PASS tagged Python investigations and exact source, bounded cursor, policy and owner rechecks")


if __name__ == "__main__":
    main()
