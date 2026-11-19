"""Bounded, opt-in process transport for an explicitly configured adapter.

This host has no production ingestion path. A caller must durably commit a
validated batch before ``ack``; the host closes a worker on failed commit.
The child is a separate crash domain, not a same-user security sandbox.
"""
from __future__ import annotations

import hashlib
import os
import queue
import signal
import stat
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from typing import BinaryIO, Callable

from .plugin_frames import FrameError, MAX_FRAME_BYTES, read_frame, write_frame
from .plugin_manifest import PluginManifest, plan_activation
from .plugin_wire import (MAX_BATCH_RECORDS, MAX_CONTROL_FRAME_BYTES, PROTOCOL_MAJOR, PollBatch, ProtocolError,
                          decode_message, encode_message, validate_batch,
                          validate_hello_ack)


MAX_ARTIFACT_BYTES = 64 * 1024 * 1024
MAX_STDERR_BYTES = 16 * 1024
MAX_PROCESSES = 4
_process_slots = threading.BoundedSemaphore(MAX_PROCESSES)


def rust_host_artifact(path: str | None, digest: str | None) -> bytes | None:
    """Verify an explicitly selected host before any plugin/source activation."""
    if path is None and digest is None:
        return None
    if (os.name != "posix" or not isinstance(path, str) or not Path(path).is_absolute()
            or not isinstance(digest, str) or len(digest) != 64
            or any(char not in "0123456789abcdef" for char in digest)):
        raise ValueError("Rust plugin host requires an absolute POSIX path and SHA-256")
    artifact = Path(path)
    if artifact.is_symlink():
        raise ProtocolError("Rust plugin host must be a regular file")
    try:
        info = artifact.stat()
        if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_ARTIFACT_BYTES:
            raise ProtocolError("invalid Rust plugin host artifact")
        raw = artifact.read_bytes()
    except OSError as exc:
        raise ProtocolError("cannot read Rust plugin host artifact") from exc
    if len(raw) > MAX_ARTIFACT_BYTES or hashlib.sha256(raw).hexdigest() != digest:
        raise ProtocolError("Rust plugin host artifact hash mismatch")
    return raw


