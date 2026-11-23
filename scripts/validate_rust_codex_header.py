"""Qualify installed Codex header discovery and check evidence using disposable rollouts."""

from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile

from validate_rust_real_session_roster import Cockpit, require

CODEX_PLUGIN = "example.rust-codex-harness"
SESSION = "f0710101-caca-4a4a-8081-000000000001"
GUARDIAN = "f0710101-caca-4a4a-8081-000000000002"
TURN = "fixture-older-running-turn"
PRIVATE = "PRIVATE_HEADER_ONLY_VALIDATION_SENTINEL"


def stamp(time):
    return time.isoformat(timespec="seconds").replace("+00:00", "Z")


def line(value):
    return (json.dumps(value, separators=(",", ":")) + "\n").encode()


def header(session, time, **metadata):
    return line({
        "type": "session_meta",
        "payload": {"id": session, "timestamp": stamp(time), "source": "cli", **metadata},
    })


def event(time, payload):
    return line({"type": "event_msg", "timestamp": stamp(time), "payload": payload})


class Witness:
    def __init__(self, root, current, previous):
        self.root = root
        self.app = current
        self.previous = previous
        self.codex = root / "codex"
        self.env = {
            "HOME": str(root), "XDG_STATE_HOME": str(root / "state"),
            "XDG_CONFIG_HOME": str(root / "config"), "XDG_DATA_HOME": str(root / "data"),
            "CODEX_HOME": str(self.codex), "AGENTURM_OTLP_PORT": "0",
            "PATH": os.defpath, "LANG": "C.UTF-8", "TERM": "xterm-256color", "NO_COLOR": "1",
        }

    def run(self, app, *arguments, status=0):
        output = subprocess.run(
            [app, *arguments], cwd=self.root, env=self.env,
            capture_output=True, text=True, timeout=30, check=False,
        )
        require(output.returncode == status,
                f"{app.name} {arguments[0]} exited {output.returncode}, expected {status}")
        require(PRIVATE not in output.stdout and PRIVATE not in output.stderr,
                f"{arguments[0]} reflected the private fixture marker")
        return output.stdout

    def discover(self, app, path, config_path):
        config = json.loads(self.run(app, "discover", "--sessions-per-harness", "8"))
        require(config.get("version") == 2 and bool(config.get("plugin_catalog")),
                "Installed discovery did not select its pinned bundle catalog")
        ordinary = [source for source in config["sources"] if Path(source["source_root"]) == path]
        require(len(ordinary) == 1 and ordinary[0]["plugin_id"] == CODEX_PLUGIN,
                "Discovery did not find the unique exact Codex header")
        config_path.write_text(json.dumps(config), encoding="utf-8")
        return ordinary[0]["source_id"], config

    def collect(self, db, config, *, app=None, status=0):
        self.run(app or self.app, "collect", "--db", db, "--config", config, status=status)

    def summary(self, db, *, app=None, rendered=False):
        args = ("cockpit", "--db", db, "--once")
        if rendered:
            return self.run(app or self.app, *args)
        summary = json.loads(self.run(app or self.app, *args, "--json"))
        require(summary.get("format") == "agenturm-summary/1" and not summary.get("has_more"),
                "Installed cockpit did not return a complete fixture roster")
        return summary["sessions"]

    def query(self, db, source, *, app=None):
        data = json.loads(self.run(app or self.app, "query", "--db", db, "--harness-id", source))
        require(data.get("next_before") is None, "Fixture query unexpectedly paginated")
        return data["observations"]

    @staticmethod
    def cursor(db, source):
        with sqlite3.connect(db.as_uri() + "?mode=ro", uri=True, timeout=3) as history:
            row = history.execute("SELECT cursor FROM agenturm_source_state WHERE source_id=?",
                                  (source,)).fetchone()
        require(row is not None, "Missing committed adapter checkpoint")
        proof = row[0].split(":", 7)
        require(len(proof) == 8 and proof[0] == "v2", "Unexpected Codex checkpoint encoding")
        require(all(value.isdecimal() for value in proof[1:4]), "Invalid Codex checkpoint position")
        return row[0], proof

    def check_unknown(self, db, source):
        sessions = self.summary(db)
        require(len(sessions) == 1, "Roster should contain exactly one ordinary fixture session")
        row = sessions[0]
        require((row["harness_id"], row["session_id"], row["lifecycle"]) == (source, SESSION, None),
                "Header-only evidence changed exact source, session or lifecycle")
        require(f"{source} / {SESSION} · unknown ·" in self.summary(db, rendered=True),
                "Installed summary did not display the header-only session as unknown")
        observations = self.query(db, source)
        require(len(observations) == 1, "Header-only session should have one durable observation")
        discovery = observations[0]
        require(discovery["observation"] == {"kind": "activity", "category": "session_discovered"}
                and discovery["identity"]["session_id"] == SESSION
                and discovery["identity"]["turn_id"] is None,
                "Header discovery evidence has the wrong identity or kind")
        return discovery

    def execute(self):
        now = datetime.now(timezone.utc) - timedelta(seconds=5)
        directory = self.codex / "sessions" / now.strftime("%Y/%m/%d")
        directory.mkdir(parents=True)
        path = directory / f"rollout-{now:%Y-%m-%dT%H-%M-%S}-{SESSION}.jsonl"
        data = header(SESSION, now, cwd=str(self.root), instructions=PRIVATE)
        path.write_bytes(data)
        old_config, config = self.root / "old.json", self.root / "current.json"
        upgraded, fresh, moved = (self.root / name for name in ("upgraded.sqlite", "fresh.sqlite", "moved.sqlite"))

        old_source, _ = self.discover(self.previous, path, old_config)
        self.collect(upgraded, old_config, app=self.previous)
        self.collect(moved, old_config, app=self.previous)
        old_cursor, old_proof = self.cursor(upgraded, old_source)
        require(int(old_proof[3]) == len(data) and not old_proof[7],
                "Previous implementation did not leave the exact header-only checkpoint")
        require(self.summary(upgraded, app=self.previous) == []
                and self.query(upgraded, old_source, app=self.previous) == [],
                "Previous bundle already projects a discovery row; choose a pre-discovery bundle")

        source, current_config = self.discover(self.app, path, config)
        require(source == old_source, "Installation update changed source identity")
        self.collect(fresh, config)
        first = self.check_unknown(fresh, source)
        require(int(first["observed_at_unix_ns"]) == int(now.timestamp()) * 1_000_000_000,
                "Discovery did not retain the recorded header timestamp")
        self.collect(upgraded, config)
        backfill = self.check_unknown(upgraded, source)
        settled_cursor, settled_proof = self.cursor(upgraded, source)
        require(first == backfill and old_cursor != settled_cursor
                and old_proof[:7] == settled_proof[:7] and bool(settled_proof[7]),
                "Backfill did not preserve the original generation, offset, hashes and event identity")
        self.collect(upgraded, config)
        require(self.cursor(upgraded, source)[0] == settled_cursor
                and self.query(upgraded, source) == [backfill],
                "A restarted collector re-admitted the same header")
        print("PASS installed static discovery, exact old-header backfill, repeated collection and Unknown roster")

        guardian = directory / f"rollout-{now:%Y-%m-%dT%H-%M-%S}-{GUARDIAN}.jsonl"
        guardian.write_bytes(header(GUARDIAN, now, thread_source="guardian_review", instructions=PRIVATE))
        guardian_source = "review.codex." + hashlib.sha256(os.fsencode(guardian)).hexdigest()
        guardian_config = self.root / "guardian.json"
        guardian_config.write_text(json.dumps({
            **current_config, "sources": [{"plugin_id": CODEX_PLUGIN, "source_id": guardian_source,
                                            "source_root": str(guardian)}],
        }), encoding="utf-8")
        self.collect(upgraded, guardian_config)
        private_review = self.query(upgraded, guardian_source)
        require(len(private_review) == 1 and private_review[0]["observation"]["kind"] == "internal_review",
                "Guardian source did not retain exactly its reserved classification")
        require(self.check_unknown(upgraded, source) == backfill,
                "Guardian review inserted an ordinary roster session")
        print("PASS guardian classification is isolated from ordinary session inventory")

        held = directory / "held-source.fixture"
        path.rename(held)
        self.collect(moved, config, status=1)
        require(self.cursor(moved, source)[0] == old_cursor and self.query(moved, source) == [],
                "Missing path advanced the verified old header checkpoint")
        invalid = data.replace(f'"id":"{SESSION}"'.encode(), f'"id":"{SESSION}","id":"duplicate"'.encode())
        require(invalid != data, "Duplicate-identity fixture did not alter its marker")
        path.write_bytes(invalid)
        self.collect(moved, config, status=1)
        require(self.cursor(moved, source)[0] == old_cursor and self.query(moved, source) == [],
                "Malformed replacement advanced or published old-header evidence")
        path.write_bytes(data)
        self.collect(moved, config)
        moved_rows = self.query(moved, source)
        moved_discovery = [row for row in moved_rows if row["observation"].get("category") == "session_discovered"]
        require(len(moved_discovery) == 1 and moved_discovery[0]["event_id"] != first["event_id"]
                and any(row["observation"].get("code") == "possible_source_gap" for row in moved_rows)
                and self.cursor(moved, source)[1][6] != old_proof[6],
                "New inode was treated as the acknowledged old header")
        path.unlink()
        held.rename(path)
        print("PASS aborted and moved-header reads preserve authority and require a new generation")

        commands = ("cargo fmt --all -- --check", "pytest --fixtures")
        with path.open("ab") as transcript:
            transcript.write(event(now - timedelta(seconds=1), {"type": "task_started", "turn_id": TURN}))
            for index, command in enumerate(commands):
                transcript.write(event(now + timedelta(seconds=index + 1), {
                    "type": "item_completed", "turn_id": TURN,
                    "item": {"type": "CommandExecution", "id": f"command-{index}", "status": "completed",
                             "exit_code": 0, "command": ["/usr/bin/zsh", "-lc", command],
                             "aggregated_output": PRIVATE},
                }))
        self.collect(upgraded, config)
        rows = self.query(upgraded, source)
        retained = self.summary(upgraded)
        require(len(retained) == 1 and retained[0]["lifecycle"]["state"] == "running"
                and retained[0]["lifecycle"]["turn_id"] == TURN,
                "A newer header timestamp masked the exact recorded running lifecycle")
        checks = [row for row in rows if row["observation"]["kind"] == "check"]
        tools = [row for row in rows if row["observation"]["kind"] == "tool"]
        require(len(checks) == 1 and checks[0]["observation"] == {
                    "kind": "check", "category": "lint", "result": "passed",
                } and len(tools) == 2
                and sum(row["identity"] == checks[0]["identity"] for row in tools) == 1,
                "Exact formatting validation and informational shell operation were not separated")
        require(all(command not in json.dumps(rows) for command in commands),
                "Check or informational command text entered durable metadata")
        self.collect(upgraded, config)
        require(self.query(upgraded, source) == rows, "Running and check evidence was duplicated after restart")
        print("PASS exact running lifecycle, recorded formatting check and informational command boundary")

        with Cockpit(self.app, self.env, self.root, 60, 18, db=fresh) as cockpit:
            cockpit.markers("header-only command center", "Agenturm command center", "Unknown")
            cockpit.quit()
        print("PASS 60×18 installed Unknown cockpit and terminal restoration")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", required=True, type=Path, help="Current installed bundle's bin/agenturm")
    parser.add_argument("--previous", required=True, type=Path, help="Pre-discovery installed bundle's bin/agenturm")
    args = parser.parse_args()
    current, previous = (value.resolve(strict=True) for value in (args.candidate, args.previous))
    require(current != previous, "Candidate and previous binaries must be separate installations")
    with tempfile.TemporaryDirectory(prefix="agenturm-installed-codex-header-") as directory:
        Witness(Path(directory), current, previous).execute()


if __name__ == "__main__":
    main()
