"""Exercise exact Findings disclosure against tagged Python receiver storage and HTTP."""

from __future__ import annotations

import argparse
from contextlib import ExitStack, contextmanager
import fcntl
import json
import os
from pathlib import Path
import pty
import secrets
import select
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
import urllib.request

from validate_rust_private_eval_details import TerminalCells

SIZES = ((120, 32), (80, 24), (60, 18))
LIST_TITLE = "Agenturm / Findings"
DETAIL_TITLE = "Selected finding"
PRIVATE_TITLE = "verified finding source"
LOG = "PRIVATE_LOG_EVIDENCE_36492"
EVAL = "PRIVATE_EVAL_EVIDENCE_81635"
REPLACED = "WRONG_MEMBER_EVIDENCE_49023"
EXCLUDED = (LOG, EVAL, REPLACED)
PRODUCER = "opencode"
SESSION = "findings-session"
TRACE = "a" * 32


class CapturePolicy:
    """Only the monitor is synthetic; tagged Store and authenticated HTTP are real."""

    def __init__(self, actor):
        self.actor = actor
        self.capture = True
        self.generation = 1

    def change(self, capture):
        self.capture = capture
        self.generation += 1

    def status(self):
        return {
            "enabled": True,
            "capture_content": self.capture,
            "capture_policy_schema": 1,
            "capture_global_revision": self.generation,
            "sessions": [{
                "service": PRODUCER,
                "agent_id": self.actor,
                "session_id": SESSION,
                "capture_content": self.capture,
                "capture_revision": self.generation,
            }],
        }


