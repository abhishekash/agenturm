"""Exercise opt-in OTLP log text, independent grant and exact Findings return."""
import argparse
from contextlib import contextmanager
import hashlib
from http.client import HTTPConnection
import io
import json
import os
from pathlib import Path
import re
import secrets
import select
import subprocess
import sys
from tempfile import TemporaryDirectory
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--checkout", type=Path, default=Path(__file__).resolve().parents[1])
parser.add_argument("--app", type=Path, required=True)
args = parser.parse_args()
sys.path.insert(0, str(args.checkout / "scripts"))
from validate_rust_default_cockpit import Cockpit

APP = args.app.resolve(strict=True)
SOURCE = "otlp.log-finding-fixture"
BODY = "Recorded primary log body fixture"
SIBLING = "Recorded sibling log body fixture"
UNCAPTURED = "Default metadata-only log body fixture"
ACTIVITY = "Explicit activity body private fixture"
BODY_MARKERS = (BODY, SIBLING, UNCAPTURED, ACTIVITY)
TRACE = "a" * 32
READY = re.compile(rb"Listening for OTLP traces at http://127\.0\.0\.1:(\d+)/v1/traces\s")
SIZES = [(120, 32), (80, 24), (60, 18)]


def log(record_id, text, stamp, *, activity=False, trace=None):
    result = {
        "eventName": "agent.activity" if activity else "sdk.fixture.error",
        "severityNumber": 17,
        "timeUnixNano": str(stamp),
        "body": {"stringValue": text},
        "attributes": [{"key": "agenturm.event.id", "value": {"stringValue": record_id}}],
    }
    if activity:
        result["attributes"].append({
            "key": "session.id", "value": {"stringValue": "body-free-activity-session"}
        })
    if trace:
        result["traceId"] = trace
    return result


def send(port, token, rows):
    body = json.dumps({"resourceLogs": [{"scopeLogs": [{"logRecords": rows}]}]})
    connection = HTTPConnection("127.0.0.1", port, timeout=4)
    try:
        connection.request("POST", "/v1/logs", body, {
            "Content-Type": "application/json", "Authorization": f"Bearer {token}",
        })
        response = connection.getresponse()
        reply = response.read()
        assert response.status == 200, (response.status, reply)
        assert not json.loads(reply).get("partialSuccess"), reply
    finally:
        connection.close()


def clean(text, *, allow=None):
    return all(marker == allow or marker not in text for marker in BODY_MARKERS)


