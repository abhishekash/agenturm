"""Exact-binary, isolated retained-cockpit smoke gate (not full UI acceptance).

No builds, discovery, adapters, real logs or default live receiver are started.
Uses the existing Cockpit/TerminalCells and validation ownership contracts.
"""
from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import struct
import subprocess
import sys
import termios
import time

from validate_rust_default_cockpit import Cockpit
from validate_rust_private_eval_details import TerminalCells
from validation_support import (application_revision, create_owned_directory,
                                file_hash, isolated_env)

SIZES = ((120, 32), (80, 24), (60, 18))
ROOT = Path(__file__).resolve().parents[1]


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def exact_copy(binary, expected, target):
    if not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise ValueError("Expected executable SHA-256 must be 64 lowercase hex digits")
    if file_hash(binary) != expected:
        raise ValueError("Candidate executable hash differs from the supplied frozen identity")
    # Pin a private copy so a later parent build cannot change this run's binary.
    with binary.open("rb") as source, target.open("xb") as destination:
        shutil.copyfileobj(source, destination)
    target.chmod(0o700)
    if file_hash(target) != expected or file_hash(binary) != expected:
        raise ValueError("Candidate executable changed during the snapshot copy")


def environment(base):
    env = isolated_env(base, ROOT)
    env.update(HOME=str(base / "home"), TMPDIR=str(base / "tmp"),
        PATH=str(base / "empty-path"), LANG="C.UTF-8", TZ="UTC",
        XDG_CACHE_HOME=str(base / "cache"), AGENTURM_UI_PREFERENCES="0",
        NO_COLOR="1")
    return env


class AcceptanceCockpit(Cockpit):
    def read(self, delay=0.05):
        result = super().read(delay)
        if self.transcript.tell() > 8 * 1024 * 1024:
            raise AssertionError("PTY transcript exceeded the 8 MiB synthetic-fixture budget")
        return result

    def resize(self, width, height):
        if (width, height) == (self.screen.columns, self.screen.rows):
            # No actual terminal resize occurred. Ratatui correctly need not
            # repaint its whole frame; keep the existing cell parser state.
            return
        # Drain the old frame before replacing the cell model. SIGWINCH is sent
        # only to this harness-owned child, never to an external tmux session.
        while self.read(0):
            pass
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        self.screen = TerminalCells(width, height)
        os.kill(self.process.pid, signal.SIGWINCH)

    def finish(self, keys):
        self.send(keys)
        deadline = time.monotonic() + 10
        while self.process.poll() is None and time.monotonic() < deadline:
            self.read()
        assert self.process.poll() == 0, f"cockpit quit status: {self.process.poll()}"
        while self.read(0):
            pass
        assert self.screen.departed is not None, "cockpit did not leave the alternate screen"
        assert termios.tcgetattr(self.slave) == self.original_termios, "terminal mode changed"


