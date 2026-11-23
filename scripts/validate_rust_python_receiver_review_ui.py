"""Exercise protected Review in the Rust cockpit against the actual tagged Python receiver."""

from __future__ import annotations

import argparse
import fcntl
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import termios
import threading
import time

from observatory.core import Store
from observatory.server import BoundedHTTPServer, handler_for
from tests.test_current_native_schema import CurrentOpenCodeBoundaryTests
from validate_rust_private_eval_details import TerminalCells
from validate_rust_python_receiver_review import Policy
from validate_rust_real_session_roster import require

SIZES = ((120, 32), (80, 24), (60, 18))
TITLE = "Python receiver · verified Review"
FIRST_STEP = "First assistant step"
TURN_REQUEST = "Also preserve the pending request boundary"
TURN_UPDATE = "Current turn implementation complete"
FOREIGN = "An unrelated following request"
TRACES = "Agenturm / Traces"


class ReviewPty:
    def __init__(self, app, endpoint, token, home, size):
        width, height = size
        self.width = width
        self.screen = TerminalCells(width, height)
        self.raw = bytearray()
        self.master, self.slave = pty.openpty()
        try:
            fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
            self.original = termios.tcgetattr(self.slave)
            self.process = subprocess.Popen(
                [app, "cockpit", "--endpoint", endpoint, "--token-file", token,
                 "--receiver-protocol", "python-v0.2.8"],
                cwd=home, stdin=self.slave, stdout=self.slave, stderr=self.slave,
                env={"HOME": str(home), "XDG_STATE_HOME": str(home / "state"),
                     "XDG_CONFIG_HOME": str(home / "config"), "PATH": os.defpath,
                     "LANG": "C.UTF-8", "TERM": "xterm-256color", "NO_COLOR": "1"},
            )
        except BaseException:
            os.close(self.master)
            os.close(self.slave)
            raise

    def __enter__(self):
        return self

    def __exit__(self, *exception):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=5)
        os.close(self.master)
        os.close(self.slave)

    def receive(self, timeout=0.05):
        if not select.select([self.master], [], [], timeout)[0]:
            return False
        data = os.read(self.master, 8_192)
        if not data:
            return False
        self.raw.extend(data)
        require(len(self.raw) <= 8 * 1024 * 1024, "Fixture exceeded its PTY transcript budget")
        self.screen.feed(data)
        return True

    def until(self, phase, condition, *, forbidden=(), timeout=12):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.receive()
            frame = self.screen.contents()
            require(not any(text in frame for text in forbidden),
                    f"Protected/foreign body became visible during {phase} at width {self.width}")
            if condition(frame):
                return frame
            require(self.process.poll() is None, f"Cockpit exited during {phase}")
        raise AssertionError(f"Cockpit did not reach {phase} at width {self.width}")

    def keys(self, data):
        os.write(self.master, data)

    def wait_redraw(self, phase, trace=None):
        protected = (TITLE, FIRST_STEP, TURN_REQUEST, TURN_UPDATE, FOREIGN)

        def redacted(frame):
            return (TRACES in frame and (trace is None or selected(frame, trace))
                    and "Verifying selected Review" not in frame
                    and all(marker not in frame for marker in protected))

        # The header and selected row can reach the PTY before Ratatui's
        # lower-body clears. Wait for the complete public frame and ensure
        # the previous private body does not reappear on later chunks.
        self.until(phase, redacted)
        deadline = time.monotonic() + .2
        while time.monotonic() < deadline:
            self.receive()
            require(all(marker not in self.screen.contents() for marker in protected),
                    f"Protected content returned after {phase}")
        self.until(f"{phase} and acknowledgement", redacted, forbidden=protected)

    def pick_trace(self, trace):
        self.until("metadata roster", lambda frame: "Agenturm / Agents" in frame,
                   forbidden=(FIRST_STEP, TURN_REQUEST, TURN_UPDATE, FOREIGN))
        self.keys(b"p")  # Protected policy checks must work even with ordinary refresh paused.
        self.keys(b"4")
        self.until("trace inventory", lambda frame: TRACES in frame and trace in frame,
                   forbidden=(FIRST_STEP, TURN_REQUEST, TURN_UPDATE, FOREIGN))
        for _ in range(6):
            frame = self.screen.contents()
            if selected(frame, trace):
                self.until("exact Review controls", lambda value: "Ctrl+R Review trace" in value
                           and "Ctrl+T Review turn" in value)
                return
            self.keys(b"j")
            self.until("next trace selection", lambda value: choice(value) != choice(frame))
        raise AssertionError("Could not select the exact expected trace")

    def quit(self, key):
        self.keys(key)
        deadline = time.monotonic() + 8
        while self.process.poll() is None and time.monotonic() < deadline:
            self.receive()
        require(self.process.poll() == 0, "Cockpit quit failed")
        while self.receive(0):
            pass
        require(termios.tcgetattr(self.slave) == self.original, "Cockpit did not restore terminal modes")
        departed = self.screen.departed
        require(departed is not None and all(value not in departed for value in
                (TITLE, FIRST_STEP, TURN_REQUEST, TURN_UPDATE)),
                "Cockpit exited the alternate screen while protected evidence remained visible")


def choice(frame):
    return next((row for line in frame.splitlines()
                 if (row := line.lstrip(" │┃║")).startswith("›")), "")


