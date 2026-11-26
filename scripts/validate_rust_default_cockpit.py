"""Validate the installed cockpit and inspector in an empty-PATH child PTY."""

import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import termios
import time

from validate_rust_private_eval_details import TerminalCells


WIDTH, HEIGHT = 60, 18
USAGE = {
    "bundle-codex-session": ("harness.codex.", 7, 3),
    "bundle-pi-session": ("harness.pi.", 8, 3),
}


class Cockpit:
    def __init__(self, arguments, environment, transcript, *, width=WIDTH, height=HEIGHT):
        self.master, self.slave = pty.openpty()
        self.screen = TerminalCells(width, height)
        self.transcript = transcript
        self.tail = b""
        try:
            fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
            self.original_termios = termios.tcgetattr(self.slave)
            self.process = subprocess.Popen(
                arguments, env=environment, stdin=self.slave, stdout=self.slave, stderr=self.slave,
                start_new_session=True,
            )
        except BaseException:
            os.close(self.slave)
            os.close(self.master)
            raise

    def read(self, delay=0.05):
        if not select.select([self.master], [], [], delay)[0]:
            return False
        output = os.read(self.master, 65536)
        self.transcript.write(output)
        self.tail = (self.tail + output)[-8192:]
        self.screen.feed(output)
        return True

    def send(self, keys):
        os.write(self.master, keys)

    def check_running(self):
        status = self.process.poll()
        if status is not None:
            raise AssertionError(f"cockpit exited with status {status}")

    def frame(self, description, matches):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            self.check_running()
            # Ratatui can deliver a frame in several writes. Check the current
            # cells after pending writes have been read.
            if not self.read():
                cells = self.screen.contents()
                if matches(cells):
                    return cells
        raise AssertionError(f"cockpit did not show {description}")

    def quit(self):
        self.send(b"q")
        deadline = time.monotonic() + 10
        while self.process.poll() is None and time.monotonic() < deadline:
            self.read()
        assert self.process.poll() == 0, f"cockpit quit status: {self.process.poll()}"
        while self.read(0):
            pass
        assert self.screen.departed is not None, "cockpit did not leave its alternate screen"
        assert termios.tcgetattr(self.slave) == self.original_termios, "terminal mode changed"

    def close(self):
        try:
            if self.process.poll() is None:
                os.killpg(self.process.pid, signal.SIGTERM)
                try:
                    self.process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    os.killpg(self.process.pid, signal.SIGKILL)
                    self.process.wait(timeout=2)
        finally:
            os.close(self.slave)
            os.close(self.master)


def query(binary, environment, history):
    result = subprocess.run(
        [binary, "query", "--db", history], env=environment, capture_output=True, timeout=4,
        check=False,
    )
    if result.returncode:
        return None, result.stderr.decode(errors="replace")[-2048:]
    return json.loads(result.stdout)["observations"], ""


def publish(paths):
    source, directory = paths
    temporary = directory / "new.tmp"
    shutil.copyfile(source, temporary)
    temporary.replace(directory / "new.jsonl")


def observed_usage(binary, environment, history, cockpit):
    deadline = time.monotonic() + 25
    problem = ""
    while time.monotonic() < deadline:
        cockpit.check_running()
        rows, problem = query(binary, environment, history)
        if rows is not None:
            entries = []
            for row in rows:
                observation = row["observation"]
                if observation["kind"] == "usage":
                    entries.append((
                        row["identity"]["session_id"], row["harness_id"],
                        observation["input_tokens"], observation["output_tokens"],
                    ))
            if len(entries) == len(USAGE) and all(
                session in USAGE
                and source.startswith(USAGE[session][0])
                and (inputs, outputs) == USAGE[session][1:]
                for session, source, inputs, outputs in entries
            ):
                return {session: source for session, source, _, _ in entries}
            problem = f"observed usage: {entries!r}"
        cockpit.read(.2)
    raise AssertionError(f"both fixture usage records did not arrive: {problem}")


def selected_task(frame):
    return next(
        line[:39].rstrip() for line in frame.splitlines() if line.startswith("› ")
    )


def roster_search(cells):
    return next((line.strip()[2:] for line in cells.splitlines() if line.startswith(" / ")), "")

def filter_roster(cockpit, session):
    cockpit.send(b"/")
    cockpit.frame("active search", lambda cells: "Backspace edit" in cells)
    cockpit.send(session.encode("ascii") + b"\r")
    return cockpit.frame(
        f"confirmed filter for {session}",
        lambda cells: (
            "Agenturm command center" in cells and roster_search(cells) == session
            # A non-empty search replaces the ordinary inspect hint even
            # after Enter has finished editing the filter. Enter still opens
            # the selected row; check the filtered row and then verify that
            # journey below.
            and ("Enter / Esc search" in cells or "Enter inspect" in cells)
            and "› " in cells
            and re.search(r"(?:Project unavailable|No recorded project)\s+1\b", cells)
        ),
    )