def main():
    with TemporaryDirectory(prefix="agenturm-log-finding-pty-") as directory:
        root = Path(directory)
        database = root / "logs.sqlite"
        token_path = root / "producer.token"
        token = secrets.token_hex(32)
        token_path.write_text(token + "\n")
        token_path.chmod(0o600)
        env = {**os.environ, "HOME": directory, "XDG_STATE_HOME": str(root / "state"),
               "XDG_CONFIG_HOME": str(root / "config"), "XDG_DATA_HOME": str(root / "data"),
               "PATH": "", "TERM": "xterm-256color", "NO_COLOR": "1"}

        def run(*argv, succeeds=True, allowed_body=None):
            call = subprocess.run([APP, *argv], cwd=root, env=env, capture_output=True,
                                  text=True, timeout=8)
            assert (call.returncode == 0) == succeeds, (argv, call.returncode, call.stderr)
            assert clean(call.stdout + call.stderr, allow=allowed_body)
            return call.stdout

        @contextmanager
        def receiver(*, capture):
            extra = ["--capture-log-text"] if capture else []
            child = subprocess.Popen([APP, "serve-traces", "--listen", "127.0.0.1:0",
                                      "--db", database, "--source-id", SOURCE,
                                      "--payload-scope", "standalone", "--token-file", token_path,
                                      *extra], cwd=root, env=env,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                data = b""
                deadline = time.monotonic() + 7
                port = None
                while time.monotonic() < deadline and child.poll() is None:
                    if select.select([child.stdout], [], [], .1)[0]:
                        data += os.read(child.stdout.fileno(), 4096)
                        matched = READY.search(data)
                        if matched:
                            port = int(matched[1])
                            break
                    assert len(data) < 16384
                assert port, child.stderr.read() if child.poll() is not None else data
                yield port
            finally:
                if child.poll() is None:
                    child.terminate()
                try:
                    child.wait(timeout=4)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=4)

        stamp = time.time_ns() - 5_000_000_000
        with receiver(capture=False) as port:
            send(port, token, [log("uncaptured", UNCAPTURED, stamp + 1)])
        with receiver(capture=True) as port:
            send(port, token, [
                log("activity", ACTIVITY, stamp, activity=True),
                log("captured-sibling", SIBLING, stamp + 2, trace="b" * 32),
                log("captured-primary", BODY, stamp + 3, trace=TRACE),
            ])

        binding = json.loads(run("log-text", "binding", "--source-id", SOURCE,
                                 "--payload-scope", "standalone"))
        assert binding["kind"] == "telemetry.log_text"
        report = json.loads(run("log-text", "snapshot", "--db", database, "--source-id", SOURCE))
        chosen, sibling, uncaptured = report["selections"]
        for selection in (chosen, sibling, uncaptured):
            assert all(selection[name] is None for name in ("session_id", "turn_id", "worker_id"))
            assert selection["binding"] == {"source_id": SOURCE, "native_source_id": None}
        assert chosen["observed_at_unix_ns"] == stamp + 3
        assert not json.loads(run("log-text", "snapshot", "--db", database,
                                  "--source-id", SOURCE, "--session-id",
                                  "body-free-activity-session"))["selections"]
        selected_file = root / "selected.json"
        selected_file.write_text(json.dumps(chosen))
        uncaptured_file = root / "uncaptured.json"
        uncaptured_file.write_text(json.dumps(uncaptured))
        run("trace-payload", "grant", "--db", database, "--source-id", SOURCE,
            "--payload-scope", "standalone")
        run("log-text", "preview", "--db", database, "--selection", selected_file, succeeds=False)
        run("log-text", "grant", "--db", database, "--source-id", SOURCE,
            "--payload-scope", "standalone")
        assert json.loads(run("log-text", "preview", "--db", database, "--selection", selected_file,
                              allowed_body=BODY)) == {"origin": "producer_captured_log_text", "text": BODY}
        run("log-text", "preview", "--db", database, "--selection", uncaptured_file, succeeds=False)
        run("query", "--db", database)
        print("Exact metadata, independent grant, and default capture off passed", flush=True)

        def visit(width, height, granted):
            transcript = io.BytesIO()
            cockpit = Cockpit([APP, "cockpit", "--db", database], env, transcript,
                              width=width, height=height)

            def frame(phase, *parts, allowed_body=None):
                return cockpit.frame(phase, lambda text: all(part in text for part in parts)
                                     and clean(text, allow=allowed_body))

            try:
                frame("loaded command center", "Agenturm command center", "Loaded page", "› ")
                cockpit.send(b"p")
                frame("pinned catalog after startup", "Auto-refresh paused")
                cockpit.send(b"7")
                frame("error log groups", "Recorded error log", "Reported activity error")
                cockpit.send(b"\r")
                frame("source-scoped occurrences", "3 occurrences", "Source scope")
                cockpit.send(b"\r")
                frame("exact primary source receipt", "Exact retained occurrence", "s source",
                      "Event  otlp-error-log")
                cockpit.send(b"s")
                if granted:
                    frame("protected log text", "Producer-captured log text", BODY, allowed_body=BODY)
                    cockpit.send(b"cx")
                    assert b"\x1b]52;" not in transcript.getvalue()
                    cockpit.send(b"\x1b")
                    frame("redacted selected log", "Exact retained occurrence",
                          "Event  otlp-error-log")
                else:
                    frame("current grant denial", "Exact retained occurrence", "grant")
                    assert BODY.encode() not in transcript.getvalue()
                cockpit.quit()
                assert clean(cockpit.screen.departed)
                assert b"\x1b]52;" not in transcript.getvalue()
                assert all(marker.encode() not in transcript.getvalue() for marker in BODY_MARKERS[1:])
                print(f"{width}x{height}: {'exact body and redacted return' if granted else 'revoked body withheld'}, terminal restored",
                      flush=True)
            except Exception:
                print(cockpit.screen.contents(), file=sys.stderr)
                raise
            finally:
                cockpit.close()

        for width, height in SIZES:
            visit(width, height, True)
        run("log-text", "revoke", "--db", database, "--source-id", SOURCE,
            "--payload-scope", "standalone")
        visit(60, 18, False)
        print("App SHA-256", hashlib.sha256(APP.read_bytes()).hexdigest())


if __name__ == "__main__":
    main()
