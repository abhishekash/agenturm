"""Open and persist Workflow settings with real native pending and terminal facts."""
import argparse
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--checkout", type=Path, default=Path(__file__).resolve().parents[1])
parser.add_argument("--bin-dir", type=Path, required=True)
parser.add_argument("--provision-only", action="store_true")
options = parser.parse_args()
sys.path.insert(0, str(options.checkout / "scripts"))
from validate_rust_default_cockpit import Cockpit

SOURCE = "harness.codex.settings"
SESSION = "workflow-settings-session"
PRIVATE = "private_native_error_message_excluded"
PENDING = "pending-turn"
FAILED = "failed-turn"


def stamp(seconds):
    return datetime.fromtimestamp(time.time() + seconds, timezone.utc).isoformat()


def fact(kind, turn, seconds, **payload):
    return {"type": "event_msg", "timestamp": stamp(seconds), "payload": {
        "type": kind, "turn_id": turn, "thread_id": SESSION, **payload,
    }}


def checked(pty, description, *parts):
    return pty.frame(description, lambda text: all(part in text for part in parts) and PRIVATE not in text and "Loading" not in text)


def value(pty, field, item):
    pty.frame(f"{field}={item}", lambda text: any(
        field in line and item in line.partition(field)[2] for line in text.splitlines()
    ) and "Reading Workflow policy" not in text)


def opened(pty):
    checked(pty, "command center", "Agenturm command center", "codex.settings session")
    pty.send(b"w")
    checked(pty, "pending native approval", "Workflow", SESSION, "S settings", "c copy resume")
    pty.send(b"S")
    checked(pty, "settings form", "Workflow settings", "Task tokens", "Task USD", "Task minutes",
            "0 disables a threshold", "Ctrl+S save thresholds")


def settings(app, env, db, width, height, stored):
    transcript = io.BytesIO()
    pty = Cockpit([app, "cockpit", "--db", db], env, transcript, width=width, height=height)
    try:
        opened(pty)
        if not stored:
            value(pty, "Desktop notifications", "Use watch configuration")
            pty.send(b"\r")
            value(pty, "Desktop notifications", "On")
            # Del clears the selected input without a repeated Backspace.
            for field, data in [("Task tokens", b"21"), ("Task USD", b"1.25"), ("Task minutes", b"2.5")]:
                pty.send(b"\t\x1b[3~" + data)
                value(pty, field, data.decode())
            pty.send(b"\x13")
            checked(pty, "thresholds saved", "Workflow settings", "Thresholds saved")
            pty.send(b"z")
            value(pty, "Snooze notifications", "End snooze")
            pty.send(b"z")
            value(pty, "Snooze notifications", "Snooze 10 minutes")
        else:
            for field, expected in [("Desktop notifications", "On"), ("Task tokens", "21"),
                                    ("Task USD", "1.25"), ("Task minutes", "2.5")]:
                value(pty, field, expected)
        pty.send(b"\x1b")
        checked(pty, "return exact native pending request", "Workflow", SESSION, "S settings", "c copy resume")
        pty.send(b"\t")
        checked(pty, "typed incidents inventory", "Workflow incidents", "Run failed")
        pty.send(b"\r")
        checked(pty, "exact failed occurrence", "Selected incident", FAILED, SOURCE)
        pty.send(b"\x1b")
        checked(pty, "return incident list", "Workflow incidents", "Run failed", "Enter inspect")
        pty.send(b"\t")
        checked(pty, "same pending list after incidents", "Workflow", SESSION, "c copy resume")
        pty.quit()
        assert PRIVATE.encode() not in transcript.getvalue()
        assert b"\x1b]52;" not in transcript.getvalue()
        print(f"{width}×{height}: typed settings, scoped incidents, exact pending return and termios", flush=True)
    except Exception:
        print(pty.screen.contents(), file=sys.stderr)
        raise
    finally:
        pty.close()


def main():
    app = options.bin_dir.resolve(strict=True) / "agenturm"
    with tempfile.TemporaryDirectory(prefix="agenturm-workflow-settings-") as folder:
        root = Path(folder)
        db = root / "history.sqlite"
        rollout = root / "fixtures.jsonl"
        catalog = root / "catalog.json"
        config = root / "sources.json"
        adapter = app.with_name("agenturm-codex-harness")
        catalog.write_text(json.dumps({"version":1, "plugins":[{
            "plugin_id":"example.rust-codex-harness", "adapter":str(adapter),
            "sha256":hashlib.sha256(adapter.read_bytes()).hexdigest(), "protocol":1,
            "features":["source-adapter/1", "configured-root/1"], "observation_format":"agenturm-observations/1",
        }]}))
        config.write_text(json.dumps({"version":2, "plugin_host":str(app.with_name("agenturm-plugin-host")),
            "plugin_catalog":str(catalog), "sources":[{
                "plugin_id":"example.rust-codex-harness","source_id":SOURCE,"source_root":str(rollout),
            }]}))
        config.chmod(0o600)
        catalog.chmod(0o600)
        rollout.write_text("".join(json.dumps(row) + "\n" for row in [
            {"type":"session_meta", "payload":{"id":SESSION,"source":"cli"}},
            fact("task_started", FAILED, -12),
            fact("task_complete", FAILED, -10, error={"message":PRIVATE}),
            fact("task_started", PENDING, -6),
            fact("approval_request", PENDING, -5, request_id="settings-fixture-approval"),
        ]))
        env = {**os.environ, "HOME":folder, "XDG_STATE_HOME":str(root/"state"),
               "XDG_CONFIG_HOME":str(root/"config"), "XDG_DATA_HOME":str(root/"data"), "PATH":"",
               "NO_COLOR":"1", "TERM":"xterm-256color"}
        def run(*flags):
            result = subprocess.run([app, *flags], env=env, cwd=root, capture_output=True, text=True, timeout=15)
            assert result.returncode == 0, (flags, result.stdout, result.stderr)
            return result.stdout
        run("collect", "--config", config, "--db", db)
        report = run("query", "--db", db)
        assert PRIVATE not in report
        facts = json.loads(report)["observations"]
        assert any(row["observation"].get("state") == "failed" for row in facts)
        if options.provision_only:
            print("Provisioned one typed failure and pending approval")
            return
        for i, (width, height) in enumerate([(120,32),(80,24),(60,18)]):
            settings(app, env, db, width, height, stored=i != 0)
        print("App SHA-256", hashlib.sha256(app.read_bytes()).hexdigest())


if __name__ == "__main__":
    main()
