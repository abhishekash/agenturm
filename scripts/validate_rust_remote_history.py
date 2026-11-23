"""Verify the remote Rust reader and cockpit in a disposable Linux guest."""

import argparse
from contextlib import closing
import fcntl
import grp
import http.client
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import pty
import pwd
import secrets
import select
import stat
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
from urllib.parse import urlencode, urlsplit


CODEX = "harness.codex"
PI = "harness.pi"
OTLP = "otlp.remote"
TRACE = "11" * 16
SESSION = "remote-codex-session"
TURN = "turn-1"
PROMPT = "SYNTHETIC_PRIVATE_PROMPT"


def credentials(uid, gid):
    def drop():
        os.setgroups([])
        os.setgid(gid)
        os.setuid(uid)

    return drop


def private_directory(path, uid, gid, mode=0o700):
    path.mkdir()
    os.chown(path, uid, gid)
    path.chmod(mode)


def write_token(path, token, uid, gid):
    path.write_text(token + "\n")
    os.chown(path, uid, gid)
    path.chmod(0o600)


def write_transcript(path, rows):
    path.write_text("".join(json.dumps(row) + "\n" for row in rows))


def wait_for(predicate, process, message):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        assert process.poll() is None, message
        if predicate():
            return
        time.sleep(0.03)
    raise AssertionError(message)


def stop(process):
    if process.poll() is None:
        process.terminate()
        assert process.wait(timeout=8) == 0


def request(endpoint, token, operation, identity=None, *, duplicate=False):
    address = urlsplit(endpoint)
    body = json.dumps({"version": 1, "history_identity": identity, "request": operation})
    with closing(http.client.HTTPConnection(address.hostname, address.port, timeout=5)) as conn:
        conn.putrequest("POST", "/v1/history/read")
        conn.putheader("Content-Type", "application/json")
        conn.putheader("Content-Length", str(len(body)))
        if token is not None:
            conn.putheader("Authorization", f"Bearer {token}")
            if duplicate:
                conn.putheader("Authorization", f"Bearer {token}")
        conn.endheaders(body.encode())
        response = conn.getresponse()
        assert response.getheader("Cache-Control") == "no-store"
        return response.status, json.loads(response.read())


def request_get(endpoint, token, path, binding=None):
    address = urlsplit(endpoint)
    headers = {} if token is None else {"Authorization": f"Bearer {token}"}
    if binding is not None:
        headers["X-Agenturm-History-Binding"] = binding
    with closing(http.client.HTTPConnection(address.hostname, address.port, timeout=5)) as connection:
        connection.request("GET", path, headers=headers)
        response = connection.getresponse()
        assert response.getheader("Cache-Control") == "no-store"
        return response.status, json.loads(response.read())


def assert_seen_in_cockpit(command, drop, env, root):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 18, 60, 0, 0))
    original = termios.tcgetattr(slave)
    process = subprocess.Popen(command, cwd=root, env=env, preexec_fn=drop,
                               stdin=slave, stdout=slave, stderr=slave)

    def screen_until(needle):
        screen = bytearray()
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.1)[0]:
                screen.extend(os.read(master, 8192))
            assert PROMPT.encode() not in screen
            if needle in screen:
                return
            assert process.poll() is None, screen
        raise AssertionError((needle, screen))

    try:
        screen_until(SESSION.encode())
        os.write(master, b"w")
        screen_until(b"Pending")
        os.write(master, b"s")
        screen_until(b"seen")
        os.write(master, b"q")
        assert process.wait(timeout=8) == 0
        assert termios.tcgetattr(slave) == original
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        os.close(master)
        os.close(slave)


class MissingIdentityHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_POST(self):
        self.rfile.read(int(self.headers["Content-Length"]))
        body = b'{"version":1,"result":{"Ok":{"type":"identity","value":null}}}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("Run in a disposable Linux guest with support for separate UIDs.")

    occupied_uids = {account.pw_uid for account in pwd.getpwall()}
    occupied_gids = {group.gr_gid for group in grp.getgrall()}
    broker, collector, reader, maintenance, ui = [
        uid for uid in range(45000, 46000) if uid not in occupied_uids
    ][:5]
    client_gid, ui_gid = [gid for gid in range(45000, 46000) if gid not in occupied_gids][:2]
    binaries = args.bin_dir.resolve(strict=True)
    app = binaries / "agenturm"

    with tempfile.TemporaryDirectory(prefix="agenturm-remote-history-") as name:
        root = Path(name)
        root.chmod(0o755)
        data, transport, source, viewer = [root / name for name in
                                           ["history", "transport", "source", "viewer"]]
        private_directory(data, broker, broker)
        backups = root / "backups"
        private_directory(backups, broker, broker)
        private_directory(transport, broker, client_gid, 0o2710)
        private_directory(source, collector, client_gid)
        private_directory(viewer, ui, ui_gid)
        socket = transport / "agenturm-native-storage-v1.sock"
        database = data / "history.sqlite"
        config = root / "broker.json"
        config.write_text(json.dumps({
            "version": 1, "socket": str(socket), "receiver_id": "receiver:remote-fixture",
            "service_uid": broker, "service_gid": broker,
        }))
        config.chmod(0o644)
        token = secrets.token_hex(32)
        server_token = root / "reader-token"
        client_token = viewer / "receiver-token"
        write_token(server_token, token, reader, client_gid)
        write_token(client_token, token, ui, ui_gid)
        sources = source / "sources.json"
        codex = source / "rollout.jsonl"
        pi = source / "pi.jsonl"
        write_transcript(codex, [
            {"type": "session_meta", "payload": {"id": SESSION}},
            {"type": "event_msg", "timestamp": "2026-11-20T12:00:00Z",
             "payload": {"type": "task_started", "turn_id": TURN}},
            {"type": "response_item", "timestamp": "2026-11-20T12:00:01Z",
             "payload": {"type": "message", "turn_id": TURN, "role": "user",
                         "content": [{"type": "input_text", "text": PROMPT}]}},
            {"type": "event_msg", "timestamp": "2026-11-20T12:00:02Z",
             "payload": {"type": "request_user_input", "turn_id": TURN}},
        ])
        write_transcript(pi, [
            {"type": "session", "id": "pi-session", "agentId": "worker-1"},
            {"type": "message", "id": "worker-turn", "timestamp": "2026-11-20T12:00:03Z",
             "message": {"role": "user", "content": PROMPT}},
        ])
        sources.write_text(json.dumps({
            "version": 2, "plugin_catalog": str(binaries.parent / "agenturm-plugins.json"),
            "plugin_host": str(binaries / "agenturm-plugin-host"),
            "sources": [{"plugin_id": f"example.rust-{provider}-harness",
                         "source_id": harness, "source_root": str(transcript)}
                        for provider, harness, transcript in [
                            ("codex", CODEX, codex), ("pi", PI, pi)]],
        }))

        def command(verb, *arguments):
            return [app, "broker", verb, "--broker-config", config, *arguments]

        def run(uid, *args, code=0, gid=client_gid, env=None):
            output = subprocess.run(args, cwd=root, env=env, capture_output=True, text=True,
                                    user=uid, group=gid, extra_groups=[], timeout=12)
            assert output.returncode == code, (args[1:3], output.stdout, output.stderr)
            return output

        def service_command():
            return [
                binaries / "agenturm-storage-service", "--data-root", data,
                "--transport-dir", transport, "--receiver-id", "receiver:remote-fixture",
                "--client-uid", str(collector), "--client-gid", str(client_gid),
                "--native-reader", f"{reader}:{client_gid}",
                "--native-maintenance", f"{maintenance}:{client_gid}",
                "--native-sources", f"{CODEX},{PI},{OTLP}",
                "--native-backup-root", backups,
            ]

        def start_service():
            process = subprocess.Popen(service_command(), cwd=root,
                                       preexec_fn=credentials(broker, broker))
            try:
                wait_for(lambda: socket.exists() and database.exists() and
                         stat.S_IMODE(socket.stat().st_mode) == 0o660,
                         process, "Broker socket not ready")
            except BaseException:
                stop(process)
                raise
            return process

        def start_gateway(listen="127.0.0.1:0"):
            process = subprocess.Popen(command("serve-history", "--listen", listen,
                                               "--token-file", server_token),
                                       cwd=root, text=True, stdout=subprocess.PIPE,
                                       preexec_fn=credentials(reader, client_gid))
            gateway_processes.append(process)
            assert select.select([process.stdout], [], [], 8)[0], "Gateway not ready"
            banner = process.stdout.readline().strip()
            assert banner.startswith("Listening for history reads at http://127.0.0.1:"), banner
            return process, banner.split()[-1]

        env = {**os.environ, "HOME": str(viewer), "XDG_STATE_HOME": str(viewer / "state"),
               "TERM": "xterm-256color"}

        def attach(endpoint):
            return ["--endpoint", endpoint, "--token-file", client_token]

        def attention(endpoint, *poll):
            output = run(ui, app, "attention", *poll, *attach(endpoint), gid=ui_gid, env=env)
            return json.loads(output.stdout)

        service = start_service()
        gateway_processes = []
        try:
            run(collector, *command("collect", "--config", sources))
            for uid in [collector, maintenance]:
                denied = run(uid, *command("serve-history", "--listen", "127.0.0.1:0",
                                           "--token-file", server_token), code=1)
                assert not denied.stdout and "dedicated broker reader" in denied.stderr
            server_token.chmod(0o644)
            denied = run(reader, *command("serve-history", "--listen", "127.0.0.1:0",
                                          "--token-file", server_token), code=1)
            assert not denied.stdout and "owner-only" in denied.stderr
            server_token.chmod(0o600)
            gateway, endpoint = start_gateway()
            identity_request = {"op": "identity"}
            assert request(endpoint, None, identity_request)[0] == 401
            assert request(endpoint, "f" * 64, identity_request)[0] == 401
            assert request(endpoint, token, identity_request, duplicate=True)[0] == 400
            status, reply = request(endpoint, token, identity_request)
            identity = reply["history_identity"]
            assert status == 200 and reply["result"]["Ok"]["value"] == identity
            assert request(endpoint, token, identity_request, "0" * 32)[1]["result"] == {
                "Err": {"code": "history_replaced"},
            }
            for op in ["admit", "prune", "reveal_private", "list_private_prompts"]:
                assert request(endpoint, token, {"op": op}, identity)[0] == 400

            def history(harness, session, selection, snapshot=None):
                return request(endpoint, token, {
                    "op": "history", "selection": selection,
                    "query": {"harness_id": harness, "session_id": session,
                              "limit": 20, "snapshot_id": snapshot},
                }, identity)[1]["result"]

            assert history("harness.outside", "pi-session", {"kind": "observations"}) == {
                "Err": {"code": "unauthorized"},
            }
            exact = {"kind": "worker", "turn_id": "worker-turn", "worker_id": "worker-1"}
            page = history(PI, "pi-session", exact)["Ok"]["value"]
            rows = page["observations"]
            assert rows and PROMPT not in json.dumps(rows)
            assert all(row["identity"]["worker_id"] == "worker-1" for row in rows)
            assert history(PI, "pi-session", {**exact, "worker_id": "other"},
                           page["snapshot_id"])["Ok"]["value"]["observations"] == []
            assert history(PI, "other-session", exact,
                           page["snapshot_id"])["Ok"]["value"]["observations"] == []
            assert history(PI, "pi-session", exact, 0)["Ok"]["value"]["observations"] == []

            assert request_get(endpoint, None, "/api/health")[0] == 401
            status, health = request_get(endpoint, token, "/api/health")
            assert status == 200 and health["history_identity"] == identity and health["read_only"]
            assert request_get(endpoint, token, "/api/sessions")[0] == 428
            status, roster = request_get(endpoint, token, "/api/sessions?source_id=" + PI, identity)
            assert status == 200 and roster["sessions"]
            assert all(row["harness_id"] == PI for row in roster["sessions"])
            assert PROMPT not in json.dumps(roster)
            assert request_get(endpoint, token, "/api/activity?source_id=harness.outside", identity)[0] == 403
            assert request_get(endpoint, token, "/api/activity?source_id=host.guest", identity)[0] == 403
            assert request_get(endpoint, token, "/api/activity?source_id=" + PI + "&source_id=" + CODEX, identity)[0] == 400

            backup = json.loads(run(maintenance, *command("backup")).stdout)
            trace_export = source / "traces.json"
            trace_scope = {"agenturm.source.id": PI, "session.id": "pi-session",
                           "agenturm.turn.id": "worker-turn", "agenturm.worker.id": "worker-1"}
            trace_export.write_text(json.dumps({"resourceSpans": [{"scopeSpans": [{"spans": [{
                "traceId": TRACE, "spanId": "22" * 8, "name": PROMPT,
                "startTimeUnixNano": "10", "endTimeUnixNano": "20",
                "attributes": [{"key": key, "value": {"stringValue": value}}
                               for key, value in trace_scope.items()],
            }]}]}]}))
            run(collector, *command("import-traces", "--file", trace_export, "--source-id", OTLP))
            status, spans = request_get(endpoint, token,
                f"/api/traces/{TRACE}/spans?" + urlencode({"source_id":OTLP,"limit":1}), identity)
            assert status == 200 and spans["observations"]
            assert spans["observations"][0]["kind"]["span_id"] == "22" * 8
            assert PROMPT not in json.dumps(spans)
            assert request_get(endpoint, token, f"/api/traces/{TRACE}/spans", identity)[0] == 400

            trace_review = {
                "page": {"harness_id": PI, "session_id": "pi-session", "limit": 20},
                "trace_source_id": OTLP, "trace_id": TRACE, "expected_worker_id": "worker-1",
                "linked_traces": True,
            }

            def review(query):
                return request(endpoint, token, {"op": "trace_review", "query": query},
                               identity)[1]["result"]["Ok"]["value"]

            linked = review(trace_review)
            assert linked["turn_id"] == "worker-turn" and linked["worker_id"] == "worker-1"
            assert len(linked["page"]["observations"]) == 1 and PROMPT not in json.dumps(linked)
            assert review({**trace_review, "expected_worker_id": "another"}) is None
            old_page = {**trace_review["page"], "snapshot_id": page["snapshot_id"]}
            assert review({**trace_review, "page": old_page}) is None

            probe = subprocess.run([sys.executable, "-c", "import sys; open(sys.argv[1])",
                                    database], capture_output=True, timeout=5,
                                   preexec_fn=credentials(ui, ui_gid))
            assert probe.returncode and b"PermissionError" in probe.stderr
            denied = run(ui, *command("query"), gid=ui_gid, code=1)
            assert not denied.stdout
            broker_summary = json.loads(run(reader, *command("cockpit", "--once", "--json")).stdout)
            remote_summary = json.loads(run(ui, app, "cockpit", "--once", "--json",
                                            *attach(endpoint), gid=ui_gid, env=env).stdout)
            assert broker_summary['format'] == remote_summary['format'] == 'agenturm-summary/1'
            assert broker_summary['sessions'] == remote_summary['sessions']
            assert broker_summary['snapshot_id'] == remote_summary['snapshot_id']
            assert PROMPT not in json.dumps(remote_summary)
            pending = attention(endpoint, "watch", "--once")["pending"]
            assert len(pending) == 1 and pending[0]["turn_id"] == TURN and not pending[0]["seen"]
            assert_seen_in_cockpit([app, "cockpit", *attach(endpoint)],
                                   credentials(ui, ui_gid), env, root)
            assert attention(endpoint, "status")["pending"][0]["seen"]
            _, second_endpoint = start_gateway()
            assert not attention(second_endpoint, "status")["pending"][0]["seen"]

            # Restore this same logical history at the same service and HTTP paths,
            # then admit an alternate span at the exact old receipt watermark.
            previous_snapshot = linked["page"]["snapshot_id"]
            stop(service)
            data.rename(root / "before-restore")
            private_directory(data, broker, broker)
            run(broker, *service_command(), "--native-restore", backup["name"], gid=broker)
            service = start_service()
            trace_export.write_text(trace_export.read_text().replace("22" * 8, "33" * 8))
            run(collector, *command("import-traces", "--file", trace_export, "--source-id", OTLP))
            assert request(endpoint, token, {
                "op": "trace_review", "query": trace_review,
            }, identity)[1]["result"] == {"Err": {"code": "snapshot_replaced"}}
            stop(gateway)
            gateway, restored_endpoint = start_gateway(urlsplit(endpoint).netloc)
            assert restored_endpoint == endpoint
            _, fresh_identity = request(endpoint, token, identity_request)
            binding = fresh_identity["history_identity"]
            assert fresh_identity["result"]["Ok"]["value"] == identity
            assert binding.startswith(identity + ":")
            assert request_get(endpoint, token, "/api/sessions", identity)[0] == 409
            assert request_get(endpoint, token, "/api/sessions", binding)[0] == 200
            assert request_get(endpoint, token, "/api/activity?" + urlencode({"source_id":OTLP,"snapshot_id":previous_snapshot}), binding)[0] == 409
            assert request(endpoint, token, identity_request, identity)[1]["result"] == {
                "Err": {"code": "snapshot_replaced"},
            }
            old_query = {**trace_review, "page": {
                **trace_review["page"], "snapshot_id": previous_snapshot,
            }}
            assert request(endpoint, token, {
                "op": "trace_review", "query": old_query,
            }, binding)[1]["result"] == {"Err": {"code": "snapshot_replaced"}}
            _, restored_reply = request(endpoint, token, {
                "op": "trace_review", "query": trace_review,
            }, binding)
            restored_page = restored_reply["result"]["Ok"]["value"]["page"]
            assert restored_page["snapshot_id"] == f'r1:{binding.split(":")[1]}:{previous_snapshot}'
            assert restored_page["observations"][0]["kind"]["span_id"] == "33" * 8
            denied = run(reader, *command(
                "review", "--harness-id", PI, "--session-id", "pi-session",
                "--turn-of-trace", TRACE, "--trace-source-id", OTLP, "--linked-traces",
                "--snapshot-id", str(previous_snapshot),
            ), code=1)
            assert not denied.stdout, denied.stdout
            assert "broker history was restored or replaced" in denied.stderr, denied.stderr
            assert attention(endpoint, "status")["pending"][0]["seen"]
            print("PASS offline restore and receipt regrowth fence the same socket and gateway port",
                  flush=True)

            stop(service)
            data.rename(root / "prior-history")
            private_directory(data, broker, broker)
            service = start_service()
            assert request(endpoint, token, identity_request, binding)[1]["result"] == {
                "Err": {"code": "history_replaced"},
            }
            stop(gateway)
            address = urlsplit(endpoint)
            before = sorted(viewer.rglob("*.sqlite"))
            with HTTPServer((address.hostname, address.port), MissingIdentityHandler) as unknown:
                unknown.timeout = 8
                responder = threading.Thread(target=unknown.handle_request)
                responder.start()
                try:
                    denied = run(ui, app, "attention", "status", *attach(endpoint), gid=ui_gid,
                                 env=env, code=1)
                    assert not denied.stdout and "history receiver response" in denied.stderr
                    assert sorted(viewer.rglob("*.sqlite")) == before
                finally:
                    responder.join(timeout=10)
            _, restarted_endpoint = start_gateway(urlsplit(endpoint).netloc)
            assert restarted_endpoint == endpoint
            assert request(endpoint, token, identity_request)[1]["history_identity"] != identity
            run(collector, *command("collect", "--config", sources))
            assert not attention(endpoint, "status")["pending"][0]["seen"]
            print("PASS authenticated reader scope, exact pinned history, and separate cockpit state",
                  flush=True)
        finally:
            for gateway in reversed(gateway_processes):
                stop(gateway)
                gateway.stdout.close()
            stop(service)


if __name__ == "__main__":
    main()
