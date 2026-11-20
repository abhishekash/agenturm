"""Explicit, scoped process capability proof for the generic pane registry.

Only host-computed PromptSignalFacts cross the child pipe. UI queries read a
bounded cache and schedule background work; they never wait on SQLite or the
child process. This is a fixture contract, not a production plugin SDK.
"""
from __future__ import annotations

import hashlib
import json
import os
import queue
import signal
import stat
import subprocess
import tempfile
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, TYPE_CHECKING

from .agent_hierarchy import AGENT_ID
from .capability_facts import PromptSignalFacts, prompt_signal_facts
from .capability_model import ModelRowSpec, model_from_facts, parse_model_rows
from .plugin_frames import FrameError, read_frame, write_frame
from .plugin_manifest import PluginManifest, plan_activation
from .plugin_wire import ProtocolError, decode_message, encode_message
from .ui_contributions import (PaneContribution, PaneModel, PaneRegistry,
                               PaneRow, PaneScope, validate_model)

if TYPE_CHECKING:
    from .core import Store


MAX_CONTROL = 64 * 1024
MAX_ARTIFACT = 64 * 1024 * 1024
REFRESH_SECONDS = 2.0
FEATURE = "capability-adapter/1"
GRANT = "scoped-aggregate-read"
_COLUMNS = (("Signal", 40, 2), ("Observed", 14, 0), ("Coverage", 23, 1))
_V2_HEADINGS = ("Signal", "Observed", "Coverage")
_ROW_LABELS = {"users": "Recorded user messages", "shell": "Recorded shell actions",
               "first-shell": "First shell action after a user message"}


@dataclass(frozen=True)
class CapabilityConfig:
    manifest: PluginManifest
    pane_id: str
    title: str
    columns: tuple[tuple[str, int, int], ...]
    grants: tuple[str, ...]
    model_rows: tuple[ModelRowSpec, ...] | None = None


def read_capability_config(path: str | Path) -> CapabilityConfig:
    """Read only a named descriptor. This does not import or launch its code."""
    path = Path(path)
    try:
        if path.is_symlink() or not stat.S_ISREG(path.stat().st_mode):
            raise ValueError("capability config must be a regular file")
        with path.open("rb") as stream:
            raw = stream.read(4097)
    except OSError as exc:
        raise ValueError("cannot read capability config") from exc
    if len(raw) > 4096:
        raise ValueError("capability config exceeds byte budget")
    try:
        descriptor = decode_message(raw.decode("utf-8", "strict"))
    except UnicodeError as exc:
        raise ValueError("invalid capability config encoding") from exc
    schema = descriptor.get("schema")
    expected_keys = ({"schema", "manifest", "pane", "grants"} if schema == 1 else
                     {"schema", "manifest", "pane", "grants", "model"} if schema == 2 else set())
    if (type(schema) is not int or schema not in (1, 2)
            or set(descriptor) != expected_keys
            or not isinstance(descriptor["manifest"], str)
            or not Path(descriptor["manifest"]).is_absolute()
            or descriptor["grants"] != [GRANT]
            or not isinstance(descriptor["pane"], dict)
            or set(descriptor["pane"]) != {"id", "title", "columns"}):
        raise ValueError("invalid capability config")
    pane = descriptor["pane"]
    if schema == 1:
        if (pane["id"] != "prompt-signals" or pane["title"] != "Prompt signals"
                or pane["columns"] != [list(column) for column in _COLUMNS]):
            raise ValueError("unsupported legacy capability pane")
        columns = _COLUMNS
    else:
        raw_columns = pane["columns"]
        if (not isinstance(raw_columns, list) or len(raw_columns) not in (2, 3)
                or any(not isinstance(column, list) or len(column) != 3
                       for column in raw_columns)
                or [column[0] for column in raw_columns] != list(_V2_HEADINGS[:len(raw_columns)])
                or any(type(column[1]) is not int or type(column[2]) is not int
                       for column in raw_columns)):
            raise ValueError("unsupported capability pane columns")
        columns = tuple(tuple(column) for column in raw_columns)
        if sum(column[1] for column in columns) > 54:
            raise ValueError("capability pane exceeds compact width")
    model_rows = parse_model_rows(descriptor["model"]) if schema == 2 else None
    manifest = PluginManifest.read(descriptor["manifest"])
    plan_activation({manifest.id: manifest}, (manifest.id,),
                    host_features=("pane-model/1",), allowed_grants=(GRANT,))
    if FEATURE not in manifest.provides or GRANT not in manifest.grants:
        raise ValueError("capability manifest lacks required feature or grant")
    declaration = PaneContribution(pane["id"], pane["title"], columns,
                                   lambda _scope: PaneModel("unavailable", "Not started"))
    PaneRegistry((declaration,))
    return CapabilityConfig(manifest, declaration.id, declaration.title, columns,
                            (GRANT,), model_rows)


