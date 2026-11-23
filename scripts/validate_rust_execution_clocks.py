#!/usr/bin/env python3
"""Qualify exact execution clocks, reader roles, and restore fencing in a Linux guest."""

import argparse
from contextlib import ExitStack
from datetime import datetime, timezone
import grp
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import pwd
import secrets
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
from urllib.parse import urlsplit

from validate_rust_native_attention import pending
from validate_rust_remote_history import request as history_request
from validate_rust_telemetry_payloads import private_directory, ready, stop


HARNESS = "harness.codex.native-attention-fixture"
COLLECTOR = "otlp.native-attention-fixture"
OTHER = "harness.codex.different-fixture"
SESSION = "native-attention-session"
TURN = "native-attention-turn"
PATH = "/v1/native-attention"


def exchange(endpoint, path, body, token, binding=None):
    address = urlsplit(endpoint)
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if binding:
        headers["X-Agenturm-History-Binding"] = binding
    payload = body if isinstance(body, bytes) else json.dumps(body).encode()
    connection = http.client.HTTPConnection(address.hostname, address.port, timeout=6)
    try:
        connection.request("POST", path, payload, headers)
        response = connection.getresponse()
        return response.status, response.getheader("X-Agenturm-History-Binding"), response.read()
    finally:
        connection.close()


