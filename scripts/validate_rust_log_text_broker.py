"""Qualify OTLP error log capture, disclosure, and restore via real Linux UIDs."""

from __future__ import annotations

import argparse
import contextlib
import copy
import fcntl
import grp
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

from validate_rust_private_eval_details import TerminalCells
from validate_rust_telemetry_payloads import post, private_directory, ready, stop

BOUND = "otlp.bound-logs"
STANDALONE = "otlp.standalone-logs"
DEFAULT = "otlp.default-logs"
NATIVE = "harness.codex"
SESSION = "log-session"
TURN = "log-turn"
WORKER = "log-worker"
TEXT = "EXACT_CAPTURED_LOG_62046"
STANDALONE_TEXT = "EXACT_STANDALONE_LOG_90172"
SESSION_TEXT = "EXACT_SESSION_LOG_41038"
DEFAULT_TEXT = "RAW_DEFAULT_LOG_08423"
UNRELATED = "UNRETAINED_LOG_ATTRIBUTE_77391"
MARKERS = (TEXT, STANDALONE_TEXT, SESSION_TEXT, DEFAULT_TEXT, UNRELATED)
TRACE = "1234567890abcdef1234567890abcdef"
SPAN = "1234567890abcdef"


def error_log(label, body, at, scope=()):
    return {
        "severityNumber": 17,
        "eventName": UNRELATED,
        "timeUnixNano": str(at),
        "observedTimeUnixNano": str(at + 1),
        "traceId": TRACE,
        "spanId": SPAN,
        "body": {"stringValue": body},
        "attributes": [
            {"key": key, "value": {"stringValue": value}}
            for key, value in [("agenturm.log.id", label), ("log.raw", UNRELATED), *scope]
        ],
    }


def export(*records):
    return {"resourceLogs": [{"scopeLogs": [{"logRecords": records}]}]}


def safe_metadata(value):
    text = json.dumps(value)
    assert not any(marker in text for marker in MARKERS)