class Terminal:
    def __init__(self, app, endpoint, token_file, home, size):
        self.width, self.height = size
        self.screen = TerminalCells(*size)
        self.raw = bytearray()
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", self.height, self.width, 0, 0))
        self.original = termios.tcgetattr(self.slave)
        self.process = subprocess.Popen(
            [app, "cockpit", "--endpoint", endpoint, "--token-file", token_file,
             "--receiver-protocol", "python-v0.2.8"],
            cwd=home, stdin=self.slave, stdout=self.slave, stderr=self.slave,
            env={
                "HOME": str(home), "XDG_STATE_HOME": str(home / "state"),
                "XDG_CONFIG_HOME": str(home / "config"), "PATH": os.defpath,
                "TERM": "xterm-256color", "LANG": "C.UTF-8", "NO_COLOR": "1",
            },
        )

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=5)
        os.close(self.master)
        os.close(self.slave)

    def receive(self, timeout=.05):
        if not select.select([self.master], [], [], timeout)[0]:
            return False
        data = os.read(self.master, 16384)
        if not data:
            return False
        self.raw.extend(data)
        assert len(self.raw) <= 8 * 1024 * 1024
        self.screen.feed(data)
        return True

    def until(self, phase, predicate, *, forbidden=EXCLUDED, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.receive()
            frame = self.screen.contents() or ""
            assert not any(value in frame for value in forbidden), (phase, self.width, frame)
            if predicate(frame):
                return frame
            assert self.process.poll() is None, (phase, self.width, frame)
        raise AssertionError((phase, self.width, self.screen.contents()))

    def keys(self, value):
        os.write(self.master, value)

    def select_finding(self, selected, label):
        self.until("initial roster",
                   lambda frame: "Agenturm / Agents" in frame and "Connected" in frame)
        self.keys(b"p")
        self.until("pause routine metadata refresh", lambda frame: "Paused" in frame)
        self.keys(b"7")
        self.until("findings list", lambda frame: LIST_TITLE in frame and label in frame)
        for _ in range(4):
            frame = self.screen.contents()
            row = next((line.lstrip(" │┃║") for line in frame.splitlines()
                        if line.lstrip(" │┃║").startswith("›")), "")
            if label in row:
                break
            self.keys(b"\x1b[B")
            self.until("next row", lambda value:
                       any(label in line and line.lstrip(" │┃║").startswith("›")
                           for line in value.splitlines()))
        else:
            raise AssertionError(("exact selected row", label, self.screen.contents()))
        self.keys(b"\r")
        self.until("retained finding inspector",
                   lambda value: DETAIL_TITLE in value and selected in value and "s source" in value)
        assert not any(value.encode() in self.raw for value in EXCLUDED)

    def disclosure(self, marker):
        self.keys(b"s")
        self.until("protected exact source",
                   lambda value: PRIVATE_TITLE in value and marker in value,
                   forbidden=tuple(value for value in EXCLUDED if value != marker))

    def redacted(self, selected=None):
        def frame_matches(frame):
            return (LIST_TITLE in frame and PRIVATE_TITLE not in frame
                    and not any(value in frame for value in EXCLUDED)
                    and (selected is None or DETAIL_TITLE in frame and selected in frame))

        self.until("complete redaction", frame_matches, forbidden=())
        deadline = time.monotonic() + .2
        while time.monotonic() < deadline:
            self.receive()
            assert frame_matches(self.screen.contents())

    def denied(self):
        self.until("closed source failure", lambda frame:
                   PRIVATE_TITLE not in frame and any(value in frame.casefold()
                   for value in ("withheld", "unavailable", "denied", "changed", "not retained",
                                 "finding is no longer retained", "capture policy denies private finding source",
                                 "sensitive", "unsupported", "could not", "interrupted")))
        assert not any(value.encode() in self.raw for value in EXCLUDED)

    def fill_output(self):
        # Open a different file description, leaving the child's output flags alone.
        output = os.open(os.ttyname(self.slave), os.O_WRONLY | os.O_NOCTTY | os.O_NONBLOCK)
        total = 0
        try:
            while total < 512 * 1024:
                try:
                    total += os.write(output, b"\x1b[0m" * 1024)
                except BlockingIOError:
                    return total
        finally:
            os.close(output)
        raise AssertionError("PTY output did not fill at its bounded capacity")

    def quit(self, key=b"q"):
        self.keys(key)
        deadline = time.monotonic() + 7
        while self.process.poll() is None and time.monotonic() < deadline:
            self.receive()
        assert self.process.poll() == 0
        while self.receive(0):
            pass
        assert termios.tcgetattr(self.slave) == self.original
        assert self.screen.departed is not None
        assert not any(value in self.screen.departed for value in (*EXCLUDED, PRIVATE_TITLE))


@contextmanager
def terminal(*args):
    tty = Terminal(*args)
    try:
        yield tty
    finally:
        tty.close()


@contextmanager
def receiver(python, root, kind):
    from observatory.agent_hierarchy import canonical_agent_id
    from observatory.core import Store, normalize_native
    from observatory.server import BoundedHTTPServer, handler_for

    assert Path(sys.modules[Store.__module__].__file__).resolve().is_relative_to(python)
    root.mkdir()
    store = Store(root / "python.sqlite")
    actor = canonical_agent_id(PRODUCER, "qualification", "selected-agent")
    scoped = kind != "source-log"
    scope = {"agent.id": actor, "session.id": SESSION} if scoped else {}
    now = time.time_ns()
    retained_log = {
        "trace_id": TRACE, "span_id": None, "time_unix_nano": now, "service": PRODUCER,
        "scope_name": "sdk.telemetry", "event_name": "sdk.error", "body": LOG,
        "severity_number": 17, "severity_text": "ERROR", "attributes": scope,
        "resource_attributes": {},
    }
    if kind != "eval":
        store.ingest_logs([retained_log])
    else:
        store.ingest([normalize_native({
            "trace_id": TRACE, "span_id": "b" * 16, "name": "agent.run", "status": "OK",
            "start_unix_nano": now - 10, "end_unix_nano": now, "attributes": scope,
            "events": [],
        }, PRODUCER)])
        store.ingest_outcomes([{"trace_id": TRACE, "passed": False, "error": EVAL}])
    policy = CapturePolicy(actor)
    store.local_agent_monitor = policy
    expected = "failed_outcome" if kind == "eval" else "error_log"
    tagged_page = store.finding_page()
    [selected] = [row for row in tagged_page["findings"] if row["kind"] == expected]
    assert selected["service"] == PRODUCER, selected
    assert selected["session_id"] == (SESSION if scoped else None), selected
    assert selected["agent_id"] == (actor if scoped else None), selected
    finding = selected["id"]
    body = EVAL if kind == "eval" else LOG
    assert body in json.dumps(store.finding_source(finding))
    token = secrets.token_urlsafe(32)
    token_file = root / "receiver.token"
    token_file.write_text(token + "\n")
    token_file.chmod(0o600)
    hooks = dict(mode=None, source_calls=0, mutated=threading.Event(),
                 hold=threading.Event(), release=threading.Event(),
                 release_final=threading.Event())
    finding_source = store.finding_source

    def source(identity):
        if identity == finding:
            hooks["source_calls"] += 1
            count = hooks["source_calls"]
            mode = hooks["mode"]
            if count == 1 and mode in ("remove-cache", "owner-changed", "member-changed"):
                # Make a real committed change under exactly the same URL.
                if mode == "member-changed":
                    store.ingest_logs([{
                        "trace_id": TRACE, "span_id": None, "time_unix_nano": now - 1,
                        "service": PRODUCER, "scope_name": "sdk.telemetry",
                        "event_name": "sdk.error", "body": REPLACED, "severity_number": 17,
                        "severity_text": "ERROR", "attributes": scope,
                        "resource_attributes": {},
                    }])
                    [alternate] = [row for row in store.finding_page()["findings"]
                                   if REPLACED in row["detail"]]
                    assert alternate["session_id"] == SESSION and alternate["agent_id"] == actor
                    assert alternate["id"] != finding
                with store._connect() as db:
                    if mode == "remove-cache":
                        db.execute("DELETE FROM finding_cache WHERE finding_id=?", (identity,))
                    elif mode == "owner-changed":
                        db.execute(
                            "UPDATE finding_cache SET session_id=? WHERE finding_id=?",
                            ("another-owner-session", identity),
                        )
                    else:
                        db.execute(
                            "UPDATE finding_cache SET finding_json=json_set(finding_json, "
                            "'$.evidence',json(?)) WHERE finding_id=?",
                            (json.dumps(alternate["evidence"]), identity),
                        )
                hooks["mutated"].set()
                if mode == "member-changed":
                    response = finding_source(identity)
                    assert REPLACED in json.dumps(response)
                    assert response["finding_scope"]["session_id"] == SESSION
                    return response
            if mode in ("unknown-field", "sensitive-field"):
                response = finding_source(identity)
                record = response["sources"][0]["record"]
                if mode == "unknown-field":
                    record["raw_private_note"] = REPLACED
                else:
                    record["body"] = f"Authorization: Bearer {REPLACED}"
                hooks["mutated"].set()
                return response
            if count == 1 and mode == "payload-drift":
                # A tagged outcome correction updates its current exact record
                # without changing the finding ID or member reference.
                result = finding_source(identity)
                store.ingest_outcomes([{"trace_id": TRACE, "passed": False, "error": REPLACED}])
                hooks["mutated"].set()
                return result
            if count == 1 and mode == "backpressure":
                hooks["hold"].set()
                assert hooks["release"].wait(timeout=7), "PTY hold was not released"
            if count == 2 and mode == "backpressure":
                result = finding_source(identity)
                hooks["mutated"].set()
                assert hooks["release_final"].wait(timeout=7), "final source hold was not released"
                return result
        return finding_source(identity)

    store.finding_source = source
    handler = handler_for(store, token, quiet=True)
    try:
        with ExitStack() as replacements, BoundedHTTPServer(("127.0.0.1", 0), handler) as server:
            worker = threading.Thread(target=server.serve_forever, daemon=True)
            worker.start()
            endpoint = f"http://127.0.0.1:{server.server_port}"

            def replace_history():
                # Reinitialize the tagged receiver's Store and request handler
                # at the same authenticated endpoint. Keep the exact finding
                # and enabled policy so only the receiver identity changes.
                replacement = Store(root / "replacement.sqlite")
                replacements.callback(replacement.close)
                replacement.ingest_logs([retained_log])
                replacement.local_agent_monitor = policy
                assert LOG in json.dumps(replacement.finding_source(finding))
                previous_id = store.receiver_namespace()
                replacement_id = replacement.receiver_namespace()
                assert replacement_id != previous_id
                next_handler = handler_for(replacement, token, quiet=True)
                replacements.callback(next_handler.response_cache.close)
                server.RequestHandlerClass = next_handler
                request = urllib.request.Request(
                    endpoint + "/api/health", headers={"Authorization": f"Bearer {token}"}
                )
                with urllib.request.urlopen(request, timeout=5) as response:
                    assert response.headers["X-Agenturm-Receiver-ID"] == replacement_id
                    health = json.load(response)
                assert health["receiver_id"] == replacement_id and health["indexing"]["ready"]
                assert health["local_agents"]["capture_content"] is True

            hooks["replace_history"] = replace_history
            try:
                request = urllib.request.Request(
                    endpoint + "/api/findings?limit=20", headers={"Authorization": f"Bearer {token}"}
                )
                with urllib.request.urlopen(request, timeout=5) as response:
                    assert response.headers["X-Agenturm-Receiver-ID"] == store.receiver_namespace()
                    listed = json.load(response)
                assert any(row["id"] == finding for row in listed["findings"])
                # The real tagged wire can include source-bearing detail. Rust
                # must drop it from the normal Findings projection.
                assert body in json.dumps(listed["findings"])
                yield store, policy, hooks, endpoint, token_file, finding
            finally:
                hooks["release"].set()
                hooks["release_final"].set()
                server.shutdown()
                worker.join(timeout=5)
    finally:
        handler.response_cache.close()
        store.close()


def acceptance(app, python, temp):
    for kind, label, text in (("log", "Error log", LOG), ("eval", "Failed outcome", EVAL)):
        home = temp / kind
        with receiver(python, home, kind) as (_, _, hooks, endpoint, token, finding):
            for size in SIZES:
                with terminal(app, endpoint, token, home, size) as tty:
                    tty.select_finding(finding, label)
                    before = hooks["source_calls"]
                    tty.disclosure(text)
                    assert hooks["source_calls"] >= before + 2
                    existing = {path.relative_to(home) for path in home.rglob("*") if path.is_file()}
                    copied = len(tty.raw)
                    tty.keys(b"cX")
                    tty.until("protected shortcuts remain in the viewer",
                              lambda frame: PRIVATE_TITLE in frame and text in frame,
                              forbidden=tuple(value for value in EXCLUDED if value != text))
                    deadline = time.monotonic() + .2
                    while time.monotonic() < deadline:
                        tty.receive()
                        assert PRIVATE_TITLE in tty.screen.contents()
                    assert b"\x1b]52;" not in tty.raw[copied:]
                    assert existing == {path.relative_to(home) for path in home.rglob("*") if path.is_file()}
                    tty.keys(b"\x1b")
                    tty.redacted(finding)
                    tty.quit(b"\x03" if size[0] == 60 else b"q")
            print(f"PASS actual tagged {kind} source and exact return at 120/80/60", flush=True)

    home = temp / "source-log"
    with receiver(python, home, "source-log") as (_, policy, hooks, endpoint, token, finding):
        with terminal(app, endpoint, token, home, SIZES[0]) as tty:
            tty.select_finding(finding, "Error log")
            tty.disclosure(LOG)
            policy.change(False)
            tty.redacted(finding)
            assert hooks["source_calls"] >= 2
            tty.quit()
    print("PASS actual tagged source-only occurrence with global capture revocation", flush=True)

    for mode in ("remove-cache", "owner-changed", "member-changed", "payload-drift",
                 "unknown-field", "sensitive-field"):
        kind = "eval" if mode == "payload-drift" else "log"
        home = temp / mode
        with receiver(python, home, kind) as (_, _, hooks, endpoint, token, finding):
            with terminal(app, endpoint, token, home, SIZES[0]) as tty:
                tty.select_finding(finding, "Failed outcome" if kind == "eval" else "Error log")
                hooks["mode"] = mode
                tty.keys(b"s")
                tty.denied()
                assert hooks["mutated"].is_set() and hooks["source_calls"] >= 1
                if mode == "payload-drift":
                    assert hooks["source_calls"] >= 2
                tty.quit()
        print(f"PASS tagged same-URL {mode} withheld", flush=True)

    for mode in ("policy-before", "policy-visible", "restore-visible", "backpressure"):
        home = temp / mode
        with receiver(python, home, "log") as (_, policy, hooks, endpoint, token, finding):
            with terminal(app, endpoint, token, home, SIZES[0]) as tty:
                tty.select_finding(finding, "Error log")
                if mode == "policy-before":
                    policy.change(False)
                    tty.keys(b"s")
                    tty.denied()
                    assert hooks["source_calls"] == 0
                elif mode == "backpressure":
                    hooks["mode"] = mode
                    tty.keys(b"s")
                    assert hooks["hold"].wait(timeout=5), "private source read never started"
                    assert tty.fill_output() > 0
                    hooks["release"].set()
                    assert hooks["mutated"].wait(timeout=5), "no second proof before publication"
                    policy.change(False)
                    hooks["release_final"].set()
                    tty.denied()
                else:
                    tty.disclosure(LOG)
                    if mode == "policy-visible":
                        policy.change(False)
                        tty.redacted(finding)
                    else:
                        hooks["replace_history"]()
                        tty.redacted()
                tty.quit()
        print(f"PASS tagged {mode} redaction and terminal restore", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", required=True, type=Path)
    parser.add_argument("--python-root", required=True, type=Path)
    options = parser.parse_args()
    python = options.python_root.resolve(strict=True)
    app = (options.bin_dir / "agenturm").resolve(strict=True)
    sys.path.insert(0, str(python))
    with tempfile.TemporaryDirectory(prefix="agenturm-tagged-findings-") as directory:
        acceptance(app, python, Path(directory))


if __name__ == "__main__":
    main()
