"""Explicit, opt-in composition of a process adapter and the durable inbox.

One session keeps one worker alive across polls. A caller owns scheduling and
must create a fresh session after a crash; activation resumes the Store cursor.
No default collector instantiates this class yet.
"""
from __future__ import annotations

from typing import TYPE_CHECKING

from .plugin_admission import SourceLease, activate_source, commit_batch
from .plugin_manifest import PluginManifest
from .plugin_process import PluginProcess
from .plugin_wire import PollBatch, ProtocolError

if TYPE_CHECKING:
    from .core import Store


class PluginSession:
    def __init__(self, store: Store, manifest: PluginManifest, *, source_id: str,
                 allowed_grants: tuple[str, ...] = (), timeout: float = 5.0):
        if not isinstance(manifest, PluginManifest):
            raise TypeError("plugin manifest required")
        self.store = store
        self.process = PluginProcess(manifest, source_id=source_id,
                                     allowed_grants=allowed_grants, timeout=timeout)
        self.lease: SourceLease | None = None
        self.cursor: str | None = None

    def start(self) -> PluginSession:
        if self.lease is not None:
            raise ProtocolError("plugin session already started")
        manifest = self.process.manifest
        self.lease = activate_source(self.store, plugin_id=manifest.id,
                                     source_id=self.process.source_id,
                                     generation=manifest.artifact_sha256)
        self.cursor = self.lease.cursor
        try:
            self.process.start()
        except BaseException:
            self.lease = None
            self.cursor = None
            raise
        return self

    def poll_once(self) -> PollBatch:
        if self.lease is None or self.cursor is None:
            raise ProtocolError("plugin session is not running")
        expected = self.cursor
        batch = self.process.poll_commit(expected, lambda received: commit_batch(
            self.store, self.lease, expected, received))
        self.cursor = batch.cursor
        return batch

    def close(self) -> None:
        self.process.close()
        self.lease = None
        self.cursor = None

    def __enter__(self) -> PluginSession:
        return self.start()

    def __exit__(self, _type, _value, _traceback) -> None:
        self.close()