def validate(binaries, root):
    app = binaries / "agenturm"
    service = binaries / "agenturm-storage-service"
    taken = {user.pw_uid for user in pwd.getpwall()}
    owner, collector, reader, maintenance, sender = [
        uid for uid in range(45000, 46000) if uid not in taken
    ][:5]
    groups = {group.gr_gid for group in grp.getgrall()}
    group = next(number for number in range(45000, 46000) if number not in groups)

    root.chmod(0o755)
    data, transport, backups, state_root = [
        root / name for name in ("history", "transport", "backups", "sender")
    ]
    private_directory(data, owner, owner)
    private_directory(transport, owner, group, 0o2710)
    private_directory(backups, owner, owner)
    private_directory(state_root, sender, group)
    socket_path = transport / "agenturm-native-storage-v1.sock"
    database = data / "history.sqlite"
    broker_config = root / "broker.json"
    receiver_id = "receiver:native-attention-fixture"
    broker_config.write_text(json.dumps({
        "version": 1,
        "socket": str(socket_path),
        "receiver_id": receiver_id,
        "service_uid": owner,
        "service_gid": owner,
    }))

    def principal(uid):
        return {"user": uid, "group": owner if uid == owner else group, "extra_groups": []}

    def write_token(path, uid, token):
        path.write_text(token + "\n")
        os.chown(path, uid, group)
        path.chmod(0o600)
        return path

    reader_token = secrets.token_hex(32)
    collector_token = secrets.token_hex(32)
    reader_local = write_token(root / "reader", reader, reader_token)
    collector_local = write_token(root / "collector", collector, collector_token)
    reader_sender = write_token(state_root / "reader", sender, reader_token)
    collector_sender = write_token(state_root / "collector", sender, collector_token)

    def command(*args):
        return [app, "broker", "--broker-config", broker_config, *args]

    def run(uid, *args, code=0):
        env = dict(os.environ)
        if uid == sender:
            env.update(HOME=str(state_root), XDG_STATE_HOME=str(state_root / "xdg"))
        result = subprocess.run(
            args, cwd=root, **principal(uid), env=env, capture_output=True, text=True, timeout=18,
        )
        assert result.returncode == code, (args[1:4], result.stdout, result.stderr)
        assert collector_token not in result.stdout + result.stderr
        assert reader_token not in result.stdout + result.stderr
        return result.stdout

    launch = [
        service, "--data-root", data, "--transport-dir", transport, "--receiver-id", receiver_id,
        "--client-uid", str(collector), "--client-gid", str(group),
        "--native-reader", f"{reader}:{group}", "--native-maintenance", f"{maintenance}:{group}",
        "--native-backup-root", backups, "--native-sources", f"{COLLECTOR},{HARNESS},{OTHER}",
    ]

    with ExitStack() as cleanup:
        def start_service():
            process = subprocess.Popen(launch, cwd=root, **principal(owner))
            cleanup.callback(stop, process)
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                assert process.poll() is None
                if socket_path.is_socket() and socket_path.stat().st_mode & 0o777 == 0o660:
                    return process
                time.sleep(0.03)
            raise AssertionError("Broker socket did not become ready")

        def listener(uid, banner, *args):
            process = subprocess.Popen(
                command(*args), cwd=root, **principal(uid), stdout=subprocess.PIPE, text=True,
            )
            cleanup.callback(stop, process)
            return process, ready(process, banner)

        original = start_service()
        seed = root / "rollout.jsonl"
        seed.write_text("\n".join(map(json.dumps, [
            {"type": "session_meta", "payload": {"id": SESSION, "cwd": str(root), "source": "cli"}},
            {"type": "event_msg", "timestamp": datetime.now(timezone.utc).isoformat(),
             "payload": {"type": "task_started", "turn_id": TURN}},
        ])) + "\n")
        seed.chmod(0o644)
        config = root / "sources.json"
        config.write_text(json.dumps({
            "version": 2,
            "plugin_host": str(binaries / "agenturm-plugin-host"),
            "plugin_catalog": str(binaries.parent / "agenturm-plugins.json"),
            "sources": [{
                "plugin_id": "example.rust-codex-harness",
                "source_id": HARNESS,
                "source_root": str(seed),
            }],
        }))
        run(collector, *command("collect", "--config", config))

        read_proc, read_url = listener(
            reader, "Listening for history reads at http://127.0.0.1:",
            "serve-history", "--listen", "127.0.0.1:0", "--token-file", reader_local,
        )
        read_addr = urlsplit(read_url).netloc

        def collector_listener():
            process, intake = listener(
                collector, "Listening for OTLP traces at http://127.0.0.1:",
                "serve-traces", "--listen", "127.0.0.1:0", "--source-id", COLLECTOR,
                "--token-file", collector_local, "--native-attention-source", HARNESS,
            )
            return process, intake.removesuffix("/v1/traces")

        write_proc, write_url = collector_listener()

        forwarding = {"endpoint": write_url, "lost": True, "accepted": 0}

        class LostReply(BaseHTTPRequestHandler):
            def do_POST(self):
                assert self.path == PATH
                length = int(self.headers["Content-Length"])
                wire = self.rfile.read(length)
                _, token = self.headers["Authorization"].split(" ", 1)
                status, binding, reply = exchange(
                    forwarding["endpoint"], self.path, wire, token,
                    self.headers["X-Agenturm-History-Binding"],
                )
                if status == 200:
                    forwarding["accepted"] += 1
                    if forwarding["lost"]:
                        forwarding["lost"] = False
                        self.connection.shutdown(socket.SHUT_RDWR)
                        return
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(reply)))
                if binding:
                    self.send_header("X-Agenturm-History-Binding", binding)
                self.end_headers()
                self.wfile.write(reply)

            def log_message(self, *_):
                pass

        relay = cleanup.enter_context(ThreadingHTTPServer(("127.0.0.1", 0), LostReply))
        relay_thread = threading.Thread(target=relay.serve_forever, daemon=True)
        relay_thread.start()
        cleanup.callback(relay_thread.join, 5)
        cleanup.callback(relay.shutdown)
        relay_url = f"http://127.0.0.1:{relay.server_port}"

        def count():
            with sqlite3.connect(f"{database.as_uri()}?mode=ro", uri=True) as db:
                return db.execute("SELECT count(*) FROM agenturm_native_attention_events").fetchone()[0]

        binding_file = state_root / "binding.json"
        outbox = state_root / "state.sqlite"
        native = [app, "attention", "native"]
        run(sender, *native, "bind", "--receiver-protocol", "rust-v1",
            "--endpoint", read_url, "--token-file", reader_sender,
            "--collector-endpoint", relay_url, "--collector-token-file", collector_sender,
            "--collector-source", COLLECTOR, "--source-id", HARNESS,
            "--session-id", SESSION, "--turn-id", TURN, "--output", binding_file)
        saved = json.loads(binding_file.read_text())
        old_binding = saved["history_binding"]

        def capture(sequence, kind, request_id="native/request"):
            return {
                "history_binding": old_binding, "scope": dict(saved["scope"]), "sender_id": saved["sender_id"],
                "event_id": f"native/event-{sequence}", "sequence": sequence,
                "kind": kind, "request_id": request_id, "source_time_unix_ns": None,
            }

        def sender_command(verb, cap=None, code=0):
            args = [verb, "--binding", binding_file, "--state", outbox,
                    "--token-file", reader_sender, "--collector-token-file", collector_sender]
            if cap is not None:
                args.append(json.dumps({"schema": "agenturm-rust-native-event-v1", **cap}))
            assert run(sender, *native, *args, code=code) == ""

        first = capture(1, "approval_requested")
        assert exchange(write_url, PATH, first, reader_token, old_binding)[0] == 401
        assert exchange(write_url, PATH, first, collector_token)[0] == 400
        assert exchange(write_url, PATH, first, collector_token, "f" * 32)[0] == 409
        bad = capture(1, "approval_requested")
        bad["scope"]["source_id"] = OTHER
        assert exchange(write_url, PATH, bad, collector_token, old_binding)[0] == 403
        bad["scope"].update(source_id=HARNESS, worker_id="foreign-worker")
        assert exchange(write_url, PATH, bad, collector_token, old_binding)[0] >= 400
        assert count() == 0

        sender_command("send", first, code=1)
        waiting = pending(outbox)
        assert len(waiting) == 1 and waiting[0][0] == 1
        assert count() == 1 and forwarding["accepted"] == 1
        def attention():
            return json.loads(run(
                sender, app, "attention", "status",
                "--endpoint", read_url, "--token-file", reader_sender,
            ))["pending"]

        request, = attention()
        assert (request["source_id"], request["session_id"], request["turn_id"], request["worker_id"]) == (
            HARNESS, SESSION, TURN, None,
        )
        assert "approval" in request["reason_code"].lower()
        sender_command("retry")
        assert not pending(outbox) and count() == 1 and forwarding["accepted"] == 2
        sender_command("send", capture(2, "request_resolved", "sibling-request"), code=1)
        sender_command("send", capture(2, "request_resolved"))
        assert not pending(outbox) and count() == 2 and forwarding["accepted"] == 3
        assert not attention()

        def head(identity):
            status, reply = history_request(read_url, reader_token, {
                "op": "history", "selection": {"kind": "observations"},
                "query": {"limit": 1},
            }, identity)
            assert status == 200
            return reply["result"]["Ok"]["value"]["snapshot_id"]

        def clocks(identity, snapshot, scopes, token=reader_token):
            return history_request(read_url, token, {"op": "execution_clocks",
                "query": {"snapshot_id": snapshot, "scopes": scopes}}, identity)

        scope = dict(saved["scope"])
        siblings = [scope, {**scope, "worker_id": "sibling-worker"}]
        prior = head(old_binding)
        assert clocks(old_binding, prior, siblings)[1]["result"]["Ok"]["value"]["active"] == []
        sender_command("send", capture(3, "turn_started", None))
        snapshot = head(old_binding)
        status, reply = clocks(old_binding, snapshot, siblings)
        assert status == 200
        value = reply["result"]["Ok"]["value"]
        start, = value["active"]
        assert start["scope"] == scope and start["event_id"].startswith("native-attention:")
        assert start["receipt_time_unix_ns"] > 0
        assert start["receipt_time_unix_ns"] <= value["receiver_time_unix_ns"]
        assert clocks(old_binding, snapshot, siblings, token=None)[0] == 401
        assert clocks(old_binding, prior, siblings)[1]["result"] == {"Err": {"code": "invalid_request"}}
        assert clocks(old_binding, snapshot, [{**scope, "source_id": "harness.denied"}])[1]["result"] == {
            "Err": {"code": "unauthorized"},
        }
        assert clocks(old_binding, snapshot, [scope, scope])[1]["result"] == {
            "Err": {"code": "invalid_request"},
        }

        backup = json.loads(run(maintenance, *command("backup")).strip())
        forwarding["lost"] = True
        later = capture(4, "turn_completed", None)
        sender_command("send", later, code=1)
        queued = pending(outbox)
        assert len(queued) == 1 and count() == 4
        stop(read_proc)
        stop(write_proc)
        stop(original)
        data.rename(root / "old-history")
        private_directory(data, owner, owner)
        run(owner, *launch, "--native-restore", backup["name"])
        start_service()
        assert count() == 3
        _, current_read = listener(
            reader, "Listening for history reads at http://127.0.0.1:",
            "serve-history", "--listen", read_addr, "--token-file", reader_local,
        )
        assert current_read == read_url
        _, fresh_write = collector_listener()
        forwarding["endpoint"] = fresh_write

        sender_command("retry", code=1)
        assert pending(outbox) == queued
        assert exchange(fresh_write, PATH, later, collector_token, old_binding)[0] >= 400
        assert count() == 3

        status, identity_reply = history_request(read_url, reader_token, {"op": "identity"})
        identity = identity_reply["history_identity"]
        assert status == 200 and identity != old_binding
        restored_snapshot = head(identity)
        stale_binding = clocks(old_binding, snapshot, siblings)
        assert stale_binding[1]["result"] == {"Err": {"code": "snapshot_replaced"}}, stale_binding
        assert clocks(identity, snapshot, siblings)[1]["result"] == {"Err": {"code": "invalid_request"}}
        restored = clocks(identity, restored_snapshot, siblings)[1]["result"]["Ok"]["value"]
        assert not restored["active"]

        fresh = capture(1, "turn_started", None)
        fresh["history_binding"] = identity
        fresh["sender_id"] = secrets.token_hex(16)
        status, _, payload = exchange(fresh_write, PATH, fresh, collector_token, identity)
        assert status == 200
        fresh_id = json.loads(payload)["event_id"]
        active, = clocks(identity, head(identity), siblings)[1]["result"]["Ok"]["value"]["active"]
        assert active["scope"] == scope and active["event_id"] == fresh_id != start["event_id"]
        assert count() == 4
        print("PASS broker roles, request replay, exact current execution clock, restore fence, and fresh start")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    options = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("Run in a disposable Linux guest")
    with tempfile.TemporaryDirectory(prefix="agenturm-native-attention-broker-") as work:
        validate(options.bin_dir.resolve(strict=True), Path(work))


if __name__ == "__main__":
    main()
