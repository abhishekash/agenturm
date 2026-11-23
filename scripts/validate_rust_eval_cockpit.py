"""Verify exact evaluation metadata selection and return navigation in a Linux guest."""

import argparse
import fcntl
import grp
import json
import os
from pathlib import Path
import pty
import pwd
import re
import secrets
import select
import stat
import struct
import subprocess
import tempfile
import termios
import time

from validate_rust_remote_history import private_directory, request, stop, wait_for, write_token


LEFT = "otlp.left"
RIGHT = "otlp.right"
TRACE = "11" * 16
EMPTY = "22" * 16
PRIVATE = "PRIVATE_FIXTURE_EVAL_BODY"


def span(trace_id, span_id, ended, status):
    return {"traceId": trace_id, "spanId": span_id, "name": PRIVATE,
            "startTimeUnixNano": str(ended - 1), "endTimeUnixNano": str(ended),
            "status": {"code": status},
            "attributes": [{"key": "session.id", "value": {"stringValue": "eval-session"}}]}


def screen(buffer, width, height):
    """Replay Ratatui cursor positioning and text writes used by the fixture."""
    rows = [[" "] * width for _ in range(height)]
    row = column = 1
    for item in re.split(rb"(\x1b\[[0-?]*[ -/]*[@-~])", buffer):
        cursor = re.fullmatch(rb"\x1b\[(\d+);(\d+)H", item)
        if cursor:
            row, column = map(int, cursor.groups())
        elif item in (b"\x1b[2J", b"\x1b[?1049h"):
            rows = [[" "] * width for _ in range(height)]
        elif item and not item.startswith(b"\x1b"):
            for character in item.decode(errors="replace"):
                if 1 <= row <= height and 1 <= column <= width and character.isprintable():
                    rows[row - 1][column - 1] = character
                column += 1
    return "\n".join("".join(row) for row in rows)