def selected(frame, trace):
    return trace in choice(frame)


def run_size(app, size):
    fixture = CurrentOpenCodeBoundaryTests()
    fixture.setUp()
    try:
        fixture.poll()
        trace = fixture.trace()
        turn = fixture.store.review_turn(trace)
        turn_id, owner = turn["turn_id"], turn["scope_owner"]
        # Verify the tagged receiver's real list supplies the identity the GUI
        # must bind. No fabricated live response is used for either selector.
        trace_rows = fixture.store.trace_page(limit=12)["traces"]
        exact = [row for row in trace_rows if row["trace_id"] == trace]
        require(len(exact) == 1 and exact[0]["turn_id"] == turn_id and exact[0]["review_owner"] == owner,
                "Tagged fixture did not supply an authoritative selectable trace and turn")
        require(any(row["turn_id"] != turn_id for row in trace_rows), "Missing out-of-scope adjacent turn")
        policy = Policy(owner)
        fixture.store.local_agent_monitor = policy
        token = "synthetic-python-review-ui-token-for-tagged-fixture"
        token_file = fixture.root / "token"
        token_file.write_text(token + "\n", encoding="utf-8")
        token_file.chmod(0o600)
        with BoundedHTTPServer(("127.0.0.1", 0), handler_for(fixture.store, token, quiet=True)) as server:
            worker = threading.Thread(target=server.serve_forever, daemon=True)
            worker.start()
            try:
                endpoint = f"http://127.0.0.1:{server.server_port}"
                home = fixture.root / "app-home"
                home.mkdir()
                with ReviewPty(app, endpoint, token_file, home, size) as tty:
                    tty.pick_trace(trace)
                    require(all(value.encode() not in tty.raw for value in
                                (FIRST_STEP, TURN_REQUEST, TURN_UPDATE, FOREIGN)),
                            "Protected evidence reached the initial metadata-only PTY")
                    tty.keys(b"\x12")
                    tty.until("exact step Review", lambda frame: TITLE in frame and trace in frame
                              and FIRST_STEP in frame, forbidden=(TURN_REQUEST, TURN_UPDATE, FOREIGN))
                    tty.keys(b"\x1b")
                    tty.wait_redraw("exact trace restored after Escape", trace)
                    tty.keys(b"\x14")
                    tty.until("exact turn Review", lambda frame: TITLE in frame and turn_id in frame
                              and TURN_REQUEST in frame and TURN_UPDATE in frame, forbidden=(FOREIGN,))
                    files = {path.relative_to(home) for path in home.rglob("*") if path.is_file()}
                    prior = len(tty.raw)
                    tty.keys(b"cx")
                    next_check = time.monotonic() + .25
                    while time.monotonic() < next_check:
                        tty.receive()
                    require(TITLE in tty.screen.contents() and TURN_REQUEST in tty.screen.contents(),
                            "Copy/export shortcut escaped the protected Review")
                    require(b"\x1b]52;" not in tty.raw[prior:]
                            and files == {path.relative_to(home) for path in home.rglob("*") if path.is_file()},
                            "Protected Review exposed a clipboard or export side effect")
                    tty.quit(b"\x03" if size[0] == 60 else b"q")

                with ReviewPty(app, endpoint, token_file, home, size) as tty:
                    tty.pick_trace(trace)
                    tty.keys(b"\x14")
                    tty.until("Review before live capture revocation", lambda frame:
                              TITLE in frame and TURN_REQUEST in frame, forbidden=(FOREIGN,))
                    policy.capture = False
                    policy.generation += 1
                    tty.wait_redraw("capture revocation redraw", trace)
                    cursor = len(tty.raw)
                    tty.keys(b"\x14")
                    tty.until("revoked Review denied", lambda frame: any(notice in frame for notice in
                              ("Private Review is unavailable", "Capture policy denies private Review")),
                              forbidden=(TURN_REQUEST, TURN_UPDATE, FOREIGN))
                    require(TURN_REQUEST.encode() not in tty.raw[cursor:], "Denied Review published private output")

                    policy.capture = True
                    policy.generation += 1
                    if TITLE in tty.screen.contents():
                        tty.keys(b"\x1b")
                        tty.wait_redraw("return from metadata-only Review", trace)
                    tty.keys(b"\x14")
                    tty.until("Review before receiver replacement", lambda frame:
                              TITLE in frame and TURN_REQUEST in frame, forbidden=(FOREIGN,))
                    replacement = fixture.root / "replacement.sqlite"
                    other = Store(replacement)
                    replacement_id = other.receiver_id
                    require(replacement_id != fixture.store.receiver_id,
                            "Replacement must have a distinct receiver identity")
                    other.close()
                    os.replace(replacement, fixture.store.path)
                    require(fixture.store.receiver_namespace() == replacement_id,
                            "Tagged receiver did not bind its replacement history")
                    tty.wait_redraw("receiver replacement redraw")
                    tty.quit(b"q")
            finally:
                server.shutdown()
                worker.join(timeout=5)
    finally:
        fixture.doCleanups()
    print(f"PASS tagged Python exact step/turn Review, private redraw, policy and identity fences at {size[0]}×{size[1]}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path, help="Installed Rust bin/agenturm")
    arguments = parser.parse_args()
    app = arguments.binary.resolve(strict=True)
    for size in SIZES:
        run_size(app, size)


if __name__ == "__main__":
    main()
