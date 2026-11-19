"""Opt-in, bounded process supervision for explicit source adapter manifests."""
from __future__ import annotations

import threading
import time
import sqlite3
from dataclasses import dataclass
from typing import TYPE_CHECKING

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


def _failure(exc: BaseException, store: Store, source_id: str) -> tuple[str, bool]:
    if isinstance(exc, ProtocolError):
        message = str(exc)
        if "capture revoked" in message:
            return "capture_revoked", True
        if "stale plugin source lease" in message:
            try:
                receiver_id = store.receiver_namespace()
                with store._connect() as db:
                    row = db.execute("SELECT capture_enabled FROM plugin_source_cursor_v1 "
                                     "WHERE source_id=? AND receiver_id=?",
                                     (source_id, receiver_id)).fetchone()
                if row is not None and not row["capture_enabled"]:
                    return "capture_revoked", True
            except (OSError, sqlite3.Error, ValueError):
                pass
            return "source_fenced", True
        if "ownership or artifact" in message:
            return "ownership_changed", True
        if "artifact hash mismatch" in message:
            return "artifact_changed", True
        if "timed out" in message:
            return "timeout", False
        return "protocol_error", False
    if isinstance(exc, (OSError, ValueError)):
        return "startup_error", False
    return "worker_error", False


class PluginSupervisor:
    """One bounded polling thread per explicitly enabled source.

    The Store cursor is the recovery point. A failed worker is closed, then a
    new process receives a fresh lease and resumes that cursor. Repeated
    failures quarantine the source until the owner explicitly restarts it.
    """
    def __init__(self, store: Store, sources: tuple[PluginSource, ...], *,
                 max_failures: int = 5, backoff_seconds: float = 0.25):
        sources = tuple(sources)
        if len(sources) > MAX_PROCESSES or len({item.source_id for item in sources}) != len(sources):
            raise ValueError("duplicate or excessive plugin sources")
        if type(max_failures) is not int or not 1 <= max_failures <= 20:
            raise ValueError("invalid plugin failure limit")
        if type(backoff_seconds) not in (int, float) or not 0.01 <= backoff_seconds <= 30:
            raise ValueError("invalid plugin backoff")
        for source in sources:
            if not isinstance(source, PluginSource) or not isinstance(source.manifest, PluginManifest):
                raise TypeError("plugin source requires a manifest")
            if (type(source.poll_interval) not in (int, float)
                    or not 0.05 <= source.poll_interval <= 60):
                raise ValueError("invalid plugin poll interval")
            PluginSession(store, source.manifest, source_id=source.source_id,
                          allowed_grants=source.allowed_grants, timeout=source.timeout)
        self.store = store
        self.sources = sources
        self.max_failures = max_failures
        self.backoff_seconds = float(backoff_seconds)
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
            "projection_error_code": ""} for item in sources}

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
            session = PluginSession(self.store, source.manifest, source_id=source_id,
                                    allowed_grants=source.allowed_grants,
                                    timeout=source.timeout)
            try:
                self._update(source_id, state="starting", restart_count=max(0, attempts))
                attempts += 1
                with session:
                    with self.lock:
                        self.current[source_id] = session
                    self._update(source_id, state="running", last_cursor=session.cursor or "")
                    while not self.stop.is_set():
                        batch = session.poll_once()
                        failures = 0
                        if batch.records:
                            last_commit_ns = time.time_ns()
                        self._update(source_id, state="running", consecutive_failures=0,
                                     last_error_code="", reason="",
                                     last_success_ns=time.time_ns(), last_cursor=batch.cursor,
                                     last_commit_ns=last_commit_ns,
                                     projection_error_code=(session.projection_error or "").split(":", 1)[0])
                        if self.stop.wait(source.poll_interval):
                            break
            except Exception as exc:
                if self.stop.is_set():
                    break
                code, permanent = _failure(exc, self.store, source_id)
                failures += 1
                self._update(source_id, state="failed" if permanent else "degraded",
                             last_error_code=code, reason=_REASONS[code],
                             last_failure_code=code, last_failure_reason=_REASONS[code],
                             last_failure_ns=time.time_ns(),
                             consecutive_failures=failures)
                if permanent or failures >= self.max_failures:
                    self._update(source_id, state="revoked" if code == "capture_revoked"
                                 else "fenced" if code == "source_fenced"
                                 else "quarantined")
                    break
                self.stop.wait(min(30.0, self.backoff_seconds * 2 ** (failures - 1)))
            finally:
                with self.lock:
                    self.current.pop(source_id, None)
                session.close()
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
