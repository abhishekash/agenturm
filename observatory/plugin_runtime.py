"""Explicit, opt-in composition of a process adapter and the durable inbox.

One session keeps one worker alive across polls. A caller owns scheduling and
must create a fresh session after a crash; activation resumes the Store cursor.
No default collector instantiates this class yet.
"""
from __future__ import annotations

import threading
import time
from typing import TYPE_CHECKING

from .plugin_admission import (SourceLease, activate_source, commit_batch,
                               last_source_diagnostic, member_cursors)
from .plugin_manifest import PluginManifest
from .plugin_process import PluginProcess
from .plugin_projection import project_source
from .plugin_wire import CommandPollBatch, ContentPollBatch, PollBatch, ProtocolError

if TYPE_CHECKING:
    from .core import Store
    from .plugin_supervisor import PluginSource


class PluginSession:
    def __init__(self, store: Store, manifest: PluginManifest, *, source_id: str,
                 allowed_grants: tuple[str, ...] = (), timeout: float = 5.0,
                 source_root: str | None = None,
                 required_features: tuple[str, ...] = (),
                 rust_host_executable: str | None = None,
                 rust_host_sha256: str | None = None,
                 content_source: PluginSource | None = None,
                 command_source: PluginSource | None = None):
        if not isinstance(manifest, PluginManifest):
            raise TypeError("plugin manifest required")
        self.store = store
        self.content_source = None
        self.content_grant = None
        self.command_source = None
        self.command_grant = None
        if content_source is not None and command_source is not None:
            raise ProtocolError("Pi reference capture modes are mutually exclusive")
        if content_source is not None:
            from .plugin_supervisor import PluginSource
            if (not isinstance(content_source, PluginSource)
                    or content_source.manifest != manifest
                    or content_source.source_id != source_id
                    or content_source.source_root != source_root
                    or set(content_source.allowed_grants) != set(allowed_grants)
                    or set(content_source.runtime_features) != set(required_features)
                    or content_source.capture_mode != "pi-review-text-v1"
                    or "pi-content-ref/1" not in required_features):
                raise ProtocolError("Pi content source differs from trusted process configuration")
            self.content_source = content_source
        elif "pi-content-ref/1" in required_features:
            raise ProtocolError("Pi content feature requires trusted configuration")
        if command_source is not None:
            from .plugin_supervisor import PluginSource
            if (not isinstance(command_source, PluginSource)
                    or command_source.manifest != manifest
                    or command_source.source_id != source_id
                    or command_source.source_root != source_root
                    or set(command_source.allowed_grants) != set(allowed_grants)
                    or set(command_source.runtime_features) != set(required_features)
                    or command_source.capture_mode != "pi-review-text-command-v1"
                    or "pi-command-ref/1" not in required_features):
                raise ProtocolError("Pi command source differs from trusted process configuration")
            self.command_source = command_source
        elif "pi-command-ref/1" in required_features:
            raise ProtocolError("Pi command feature requires trusted configuration")
        self.process = PluginProcess(manifest, source_id=source_id,
                                     allowed_grants=allowed_grants, timeout=timeout,
                                     source_root=source_root, required_features=required_features,
                                     rust_host_executable=rust_host_executable,
                                     rust_host_sha256=rust_host_sha256,
                                     content_refs_enabled=self.content_source is not None)
        self.lease: SourceLease | None = None
        self.cursor: str | None = None
        self.members: dict[str, str] | None = None
        self.projection_error: str | None = None
        self.last_diagnostic: dict = {}
        self._projection_event = threading.Event()
        self._projection_stop = threading.Event()
        self._projection_thread: threading.Thread | None = None

    def _projection_loop(self) -> None:
        while not self._projection_stop.is_set():
            self._projection_event.wait(0.5)
            if self._projection_stop.is_set():
                break
            if not self._projection_event.is_set():
                continue
            self._projection_event.clear()
            try:
                project_source(self.store, self.process.source_id)
                self.projection_error = None
            except Exception as exc:
                self.projection_error = type(exc).__name__ + ": " + str(exc)
                if isinstance(exc, ProtocolError) and "source changed; retry" in str(exc):
                    time.sleep(0.1)
                    self._projection_event.set()

    def start(self) -> PluginSession:
        if self.lease is not None:
            raise ProtocolError("plugin session already started")
        manifest = self.process.manifest
        from .plugin_pi_content import PiContentGrant
        from .plugin_pi_command import PiCommandGrant
        fingerprint = (PiContentGrant.fingerprint_for_source(self.content_source)
                       if self.content_source is not None else
                       PiCommandGrant.fingerprint_for_source(self.command_source)
                       if self.command_source is not None else "")
        self.lease = activate_source(self.store, plugin_id=manifest.id,
                                     source_id=self.process.source_id,
                                     generation=manifest.artifact_sha256,
                                     content_fingerprint=fingerprint)
        self.cursor = self.lease.cursor
        try:
            self.process.start()
            if self.content_source is not None:
                self.content_grant = PiContentGrant.from_source(self.content_source, self.lease)
            if self.command_source is not None:
                self.command_grant = PiCommandGrant.from_source(self.command_source, self.lease)
            if "source-discovery/1" in self.process.required_features:
                self.members = member_cursors(self.store, self.lease)
            self.last_diagnostic = last_source_diagnostic(self.store, self.lease)
        except BaseException:
            try:
                self.process.close(force=True)
            except BaseException:
                pass  # Preserve the startup error after best-effort cleanup.
            self.lease = None
            self.cursor = None
            self.members = None
            self.content_grant = None
            self.command_grant = None
            raise
        self._projection_stop.clear()
        self._projection_thread = threading.Thread(target=self._projection_loop,
                                                   name="agenturm-plugin-projection", daemon=True)
        self._projection_thread.start()
        self._projection_event.set()  # Resume a dirty projection after a crash.
        return self

    def poll_once(self) -> PollBatch:
        if self.lease is None or self.cursor is None:
            raise ProtocolError("plugin session is not running")
        expected = self.cursor
        batch = self.process.poll_commit(expected, lambda received: commit_batch(
            self.store, self.lease, expected, received,
            allow_root_gap="source-discovery-root-gap/1" in self.process.required_features,
            content_refs=(received.content_refs
                          if isinstance(received, ContentPollBatch) else ()),
            content_grant=(self.content_grant
                           if isinstance(received, ContentPollBatch) and received.content_refs
                           else None),
            command_refs=(received.command_refs
                          if isinstance(received, CommandPollBatch) else ()),
            command_grant=(self.command_grant
                           if isinstance(received, CommandPollBatch) and received.command_refs
                           else None)),
            members=self.members)
        self.cursor = batch.cursor
        if batch.member_id and self.members is not None:
            if batch.member_cursor:
                self.members[batch.member_id] = batch.member_cursor
            else:
                self.members.pop(batch.member_id, None)
        for record in batch.records:
            if record["kind"] == "diagnostic":
                self.last_diagnostic = {**record["payload"],
                                        "time_ns": record["time_ns"]}
        if batch.records:
            # Coalesce rebuilds off the acknowledgment path. Dirty sequence
            # hides prior rows until the current revision is published.
            self._projection_event.set()
        return batch

    def close(self) -> None:
        self._projection_stop.set()
        self._projection_event.set()
        if self._projection_thread is not None:
            self._projection_thread.join(timeout=2.0)
            self._projection_thread = None
        self.process.close()
        self.lease = None
        self.cursor = None
        self.members = None
        self.content_grant = None
        self.command_grant = None

    def __enter__(self) -> PluginSession:
        return self.start()

    def __exit__(self, _type, _value, _traceback) -> None:
        self.close()
