"""Opt-in, bounded process supervision for explicit source adapter manifests."""
from __future__ import annotations

import threading
import time
from collections.abc import Callable
from dataclasses import dataclass
from typing import TYPE_CHECKING

from .plugin_batch_handlers import BatchHandlerRegistry, DEFAULT_BATCH_HANDLERS
from .plugin_capture_modes import CaptureModeRegistry, DEFAULT_CAPTURE_MODES
from .plugin_admission_port import (PluginAdmissionPort,
                                    PythonStoreAdmissionPort,
                                    require_plugin_admission_port)
from .plugin_reference_admission import (DEFAULT_REFERENCE_ADMISSIONS,
                                         ReferenceAdmissionRegistry)
from .plugin_manifest import PluginManifest
from .plugin_process import MAX_PROCESSES
from .plugin_runtime import PluginSession
from .plugin_wire import ProtocolError

if TYPE_CHECKING:
    from .core import Store


@dataclass(frozen=True)
class PluginSource:
    manifest: PluginManifest
    source_id: str
    allowed_grants: tuple[str, ...] = ()
    poll_interval: float = 0.5
    timeout: float = 5.0
    replaces: tuple[str, ...] = ()
    source_root: str | None = None
    required_features: tuple[str, ...] = ()
    capture_mode: str = "metadata-only"

    @property
    def runtime_features(self) -> tuple[str, ...]:
        """Manifest/handshake features required by this explicit source."""
        return tuple(sorted({*self.required_features,
                             *(f"harness.{name}/1" for name in self.replaces)}))


_REASONS = {
    "capture_revoked": "Capture for this plugin source was revoked.",
    "source_fenced": "Another worker owns this source lease.",
    "ownership_changed": "Source ownership or executable generation changed.",
    "artifact_changed": "The configured executable no longer matches its approved hash.",
    "protocol_error": "The plugin violated the source protocol or lost its data pipe.",
    "timeout": "The plugin did not answer within its configured deadline.",
    "startup_error": "The plugin executable could not start.",
    "worker_error": "The plugin worker stopped unexpectedly.",
}


def _failure(exc: BaseException, admission_port: PluginAdmissionPort | None,
             source_id: str) -> tuple[str, bool]:
    if admission_port is not None:
        try:
            lease_reason = admission_port.classify_lease_failure(exc, source_id)
        except Exception:
            lease_reason = None
        if lease_reason == "capture_revoked":
            return "capture_revoked", True
        if lease_reason == "source_fenced":
            return "source_fenced", True
    if isinstance(exc, ProtocolError):
        message = str(exc)
        if "capture revoked" in message:
            return "capture_revoked", True
        if "stale plugin source lease" in message:
            # The port failed to classify a known stale-lease signal. Fencing
            # is safer than retrying a batch against an uncertain lease.
            return "source_fenced", True
        if "ownership or artifact" in message:
            return "ownership_changed", True
        if "artifact hash mismatch" in message:
            return "artifact_changed", True
        if "timed out" in message:
            return "timeout", False
        return "protocol_error", False
    if isinstance(exc, (OSError, TypeError, ValueError)):
        return "startup_error", False
    return "worker_error", False


