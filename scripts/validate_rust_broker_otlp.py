"""Qualify broker-backed live OTLP receiver and launcher in a disposable Linux VM."""

import argparse
from contextlib import closing
import grp
import http.client
import json
import os
from pathlib import Path
import pwd
import re
import secrets
import select
import sqlite3
import stat
import struct
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlparse


NATIVE_SOURCE = "harness.pi"
LIVE_SOURCE = "otlp.live"
RUN_SOURCE = "otlp.launch"
SESSION = "otlp-fixture"
TURN = "turn-1"
WORKER = "worker-1"
LIVE_TRACE = "11" * 16
RUN_TRACE = "22" * 16
CHILD_ARGS = ["--db", "child.sqlite", "--broker-config", "child.json"]
SCOPE = {
    "agenturm.source.id": NATIVE_SOURCE,
    "session.id": SESSION,
    "agenturm.turn.id": TURN,
    "agenturm.worker.id": WORKER,
}


def post(url, body, media_type, token=None):
    address = urlparse(url)
    assert address.hostname == "127.0.0.1"
    headers = {"Content-Type": media_type}
    if token is not None:
        headers["Authorization"] = f"Bearer {token}"
    with closing(http.client.HTTPConnection(address.hostname, address.port, timeout=5)) as connection:
        connection.request("POST", address.path, body, headers)
        response = connection.getresponse()
        return response.status, response.read()


def varint(number):
    encoded = bytearray()
    while number > 127:
        encoded.append((number & 127) | 128)
        number >>= 7
    encoded.append(number)
    return encoded


def field(number, data):
    return varint(number << 3 | 2) + varint(len(data)) + data


def launcher_producer(args):
    assert args == CHILD_ARGS, args
    assert os.environ["OTEL_EXPORTER_OTLP_TRACES_PROTOCOL"] == "http/protobuf"
    url = os.environ["OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"]
    authorization = re.fullmatch(
        r"authorization=Bearer%20([0-9a-f]{64})",
        os.environ["OTEL_EXPORTER_OTLP_TRACES_HEADERS"],
    )
    assert authorization is not None, "Missing launch credential"
    for span_id, attributes in [(b"\x33" * 8, SCOPE), (b"\x44" * 8, {})]:
        span = field(1, bytes.fromhex(RUN_TRACE)) + field(2, span_id)
        span += field(5, b"private span name omitted at admission")
        span += bytes([7 << 3 | 1]) + struct.pack("<Q", 10)
        span += bytes([8 << 3 | 1]) + struct.pack("<Q", 20)
        for key, value in attributes.items():
            span += field(9, field(1, key.encode()) + field(2, field(1, value.encode())))
        export = field(1, field(2, field(2, span)))
        assert post(url, export, "application/x-protobuf")[0] == 401
        assert post(url, export, "application/x-protobuf", authorization[1]) == (200, b"")
    return 23


