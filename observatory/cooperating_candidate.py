"""Feature-off candidate preparation from explicitly cooperating store instances.

The caller must separately exclude other instances, direct SQLite users, and
legacy processes. Handoff fences are one-way and are never released here.
"""
from __future__ import annotations

import os
from pathlib import Path
import time

from .core import Store
from .migration_candidate import CandidateError, prepare_candidate
from .outbound_delivery import OutboundQueue
from .saved_tasks import TaskStore
from .workflow_store import WorkflowStore


def _invalidate_prepared(directory: Path) -> None:
    """A post-backup guard failure must not leave a verified handoff result."""
    marker = directory / "state.json"
    replacement = directory / ".coordinator-incomplete.tmp"
    try:
        fd = os.open(replacement, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC, 0o600)
        with os.fdopen(fd, "wb") as stream:
            stream.write(b'{"format":"migration_candidate_preparation_v1","state":"incomplete"}\n')
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(replacement, marker)
        fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
    except BaseException as exc:
        # A missing marker is always incomplete. If even removal fails, the
        # operator must quarantine this candidate before inspecting it.
        try:
            marker.unlink(missing_ok=True)
        except BaseException as removal_error:
            raise CandidateError(
                f"candidate handoff failed; quarantine {directory} manually") from removal_error
        raise CandidateError(
            f"candidate handoff failed; incomplete state may need quarantine at {directory}") from exc


def prepare_cooperating_candidate(*, telemetry: Store, workflow: WorkflowStore,
                                  tasks: TaskStore, outbound: OutboundQueue,
                                  output: os.PathLike[str] | str,
                                  verifier: os.PathLike[str] | str,
                                  timeout: float) -> Path:
    """Fence supplied instances and prepare a detached historical candidate.

    This coordinates only these object instances and cooperating outbound
    dispatchers on the telemetry path. It never transfers writer authority.
    A failed drain leaves any already-requested handoff fences in place.
    """
    if not isinstance(telemetry, Store) or not isinstance(workflow, WorkflowStore):
        raise TypeError("live telemetry and workflow stores required")
    if not isinstance(tasks, TaskStore) or not isinstance(outbound, OutboundQueue):
        raise TypeError("live task store and outbound queue required")
    if outbound.store is not telemetry:
        raise CandidateError("outbound queue must use the supplied telemetry Store")
    if type(timeout) not in (int, float) or not 0 < timeout <= 60:
        raise ValueError("cooperating candidate timeout must be above 0 and at most 60 seconds")
    deadline = time.monotonic() + timeout

    def remaining() -> float:
        left = deadline - time.monotonic()
        if left <= 0:
            raise CandidateError("cooperating candidate handoff deadline expired")
        return left

    # Dispatch can remain outside a Store connection while its external call
    # runs. Hold this lock for the entire drain and backup, before asking Store
    # to reject new connections. Grant/revoke/enqueue are Store operations.
    prepared: Path | None = None
    try:
        with outbound.dispatch_handoff_guard(remaining()) as guard:
            for label, store in (("workflow", workflow), ("tasks", tasks),
                                 ("telemetry", telemetry)):
                if not store.quiesce_for_writer_handoff(remaining()):
                    raise CandidateError(f"{label} store did not quiesce for candidate preparation")
            guard.verify()
            if time.monotonic() > deadline:
                raise CandidateError("cooperating candidate handoff deadline expired")
            prepared = prepare_candidate(telemetry=telemetry.path, workflow=workflow.path,
                                         tasks=tasks.path, output=output, verifier=verifier)
            guard.verify()
        return prepared
    except BaseException:
        if prepared is not None:
            _invalidate_prepared(prepared.parent)
        raise