class PluginSupervisor:
    """One bounded polling thread per explicitly enabled source.

    The Store cursor is the recovery point. A failed worker is closed, then a
    new process receives a fresh lease and resumes that cursor. Repeated
    failures quarantine the source until the owner explicitly restarts it.
    """
    def __init__(self, store: Store, sources: tuple[PluginSource, ...], *,
                 max_failures: int = 5, backoff_seconds: float = 0.25,
                 rust_host_executable: str | None = None,
                 rust_host_sha256: str | None = None,
                 capture_modes: CaptureModeRegistry = DEFAULT_CAPTURE_MODES,
                 reference_admissions: ReferenceAdmissionRegistry =
                 DEFAULT_REFERENCE_ADMISSIONS,
                 batch_handlers: BatchHandlerRegistry = DEFAULT_BATCH_HANDLERS,
                 admission_port_factory: Callable[[Store], PluginAdmissionPort] | None = None):
        sources = tuple(sources)
        if len(sources) > MAX_PROCESSES or len({item.source_id for item in sources}) != len(sources):
            raise ValueError("duplicate or excessive plugin sources")
        if type(max_failures) is not int or not 1 <= max_failures <= 20:
            raise ValueError("invalid plugin failure limit")
        if type(backoff_seconds) not in (int, float) or not 0.01 <= backoff_seconds <= 30:
            raise ValueError("invalid plugin backoff")
        if not isinstance(capture_modes, CaptureModeRegistry):
            raise TypeError("trusted capture mode registry required")
        if not isinstance(reference_admissions, ReferenceAdmissionRegistry):
            raise TypeError("trusted reference admission registry required")
        if not isinstance(batch_handlers, BatchHandlerRegistry):
            raise TypeError("trusted batch handler registry required")
        self.admission_port_factory = (
            admission_port_factory if admission_port_factory is not None
            else PythonStoreAdmissionPort)
        if not callable(self.admission_port_factory):
            raise TypeError("plugin admission port factory must be callable")
        for source in sources:
            if not isinstance(source, PluginSource) or not isinstance(source.manifest, PluginManifest):
                raise TypeError("plugin source requires a manifest")
            if (type(source.poll_interval) not in (int, float)
                    or not 0.05 <= source.poll_interval <= 60):
                raise ValueError("invalid plugin poll interval")
            admission_port = require_plugin_admission_port(
                self.admission_port_factory(store))
            try:
                PluginSession(store, source.manifest, source_id=source.source_id,
                              allowed_grants=source.allowed_grants, timeout=source.timeout,
                              source_root=source.source_root,
                              required_features=source.runtime_features,
                              capture_source=source, capture_modes=capture_modes,
                              reference_admissions=reference_admissions,
                              batch_handlers=batch_handlers,
                              admission_port=admission_port,
                              rust_host_executable=rust_host_executable,
                              rust_host_sha256=rust_host_sha256)
            finally:
                admission_port.close()
        self.store = store
        self.sources = sources
        self.max_failures = max_failures
        self.backoff_seconds = float(backoff_seconds)
        self.rust_host_executable = rust_host_executable
        self.rust_host_sha256 = rust_host_sha256
        self.capture_modes = capture_modes
        self.reference_admissions = reference_admissions
        self.batch_handlers = batch_handlers
        self.stop = threading.Event()
        self.lock = threading.Lock()
        self.threads: list[threading.Thread] = []
        self.current: dict[str, PluginSession] = {}
        self.statuses = {item.source_id: {
            "source_id": item.source_id, "plugin_id": item.manifest.id,
            "state": "configured", "last_error_code": "", "reason": "",
            "last_failure_code": "", "last_failure_reason": "", "last_failure_ns": 0,
            "restart_count": 0, "consecutive_failures": 0,
            "last_success_ns": 0, "last_commit_ns": 0, "last_cursor": "",
            "projection_error_code": "", "last_source_diagnostic": {},
            "last_cleanup_error": ""} for item in sources}

    def _update(self, source_id: str, **values) -> None:
        with self.lock:
            self.statuses[source_id].update(values)

    def snapshot(self) -> list[dict]:
        with self.lock:
            return [dict(self.statuses[item.source_id]) for item in self.sources]

    def start(self) -> PluginSupervisor:
        if self.threads or self.stop.is_set():
            raise RuntimeError("plugin supervisor already started")
        if getattr(self.store, "plugin_supervisor", None) not in (None, self):
            raise RuntimeError("store already has a plugin supervisor")
        self.store.plugin_supervisor = self
        for source in self.sources:
            thread = threading.Thread(target=self._run, args=(source,),
                                      name="agenturm-plugin-" + source.manifest.id, daemon=True)
            self.threads.append(thread)
            thread.start()
        return self

    def _run(self, source: PluginSource) -> None:
        source_id = source.source_id
        failures = 0
        attempts = 0
        last_commit_ns = 0
        while not self.stop.is_set():
            session = None
            admission_port = None
            try:
                self._update(source_id, state="starting", restart_count=max(0, attempts))
                attempts += 1
                admission_port = require_plugin_admission_port(
                    self.admission_port_factory(self.store))
                session = PluginSession(
                    self.store, source.manifest, source_id=source_id,
                    allowed_grants=source.allowed_grants, timeout=source.timeout,
                    source_root=source.source_root,
                    required_features=source.runtime_features,
                    capture_source=source, capture_modes=self.capture_modes,
                    reference_admissions=self.reference_admissions,
                    batch_handlers=self.batch_handlers,
                    admission_port=admission_port,
                    rust_host_executable=self.rust_host_executable,
                    rust_host_sha256=self.rust_host_sha256)
                admission_port = None  # The session owns the port from here.
                with session:
                    with self.lock:
                        self.current[source_id] = session
                    self._update(source_id, state="running", last_cursor=session.cursor or "",
                                 last_source_diagnostic=dict(session.last_diagnostic))
                    while not self.stop.is_set():
                        batch = session.poll_once()
                        failures = 0
                        if batch.records:
                            last_commit_ns = time.time_ns()
                        self._update(source_id, state="running", consecutive_failures=0,
                                     last_error_code="", reason="",
                                     last_success_ns=time.time_ns(), last_cursor=batch.cursor,
                                     last_commit_ns=last_commit_ns,
                                     last_source_diagnostic=dict(session.last_diagnostic),
                                     projection_error_code=(session.projection_error or "").split(":", 1)[0])
                        if self.stop.wait(source.poll_interval):
                            break
            except Exception as exc:
                if self.stop.is_set():
                    break
                code, permanent = _failure(
                    exc, (session.admission_port if session is not None
                          else admission_port), source_id)
                failures += 1
                diagnostic_reset = {"last_source_diagnostic": {}} if code == "capture_revoked" else {}
                self._update(source_id, state="failed" if permanent else "degraded",
                             last_error_code=code, reason=_REASONS[code],
                             last_failure_code=code, last_failure_reason=_REASONS[code],
                             last_failure_ns=time.time_ns(),
                             consecutive_failures=failures, **diagnostic_reset)
                if permanent or failures >= self.max_failures:
                    self._update(source_id, state="revoked" if code == "capture_revoked"
                                 else "fenced" if code == "source_fenced"
                                 else "quarantined")
                    break
                self.stop.wait(min(30.0, self.backoff_seconds * 2 ** (failures - 1)))
            finally:
                with self.lock:
                    self.current.pop(source_id, None)
                if session is not None:
                    try:
                        session.close()
                    except Exception as close_error:
                        self._update(source_id,
                                     last_cleanup_error=type(close_error).__name__)
                    if session.admission_port_close_error:
                        self._update(source_id,
                                     last_cleanup_error=session.admission_port_close_error)
                elif admission_port is not None:
                    try:
                        admission_port.close()
                    except Exception as close_error:
                        self._update(source_id,
                                     last_cleanup_error=type(close_error).__name__)
                    admission_port = None
        if self.stop.is_set():
            self._update(source_id, state="stopped")

    def close(self) -> None:
        self.stop.set()
        with self.lock:
            sessions = list(self.current.values())
        for session in sessions:
            session.process.close(force=True)
        for thread in self.threads:
            thread.join(timeout=8.0)
        if getattr(self.store, "plugin_supervisor", None) is self:
            self.store.plugin_supervisor = None
