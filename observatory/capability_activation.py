"""Explicit cockpit composition for independently installed capability processes.

The UI receives only ordinary pane contributions. Network reads and process
traffic remain in the capability worker, never in Textual's event loop.
"""
from __future__ import annotations

import time
import threading
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlencode

from .capability_process import PromptCapabilityHost, read_capability_config
from .ui_contributions import PaneRegistry, PaneScope


POLICY_MAX_AGE_SECONDS = 10
LIVE_APPROVAL_SECONDS = 0.25
LIVE_POLL_SECONDS = 0.1
SCOPE_IDLE_SECONDS = 3.0


def _exact_capture(local: dict, scope: PaneScope) -> bool:
    if not isinstance(local, dict) or local.get("capture_content") is not True:
        return False
    rows = local.get("sessions")
    if not isinstance(rows, list):
        return False
    matches = [row for row in rows if isinstance(row, dict)
               and row.get("service") == scope.service
               and row.get("session_id") == scope.session_id
               and row.get("agent_id") == scope.agent_id]
    return bool(matches) and all(row.get("capture_content") is True for row in matches)


class LiveCaptureGate:
    """Fast policy cache; a separate watcher refreshes exact receiver health."""

    def __init__(self, api, feed):
        self.api = api
        self.feed = feed
        self._lock = threading.Lock()
        self._wake = threading.Event()
        self._stop = threading.Event()
        self._scope: PaneScope | None = None
        self._receiver: str | None = None
        self._approved_at = 0.0
        self._last_query = 0.0
        self._denied = False
        self._thread = threading.Thread(target=self._watch, name="agenturm-capability-policy", daemon=True)
        self._thread.start()

    def _snapshot_receiver(self, scope: PaneScope, *, worker: bool = False) -> str | None:
        # The Feed lock protects a new capture denial before it publishes the
        # sanitized snapshot. Never wait for a network or Store read here.
        lock = self.feed._capture_lock
        # Worker checks can wait briefly for a concurrent Feed commit. The UI
        # path never waits and instead withholds its cached model for this tick.
        if not (lock.acquire(timeout=0.05) if worker else lock.acquire(blocking=False)):
            return None
        try:
            snapshot = self.feed.snapshot
            if (self.feed.stop.is_set() or snapshot.error or snapshot.native_bootstrap
                    or not snapshot.receiver_id or not snapshot.updated
                    or time.time() - snapshot.updated > POLICY_MAX_AGE_SECONDS
                    or not _exact_capture(snapshot.local_status, scope)):
                return None
            return snapshot.receiver_id
        finally:
            lock.release()

    def __call__(self, scope: PaneScope) -> bool:
        if not isinstance(scope, PaneScope):
            return False
        receiver = self._snapshot_receiver(scope)
        if receiver is None:
            return False
        now = time.monotonic()
        if not self._lock.acquire(blocking=False):
            return False
        try:
            if scope != self._scope or receiver != self._receiver:
                self._scope, self._receiver = scope, receiver
                self._approved_at = 0.0
                self._denied = False
            self._last_query = now
            allowed = (not self._denied and self._approved_at > 0
                       and now - self._approved_at <= LIVE_APPROVAL_SECONDS)
        finally:
            self._lock.release()
        self._wake.set()
        return allowed

    def _deny(self, scope: PaneScope):
        with self._lock:
            if scope == self._scope:
                self._denied = True
                self._approved_at = 0.0

    def _invalidate(self, scope: PaneScope):
        """Expire an uncertain read; let the health watcher reapprove later."""
        with self._lock:
            if scope == self._scope:
                self._approved_at = 0.0
        self._wake.set()

    def _health(self, scope: PaneScope, receiver: str) -> None:
        health = self.api.get("/api/health")
        if (not isinstance(health, dict) or health.get("receiver_id") != receiver
                or not _exact_capture(health.get("local_agents"), scope)
                or self._snapshot_receiver(scope, worker=True) != receiver):
            raise PermissionError("receiver capture policy denied exact scope")
        with self._lock:
            if scope == self._scope and receiver == self._receiver and not self._denied:
                self._approved_at = time.monotonic()

    def _watch(self):
        while not self._stop.is_set():
            self._wake.wait(LIVE_POLL_SECONDS)
            self._wake.clear()
            if self._stop.is_set():
                break
            with self._lock:
                scope, receiver = self._scope, self._receiver
                active = time.monotonic() - self._last_query < SCOPE_IDLE_SECONDS
                denied = self._denied
                if not active:
                    self._approved_at = 0.0
            if scope is None or receiver is None or not active or denied:
                continue
            try:
                self._health(scope, receiver)
            except Exception:
                self._deny(scope)

    def confirm_live(self, scope: PaneScope) -> str:
        receiver = self._snapshot_receiver(scope, worker=True)
        with self._lock:
            approved = (not self._denied and scope == self._scope
                        and receiver == self._receiver)
        if not approved or receiver is None:
            raise PermissionError("current capture policy unavailable")
        try:
            self._health(scope, receiver)
        except Exception:
            self._deny(scope)
            raise
        return receiver

    def close(self):
        self._stop.set()
        self._wake.set()
        self._thread.join(timeout=4)


class ReceiverActivityReader:
    """Store-shaped, bounded HTTP adapter for the existing facts reducer."""

    def __init__(self, api, gate: LiveCaptureGate):
        self.api = api
        self.gate = gate

    def activity_page(self, *, limit, service, session_id, agent_id,
                      subtree, include_reviews):
        if (type(limit) is not int or not 1 <= limit <= 200
                or subtree is not False or include_reviews is not False):
            raise ValueError("invalid capability Activity query")
        scope = PaneScope(agent_id, service, session_id)
        receiver = self.gate.confirm_live(scope)
        params = {"limit": limit, "service": service, "session_id": session_id,
                  "agent_id": agent_id, "subtree": 0, "include_reviews": 0,
                  "capture_scope": 1}
        try:
            page = self.api.get("/api/activity?" + urlencode(params))
            activity = page.get("activity") if isinstance(page, dict) else None
            if (not isinstance(page, dict) or page.get("receiver_id") != receiver
                    or not isinstance(activity, list) or len(activity) > limit
                    or any(not isinstance(item, dict) for item in activity)):
                raise ValueError("capability Activity receiver or schema mismatch")
            self.gate.confirm_live(scope)
        except Exception as exc:
            if isinstance(exc, HTTPError) and exc.code == 409:
                self.gate._invalidate(scope)
            else:
                self.gate._deny(scope)
            raise
        return page


def activate_capabilities(config_paths: tuple[Path, ...], api, feed):
    """Admit named configs without executing them; return hosts and UI panes."""
    if len(config_paths) > 4:
        raise ValueError("at most four cockpit capabilities may be activated")
    configs = tuple(read_capability_config(path) for path in config_paths)
    if len({config.pane_id for config in configs}) != len(configs):
        raise ValueError("duplicate cockpit capability pane")
    activations = []
    try:
        for config in configs:
            gate = LiveCaptureGate(api, feed)
            try:
                host = PromptCapabilityHost(ReceiverActivityReader(api, gate), config, gate).start()
            except BaseException:
                gate.close()
                raise
            activations.append((host, gate))
        panes = tuple(host.contribution() for host, _gate in activations)
        PaneRegistry(panes)
        return tuple(activations), panes
    except BaseException:
        for host, gate in reversed(activations):
            host.close()
            gate.close()
        raise