class PluginProcess:
    def __init__(self, manifest: PluginManifest, *, source_id: str,
                 allowed_grants: tuple[str, ...] = (), timeout: float = 5.0,
                 source_root: str | None = None,
                 required_features: tuple[str, ...] = (),
                 rust_host_executable: str | None = None,
                 rust_host_sha256: str | None = None):
        if not isinstance(manifest, PluginManifest):
            raise TypeError("plugin manifest required")
        plan_activation({manifest.id: manifest}, (manifest.id,),
                        host_features=("evidence-ingest/1",),
                        allowed_grants=allowed_grants,
                        protocol_major=PROTOCOL_MAJOR)
        if "source-adapter/1" not in manifest.provides:
            raise ProtocolError("manifest does not provide source-adapter/1")
        if (not isinstance(source_id, str) or not source_id or len(source_id) > 256
                or any(ord(char) < 32 or ord(char) == 127 for char in source_id)):
            raise ValueError("invalid host-owned source ID")
        if type(timeout) not in (int, float) or not 0 < timeout <= 60:
            raise ValueError("plugin timeout must be between 0 and 60 seconds")
        if source_root is not None:
            if (not isinstance(source_root, str) or not Path(source_root).is_absolute()
                    or len(source_root) > 1024 or any(ord(char) < 32 or ord(char) == 127
                                                        for char in source_root)):
                raise ValueError("plugin source root must be an absolute path")
            if ("configured-root/1" not in manifest.provides
                    or "configured-source-read" not in manifest.grants):
                raise ValueError("plugin source root requires a declared feature and grant")
        if (not isinstance(required_features, tuple)
                or any(not isinstance(feature, str) for feature in required_features)):
            raise ValueError("invalid required plugin features")
        expected = {"source-adapter/1", *required_features}
        if source_root is not None:
            expected.add("configured-root/1")
        if not expected <= set(manifest.provides):
            raise ProtocolError("manifest lacks a required plugin feature")
        self.manifest = manifest
        self.source_id = source_id
        self.timeout = float(timeout)
        self.source_root = source_root
        self.required_features = frozenset(expected)
        rust_host_artifact(rust_host_executable, rust_host_sha256)
        self.rust_host_executable = rust_host_executable
        self.rust_host_sha256 = rust_host_sha256
        self.process: subprocess.Popen | None = None
        self._group_id: int | None = None
        self._temporary: tempfile.TemporaryDirectory | None = None
        self._frames: queue.Queue[object] = queue.Queue(maxsize=2)
        self._stopping = threading.Event()
        self._slot_held = False
        self._stderr_bytes = 0
        self._stderr_lock = threading.Lock()
        self.pending: PollBatch | None = None
        self.last_ack: str | None = None

    @property
    def stderr_bytes(self) -> int:
        with self._stderr_lock:
            return self._stderr_bytes

    def _artifact_bytes(self) -> bytes:
        if len(self.manifest.entrypoint) != 1:
            raise ProtocolError("process host requires a single executable artifact")
        path = Path(self.manifest.entrypoint[0])
        if not path.is_absolute() or path.is_symlink():
            raise ProtocolError("plugin artifact must be an absolute regular file")
        try:
            info = path.stat()
            if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_ARTIFACT_BYTES:
                raise ProtocolError("invalid plugin artifact size or type")
            raw = path.read_bytes()
        except OSError as exc:
            raise ProtocolError("cannot read plugin artifact") from exc
        if len(raw) > MAX_ARTIFACT_BYTES or hashlib.sha256(raw).hexdigest() != self.manifest.artifact_sha256:
            raise ProtocolError("plugin artifact hash mismatch")
        return raw

    def verify_artifact(self) -> None:
        """Inert preflight; start rechecks immediately before execution."""
        self._artifact_bytes()

    def _artifact(self) -> Path:
        raw = self._artifact_bytes()
        self._temporary = tempfile.TemporaryDirectory(prefix="agenturm-plugin-")
        copied = Path(self._temporary.name) / "plugin"
        copied.write_bytes(raw)
        copied.chmod(0o700)
        return copied

    def _reader(self, data_pipe: BinaryIO) -> None:
        try:
            while not self._stopping.is_set():
                raw = read_frame(data_pipe)
                if raw is None:
                    raise ProtocolError("plugin closed data pipe")
                while not self._stopping.is_set():
                    try:
                        self._frames.put(raw, timeout=0.1)
                        break
                    except queue.Full:
                        continue
        except (OSError, FrameError, ProtocolError) as exc:
            while not self._stopping.is_set():
                try:
                    self._frames.put(exc, timeout=0.1)
                    break
                except queue.Full:
                    continue

    def _drain_stderr(self, error_pipe: BinaryIO) -> None:
        while not self._stopping.is_set():
            try:
                chunk = error_pipe.read(4096)
            except OSError:
                return
            if not chunk:
                return
            with self._stderr_lock:
                self._stderr_bytes = min(MAX_STDERR_BYTES, self._stderr_bytes + len(chunk))

    def _send(self, message: dict) -> None:
        if self.process is None or self.process.stdin is None:
            raise ProtocolError("plugin is not running")
        stream = self.process.stdin
        raw = encode_message(message)
        if len(raw.encode("utf-8")) > MAX_CONTROL_FRAME_BYTES:
            raise ProtocolError("control message exceeds byte limit")
        result: list[BaseException] = []

        def write() -> None:
            try:
                write_frame(stream, raw, max_bytes=MAX_CONTROL_FRAME_BYTES)
            except (OSError, FrameError) as exc:
                result.append(exc)

        thread = threading.Thread(target=write, name="agenturm-plugin-control", daemon=True)
        thread.start()
        thread.join(self.timeout)
        if thread.is_alive():
            self.close(force=True)
            raise ProtocolError("plugin control pipe timed out")
        if result:
            self.close(force=True)
            raise ProtocolError("plugin control pipe failed") from result[0]

    def _next(self) -> dict:
        try:
            item = self._frames.get(timeout=self.timeout)
        except queue.Empty as exc:
            self.close(force=True)
            raise ProtocolError("plugin data pipe timed out") from exc
        if isinstance(item, BaseException):
            self.close(force=True)
            raise ProtocolError("plugin data pipe failed") from item
        try:
            return decode_message(item)
        except ProtocolError:
            self.close(force=True)
            raise

    def start(self) -> PluginProcess:
        if self.process is not None or self._stopping.is_set():
            raise ProtocolError("plugin already started")
        if not _process_slots.acquire(blocking=False):
            raise ProtocolError("plugin process limit reached")
        self._slot_held = True
        try:
            copied = self._artifact()
            host_bytes = rust_host_artifact(self.rust_host_executable, self.rust_host_sha256)
            command = [str(copied)]
            if host_bytes is not None:
                host_copy = Path(self._temporary.name) / "rust-host"
                host_copy.write_bytes(host_bytes)
                host_copy.chmod(0o700)
                command = [str(host_copy), "--child", str(copied)]
            child_env = {"PATH": os.defpath, "LANG": "C.UTF-8",
                         "AGENTURM_PLUGIN_PROTOCOL": "1"}
            if self.source_root is not None:
                child_env["AGENTURM_PLUGIN_SOURCE_ROOT"] = self.source_root
            self.process = subprocess.Popen(
                command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.PIPE, cwd=self._temporary.name,
                env=child_env,
                bufsize=0, close_fds=True,
                start_new_session=host_bytes is not None)
            if host_bytes is not None:
                self._group_id = self.process.pid
            threading.Thread(target=self._reader, args=(self.process.stdout,),
                             name="agenturm-plugin-data", daemon=True).start()
            threading.Thread(target=self._drain_stderr, args=(self.process.stderr,),
                             name="agenturm-plugin-stderr", daemon=True).start()
            self._send({"type": "hello", "protocol": PROTOCOL_MAJOR,
                        "plugin_id": self.manifest.id, "source_id": self.source_id,
                        "max_frame_bytes": MAX_FRAME_BYTES})
            advertised = set(validate_hello_ack(self._next(), plugin_id=self.manifest.id))
            if not self.required_features <= advertised or not advertised <= set(self.manifest.provides):
                raise ProtocolError("plugin handshake features differ from approved manifest")
            return self
        except BaseException:
            self.close(force=True)
            raise

    def poll(self, cursor: str, *, max_records: int = MAX_BATCH_RECORDS) -> PollBatch:
        if self.process is None or self.process.poll() is not None:
            raise ProtocolError("plugin is not running")
        if self.pending is not None:
            raise ProtocolError("previous batch requires durable acknowledgment")
        if not isinstance(cursor, str) or len(cursor) > 512:
            raise ValueError("invalid resume cursor")
        if type(max_records) is not int or not 1 <= max_records <= MAX_BATCH_RECORDS:
            raise ValueError("invalid poll record budget")
        self._send({"type": "poll", "protocol": PROTOCOL_MAJOR,
                    "source_id": self.source_id, "cursor": cursor,
                    "max_records": max_records, "max_bytes": MAX_FRAME_BYTES})
        try:
            batch = validate_batch(self._next(), source_id=self.source_id)
        except ProtocolError:
            self.close(force=True)
            raise
        if len(batch.records) > max_records:
            self.close(force=True)
            raise ProtocolError("plugin exceeded poll record budget")
        self.pending = batch
        return batch

    def ack(self, committed_cursor: str) -> None:
        if self.pending is None or committed_cursor != self.pending.cursor:
            raise ProtocolError("acknowledgment lacks a matching committed batch")
        self._send({"type": "ack", "protocol": PROTOCOL_MAJOR,
                    "source_id": self.source_id, "cursor": committed_cursor})
        self.last_ack = committed_cursor
        self.pending = None

    def poll_commit(self, cursor: str, commit: Callable[[PollBatch], str], *,
                    max_records: int = MAX_BATCH_RECORDS) -> PollBatch:
        batch = self.poll(cursor, max_records=max_records)
        try:
            committed_cursor = commit(batch)
            if committed_cursor != batch.cursor:
                raise ProtocolError("commit did not durably accept batch cursor")
        except BaseException:
            self.close(force=True)
            raise
        self.ack(committed_cursor)
        return batch

    def close(self, *, force: bool = False) -> None:
        process = self.process
        self._stopping.set()
        if process is not None:
            # Do not reap the group leader before its remaining members have
            # been stopped. Keeping its PID reserved fences normal-close PGID
            # reuse on platforms with waitid(WNOWAIT).
            running = process.returncode is None if self._group_id is not None else process.poll() is None
            if running:
                if not force and process.stdin is not None:
                    def request_stop() -> None:
                        try:
                            write_frame(process.stdin, encode_message({"type": "stop", "protocol": 1}),
                                        max_bytes=MAX_CONTROL_FRAME_BYTES)
                        except (OSError, FrameError):
                            pass
                    thread = threading.Thread(target=request_stop, name="agenturm-plugin-stop", daemon=True)
                    thread.start()
                    thread.join(min(self.timeout, 0.2))
                if self._group_id is not None:
                    if force:
                        self._signal_group(signal.SIGTERM)
                    try:
                        self._wait_group(process, timeout=min(self.timeout, 2.0) if not force else 0.1)
                    except subprocess.TimeoutExpired:
                        self._signal_group(signal.SIGTERM)
                        try:
                            self._wait_group(process, timeout=1.0)
                        except subprocess.TimeoutExpired:
                            self._kill_group()
                            process.wait(timeout=1.0)
                else:
                    try:
                        process.wait(timeout=min(self.timeout, 2.0) if not force else 0.1)
                    except subprocess.TimeoutExpired:
                        process.terminate()
                        try:
                            process.wait(timeout=1.0)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait(timeout=1.0)
            if self._group_id is not None:
                # Also clean ordinary helpers after a successful host exit.
                # Crash paths may already have reaped the leader; this remains
                # best-effort cleanup, not a sandbox for detached descendants.
                self._kill_group()
            for stream in (process.stdin, process.stdout, process.stderr):
                if stream is not None:
                    try:
                        stream.close()
                    except OSError:
                        pass
            self.process = None
        if self._temporary is not None:
            self._temporary.cleanup()
            self._temporary = None
        if self._slot_held:
            _process_slots.release()
            self._slot_held = False

    def _signal_group(self, sig: int) -> None:
        if self._group_id is not None:
            try:
                os.killpg(self._group_id, sig)
            except ProcessLookupError:
                pass

    def _kill_group(self) -> None:
        self._signal_group(signal.SIGKILL)
        self._group_id = None

    def _wait_group(self, process: subprocess.Popen, *, timeout: float) -> None:
        if process.returncode is None and hasattr(os, "waitid") and hasattr(os, "WNOWAIT"):
            deadline = time.monotonic() + timeout
            while True:
                try:
                    exited = os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
                except ChildProcessError:
                    # Another caller may already have polled/reaped a crash.
                    break
                if exited is not None:
                    self._kill_group()
                    process.wait(timeout=1.0)
                    return
                if time.monotonic() >= deadline:
                    raise subprocess.TimeoutExpired(process.args, timeout)
                time.sleep(0.01)
        process.wait(timeout=timeout)

    def __enter__(self) -> PluginProcess:
        return self.start()

    def __exit__(self, _type, _value, _traceback) -> None:
        self.close()
