"""Verify captured OTLP text across distinct broker principals in a Linux guest."""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import grp
import http.client
import json
import os
from pathlib import Path
import pty
import pwd
import secrets
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time
from urllib.parse import urlsplit

SOURCE = "otlp.producer-payload"
HARNESS = "harness.codex"
TRACE = "1234567890abcdef1234567890abcdef"
SPAN = "1234567890abcdef"
SESSION = "telemetry-session"
INPUT = "CAPTURED_INPUT_84217"
OUTPUT = "CAPTURED_OUTPUT_76391"
UNRELATED = "synthetic excluded input value"


def private_directory(path, uid, gid, mode=0o700):
    path.mkdir()
    os.chown(path, uid, gid)
    path.chmod(mode)


def stop(process):
    if process.poll() is None:
        process.terminate()
        assert process.wait(timeout=8) == 0


def ready(process, banner):
    assert select.select([process.stdout], [], [], 8)[0], banner
    line = process.stdout.readline().strip()
    assert line.startswith(banner), line
    return line.split()[-1]


def post(endpoint, body, token=None, path="/v1/traces"):
    address = urlsplit(endpoint)
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    with contextlib.closing(http.client.HTTPConnection(address.hostname, address.port, timeout=8)) as connection:
        connection.request("POST", path, json.dumps(body).encode(), headers)
        response = connection.getresponse()
        return response.status, json.loads(response.read())