def cockpit_preview(command, principal, state, start_revocation):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 120, 0, 0))
    original = termios.tcgetattr(slave)
    env = {
        **os.environ,
        "HOME": str(state),
        "XDG_STATE_HOME": str(state / "xdg"),
        "TERM": "xterm-256color",
    }
    process = subprocess.Popen(
        command, cwd=state, stdin=slave, stdout=slave, stderr=slave, env=env, **principal
    )
    screen = TerminalCells(120, 32)

    def until(*markers, private=False):
        end = time.monotonic() + 8
        while time.monotonic() < end:
            if select.select([master], [], [], 0.05)[0]:
                new = os.read(master, 8192)
                screen.feed(new)
            frame = screen.contents()
            if frame and all(marker in frame for marker in markers):
                if not private and any(marker in frame for marker in MARKERS):
                    continue
                return frame
            assert process.poll() is None, (markers, frame)
        raise AssertionError((markers, screen.contents()))

    try:
        until("Agenturm command center")
        os.write(master, b"7")
        until("Findings", "Recorded error log")
        os.write(master, b"\r")
        until("occurrences", "Selected event")
        os.write(master, b"\r")
        until("Exact retained occurrence", BOUND, SESSION, TURN, WORKER, "s source")
        os.write(master, b"s")
        private_frame = until("Producer-captured log text", TEXT, private=True)
        assert UNRELATED not in private_frame
        revoking = start_revocation()
        time.sleep(0.2)
        assert revoking.poll() is None, "the grant fence released while its private frame was visible"
        os.write(master, b"\x1b")
        until("Exact retained occurrence", BOUND, WORKER)
        stdout, stderr = revoking.communicate(timeout=8)
        assert revoking.returncode == 0 and json.loads(stdout) is True, stderr
        os.write(master, b"q")
        end = time.monotonic() + 8
        while time.monotonic() < end:
            if select.select([master], [], [], 0.05)[0]:
                screen.feed(os.read(master, 65536))
            if process.poll() is not None and not select.select([master], [], [], 0)[0]:
                break
        assert process.wait(timeout=1) == 0
        assert screen.departed and "Exact retained occurrence" in screen.departed
        assert not any(marker in screen.departed for marker in MARKERS)
        assert termios.tcgetattr(slave) == original
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=8)
        os.close(master)
        os.close(slave)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", required=True, type=Path)
    parser.add_argument("--skip-ui", action="store_true")
    options = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("Run in a disposable Linux guest")
    binaries = options.bin_dir.resolve(strict=True)
    taken_users = {entry.pw_uid for entry in pwd.getpwall()}
    taken_groups = {entry.gr_gid for entry in grp.getgrall()}
    broker, collector, reader, admin, maintainer, denied = [
        UID for UID in range(45000, 46000) if UID not in taken_users
    ][:6]
    client_gid = next(gid for gid in range(45000, 46000) if gid not in taken_groups)

    with tempfile.TemporaryDirectory(prefix="agenturm-otlp-log-text-") as directory:
        root = Path(directory)
        root.chmod(0o755)
        data, transport, backup_root, reader_state = (
            root / part for part in ("data", "socket", "backup", "reader-state")
        )
        private_directory(data, broker, broker)
        private_directory(transport, broker, client_gid, 0o2710)
        private_directory(backup_root, broker, broker)
        private_directory(reader_state, reader, client_gid)
        socket = transport / "agenturm-native-storage-v1.sock"
        config = root / "broker.json"
        receiver_id = "receiver:private-otlp-log-text"
        config.write_text(json.dumps({
            "version": 1, "socket": str(socket), "receiver_id": receiver_id,
            "service_uid": broker, "service_gid": broker,
        }))
        config.chmod(0o644)
        producer_token = secrets.token_hex(32)
        token_file = root / "producer.token"
        token_file.write_text(producer_token + "\n")
        os.chown(token_file, collector, client_gid)
        token_file.chmod(0o600)
        viewer_token = secrets.token_hex(32)
        viewer_file = root / "viewer.token"
        viewer_file.write_text(viewer_token + "\n")
        os.chown(viewer_file, reader, client_gid)
        viewer_file.chmod(0o600)
        selector = root / "selection.json"
        selector.touch(mode=0o644)

        def principal(uid):
            return dict(user=uid, group=broker if uid == broker else client_gid, extra_groups=[])

        def command(*args):
            return [binaries / "agenturm", "broker", "--broker-config", config, *args]

        def run(uid, *args, code=0):
            result = subprocess.run(
                args, cwd=root, **principal(uid), capture_output=True, text=True, timeout=16
            )
            assert result.returncode == code, (args[1:4], result.stdout, result.stderr)
            return result

        storage = [
            binaries / "agenturm-storage-service",
            "--data-root", data, "--transport-dir", transport, "--receiver-id", receiver_id,
            "--client-uid", str(collector), "--client-gid", str(client_gid),
            "--native-reader", f"{reader}:{client_gid}",
            "--native-grant-admin", f"{admin}:{client_gid}",
            "--native-maintenance", f"{maintainer}:{client_gid}",
            "--native-backup-root", backup_root,
            "--native-sources", ",".join((BOUND, STANDALONE, DEFAULT, NATIVE)),
            "--native-telemetry-payload-sources",
            f"{BOUND}={NATIVE},{STANDALONE},{DEFAULT}",
        ]

        def service_start(cleanup):
            process = subprocess.Popen(storage, cwd=root, **principal(broker))
            cleanup.callback(stop, process)
            end = time.monotonic() + 8
            while time.monotonic() < end:
                assert process.poll() is None
                if socket.is_socket() and socket.stat().st_mode & 0o777 == 0o660:
                    return process
                time.sleep(0.03)
            raise AssertionError("broker did not bind")

        def serve_args(source, scope, capture, token=token_file):
            capture_args = ("--capture-log-text",) if capture else ()
            return command(
                "serve-traces", "--listen", "127.0.0.1:0", "--source-id", source,
                "--payload-scope", scope, "--token-file", token, *capture_args,
            )

        def receiver_start(cleanup, source, scope, *, capture=True):
            process = subprocess.Popen(
                serve_args(source, scope, capture), cwd=root, **principal(collector),
                stdout=subprocess.PIPE, text=True,
            )
            cleanup.callback(stop, process)
            return process, ready(process, "Listening for OTLP traces at http://127.0.0.1:")

        def bind(source, scope, *, span=False):
            path = root / f"{source}-{scope}-{'span' if span else 'log'}.json"
            result = run(
                reader, binaries / "agenturm", "trace-payload" if span else "log-text",
                "binding", "--source-id", source, "--payload-scope", scope,
            )
            assert json.loads(result.stdout)["kind"] == (
                "telemetry.span_payload" if span else "telemetry.log_text"
            )
            path.write_text(result.stdout)
            path.chmod(0o644)
            return path

        def grant(path, verb, *, uid=admin, code=0):
            result = run(uid, *command("grant", verb, "--binding", path), code=code)
            if code:
                assert not result.stdout
                return
            return json.loads(result.stdout)

        def snapshot(source=BOUND, session=None):
            scope = ("--session-id", session) if session is not None else ()
            result = run(reader, *command("log-text", "snapshot", "--source-id", source, *scope))
            value = json.loads(result.stdout)
            safe_metadata(value)
            return value

        def preview(selection, *, uid=reader, text=None):
            selector.write_text(json.dumps(selection))
            result = run(
                uid, *command("log-text", "preview", "--selection", selector),
                code=0 if text is not None else 1,
            )
            if text is None:
                assert not result.stdout
                assert not any(marker in result.stderr for marker in MARKERS)
            else:
                assert json.loads(result.stdout) == {
                    "origin": "producer_captured_log_text", "text": text,
                }

        def send(endpoint, body, *, token=producer_token, status=200):
            actual, response = post(endpoint, body, token, "/v1/logs")
            assert actual == status, (actual, status, response)
            safe_metadata(response)
            return response

        at = time.time_ns()
        native_scope = (
            ("session.id", SESSION), ("agenturm.source.id", NATIVE),
            ("agenturm.turn.id", TURN), ("agenturm.worker.id", WORKER),
        )
        bound = error_log("bound-log", TEXT, at, native_scope)
        bound_export = export(bound)

        with contextlib.ExitStack() as cleanup:
            service = service_start(cleanup)
            bind_log = bind(BOUND, NATIVE)
            bind_span = bind(BOUND, NATIVE, span=True)
            bind_wrong_pair = bind(BOUND, "standalone")
            grant(bind_wrong_pair, "approve", code=1)
            for wrong_uid in (reader, denied):
                role_token = root / f"role-{wrong_uid}.token"
                role_token.write_text(secrets.token_hex(32) + "\n")
                os.chown(role_token, wrong_uid, client_gid)
                role_token.chmod(0o600)
                attempted = run(wrong_uid, *serve_args(BOUND, NATIVE, True, role_token), code=1)
                assert not attempted.stdout and "broker" in attempted.stderr.lower()
            capture, intake = receiver_start(cleanup, BOUND, NATIVE)
            send(intake, bound_export, token=None, status=401)
            send(intake, bound_export, token="f" * 64, status=401)
            send(intake, bound_export)
            first = snapshot(session=SESSION)
            [issued] = first["selections"]
            assert issued["binding"] == {"source_id": BOUND, "native_source_id": NATIVE}
            assert issued["session_id"] == SESSION
            assert issued["turn_id"] == TURN and issued["worker_id"] == WORKER
            assert issued["revision"] == 0 and issued["event_id"].startswith("otlp-error-log:")
            send(intake, bound_export)
            assert snapshot(session=SESSION) == first
            metadata = json.loads(run(reader, *command("query", "--harness-id", BOUND)).stdout)
            safe_metadata(metadata)
            [row] = metadata["observations"]
            assert row["event_id"] == issued["event_id"]
            assert row["observation"]["kind"] == "otlp_error_log"
            assert row["observation"]["severity_number"] == 17
            assert row["observation"]["trace_id"] == TRACE

            preview(issued)
            assert grant(bind_span, "approve")["epoch"] == 1
            preview(issued)
            grant(bind_log, "approve", uid=reader, code=1)
            assert grant(bind_log, "approve")["epoch"] == 1
            for uid in (collector, admin, denied):
                preview(issued, uid=uid)
            mismatched = [
                {"binding": {"source_id": BOUND, "native_source_id": None}},
                {"session_id": "sibling-session"}, {"turn_id": None}, {"worker_id": None},
                {"observed_at_unix_ns": at + 100}, {"revision": 1},
                {"event_id": "otlp-error-log:" + "a" * 64},
            ]
            for mutation in mismatched:
                preview({**issued, **mutation})
            preview(issued, text=TEXT)
            def start_revocation():
                process = subprocess.Popen(
                    command("grant", "revoke", "--binding", bind_log), cwd=root,
                    **principal(admin), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                )
                cleanup.callback(stop, process)
                return process

            if options.skip_ui:
                assert grant(bind_log, "revoke") is True
            else:
                cockpit_preview(
                    command("cockpit"), principal(reader), reader_state, start_revocation
                )
            preview(issued)
            assert grant(bind_log, "approve")["epoch"] == 2
            preview(issued, text=TEXT)

            changed = copy.deepcopy(bound)
            changed["body"]["stringValue"] = "fixed replay conflict fixture"
            companion = error_log("sibling", "another log in same rejected batch", at + 2, native_scope)
            send(intake, export(changed, companion), status=503)
            assert snapshot(session=SESSION) == first
            rejected = copy.deepcopy(companion)
            rejected["body"]["stringValue"] = "Authorization: Bearer synthetic secret"
            send(intake, export(rejected), status=400)
            assert snapshot(session=SESSION) == first

            _, plain_intake = receiver_start(cleanup, STANDALONE, "standalone")
            standalone_log = error_log("anonymous-scope", STANDALONE_TEXT, at + 3)
            session_log = error_log(
                "session-scope", SESSION_TEXT, at + 4, (("session.id", SESSION),)
            )
            activity = error_log(
                "activity-record", DEFAULT_TEXT, at + 5, (("session.id", SESSION),)
            )
            activity["eventName"] = "agent.activity"
            send(plain_intake, export(standalone_log, session_log, activity))
            plain = snapshot(STANDALONE)
            assert plain["scanned_observations"] == 3 and len(plain["selections"]) == 2
            assert grant(bind(STANDALONE, "standalone"), "approve")["epoch"] == 1
            plain_entries = {entry["session_id"]: entry for entry in plain["selections"]}
            assert set(plain_entries) == {None, SESSION}
            for session, selected in plain_entries.items():
                assert selected["binding"]["native_source_id"] is None
                assert selected["turn_id"] is None and selected["worker_id"] is None
                preview(selected, text=STANDALONE_TEXT if session is None else SESSION_TEXT)

            default_receiver, default_intake = receiver_start(
                cleanup, DEFAULT, "standalone", capture=False
            )
            default_log = error_log("capture-off", DEFAULT_TEXT, at + 6)
            send(default_intake, export(default_log))
            [default_selection] = snapshot(DEFAULT)["selections"]
            assert grant(bind(DEFAULT, "standalone"), "approve")["epoch"] == 1
            preview(default_selection)
            stop(default_receiver)
            _, default_intake = receiver_start(cleanup, DEFAULT, "standalone")
            send(default_intake, export(default_log), status=503)
            preview(default_selection)

            gateway = subprocess.Popen(command(
                "serve-history", "--listen", "127.0.0.1:0", "--token-file", viewer_file,
            ), cwd=root, **principal(reader), stdout=subprocess.PIPE, text=True)
            cleanup.callback(stop, gateway)
            address = ready(gateway, "Listening for history reads at http://127.0.0.1:")
            route = "/v1/history/read"
            _, identity = post(
                address, {"version": 1, "request": {"op": "identity"}, "history_identity": None},
                viewer_token, route,
            )
            request = {
                "version": 1, "history_identity": identity["history_identity"],
                "request": {"op": "history", "selection": {"kind": "observations"},
                            "query": {"harness_id": BOUND, "limit": 16}},
            }
            status, page = post(address, request, viewer_token, route)
            assert status == 200
            assert page["result"]["Ok"]["value"]["observations"][0]["kind"]["kind"] == "otlp_error_log"
            safe_metadata(page)
            request["request"] = {"op": "reveal_telemetry_log", "selection": issued}
            assert post(address, request, viewer_token, route)[0] == 400
            probe = run(reader, sys.executable, "-c",
                        "import sys; open(sys.argv[1], 'rb').read(1)", data / "history.sqlite",
                        code=1)
            assert "PermissionError" in probe.stderr
            stop(gateway)

            backup = json.loads(run(maintainer, *command("backup")).stdout)
            stop(capture)
            stop(service)
            data.rename(root / "prior-data")
            private_directory(data, broker, broker)
            run(broker, *storage, "--native-restore", backup["name"])
            service_start(cleanup)
            preview(issued)
            assert grant(bind_log, "approve")["epoch"] == 3
            preview(issued)
            [refreshed] = snapshot(session=SESSION)["selections"]
            assert refreshed["history_identity"] == issued["history_identity"]
            assert refreshed["snapshot_id"].startswith("r1:")
            assert refreshed["event_id"] == issued["event_id"]
            preview(refreshed, text=TEXT)

            run(maintainer, *command("prune", "--before-unix-ns", str(at + 1)))
            preview(refreshed)
            assert snapshot(session=SESSION)["selections"] == []
            _, intake = receiver_start(cleanup, BOUND, NATIVE)
            send(intake, bound_export)
            preview(refreshed)
            assert snapshot(session=SESSION)["selections"] == []

    print("PASS actual broker roles, bounded log capture, current grants, immutable receipts, private restore/prune, and metadata gateway")


if __name__ == "__main__":
    main()
