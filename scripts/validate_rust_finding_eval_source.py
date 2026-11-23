"""Exact retained failed-evaluation Finding disclosure and return."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--checkout", type=Path, default=Path(__file__).resolve().parents[1])
parser.add_argument("--app", type=Path, required=True)
options = parser.parse_args()
sys.path.insert(0, str(options.checkout / "scripts"))
from validate_rust_default_cockpit import Cockpit

APP = options.app.resolve(strict=True)
SOURCE = "otlp.findings-evals"
TRACE = "a" * 32
SIBLING = "b" * 32
BODY = "Selected assertion detail fixture"
FOREIGN = "Sibling assertion detail fixture"

with TemporaryDirectory(prefix="agenturm-findings-eval-") as directory:
    base = Path(directory)
    db = base / "history.sqlite"
    env = {**os.environ, "HOME": directory, "XDG_STATE_HOME": str(base / "state"),
           "XDG_CONFIG_HOME": str(base / "config"), "XDG_DATA_HOME": str(base / "data"),
           "PATH": "", "NO_COLOR": "1", "TERM": "xterm-256color"}

    def run(*args):
        call = subprocess.run([APP, *args], cwd=base, env=env, check=False,
                              capture_output=True, text=True, timeout=10)
        assert call.returncode == 0, (args, call.stdout, call.stderr)
        assert BODY not in call.stdout + call.stderr and FOREIGN not in call.stdout + call.stderr
        return call.stdout

    def ingest(eval_rows):
        file = base / "eval.json"
        file.write_text(json.dumps({"results": eval_rows}))
        return run("import-evals", "--include-detail", "--db", db,
                   "--source-id", SOURCE, "--file", file)

    spans = [{"traceId": trace, "spanId": "c" * 16, "name": "fixture OK execution",
              "startTimeUnixNano": str(time - 10), "endTimeUnixNano": str(time),
              "status": {"code": 1},
              "attributes": [{"key": "session.id", "value": {"stringValue": session}}]}
             for trace, time, session in [(TRACE, 300, "chosen"), (SIBLING, 200, "sibling")]]
    trace_file = base / "trace.json"
    trace_file.write_text(json.dumps({"resourceSpans": [{"scopeSpans": [{"spans": spans}]}]}))
    run("import-traces", "--db", db, "--source-id", SOURCE, "--file", trace_file)
    ingest([{"trace_id": TRACE, "passed": False, "error": BODY},
            {"trace_id": SIBLING, "passed": False, "error": FOREIGN}])
    run("eval-detail", "grant", "--db", db, "--source-id", SOURCE)
    assert BODY not in run("query", "--db", db)
    assert FOREIGN not in run("evals", "--db", db, "--source-id", SOURCE, "--summary")

    for width, height, stale in [(120, 32, False), (80, 24, False),
                                  (60, 18, False), (60, 18, True)]:
        transcript = io.BytesIO()
        cockpit = Cockpit([APP, "cockpit", "--db", db], env, transcript,
                           width=width, height=height)
        try:
            def frame(label, *markers, private=False):
                return cockpit.frame(label, lambda text: all(mark in text for mark in markers)
                                     and FOREIGN not in text and (private or BODY not in text))
            frame("ready command center", "Agenturm command center", "› ")
            cockpit.send(b"7")
            frame("failed evaluation Finding", "Evaluation did not pass", "Findings")
            cockpit.send(b"\r")
            frame("select exact trace-backed outcome", "2 occurrences", "Selected trace", TRACE)
            cockpit.send(b"\r")
            frame("inspect exact outcome", "Exact retained occurrence", TRACE,
                  "Evaluation revision  1", "s source")
            if stale:
                assert "Corrected 1" in ingest([{"trace_id": TRACE, "passed": True}])
                cockpit.send(b"s")
                frame("superseded evaluation denied", "Failed evaluation changed", TRACE)
                assert BODY.encode() not in transcript.getvalue()
            else:
                cockpit.send(b"s")
                frame("one authorized evaluation detail", "Imported evaluation text", BODY, private=True)
                cockpit.send(b"cx")
                assert b"\x1b]52;" not in transcript.getvalue()
                cockpit.send(b"\x1b")
                frame("redacted return to the same exact outcome", "Exact retained occurrence", TRACE,
                      "Evaluation revision  1")
            cockpit.quit()
            assert BODY not in cockpit.screen.departed
            assert FOREIGN.encode() not in transcript.getvalue()
            assert b"\x1b]52;" not in transcript.getvalue()
            print(f"{width}x{height}: {'stale passed correction denied' if stale else 'exact protected evaluation + redacted same occurrence'}, termios", flush=True)
        except Exception:
            print(cockpit.screen.contents(), file=sys.stderr)
            raise
        finally:
            cockpit.close()
    print("App SHA-256", hashlib.sha256(APP.read_bytes()).hexdigest())