def credentials(uid, gid):
    def drop():
        os.setgroups([])
        os.setgid(gid)
        os.setuid(uid)

    return drop


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("Use a disposable Linux guest with support for separate UIDs.")

    occupied_uids = {account.pw_uid for account in pwd.getpwall()}
    occupied_gids = {group.gr_gid for group in grp.getgrall()}
    broker, collector, reader = [uid for uid in range(45000, 46000) if uid not in occupied_uids][:3]
    client_gid = next(gid for gid in range(45000, 46000) if gid not in occupied_gids)
    binaries = args.bin_dir.resolve(strict=True)
    app = binaries / "agenturm"
    service_binary = binaries / "agenturm-storage-service"
    catalog = binaries.parent / "agenturm-plugins.json"

    with tempfile.TemporaryDirectory(prefix="agenturm-broker-otlp-") as name:
        root = Path(name)
        root.chmod(0o755)
        data = root / "data"
        transport = root / "transport"
        client = root / "client"
        for path, uid, gid, mode in [
            (data, broker, broker, 0o700),
            (transport, broker, client_gid, 0o2710),
            (client, collector, client_gid, 0o700),
        ]:
            path.mkdir()
            os.chown(path, uid, gid)
            path.chmod(mode)
        socket = transport / "agenturm-native-storage-v1.sock"
        database = data / "history.sqlite"
        config = root / "broker.json"
        config.write_text(json.dumps({
            "version": 1, "socket": str(socket), "receiver_id": "receiver:otlp-fixture",
            "service_uid": broker, "service_gid": broker,
        }))
        config.chmod(0o644)
        token_path = client / "producer-token"
        token = secrets.token_hex(32)
        token_path.write_text(token + "\n")
        os.chown(token_path, collector, client_gid)
        token_path.chmod(0o600)
        transcript = client / "pi.jsonl"
        transcript.write_text("".join(json.dumps(row) + "\n" for row in [
            {"type": "session", "id": SESSION, "agentId": WORKER},
            {"type": "message", "id": TURN, "timestamp": "2026-11-20T12:00:00Z",
             "message": {"role": "user", "content": "Private synthetic prompt"}},
        ]))
        sources = client / "sources.json"
        sources.write_text(json.dumps({
            "version": 2, "plugin_catalog": str(catalog),
            "plugin_host": str(binaries / "agenturm-plugin-host"),
            "sources": [{"plugin_id": "example.rust-pi-harness",
                         "source_id": NATIVE_SOURCE, "source_root": str(transcript)}],
        }))

        def command(verb, *arguments):
            return [app, "broker", verb, "--broker-config", config, *arguments]

        def run(uid, verb, *arguments, code=0):
            result = subprocess.run(command(verb, *arguments), cwd=root, text=True,
                                    capture_output=True, timeout=12,
                                    preexec_fn=credentials(uid, client_gid))
            assert result.returncode == code, (result.stdout, result.stderr, verb)
            return result

        def query(source):
            return json.loads(run(reader, "query", "--harness-id", source).stdout)["observations"]

        def checkpoint(source):
            with sqlite3.connect(f"file:{database}?mode=ro", uri=True) as connection:
                return connection.execute(
                    "SELECT cursor FROM agenturm_source_state WHERE source_id = ?", (source,)
                ).fetchone()

        def start_service():
            process = subprocess.Popen([
                service_binary, "--data-root", data, "--transport-dir", transport,
                "--receiver-id", "receiver:otlp-fixture", "--client-uid", str(collector),
                "--client-gid", str(client_gid), "--native-reader", f"{reader}:{client_gid}",
                "--native-sources", ",".join([NATIVE_SOURCE, LIVE_SOURCE, RUN_SOURCE]),
            ], cwd=root, preexec_fn=credentials(broker, broker))
            deadline = time.monotonic() + 8
            while True:
                assert process.poll() is None, "broker exited during startup"
                if (socket.exists() and stat.S_IMODE(socket.stat().st_mode) == 0o660
                        and database.exists()):
                    return process
                if time.monotonic() >= deadline:
                    process.kill()
                    process.wait()
                    raise AssertionError("broker did not open the private history")
                time.sleep(0.03)

        def stop(process):
            if process.poll() is None:
                process.terminate()
                assert process.wait(timeout=10) == 0

        service = start_service()
        receiver = None
        try:
            run(collector, "collect", "--config", sources)
            for uid in [collector, reader]:
                for access in ["rb", "r+b"]:
                    probe = subprocess.run([sys.executable, "-c",
                                            "import sys; open(sys.argv[1], sys.argv[2])",
                                            database, access], capture_output=True, timeout=5,
                                           preexec_fn=credentials(uid, client_gid))
                    assert probe.returncode != 0 and b"PermissionError" in probe.stderr

            for uid, source in [(reader, LIVE_SOURCE), (collector, "otlp.denied")]:
                denied = run(uid, "serve-traces", "--listen", "127.0.0.1:0",
                             "--source-id", source, "--token-file", token_path, code=1)
                assert not denied.stdout and "outside the broker grant" in denied.stderr
                denied_run = run(uid, "run", "--source-id", source, "--", sys.executable,
                                 "-c", "print('child started')", code=1)
                assert not denied_run.stdout and "outside the broker grant" in denied_run.stderr
            assert query(LIVE_SOURCE) == []

            token_path.chmod(0o644)
            failed_token = run(collector, "serve-traces", "--listen", "127.0.0.1:0",
                               "--source-id", LIVE_SOURCE, "--token-file", token_path, code=1)
            assert not failed_token.stdout and "owner-only regular file" in failed_token.stderr
            assert token not in failed_token.stderr
            token_path.chmod(0o600)
            receiver = subprocess.Popen(command("serve-traces", "--listen", "127.0.0.1:0",
                                                "--source-id", LIVE_SOURCE,
                                                "--token-file", token_path),
                                        cwd=root, stdout=subprocess.PIPE, text=True,
                                        preexec_fn=credentials(collector, client_gid))
            assert select.select([receiver.stdout], [], [], 8)[0], "receiver did not open"
            ready = receiver.stdout.readline().strip()
            assert token not in ready
            url = ready.split()[-1]

            def export(attributes, span_id):
                return json.dumps({"resourceSpans": [{"scopeSpans": [{"spans": [{
                    "traceId": LIVE_TRACE, "spanId": span_id,
                    "startTimeUnixNano": "10", "endTimeUnixNano": "20",
                    "name": "private span name omitted at admission",
                    "attributes": [{"key": key, "value": {"stringValue": value}}
                                   for key, value in attributes.items()],
                }]}]}]}).encode()

            full_export = export(SCOPE, "55" * 8)
            broken_export = export({"agenturm.turn.id": TURN}, "66" * 8)
            unauthorized = subprocess.run(
                [sys.executable, Path(__file__).resolve(), "--unauthorized-post", url, full_export],
                capture_output=True, timeout=6, preexec_fn=credentials(reader, client_gid),
            )
            assert unauthorized.returncode == 0, unauthorized.stderr
            assert post(url, full_export, "application/json", "00" * 32)[0] == 401
            assert post(url, broken_export, "application/json", token)[0] == 400
            status, reply = post(url, export({}, "77" * 8), "application/json", token)
            assert status == 200 and json.loads(reply)["partialSuccess"]["rejectedSpans"] == "1"
            assert query(LIVE_SOURCE) == []
            assert checkpoint(LIVE_SOURCE) is None

            progress_url = url.removesuffix("/v1/traces") + "/native/v1/progress"
            progress = {
                "event_id": "88" * 16, "trace_id": LIVE_TRACE, "observed_at_unix_ns": "15",
                "stage": "planning", "percent_basis_points": 0, "session_id": SESSION,
                "target_source_id": NATIVE_SOURCE, "turn_id": TURN, "worker_id": WORKER,
            }
            progress_body = json.dumps(progress).encode()
            unauthorized_progress = subprocess.run(
                [sys.executable, Path(__file__).resolve(), "--unauthorized-post",
                 progress_url, progress_body],
                capture_output=True, timeout=6, preexec_fn=credentials(reader, client_gid),
            )
            assert unauthorized_progress.returncode == 0, unauthorized_progress.stderr
            private_body = json.dumps(progress | {"message": "private fixture message"}).encode()
            assert post(progress_url, private_body, "application/json", token)[0] == 400
            assert query(LIVE_SOURCE) == []
            assert post(progress_url, progress_body, "application/json", token) == (200, b"{}")
            [progress_row] = query(LIVE_SOURCE)
            assert progress_row["observation"]["kind"] == "trace_progress"
            assert progress_row["observation"]["percent_basis_points"] == 0
            assert progress_row["identity"]["worker_id"] == WORKER
            assert progress_row["identity"]["turn_id"] == TURN
            assert progress_row["observation"]["trace_id"] == LIVE_TRACE

            assert post(url, full_export, "application/json", token) == (200, b"{}")
            observed = query(LIVE_SOURCE)
            assert len(observed) == 2 and progress_row in observed
            [span_row] = [row for row in observed if row["observation"]["kind"] == "trace_span"]
            assert span_row["identity"]["turn_id"] == TURN
            assert span_row["identity"]["worker_id"] == WORKER
            assert "private span name" not in json.dumps(observed)
            committed_checkpoint = checkpoint(LIVE_SOURCE)
            assert committed_checkpoint is not None
            assert post(url, full_export, "application/json", token) == (200, b"{}")
            assert checkpoint(LIVE_SOURCE) == committed_checkpoint

            # An older event can create a receipt for the new cursor transition
            # while retaining the exact same observation.
            assert post(progress_url, progress_body, "application/json", token) == (200, b"{}")
            assert query(LIVE_SOURCE) == observed
            progress_checkpoint = checkpoint(LIVE_SOURCE)
            assert progress_checkpoint is not None
            assert post(progress_url, progress_body, "application/json", token) == (200, b"{}")
            assert query(LIVE_SOURCE) == observed
            assert checkpoint(LIVE_SOURCE) == progress_checkpoint
            amended = json.dumps(progress | {"stage": "amended"}).encode()
            assert post(progress_url, amended, "application/json", token)[0] == 503
            assert query(LIVE_SOURCE) == observed
            linked = json.loads(run(
                reader, "review", "--harness-id", NATIVE_SOURCE, "--session-id", SESSION,
                "--turn-id", TURN, "--worker-id", WORKER, "--linked-traces", "--format", "json",
            ).stdout)
            assert {row["observation"]["kind"] for row in linked["observations"]} == {
                "trace_progress", "trace_span",
            }

            stop(service)
            assert post(url, full_export, "application/json", token)[0] == 503
            service = start_service()
            assert post(url, full_export, "application/json", token) == (200, b"{}")
            assert post(progress_url, progress_body, "application/json", token) == (200, b"{}")
            assert query(LIVE_SOURCE) == observed

            child = run(collector, "run", "--source-id", RUN_SOURCE, "--",
                        sys.executable, Path(__file__).resolve(), "--emit", *CHILD_ARGS, code=23)
            assert not child.stdout and not child.stderr
            spans = query(RUN_SOURCE)
            assert len(spans) == 2
            assert any(row["identity"]["session_id"].startswith("launch-") for row in spans)
            receipt = json.loads(run(
                reader, "review", "--harness-id", NATIVE_SOURCE, "--session-id", SESSION,
                "--turn-of-trace", RUN_TRACE, "--trace-source-id", RUN_SOURCE, "--format", "json",
            ).stdout)
            assert receipt["scope"]["worker_id"] == WORKER
            assert receipt["scope"]["turn_id"] == TURN
            assert not (root / "child.sqlite").exists()
            assert not (root / "broker-history").exists()
        finally:
            if receiver is not None:
                stop(receiver)
            stop(service)

    print("PASS broker authorization, no direct DB access, native progress before spans, JSON and protobuf commits, immutable replay, restart, exact Review and child argv/exit status.")


if __name__ == "__main__":
    if sys.argv[1:2] == ["--emit"]:
        raise SystemExit(launcher_producer(sys.argv[2:]))
    if sys.argv[1:2] == ["--unauthorized-post"]:
        assert post(sys.argv[2], sys.argv[3].encode(), "application/json")[0] == 401
    else:
        main()