def cockpit(command, root, uid, gid, home, size):
    width, height = size
    env = {**os.environ, "HOME": str(home), "XDG_STATE_HOME": str(home / "state"),
           "TERM": "xterm-256color"}
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
    original = termios.tcgetattr(slave)
    process = subprocess.Popen(command, cwd=root, env=env, stdin=slave, stdout=slave, stderr=slave,
                               user=uid, group=gid, extra_groups=[])
    buffer = bytearray()

    def view(*markers):
        end = time.monotonic() + 10
        while time.monotonic() < end:
            if select.select([master], [], [], 0.1)[0]:
                buffer.extend(os.read(master, 8192))
            frame = screen(buffer, width, height)
            assert PRIVATE not in frame
            if all(marker in frame for marker in markers):
                return frame
            assert process.poll() is None, frame
        raise AssertionError((markers, frame))

    def press(key):
        os.write(master, key)

    try:
        view("Agents")
        press(b"8")
        traces_before = view("Traces", "E evaluations")
        press(b"E")
        view("Trace evaluations · loaded traces", f"› {TRACE}  {RIGHT}")
        press(b"j")
        view(f"› {TRACE}  {LEFT}")
        press(b"\r")
        view("Trace evaluations · current metadata", "r2 · Passed · Current", "r1 · Failed",
             "Loaded execution spans · 1 OK · 1 error · 0 unset", "Current evaluation revision r2")
        press(b"\x1b[6~")
        view("matcher · verdict unknown · score 0")
        press(b"j")
        view("› r1 · Failed")
        press(b"s")
        frame = view("Source evaluation summary · current", "Current page · 1 passed · 0 failed", LEFT)
        assert RIGHT not in frame
        press(b"\x1b")
        view("Trace evaluations · current metadata", "› r1 · Failed")
        press(b"\x1b")
        view("Trace evaluations · loaded traces", f"› {TRACE}  {LEFT}")
        press(b"j")
        view(f"› {EMPTY}  {LEFT}")
        press(b"\r")
        view("No evaluation outcome has been recorded for this trace.")
        press(b"\x1b")
        view("Trace evaluations · loaded traces", f"› {EMPTY}  {LEFT}")
        press(b"\x1b")
        assert view("Traces", "E evaluations") == traces_before
        press(b"q")
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
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("Run inside a disposable Linux guest with separate UID support.")
    occupied_uids = {account.pw_uid for account in pwd.getpwall()}
    occupied_gids = {group.gr_gid for group in grp.getgrall()}
    broker, collector, reader = [uid for uid in range(45000, 46000) if uid not in occupied_uids][:3]
    gid = next(gid for gid in range(45000, 46000) if gid not in occupied_gids)
    app = args.bin_dir.resolve(strict=True) / "agenturm"
    with tempfile.TemporaryDirectory(prefix="agenturm-eval-cockpit-") as path:
        root = Path(path)
        root.chmod(0o755)
        data, transport, home = (root / part for part in ("data", "transport", "home"))
        private_directory(data, broker, broker)
        private_directory(transport, broker, gid, 0o2710)
        private_directory(home, reader, gid)
        socket = transport / "agenturm-native-storage-v1.sock"
        config = root / "broker.json"
        config.write_text(json.dumps({"version": 1, "socket": str(socket),
            "receiver_id": "receiver:evaluation-fixture", "service_uid": broker, "service_gid": broker}))
        config.chmod(0o644)
        token_path = root / "reader-token"
        token = secrets.token_hex(32)
        write_token(token_path, token, reader, gid)
        traces = root / "traces.json"
        results = root / "results.json"

        def command(*argv):
            return [app, "broker", *argv, "--broker-config", config]

        def run(uid, *argv):
            result = subprocess.run(command(*argv), cwd=root, user=uid, group=gid, extra_groups=[],
                                    timeout=12, capture_output=True, text=True)
            assert result.returncode == 0, (argv, result.stdout, result.stderr)
            assert PRIVATE not in result.stdout
            return result.stdout

        def start_service(sources):
            process = subprocess.Popen([app.with_name("agenturm-storage-service"),
                "--data-root", data, "--transport-dir", transport,
                "--receiver-id", "receiver:evaluation-fixture", "--client-uid", str(collector),
                "--client-gid", str(gid), "--native-reader", f"{reader}:{gid}",
                "--native-sources", sources], cwd=root, user=broker, group=broker, extra_groups=[])
            try:
                wait_for(lambda: socket.exists() and stat.S_IMODE(socket.stat().st_mode) == 0o660,
                         process, "Evaluation broker unavailable")
            except BaseException:
                stop(process)
                raise
            return process

        def import_spans(source, spans):
            traces.write_text(json.dumps({"resourceSpans": [{"scopeSpans": [{"spans": spans}]}]}))
            run(collector, "import-traces", "--source-id", source, "--file", traces)

        def import_eval(source, passed, task):
            results.write_text(json.dumps({"results": [{"trace_id": TRACE, "passed": passed,
                "task_id": task, "scores": [{"scorer": "matcher", "score": 0, "detail": PRIVATE}],
                "error": PRIVATE, "details": {"output": PRIVATE}}]}))
            run(collector, "import-evals", "--source-id", source, "--file", results)

        service = start_service(f"{LEFT},{RIGHT}")
        gateway = None
        try:
            import_spans(LEFT, [span(EMPTY, "11" * 8, 100, 0), span(TRACE, "22" * 8, 200, 1),
                                span(TRACE, "33" * 8, 220, 2)])
            import_spans(RIGHT, [span(TRACE, "22" * 8, 400, 1)])
            import_eval(LEFT, False, "left-first")
            import_eval(LEFT, True, "left-current")
            import_eval(RIGHT, False, "right-current")
            cockpit(command("cockpit"), root, reader, gid, home, (120, 32))

            gateway = subprocess.Popen(command("serve-history", "--listen", "127.0.0.1:0",
                "--token-file", token_path), cwd=root, stdout=subprocess.PIPE, text=True,
                user=reader, group=gid, extra_groups=[])
            assert select.select([gateway.stdout], [], [], 8)[0]
            endpoint = gateway.stdout.readline().strip().split()[-1]
            _, identity = request(endpoint, token, {"op": "identity"})
            binding = identity["history_identity"]
            _, history = request(endpoint, token, {"op": "history", "query": {"limit": 12},
                                  "selection": {"kind": "observations"}}, binding)
            snapshot = history["result"]["Ok"]["value"]["snapshot_id"]
            query = {"trace_id": TRACE, "evidence_snapshot": snapshot,
                     "view": {"view": "history", "trace_id": TRACE, "limit": 1}}

            def selected(source):
                code, reply = request(endpoint, token, {"op": "selected_evals", "source_id": source,
                                      "query": query}, binding)
                assert code == 200 and PRIVATE not in json.dumps(reply)
                return reply["result"]

            assert selected(LEFT)["Ok"]["value"]["page"]["outcomes"][0]["outcome"]["passed"]
            assert not selected(RIGHT)["Ok"]["value"]["page"]["outcomes"][0]["outcome"]["passed"]
            assert request(endpoint, token, {"op": "import_eval_outcomes"}, binding)[0] == 400
            cockpit([app, "cockpit", "--endpoint", endpoint, "--token-file", token_path],
                    root, reader, gid, home, (60, 18))

            stop(service)
            service = start_service(RIGHT)
            assert selected(LEFT) == {"Err": {"code": "unauthorized"}}
            assert selected(RIGHT)["Ok"]["value"]["current_revision"] == 1
            print("PASS exact evaluation revisions, source summary, broker and remote PTY return, live allowlist removal",
                  flush=True)
        finally:
            if gateway is not None:
                stop(gateway)
            stop(service)


if __name__ == "__main__":
    main()
