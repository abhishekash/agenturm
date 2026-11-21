"""Explicit, opt-in composition of a process adapter and the durable inbox.

One session keeps one worker alive across polls. A caller owns scheduling and
must create a fresh session after a crash; activation resumes the Store cursor.
No default collector instantiates this class yet.
"""
from __future__ import annotations

import threading
import time
from typing import TYPE_CHECKING

from .plugin_batch_handlers import BatchHandlerRegistry, DEFAULT_BATCH_HANDLERS
from .plugin_capture_modes import CaptureModeRegistry, DEFAULT_CAPTURE_MODES
from .plugin_admission import SourceLease
from .plugin_admission_port import (PluginAdmissionPort,
                                    PythonStoreAdmissionPort,
                                    require_plugin_admission_port)
from .plugin_manifest import PluginManifest
from .plugin_process import PluginProcess
from .plugin_projection import project_source
from .plugin_reference_admission import (DEFAULT_REFERENCE_ADMISSIONS,
                                         ReferenceAdmissionRegistry,
                                         ReferenceSubmission)
from .plugin_wire import (CommandPollBatch, ContentPollBatch, PollBatch,
                          ProtocolError, ReferencePollBatch)

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
                 command_source: PluginSource | None = None,
                 capture_source: PluginSource | None = None,
                 capture_modes: CaptureModeRegistry = DEFAULT_CAPTURE_MODES,
                 reference_admissions: ReferenceAdmissionRegistry =
                 DEFAULT_REFERENCE_ADMISSIONS,
                 batch_handlers: BatchHandlerRegistry = DEFAULT_BATCH_HANDLERS,
                 admission_port: PluginAdmissionPort | None = None):
        if not isinstance(manifest, PluginManifest):
            raise TypeError("plugin manifest required")
        if not isinstance(capture_modes, CaptureModeRegistry):
            raise TypeError("trusted capture mode registry required")
        if not isinstance(reference_admissions, ReferenceAdmissionRegistry):
            raise TypeError("trusted reference admission registry required")
        if not isinstance(batch_handlers, BatchHandlerRegistry):
            raise TypeError("trusted batch handler registry required")
        self.store = store
        self.admission_port = require_plugin_admission_port(
            admission_port if admission_port is not None else PythonStoreAdmissionPort(store))
        self._admission_port_closed = False
        self.capture_source = None
        self.capture_grant = None
        self.content_grant = None
        self.command_grant = None
        self.content_source = None
        self.command_source = None
        if content_source is not None and command_source is not None:
            raise ProtocolError("reference capture modes are mutually exclusive")
        if capture_source is not None and (content_source is not None or command_source is not None):
            raise ProtocolError("capture source cannot be supplied twice")
        legacy_family = ("content" if content_source is not None else
                         "command" if command_source is not None else None)
        source = capture_source or content_source or command_source
        if source is not None:
            from .plugin_supervisor import PluginSource
            if (not isinstance(source, PluginSource)
                    or source.manifest != manifest
                    or source.source_id != source_id
                    or source.source_root != source_root
                    or set(source.allowed_grants) != set(allowed_grants)
                    or set(source.runtime_features) != set(required_features)):
                raise ProtocolError("capture source differs from trusted process configuration")
        mode = capture_modes.select_session(source, required_features)
        reference_admissions.require_provider(mode.reference_provider_id)
        batch_handlers.require_capture_feature(required_features, mode.reference_feature)
        if legacy_family is not None and mode.reference_family != legacy_family:
            raise ProtocolError("reference source differs from trusted capture mode")
        self.capture_source = source
        self.capture_mode = mode
        self.capture_modes = capture_modes
        self.reference_admissions = reference_admissions
        self.batch_handlers = batch_handlers
        if mode.reference_family == "content":
            self.content_source = source
        elif mode.reference_family == "command":
            self.command_source = source
        self.process = PluginProcess(manifest, source_id=source_id,
                                     allowed_grants=allowed_grants, timeout=timeout,
                                     source_root=source_root, required_features=required_features,
                                     rust_host_executable=rust_host_executable,
                                     rust_host_sha256=rust_host_sha256,
                                     content_refs_enabled=mode.reference_family == "content",
                                     batch_handlers=batch_handlers)
        self.lease: SourceLease | None = None
        self.cursor: str | None = None
        self.members: dict[str, str] | None = None
        self.projection_error: str | None = None
        self.last_diagnostic: dict = {}
        self.admission_port_close_error: str | None = None
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
        try:
            manifest = self.process.manifest
            fingerprint = (self.capture_mode.fingerprint(self.capture_source)
                           if self.capture_source is not None else "")
            if self.capture_source is not None:
                self.capture_modes.require_source(self.capture_mode, self.capture_source)
            self.admission_port.install_reference_schema(self.reference_admissions)
            self.lease = self.admission_port.activate_source(
                plugin_id=manifest.id, source_id=self.process.source_id,
                generation=manifest.artifact_sha256, content_fingerprint=fingerprint,
                reference_admissions=self.reference_admissions)
            self.cursor = self.lease.cursor
            self.process.start()
            if self.capture_source is not None:
                self.capture_grant = self.capture_mode.grant(self.capture_source, self.lease)
            if self.capture_mode.reference_family == "content":
                self.content_grant = self.capture_grant
            elif self.capture_mode.reference_family == "command":
                self.command_grant = self.capture_grant
            if "source-discovery/1" in self.process.required_features:
                self.members = self.admission_port.member_cursors(self.lease)
            self.last_diagnostic = self.admission_port.last_source_diagnostic(self.lease)
        except BaseException as startup_error:
            try:
                self.process.close(force=True)
            except BaseException:
                pass  # Preserve the startup error after best-effort cleanup.
            self.lease = None
            self.cursor = None
            self.members = None
            self.capture_grant = None
            self.content_grant = None
            self.command_grant = None
            try:
                self._close_admission_port()
            except Exception as close_error:
                startup_error.add_note(
                    f"admission port cleanup also failed: {type(close_error).__name__}")
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
        def trusted_submission(received):
            if isinstance(received, ContentPollBatch) and received.content_refs:
                if self.capture_mode.reference_family != "content":
                    raise ProtocolError("content references differ from trusted capture mode")
                return ReferenceSubmission(self.capture_mode.reference_provider_id,
                                           received.content_refs, self.capture_grant)
            if isinstance(received, CommandPollBatch) and received.command_refs:
                if self.capture_mode.reference_family != "command":
                    raise ProtocolError("command references differ from trusted capture mode")
                return ReferenceSubmission(self.capture_mode.reference_provider_id,
                                           received.command_refs, self.capture_grant)
            if isinstance(received, ReferencePollBatch):
                if (self.capture_mode.reference_family != "reference"
                        or received.reference_feature != self.capture_mode.reference_feature):
                    raise ProtocolError("references differ from trusted capture mode")
                if received.reference_refs:
                    return ReferenceSubmission(self.capture_mode.reference_provider_id,
                                               received.reference_refs, self.capture_grant)
            return None
        batch = self.process.poll_commit(expected, lambda received: self.admission_port.commit_batch(
            self.lease, expected, received,
            allow_root_gap="source-discovery-root-gap/1" in self.process.required_features,
            reference_submission=trusted_submission(received),
            reference_admissions=self.reference_admissions),
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
        try:
            self.process.close()
        finally:
            try:
                self._close_admission_port()
            finally:
                self.lease = None
                self.cursor = None
                self.members = None
                self.capture_grant = None
                self.content_grant = None
                self.command_grant = None

    def _close_admission_port(self) -> None:
        if self._admission_port_closed:
            return
        self._admission_port_closed = True
        try:
            self.admission_port.close()
        except Exception as exc:
            self.admission_port_close_error = type(exc).__name__
            raise

    def __enter__(self) -> PluginSession:
        return self.start()

    def __exit__(self, _type, _value, _traceback) -> None:
        self.close()
