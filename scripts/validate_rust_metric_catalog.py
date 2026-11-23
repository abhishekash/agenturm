"""Exercise a retained Metric search whose match is outside the first page."""
import argparse
import hashlib
from http.client import HTTPConnection
import io
import json
import os
from pathlib import Path
import re
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
READY = re.compile(rb"Listening for OTLP traces at http://127\.0\.0\.1:(\d+)/v1/traces\s")
DESCRIPTION = "Archived request duration"
NEEDLE = "ARCHIVED request duration"
SELECTED = "archive.fixture.latency"


def metric(name, description, stamp, value):
    return {"name": name, "description": description, "unit": "ms",
            "gauge": {"dataPoints": [{"timeUnixNano": str(stamp), "asInt": str(value)}]}}


def post(source, metrics, db, env):
    child = subprocess.Popen([APP, "serve-traces", "--listen", "127.0.0.1:0", "--source-id",
                              source, "--db", db], env=env, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE)
    try:
        raw = b""
        deadline = time.monotonic() + 7
        port = None
        while time.monotonic() < deadline and child.poll() is None:
            if select.select([child.stdout], [], [], .1)[0]:
                raw += os.read(child.stdout.fileno(), 8192)
                ready = READY.search(raw)
                if ready:
                    port = int(ready[1])
                    break
            assert len(raw) < 16384
        assert port, child.stderr.read() if child.poll() is not None else raw
        body = json.dumps({"resourceMetrics": [{
            "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "worker-metrics"}}]},
            "scopeMetrics": [{"scope": {"name": "schema.fixture"}, "metrics": metrics}],
        }]})
        peer = HTTPConnection("127.0.0.1", port, timeout=4)
        try:
            peer.request("POST", "/v1/metrics", body, {"Content-Type": "application/json"})
            response = peer.getresponse()
            reply = response.read()
            assert response.status == 200, (response.status, reply)
        finally:
            peer.close()
    finally:
        if child.poll() is None:
            child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)


with TemporaryDirectory(prefix="agenturm-metric-catalog-search-") as directory:
    base = Path(directory)
    db = base / "history.sqlite"
    env = {**os.environ, "HOME": directory, "XDG_STATE_HOME": str(base / "state"),
           "XDG_CONFIG_HOME": str(base / "config"), "XDG_DATA_HOME": str(base / "data"),
           "PATH": "", "TERM": "xterm-256color", "NO_COLOR": "1"}
    stamp = time.time_ns()
    post("otlp.a-catalog", [metric(f"catalog.fill.{i:02d}", "Current load", stamp, i)
                            for i in range(40)], db, env)
    post("otlp.z-catalog", [metric(SELECTED, DESCRIPTION, stamp - 3_000_000_000, 17)], db, env)
    for width, height in [(120, 32), (80, 24), (60, 18)]:
        cockpit = Cockpit([APP, "cockpit", "--db", db], env, io.BytesIO(),
                          width=width, height=height)

        def frame(description, *parts, absent=()):
            return cockpit.frame(description, lambda cells: all(part in cells for part in parts)
                                 and all(part not in cells for part in absent))

        try:
            frame("ready roster", "Agenturm command center", "Loaded page")
            cockpit.send(b"p")
            frame("paused roster", "Auto-refresh paused")
            cockpit.send(b"6")
            first = frame("initial metric page excludes old match", "OTLP series", "catalog.fill.",
                          "d details", "Snapshot", absent=(SELECTED,))
            pinned = re.search(r"Snapshot (\S+)", first)[1]
            cockpit.send(b"/" + NEEDLE.encode() + b"\r")
            frame("catalog search finds older exact series", "Search catalog", SELECTED,
                  f"Snapshot {pinned}", absent=("catalog.fill.",))
            cockpit.send(b"d")
            frame("recorded series description", "Metric series details", SELECTED,
                  "Description " + DESCRIPTION)
            cockpit.send(b"\r")
            frame("exact retained points", "Metric points", "17")
            cockpit.send(b"\r")
            frame("retained point detail", "Metric point ", "Value 17", SELECTED)
            cockpit.send(b"\x1b")
            frame("return to points", "Metric points")
            cockpit.send(b"\x1b")
            frame("return to selected series details", "Metric series details", DESCRIPTION)
            cockpit.send(b"\x1b")
            frame("return selected search result", "Search catalog", SELECTED, f"Snapshot {pinned}")
            cockpit.send(b"/" + b"\x7f" * len(NEEDLE) + b"\r")
            frame("blank query restores full pinned catalog", "catalog.fill.", f"Snapshot {pinned}",
                  absent=(SELECTED, "Search catalog"))
            cockpit.quit()
            print(f"{width}x{height}: older series search, description, pinned points, exact return and blank reset",
                  flush=True)
        except Exception:
            print(cockpit.screen.contents(), file=sys.stderr)
            raise
        finally:
            cockpit.close()
    print("App SHA-256", hashlib.sha256(APP.read_bytes()).hexdigest())
