"""Admission ports used by explicitly composed process plugin sessions.

The Python Store implementation is the production default. Other ports must
preserve the same lease, cursor, atomic receipt, replay and policy contracts.
Private reference admission remains an explicit capability of each backend.
"""
from __future__ import annotations

from typing import TYPE_CHECKING, Protocol, runtime_checkable

from .plugin_admission import (
    SourceLease,
    activate_source,
    commit_batch,
    last_source_diagnostic,
    member_cursors,
)
from .plugin_reference_admission import (
    DEFAULT_REFERENCE_ADMISSIONS,
    ReferenceAdmissionRegistry,
    ReferenceSubmission,
)
from .plugin_wire import PollBatch

if TYPE_CHECKING:
    from .core import Store


@runtime_checkable
class PluginAdmissionPort(Protocol):
    """Durable operations required by one process-backed plugin session."""

    def activate_source(self, *, plugin_id: str, source_id: str, generation: str,
                        content_fingerprint: str,
                        reference_admissions: ReferenceAdmissionRegistry) -> SourceLease:
        """Fence this artifact and return its committed cursor."""

    def install_reference_schema(
            self, reference_admissions: ReferenceAdmissionRegistry) -> None:
        """Install explicitly supplied trusted providers before activation."""

    def member_cursors(self, lease: SourceLease) -> dict[str, str]:
        """Read the durable per-member cursors for the current lease."""

    def last_source_diagnostic(self, lease: SourceLease) -> dict:
        """Read the latest policy-current source diagnostic."""

    def commit_batch(self, lease: SourceLease, expected_cursor: str,
                     batch: PollBatch, *, allow_root_gap: bool,
                     reference_submission: ReferenceSubmission | None,
                     reference_admissions: ReferenceAdmissionRegistry) -> str:
        """Atomically commit a batch and return its durable cursor."""

    def capture_revoked(self, source_id: str) -> bool:
        """Report whether the source is durably revoked in this receiver."""

    def classify_lease_failure(self, error: BaseException,
                               source_id: str) -> str | None:
        """Map backend lease failures to ``capture_revoked`` or ``source_fenced``."""

    def close(self) -> None:
        """Release backend resources owned by this session."""


class PythonStoreAdmissionPort:
    """Current Store-backed admission implementation and compatibility path."""

    def __init__(self, store: Store):
        if store is None:
            raise TypeError("Store required for Python plugin admission")
        self.store = store

    def activate_source(self, *, plugin_id: str, source_id: str, generation: str,
                        content_fingerprint: str = "",
                        reference_admissions: ReferenceAdmissionRegistry =
                        DEFAULT_REFERENCE_ADMISSIONS) -> SourceLease:
        return activate_source(
            self.store, plugin_id=plugin_id, source_id=source_id,
            generation=generation, content_fingerprint=content_fingerprint,
            reference_admissions=reference_admissions)

    def install_reference_schema(
            self, reference_admissions: ReferenceAdmissionRegistry) -> None:
        if reference_admissions is not DEFAULT_REFERENCE_ADMISSIONS:
            reference_admissions.install_schema(self.store)

    def member_cursors(self, lease: SourceLease) -> dict[str, str]:
        return member_cursors(self.store, lease)

    def last_source_diagnostic(self, lease: SourceLease) -> dict:
        return last_source_diagnostic(self.store, lease)

    def commit_batch(self, lease: SourceLease, expected_cursor: str,
                     batch: PollBatch, *, allow_root_gap: bool = False,
                     reference_submission: ReferenceSubmission | None = None,
                     reference_admissions: ReferenceAdmissionRegistry =
                     DEFAULT_REFERENCE_ADMISSIONS) -> str:
        return commit_batch(
            self.store, lease, expected_cursor, batch,
            allow_root_gap=allow_root_gap,
            reference_submission=reference_submission,
            reference_admissions=reference_admissions)

    def capture_revoked(self, source_id: str) -> bool:
        receiver_id = self.store.receiver_namespace()
        with self.store._connect() as db:
            row = db.execute(
                "SELECT capture_enabled FROM plugin_source_cursor_v1 "
                "WHERE source_id=? AND receiver_id=?",
                (source_id, receiver_id)).fetchone()
        return row is not None and not bool(row["capture_enabled"])

    def classify_lease_failure(self, error: BaseException,
                               source_id: str) -> str | None:
        from .plugin_wire import ProtocolError
        if not isinstance(error, ProtocolError):
            return None
        message = str(error)
        if "capture revoked" in message:
            return "capture_revoked"
        if "stale plugin source lease" in message:
            return ("capture_revoked" if self.capture_revoked(source_id)
                    else "source_fenced")
        return None

    def close(self) -> None:
        """The Store-backed port holds no independent resources."""


def require_plugin_admission_port(value) -> PluginAdmissionPort:
    if not isinstance(value, PluginAdmissionPort):
        raise TypeError("plugin admission port does not implement the trusted contract")
    return value