class CapabilityProcess:
    """One pinned long-lived child with a bounded single-request data pipe."""

    def __init__(self, config: CapabilityConfig, *, timeout: float = 0.5):
        if not isinstance(config, CapabilityConfig):
            raise TypeError("capability config required")
        if type(timeout) not in (int, float) or not 0.05 <= timeout <= 5:
            raise ValueError("invalid capability timeout")
        self.config = config
        self.timeout = float(timeout)
        self.process: subprocess.Popen | None = None
        self.directory: tempfile.TemporaryDirectory | None = None
        self.frames: queue.Queue = queue.Queue(maxsize=2)
        self.stopping = threading.Event()
        self._close_lock = threading.Lock()
        self.sequence = 0
        self._group_id: int | None = None

    def _artifact(self) -> Path:
        manifest = self.config.manifest
        if len(manifest.entrypoint) != 1:
            raise ProtocolError("capability requires one executable artifact")
        path = Path(manifest.entrypoint[0])
        if not path.is_absolute() or path.is_symlink():
            raise ProtocolError("capability artifact must be an absolute regular file")
        try:
            info = path.stat()
            if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_ARTIFACT:
                raise ProtocolError("invalid capability artifact")
            with path.open("rb") as stream:
                raw = stream.read(MAX_ARTIFACT + 1)
        except OSError as exc:
            raise ProtocolError("cannot read capability artifact") from exc
        if len(raw) > MAX_ARTIFACT or hashlib.sha256(raw).hexdigest() != manifest.artifact_sha256:
            raise ProtocolError("capability artifact hash mismatch")
        self.directory = tempfile.TemporaryDirectory(prefix="agenturm-capability-")
        copied = Path(self.directory.name) / "plugin"
        copied.write_bytes(raw)
        copied.chmod(0o700)
        return copied

    def _reader(self, stream):
        try:
            while not self.stopping.is_set():
                raw = read_frame(stream, max_bytes=MAX_CONTROL)
                if raw is None:
                    raise ProtocolError("capability child closed data pipe")
                while not self.stopping.is_set():
                    try:
                        self.frames.put(raw, timeout=0.1)
                        break
                    except queue.Full:
                        continue
        except (OSError, ValueError, FrameError, ProtocolError) as exc:
            if not self.stopping.is_set():
                try:
                    self.frames.put_nowait(exc)
                except queue.Full:
                    pass

    def _send(self, value: dict, *, timeout: float):
        raw = encode_message(value)
        if len(raw.encode()) > MAX_CONTROL:
            raise ProtocolError("capability control frame exceeds budget")
        errors = []
        process = self.process
        if process is None or process.stdin is None:
            raise ProtocolError("capability control pipe is closed")
        stream = process.stdin

        def write():
            try:
                write_frame(stream, raw, max_bytes=MAX_CONTROL)
            except (OSError, ValueError, FrameError) as exc:
                errors.append(exc)

        thread = threading.Thread(target=write, daemon=True)
        thread.start()
        thread.join(timeout)
        if thread.is_alive() or errors:
            self.close()
            raise ProtocolError("capability control pipe failed or timed out")

    def _next(self, *, timeout: float) -> dict:
        try:
            item = self.frames.get(timeout=timeout)
        except queue.Empty as exc:
            self.close()
            raise ProtocolError("capability reply timed out") from exc
        if isinstance(item, BaseException):
            self.close()
            raise ProtocolError("capability data pipe failed") from item
        try:
            return decode_message(item)
        except ProtocolError:
            self.close()
            raise

    def start(self):
        if self.process is not None or self.stopping.is_set():
            raise ProtocolError("capability child already started")
        try:
            with self._close_lock:
                if self.stopping.is_set():
                    raise ProtocolError("capability start cancelled")
                copied = self._artifact()
                process = subprocess.Popen(
                    [str(copied)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                    stderr=subprocess.DEVNULL, cwd=self.directory.name,
                    env={"PATH": os.defpath, "LANG": "C.UTF-8", "AGENTURM_CAPABILITY_PROTOCOL": "1"},
                    bufsize=0, close_fds=True, start_new_session=True)
                self._group_id = process.pid
                self.process = process
            threading.Thread(target=self._reader, args=(process.stdout,), daemon=True).start()
            deadline = max(1.0, self.timeout)
            self._send({"type": "hello", "protocol": 1,
                        "plugin_id": self.config.manifest.id,
                        "pane_id": self.config.pane_id}, timeout=deadline)
            reply = self._next(timeout=deadline)
            if reply != {"type": "hello_ack", "protocol": 1,
                         "plugin_id": self.config.manifest.id,
                         "features": [FEATURE]}:
                raise ProtocolError("capability handshake mismatch")
            return self
        except BaseException:
            self.close()
            raise

    def query(self, facts: PromptSignalFacts) -> PaneModel:
        if self.process is None or self.process.returncode is not None:
            raise ProtocolError("capability child is not running")
        if facts.state not in ("ready", "degraded"):
            raise ValueError("unavailable facts cannot reach child")
        self.sequence += 1
        seq = self.sequence
        self._send({"type": "facts", "protocol": 1, "seq": seq,
                    "facts": {"state": facts.state, "observed_events": facts.observed_events,
                              "user_messages": facts.user_messages,
                              "shell_actions": facts.shell_actions,
                              "first_shell_after_user_ms": facts.first_shell_after_user_ms,
                              "window_complete": facts.window_complete}}, timeout=self.timeout)
        reply = self._next(timeout=self.timeout)
        return self._validated_model(reply, seq, facts)

    def _validated_model(self, reply: dict, seq: int, facts: PromptSignalFacts) -> PaneModel:
        if (set(reply) != {"type", "protocol", "seq", "state", "summary", "rows"}
                or reply["type"] != "model" or type(reply["protocol"]) is not int
                or reply["protocol"] != 1 or type(reply["seq"]) is not int
                or reply["seq"] != seq or not isinstance(reply["rows"], list)):
            raise ProtocolError("invalid capability model envelope")
        rows = []
        for raw in reply["rows"]:
            if (not isinstance(raw, dict) or set(raw) != {"key", "cells", "detail"}
                    or not isinstance(raw["cells"], list)):
                raise ProtocolError("invalid capability model row")
            rows.append(PaneRow(raw["key"], tuple(raw["cells"]), raw["detail"]))
        model = PaneModel(reply["state"], reply["summary"], tuple(rows))
        pane = PaneContribution(self.config.pane_id, self.config.title,
                                self.config.columns, lambda _scope: model)
        try:
            validate_model(pane, model)
        except ValueError as exc:
            raise ProtocolError("invalid capability PaneModel schema") from exc
        # Every displayed value is recomputed from receiver-owned facts. A v2
        # descriptor chooses bounded formulas; it cannot authorize child text.
        if self.config.model_rows is None:
            coverage = "complete recent window" if facts.window_complete else "partial recent window"
            expected = PaneModel(facts.state,
                f"{facts.observed_events} scoped events · {coverage} · observational signals only",
                (PaneRow("users", (_ROW_LABELS["users"], str(facts.user_messages), coverage)),
                 PaneRow("shell", (_ROW_LABELS["shell"], str(facts.shell_actions), coverage)),
                 PaneRow("first-shell", (_ROW_LABELS["first-shell"],
                          f"{facts.first_shell_after_user_ms} ms"
                          if facts.first_shell_after_user_ms is not None else "unavailable", coverage))))
        else:
            expected = model_from_facts(facts, self.config.model_rows,
                                        include_coverage=len(self.config.columns) == 3)
        if model != expected:
            raise ProtocolError("capability model differs from scoped facts")
        return model

    def close(self):
        with self._close_lock:
            self.stopping.set()
            process = self.process
            if process is not None:
                # A caller that already reaped the leader has released its
                # PID reservation. Do not signal a possibly reused PGID.
                group_id = self._group_id if process.returncode is None else None
                can_waitid = hasattr(os, "waitid") and hasattr(os, "WNOWAIT")
                if group_id is not None and can_waitid:
                    try:
                        os.waitid(os.P_PID, process.pid,
                                  os.WEXITED | os.WNOHANG | os.WNOWAIT)
                    except ChildProcessError:
                        group_id = None
                if group_id is not None:
                    try:
                        os.killpg(group_id, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                # WNOWAIT observes the leader without reaping it. Its reserved
                # PID keeps the process-group ID from being reused before the
                # final group signal, including when a helper outlives it.
                if group_id is not None and can_waitid:
                    deadline = time.monotonic() + 1
                    while time.monotonic() < deadline:
                        try:
                            exited = os.waitid(os.P_PID, process.pid,
                                os.WEXITED | os.WNOHANG | os.WNOWAIT)
                        except ChildProcessError:
                            # Someone reaped the leader behind Popen's back.
                            group_id = None
                            break
                        if exited is not None:
                            break
                        time.sleep(0.01)
                # On POSIX without WNOWAIT, skip the grace period and kill the
                # group before wait() can reap its leader.
                if group_id is not None:
                    try:
                        os.killpg(group_id, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                try:
                    process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=1)
                for pipe in (process.stdin, process.stdout):
                    if pipe is not None:
                        pipe.close()
                self.process = None
            if self.directory is not None:
                self.directory.cleanup()
                self.directory = None
            self._group_id = None


class PromptCapabilityHost:
    """Supervise one child off the UI thread and publish only current scope."""

    def __init__(self, store: Store, config: CapabilityConfig,
                 capture_allowed: Callable[[PaneScope], bool], *, timeout: float = 0.5):
        if not isinstance(config, CapabilityConfig) or not callable(capture_allowed):
            raise TypeError("explicit capability config and capture policy required")
        self.store = store
        self.config = config
        self.capture_allowed = capture_allowed
        self.timeout = timeout
        self.lock = threading.Lock()
        self.wakeup = threading.Event()
        self.stop = threading.Event()
        self.thread: threading.Thread | None = None
        self.process: CapabilityProcess | None = None
        self.scope: PaneScope | None = None
        self.generation = 0
        self.model: PaneModel | None = None
        self.model_at = 0.0
        self.pending = False
        self.failures = 0

    def start(self):
        if self.thread is not None or self.stop.is_set():
            raise RuntimeError("capability already started")
        self.thread = threading.Thread(target=self._run, name="agenturm-capability", daemon=True)
        self.thread.start()
        return self

    def contribution(self) -> PaneContribution:
        return PaneContribution(self.config.pane_id, self.config.title,
                                self.config.columns, self.query)

    def query(self, scope: PaneScope) -> PaneModel:
        if self.stop.is_set():
            return PaneModel("unavailable", "Capability process is stopped")
        if (not isinstance(scope, PaneScope) or not scope.service or not scope.session_id
                or not AGENT_ID.fullmatch(scope.agent_id) or self.capture_allowed(scope) is not True):
            with self.lock:
                self.generation += 1
                self.scope = None
                self.model = None
                self.model_at = 0.0
                self.pending = False
            return PaneModel("unavailable", "Content capture or exact agent scope unavailable")
        with self.lock:
            if scope != self.scope:
                self.generation += 1
                self.scope = scope
                self.model = None
                self.model_at = 0.0
                self.pending = False
                self.failures = 0
            if self.thread is None:
                return PaneModel("unavailable", "Capability process is not started")
            if (not self.pending and self.failures < 3 and
                    (self.model is None or time.monotonic() - self.model_at >= REFRESH_SECONDS)):
                self.pending = True
                self.wakeup.set()
            return self.model or PaneModel("degraded", "Loading scoped signals")

    def _run(self):
        while not self.stop.is_set():
            self.wakeup.wait(0.5)
            self.wakeup.clear()
            if self.stop.is_set():
                break
            with self.lock:
                scope, generation = self.scope, self.generation
            if scope is None:
                continue
            try:
                allowed = self.capture_allowed(scope) is True
                facts = prompt_signal_facts(self.store, scope, capture_allowed=allowed)
                if self.stop.is_set():
                    break
                if facts.state == "unavailable":
                    model = PaneModel("unavailable", "Content capture or exact agent scope unavailable")
                else:
                    if self.capture_allowed(scope) is not True:
                        raise PermissionError("capture revoked")
                    if self.process is None:
                        self.process = CapabilityProcess(self.config, timeout=self.timeout).start()
                    if self.stop.is_set():
                        break
                    model = self.process.query(facts)
                if self.capture_allowed(scope) is not True:
                    model = PaneModel("unavailable", "Content capture or exact agent scope unavailable")
                failed = False
            except Exception:
                if self.process is not None:
                    self.process.close()
                    self.process = None
                model = PaneModel("unavailable", "Capability process unavailable")
                failed = True
            with self.lock:
                if self.stop.is_set():
                    break
                if generation != self.generation or scope != self.scope:
                    continue
                self.pending = False
                self.model = model
                self.model_at = time.monotonic()
                self.failures = min(3, self.failures + 1) if failed else 0

    def close(self):
        self.stop.set()
        self.wakeup.set()
        if self.process is not None:
            self.process.close()
        if self.thread is not None:
            self.thread.join(timeout=2)
            self.thread = None
        if self.process is not None:
            self.process.close()

    def __enter__(self):
        return self.start()

    def __exit__(self, *_args):
        self.close()