def run_journey(binary, db, base, width, height, exit_key, results):
    prefix = f"{width}x{height}-{'q' if exit_key == b'q' else 'interrupt-in-help'}"
    with (base / f"{prefix}.pty").open("xb", buffering=0) as transcript:
        cockpit = AcceptanceCockpit([binary, "cockpit", "--db", db], environment(base),
                                    transcript, width=width, height=height)
        journey = dict(size=[width, height], exit_key=exit_key.hex(), frames=[], status="running")
        results.append(journey)

        def capture(label, predicate, keys=b""):
            if keys:
                cockpit.send(keys)
            text = cockpit.frame(label, predicate)
            assert "Name unavailable" not in text, "forbidden naming fallback became visible"
            name = f"{prefix}-{label}.txt"
            (base / name).write_text(text + "\n")
            journey["frames"].append(dict(label=label, keys_hex=keys.hex(), artifact=name,
                                          sha256=file_hash(base / name), size=[cockpit.screen.columns, cockpit.screen.rows]))
            return text

        def roster(text):
            return "Agenturm command center" in text and "No sessions collected yet" in text

        def palette(text):
            return "Command palette" in text and "Search" in text

        def help_screen(text):
            return "Help" in text and "close help" in text

        try:
            capture("empty-roster", roster)
            capture("palette", palette, b"\x10")
            capture("literal-query-no-match", lambda text: "No matches" in text and "?[]123/" in text,
                    b"uiq-nonexistent-?[]123/")
            capture("palette-clear", lambda text: palette(text) and "No matches" not in text, b"\x01")
            capture("palette-cancel", roster, b"\x1b")
            capture("help", help_screen, b"\x1bOP")
            initial = cockpit.screen.contents()
            capture("help-scroll", lambda text: help_screen(text) and text != initial, b"\x1b[6~")
            capture("help-cancel", roster, b"\x1b")
            # Run an actual enabled destination through palette dispatch.
            capture("palette-again", palette, b"\x10")
            capture("palette-activity", lambda text: palette(text) and "Activity" in text, b"Activity")
            capture("activity", lambda text: "No recorded activity on this page." in text and "Command palette" not in text, b"\r")
            capture("agents-key", roster, b"1")
            # Root key and resize checks deliberately use no selected identity:
            # a seeded session/prompt gate must follow separately.
            for new_width, new_height in SIZES:
                cockpit.resize(new_width, new_height)
                capture(f"resize-{new_width}x{new_height}", roster)
            cockpit.resize(30, 7)
            capture("too-small", lambda text: "small" in text.lower() or "resize" in text.lower())
            cockpit.resize(width, height)
            capture("resize-return", roster)
            if exit_key != b"q":
                # Public Ctrl+C is copy, not interrupt. Help focus owns interrupt.
                capture("interrupt-focus-help", help_screen, b"\x1bOP")
            cockpit.finish(exit_key)
            journey.update(status="passed", exit_status=cockpit.process.returncode,
                           termios_restored=True, alternate_screen_left=True)
        except Exception as error:
            journey.update(status="failed", error=repr(error), last_frame=cockpit.screen.contents())
            raise
        finally:
            cockpit.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agenturm", type=Path, required=True)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--output", type=Path, required=True, help="New private output directory")
    args = parser.parse_args()
    args.output = args.output.absolute()
    source = application_revision(args.source_root)
    if source["head"] != args.source_sha or source["dirty"]:
        parser.error("source-root must be clean and at the supplied exact source SHA")
    create_owned_directory(args.output)
    for name in ("home", "tmp", "empty-path"):
        (args.output / name).mkdir(mode=0o700)
    binary = args.output / "agenturm-under-test"
    exact_copy(args.agenturm.resolve(strict=True), args.sha256, binary)
    results = dict(kind="retained-cockpit-empty-smoke-only", source=source,
        harness_sha256={name: file_hash(ROOT / "scripts" / name) for name in
                       ("validate_rust_ui_parity.py", "validate_rust_default_cockpit.py",
                        "validate_rust_private_eval_details.py", "validation_support.py")},
        executable_sha256=file_hash(binary), executable_bytes=binary.stat().st_size,
        executable_origin=str(args.agenturm.resolve()),
        archive_sha256=file_hash(args.archive) if args.archive else None,
        platform=platform.platform(), machine=platform.machine(), terminal=environment(args.output),
        fixture_version="uiq-empty-history-v1", journeys=[],
        limitations=["Source-to-binary relationship is the supplying builder's attestation, not reproducible-build proof.",
                     "No populated/private/provider/broker/attached/live-default acceptance; no color or clipboard emulator QA."])
    db = args.output / "synthetic.db"
    try:
        # retention disable opens the production collector Store and initializes
        # its schema, without discovery, host collection, adapters or a receiver.
        # Empty collection configs are invalid; never invent one for this gate.
        init = subprocess.run([binary, "retention", "disable", "--db", db],
                              env=environment(args.output), cwd=args.output,
                              capture_output=True, timeout=30)
        results["initialize"] = dict(status=init.returncode, stdout=init.stdout.decode(errors="replace"),
                                     stderr=init.stderr.decode(errors="replace"))
        assert init.returncode == 0, "empty synthetic history initialization failed"
        check = subprocess.run([binary, "query", "--db", db], env=environment(args.output),
                               cwd=args.output, capture_output=True, timeout=10, check=True)
        assert json.loads(check.stdout)["observations"] == [], "synthetic history was not empty"
        results["initial_database_sha256"] = file_hash(db)
        for width, height in SIZES:
            for exit_key in (b"q", b"\x03"):
                run_journey(binary, db, args.output, width, height, exit_key, results["journeys"])
        results["status"] = "passed-smoke-only"
    except Exception as error:
        results.update(status="failed", error=repr(error))
    finally:
        results["executable_unchanged"] = file_hash(binary) == args.sha256
        write_json(args.output / "results.json", results)
    print(json.dumps({key: results[key] for key in ("status", "executable_sha256", "executable_unchanged")}))
    return 0 if results["status"] == "passed-smoke-only" else 1


if __name__ == "__main__":
    sys.exit(main())