def cockpit_preview(args, principal, state, *, columns, rows, close):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
    original = termios.tcgetattr(slave)
    env = {**os.environ, "HOME": str(state), "XDG_STATE_HOME": str(state / "xdg"),
           "TERM": "xterm-256color"}
    process = subprocess.Popen(args, cwd=state, stdin=slave, stdout=slave, stderr=slave,
                               env=env, **principal)

    def until(marker):
        frame = bytearray()
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if select.select([master], [], [], .05)[0]:
                frame.extend(os.read(master, 8192))
            if marker in frame:
                return bytes(frame)
            assert process.poll() is None, bytes(frame)
        raise AssertionError((marker, bytes(frame)))

    try:
        assert INPUT.encode() not in until(SESSION.encode())
        os.write(master, b"8")
        assert INPUT.encode() not in until(b"d captured text")
        os.write(master, b"d")
        assert INPUT.encode() not in until(b"Enter reveal")
        os.write(master, b"\r")
        frame = until(OUTPUT.encode())
        assert b"Producer-captured text" in frame and INPUT.encode() in frame
        os.write(master, close)
        if close == b"\x1b":
            clear = until(b"Enter reveal")
            assert INPUT.encode() not in clear and OUTPUT.encode() not in clear
            os.write(master, b"q")
        tail = until(b"\x1b[?1049l")
        assert INPUT.encode() not in tail and OUTPUT.encode() not in tail
        assert process.wait(timeout=8) == 0
        assert termios.tcgetattr(slave) == original
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=8)
        os.close(master)
        os.close(slave)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    options = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("Run in a disposable Linux guest")
    binaries = options.bin_dir.resolve(strict=True)
    taken_users = {item.pw_uid for item in pwd.getpwall()}
    taken_groups = {item.gr_gid for item in grp.getgrall()}
    broker, collector, reader, admin, maintenance = [
        uid for uid in range(45000, 46000) if uid not in taken_users
    ][:5]
    client_gid = next(gid for gid in range(45000, 46000) if gid not in taken_groups)

    with tempfile.TemporaryDirectory(prefix="agenturm-telemetry-broker-") as path:
        root = Path(path)
        root.chmod(0o755)
        data, transport, backup_root = (root / part for part in ("history", "transport", "backup"))
        private_directory(data, broker, broker)
        private_directory(transport, broker, client_gid, 0o2710)
        private_directory(backup_root, broker, broker)
        state = root / "reader-state"
        private_directory(state, reader, client_gid)
        endpoint_socket = transport / "agenturm-native-storage-v1.sock"
        config = root / "broker.json"
        config.write_text(json.dumps({
            "version": 1, "socket": str(endpoint_socket), "receiver_id": "receiver:telemetry-payload",
            "service_uid": broker, "service_gid": broker,
        }))
        config.chmod(0o644)

        def operator_token(name, owner):
            token = secrets.token_hex(32)
            path = root / name
            path.write_text(token + "\n")
            os.chown(path, owner, client_gid)
            path.chmod(0o600)
            return token, path

        producer_token, producer_token_path = operator_token("producer-token", collector)
        viewer_token, viewer_token_path = operator_token("viewer-token", reader)

        def principal(uid):
            return dict(user=uid, group=broker if uid == broker else client_gid, extra_groups=[])

        def command(*args):
            return [binaries / "agenturm", "broker", "--broker-config", config, *args]

        def run(uid, *args, code=0):
            result = subprocess.run(args, cwd=root, **principal(uid), capture_output=True,
                                    text=True, timeout=15)
            assert result.returncode == code, (args[1:3], result.stdout, result.stderr)
            return result

        def service_command():
            return [binaries / "agenturm-storage-service",
                    "--data-root", data, "--transport-dir", transport,
                    "--receiver-id", "receiver:telemetry-payload", "--client-uid", str(collector),
                    "--client-gid", str(client_gid), "--native-reader", f"{reader}:{client_gid}",
                    "--native-grant-admin", f"{admin}:{client_gid}",
                    "--native-maintenance", f"{maintenance}:{client_gid}",
                    "--native-backup-root", backup_root, "--native-sources", f"{SOURCE},{HARNESS}",
                    "--native-telemetry-payload-sources", f"{SOURCE}={HARNESS}"]

        def service_start():
            process = subprocess.Popen(service_command(), cwd=root, **principal(broker))
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                assert process.poll() is None
                if endpoint_socket.is_socket() and endpoint_socket.stat().st_mode & 0o777 == 0o660:
                    return process
                time.sleep(0.03)
            stop(process)
            raise AssertionError("Broker did not bind")

        selector_path = root / "selection.json"
        bind_path = root / "binding.json"
        binding = {"source_id": SOURCE, "native_source_id": HARNESS}
        bind_path.write_text(run(
            reader, binaries / "agenturm", "trace-payload", "binding", "--source-id", SOURCE,
            "--payload-scope", HARNESS,
        ).stdout)
        bind_path.chmod(0o644)

        def snapshot():
            reply = run(reader, *command("trace-payload", "snapshot", "--source-id", SOURCE,
                                         "--session-id", SESSION))
            return json.loads(reply.stdout)["snapshot_id"]

        def preview(*, code=0, uid=reader):
            result = run(uid, *command("trace-payload", "preview", "--selection", selector_path),
                         code=code)
            if code:
                assert result.stdout == "" and INPUT not in result.stderr and OUTPUT not in result.stderr
                return None
            reply = json.loads(result.stdout)
            assert reply["origin"] == "producer_captured_telemetry"
            return reply["payload"]

        def grant(verb):
            return json.loads(run(admin, *command("grant", verb, "--binding", bind_path)).stdout)

        started = time.time_ns()
        span = {
            "traceId": TRACE, "spanId": SPAN, "name": UNRELATED,
            "startTimeUnixNano": str(started), "endTimeUnixNano": str(started + 1),
            "attributes": [
                {"key": key, "value": {"stringValue": value}}
                for key, value in {
                    "session.id": SESSION, "agenturm.source.id": HARNESS,
                    "agenturm.turn.id": "turn-fixture", "agenturm.worker.id": "worker-fixture",
                    "agenturm.payload.input": INPUT, "agenturm.payload.output": OUTPUT,
                    "input.value": UNRELATED,
                }.items()
            ],
        }
        export = {"resourceSpans": [{"scopeSpans": [{"spans": [span]}]}]}

        service = service_start()
        receiver = None
        gateway = None
        try:
            status = run(collector, *command(
                "serve-traces", "--listen", "127.0.0.1:0", "--source-id", SOURCE,
                "--payload-scope", "standalone", "--token-file", producer_token_path,
            ), code=1)
            assert not status.stdout
            receiver = subprocess.Popen(command(
                "serve-traces", "--listen", "127.0.0.1:0", "--source-id", SOURCE,
                "--payload-scope", HARNESS, "--token-file", producer_token_path,
            ), cwd=root, **principal(collector), stdout=subprocess.PIPE, text=True)
            intake = ready(receiver, "Listening for OTLP traces at http://127.0.0.1:")
            assert post(intake, export)[0] == 401
            assert post(intake, export, "f" * 64)[0] == 401
            assert post(intake, export, producer_token)[0] == 200
            assert post(intake, export, producer_token)[0] == 200
            unsafe = json.loads(json.dumps(export))
            unsafe["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["attributes"][4]["value"] = {
                "stringValue": "Authorization: Bearer synthetic secret",
            }
            error = post(intake, unsafe, producer_token)
            assert error[0] == 400 and "synthetic" not in str(error)
            changed = json.loads(json.dumps(export))
            changed["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["attributes"][4]["value"] = {
                "stringValue": "a different captured request",
            }
            error = post(intake, changed, producer_token)
            assert error[0] == 503 and INPUT not in str(error)
            metadata = run(reader, *command("query", "--harness-id", SOURCE)).stdout
            assert len(json.loads(metadata)["observations"]) == 1
            assert all(text not in metadata for text in [INPUT, OUTPUT, UNRELATED])

            gateway = subprocess.Popen(command(
                "serve-history", "--listen", "127.0.0.1:0", "--token-file", viewer_token_path,
            ), cwd=root, **principal(reader), stdout=subprocess.PIPE, text=True)
            address = ready(gateway, "Listening for history reads at http://127.0.0.1:")
            path = "/v1/history/read"
            _, identity = post(address, {"version": 1, "request": {"op": "identity"},
                                        "history_identity": None}, viewer_token, path)
            parameters = {
                "version": 1,
                "history_identity": identity["history_identity"],
                "request": {"op": "history", "selection": {"kind": "observations"},
                            "query": {"harness_id": SOURCE, "session_id": SESSION, "limit": 1}},
            }
            status, page = post(address, parameters, viewer_token, path)
            assert status == 200 and page["result"]["Ok"]["value"]["observations"]
            assert all(text not in json.dumps(page) for text in [INPUT, OUTPUT, UNRELATED])
            parameters["request"] = {"op": "reveal_telemetry"}
            assert post(address, parameters, viewer_token, path)[0] == 400
            database = data / "history.sqlite"
            probe = run(reader, sys.executable, "-c",
                        "import sys; open(sys.argv[1], 'rb').read(1)", database, code=1)
            assert "PermissionError" in probe.stderr

            selected = {
                "binding": binding, "trace_id": TRACE, "span_id": SPAN, "session_id": SESSION,
                "turn_id": "turn-fixture", "worker_id": "worker-fixture", "snapshot_id": snapshot(),
            }
            selector_path.write_text(json.dumps(selected))
            selector_path.chmod(0o644)
            preview(code=1)
            assert grant("approve")["epoch"] == 1
            preview(code=1, uid=collector)
            preview(code=1, uid=admin)
            assert preview() == {"input": INPUT, "output": OUTPUT}
            for columns, rows, close in [(60, 18, b"\x1b"), (120, 32, b"q"), (60, 18, b"\x03")]:
                cockpit_preview(command("cockpit"), principal(reader), state,
                                columns=columns, rows=rows, close=close)

            with contextlib.ExitStack() as cleanup:
                read_fd, write_fd = os.pipe2(os.O_NONBLOCK)
                cleanup.callback(os.close, read_fd)
                cleanup.callback(os.close, write_fd)
                filled = 0
                while True:
                    try:
                        filled += os.write(write_fd, b" " * 4096)
                    except BlockingIOError:
                        break
                assert filled > 4096
                process = subprocess.Popen(
                    command("trace-payload", "preview", "--selection", selector_path),
                    cwd=root, **principal(reader), stdout=write_fd,
                )
                cleanup.callback(stop, process)
                time.sleep(.30)
                revoking = subprocess.Popen(command("grant", "revoke", "--binding", bind_path),
                                            cwd=root, **principal(admin), stdout=subprocess.PIPE)
                cleanup.callback(stop, revoking)
                time.sleep(.20)
                assert process.poll() is None and revoking.poll() is None
                os.set_blocking(read_fd, True)
                while filled:
                    filler = os.read(read_fd, filled)
                    assert not filler.strip()
                    filled -= len(filler)
                assert process.wait(timeout=8) == 0
                assertion = json.loads(os.read(read_fd, 8192))
                assert assertion["payload"] == {"input": INPUT, "output": OUTPUT}
                assert json.loads(revoking.communicate(timeout=8)[0]) is True
            preview(code=1)
            assert grant("approve")["epoch"] == 2
            saved = json.loads(run(maintenance, *command("backup")).stdout)
            stop(receiver)
            stop(gateway)
            stop(service)
            data.rename(root / "previous-history")
            private_directory(data, broker, broker)
            run(broker, *service_command(), "--native-restore", saved["name"])
            service = service_start()
            preview(code=1)
            assert grant("approve")["epoch"] == 3
            preview(code=1)
            selected["snapshot_id"] = snapshot()
            assert selected["snapshot_id"].startswith("r1:")
            selector_path.write_text(json.dumps(selected))
            assert preview() == {"input": INPUT, "output": OUTPUT}
            run(maintenance, *command("prune", "--before-unix-ns", str(started + 2)))
            preview(code=1)
        finally:
            for process in (receiver, gateway, service):
                if process is not None:
                    stop(process)
        print("PASS payload admission, private CLI and cockpit, disclosure ACK, gateway, restore and retention",
              flush=True)


if __name__ == "__main__":
    main()