def check_sessions(cockpit, sources):
    cockpit.send(b"R")
    cockpit.frame(
        "both sessions in the command center",
        lambda cells: "Agenturm command center" in cells and re.search(r"\bAll 2\b", cells),
    )
    for index, (session, source) in enumerate(sources.items()):
        if index:
            cockpit.send(b"a")
            cockpit.frame(
                "unfiltered command center on the same page",
                lambda cells: (
                    "Agenturm command center" in cells and not roster_search(cells)
                    and re.search(r"Project unavailable\s+2\b", cells)
                ),
            )
        roster = filter_roster(cockpit, session)
        selected = selected_task(roster)
        cockpit.send(b"\r")
        cockpit.frame(
            f"the exact Overview of {session}",
            lambda cells: (
                re.search(rf"\bSource\s+{re.escape(source)}\b", cells)
                and re.search(rf"\bSession\s+{re.escape(session)}\b", cells)
            ),
        )
        cockpit.send(b"\x1b")
        returned = cockpit.frame(
            f"the same filtered roster after closing {session}",
            lambda cells: (
                "Agenturm command center" in cells and roster_search(cells) == session
                and "Enter inspect" in cells and "› " in cells
            ),
        )
        assert selected_task(returned) == selected, "the selected task changed on return"


def check_inspector(cockpit, source):
    cockpit.frame("the loaded inspector roster", lambda cells: (
        "Agenturm command center" in cells and re.search(r"\bAll 2\b", cells)
    ))
    cockpit.send(b"g")
    cockpit.frame("two sessions in recorded history", lambda cells: (
        re.search(r"\bSessions observed\s+2\b", cells)
    ))
    cockpit.send(b"T")
    cockpit.frame("a turn chooser with both recorded turns", lambda cells: (
        "Turns on this page" in cells and "select · Enter open · Esc back" in cells
        and "› running · turn-2" in cells and "completed · turn-1 · codex_task_complete" in cells
    ))
    cockpit.send(b"\x1b[B")

    def selected(cells):
        return "Turns on this page" in cells and "› completed · turn-1 · codex_task_complete" in cells

    cockpit.frame("selection of the completed turn", selected)
    cockpit.send(b"\r")
    cockpit.frame("the exact completed Codex turn", lambda cells: (
        "Turn activity" in cells
        and re.search(rf"\bSource\s+{re.escape(source)}\b", cells)
        and re.search(r"\bSession\s+bundle-codex-session\b", cells)
        and re.search(r"\bTurn\s+turn-1\b", cells)
        and re.search(r"\bLifecycle\s+completed · codex_task_complete\b", cells)
        and "turn-2" not in cells
    ))
    cockpit.send(b"\x1b")
    cockpit.frame("return to the same selected turn", selected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agenturm", required=True, type=Path)
    parser.add_argument("--home", required=True, type=Path)
    parser.add_argument("--empty-path", required=True, type=Path)
    parser.add_argument("--history", required=True, type=Path)
    parser.add_argument("--codex", nargs=2, type=Path, metavar=("FIXTURE", "DIRECTORY"))
    parser.add_argument("--pi", nargs=2, type=Path, metavar=("FIXTURE", "DIRECTORY"))
    parser.add_argument("--inspect-source")
    parser.add_argument("--transcript", required=True, type=Path)
    args = parser.parse_args()
    if not args.inspect_source and (args.codex is None or args.pi is None):
        parser.error("bare cockpit verification requires both fixture paths")
    env = {"HOME": str(args.home), "PATH": str(args.empty_path), "TERM": "xterm-256color"}
    with args.transcript.open("wb", buffering=0) as transcript:
        cockpit = (
            Cockpit([args.agenturm, "inspect", "--db", args.history], env, transcript, width=120, height=28)
            if args.inspect_source else Cockpit([args.agenturm], env, transcript)
        )
        try:
            if args.inspect_source:
                check_inspector(cockpit, args.inspect_source)
                cockpit.quit()
                return 0
            cockpit.frame(
                "a loaded empty command center",
                lambda cells: (
                    "Agenturm command center" in cells and "All 0" in cells
                    and "No sessions collected yet" in cells
                ),
            )
            empty, problem = query(args.agenturm, env, args.history)
            assert empty == [], f"the initial history was not empty: {problem}"
            publish(args.codex)
            publish(args.pi)
            sources = observed_usage(args.agenturm, env, args.history, cockpit)
            assert sources.keys() == USAGE.keys(), f"incomplete fixture sources: {sources}"
            check_sessions(cockpit, sources)
            cockpit.quit()
        except (AssertionError, OSError, subprocess.TimeoutExpired, ValueError) as error:
            print(f"Cockpit PTY failed: {error}", file=sys.stderr)
            print(cockpit.screen.contents(), file=sys.stderr)
            print(f"PTY tail ({args.transcript}): {cockpit.tail!r}", file=sys.stderr)
            return 1
        finally:
            cockpit.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
