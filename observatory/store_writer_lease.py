"""Opt-in Linux process lease for cooperating telemetry Store writers.

This is an operational fence for new Agenturm processes, not an authority or
legacy-process exclusion mechanism. A held lease is deliberately retained if
Store shutdown cannot prove worker, SQLite connection, and read-lease
quiescence.
"""
from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
import stat
import sys
import threading


class StoreWriterLeaseError(RuntimeError):
    """A cooperating telemetry writer could not safely acquire/admit work."""


@dataclass
class _LeaseState:
    key: tuple[int, str]
    path: Path
    database_resolved: Path
    database_identity: tuple[int, int] | None
    parent_identity: tuple[int, int]
    fd: int
    identity: tuple[int, int]
    references: int = 1

    def verify(self) -> None:
        """Prove the held descriptor and names still describe this lease."""
        try:
            opened = os.fstat(self.fd)
            named = self.path.lstat()
            parent = self.path.parent.stat()
            if (not stat.S_ISREG(opened.st_mode) or not stat.S_ISREG(named.st_mode)
                    or (opened.st_dev, opened.st_ino) != self.identity
                    or (named.st_dev, named.st_ino) != self.identity
                    or opened.st_nlink != 1 or named.st_nlink != 1
                    or opened.st_uid != os.geteuid() or named.st_uid != os.geteuid()
                    or stat.S_IMODE(opened.st_mode) != 0o600
                    or stat.S_IMODE(named.st_mode) != 0o600
                    or (parent.st_dev, parent.st_ino) != self.parent_identity):
                raise StoreWriterLeaseError("telemetry writer lock pathname changed")
            try:
                current = self.database_resolved.stat()
            except FileNotFoundError:
                if self.database_identity is not None:
                    raise StoreWriterLeaseError("telemetry database target disappeared")
            else:
                current_identity = (current.st_dev, current.st_ino)
                if (not stat.S_ISREG(current.st_mode) or current.st_nlink != 1
                        or self.database_identity is not None
                        and current_identity != self.database_identity):
                    raise StoreWriterLeaseError("telemetry database target changed")
        except StoreWriterLeaseError:
            raise
        except OSError as exc:
            raise StoreWriterLeaseError("telemetry writer lock or target unavailable") from exc

    def bind_database_identity(self, identity: tuple[int, int] | None) -> None:
        """Bind a just-created DB after Store's O_EXCL creation proof."""
        self.verify()
        if self.database_identity is not None:
            if identity != self.database_identity:
                raise StoreWriterLeaseError("telemetry database identity changed")
        elif identity is not None:
            try:
                current = self.database_resolved.stat()
            except OSError as exc:
                raise StoreWriterLeaseError("telemetry database target unavailable") from exc
            if (current.st_dev, current.st_ino) != identity:
                raise StoreWriterLeaseError("telemetry database target changed")
            self.database_identity = identity


_REGISTRY_LOCK = threading.RLock()
_REGISTRY: dict[tuple[int, str], _LeaseState] = {}


def _after_fork_child() -> None:
    """Drop inherited child copies without unlocking the parent's open file."""
    global _REGISTRY_LOCK, _REGISTRY
    for state in tuple(_REGISTRY.values()):
        try:
            os.close(state.fd)
        except OSError:
            pass
    _REGISTRY = {}
    _REGISTRY_LOCK = threading.RLock()


if hasattr(os, "register_at_fork"):
    os.register_at_fork(after_in_child=_after_fork_child)


class StoreWriterLease:
    """One reference to a process-shared lock for a telemetry DB target."""

    __slots__ = ("_state", "_database_lexical", "_database_identity_at_acquire", "_released")

    def __init__(self, state: _LeaseState, database_lexical: Path,
                 database_identity_at_acquire: tuple[int, int] | None):
        self._state = state
        self._database_lexical = database_lexical
        self._database_identity_at_acquire = database_identity_at_acquire
        self._released = False

    @classmethod
    def acquire(cls, database: str | os.PathLike[str]) -> "StoreWriterLease":
        if not sys.platform.startswith("linux"):
            raise StoreWriterLeaseError("telemetry writer lease is Linux-only")
        import fcntl

        lexical = Path(database).absolute()
        try:
            resolved = lexical.resolve(strict=False)
            parent = resolved.parent
            parent_info = parent.stat()
            if (not stat.S_ISDIR(parent_info.st_mode)
                    or parent.resolve(strict=True) != parent
                    or parent_info.st_uid != os.geteuid()
                    or stat.S_IMODE(parent_info.st_mode) & 0o077):
                raise StoreWriterLeaseError(
                    "opt-in telemetry lease requires an existing private database directory")
            try:
                target_info = lexical.stat()
            except FileNotFoundError:
                target_identity = None
            else:
                if not stat.S_ISREG(target_info.st_mode) or target_info.st_nlink != 1:
                    raise StoreWriterLeaseError("telemetry database target must be a single-link regular file")
                target_identity = (target_info.st_dev, target_info.st_ino)
            lock_path = parent / f".{resolved.name}.agenturm-writer.lock"
        except StoreWriterLeaseError:
            raise
        except OSError as exc:
            raise StoreWriterLeaseError("telemetry writer lease path unavailable") from exc

        key = (os.getpid(), str(lock_path))
        with _REGISTRY_LOCK:
            existing = _REGISTRY.get(key)
            if existing is not None:
                existing.verify()
                if (existing.database_resolved != resolved
                        or existing.database_identity != target_identity):
                    raise StoreWriterLeaseError("telemetry writer target differs from held lease")
                lease = cls(existing, lexical, target_identity)
                lease.verify()
                existing.references += 1
                return lease

            flags = (os.O_RDWR | os.O_CREAT | getattr(os, "O_CLOEXEC", 0)
                     | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0))
            try:
                fd = os.open(lock_path, flags, 0o600)
            except OSError as exc:
                raise StoreWriterLeaseError("telemetry writer lock unavailable") from exc
            try:
                opened = os.fstat(fd)
                try:
                    named = lock_path.lstat()
                except OSError as exc:
                    raise StoreWriterLeaseError("telemetry writer lock pathname unavailable") from exc
                if (not stat.S_ISREG(opened.st_mode) or not stat.S_ISREG(named.st_mode)
                        or opened.st_nlink != 1 or named.st_nlink != 1
                        or opened.st_uid != os.geteuid() or named.st_uid != os.geteuid()
                        or stat.S_IMODE(opened.st_mode) != 0o600
                        or stat.S_IMODE(named.st_mode) != 0o600
                        or (opened.st_dev, opened.st_ino) != (named.st_dev, named.st_ino)):
                    raise StoreWriterLeaseError("telemetry writer lock is not private and stable")
                try:
                    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError as exc:
                    raise StoreWriterLeaseError("another cooperating telemetry writer is active") from exc
                after_open = os.fstat(fd)
                after_name = lock_path.lstat()
                if ((after_open.st_dev, after_open.st_ino)
                        != (after_name.st_dev, after_name.st_ino)
                        or (after_open.st_dev, after_open.st_ino)
                        != (opened.st_dev, opened.st_ino)):
                    raise StoreWriterLeaseError("telemetry writer lock changed while acquiring")
                state = _LeaseState(
                    key=key, path=lock_path, database_resolved=resolved,
                    database_identity=target_identity,
                    parent_identity=(parent_info.st_dev, parent_info.st_ino), fd=fd,
                    identity=(opened.st_dev, opened.st_ino))
                state.verify()
                lease = cls(state, lexical, target_identity)
                lease.verify()
                _REGISTRY[key] = state
                return lease
            except BaseException:
                try:
                    os.close(fd)
                except OSError:
                    pass
                raise

    def verify(self) -> None:
        if self._released:
            raise StoreWriterLeaseError("telemetry writer lease has been released")
        self._state.verify()
        try:
            if self._database_lexical.resolve(strict=False) != self._state.database_resolved:
                raise StoreWriterLeaseError("telemetry database target changed")
            current = self._database_lexical.stat()
        except FileNotFoundError:
            if self._state.database_identity is not None:
                raise StoreWriterLeaseError("telemetry database target disappeared")
        except StoreWriterLeaseError:
            raise
        except OSError as exc:
            raise StoreWriterLeaseError("telemetry database target unavailable") from exc
        else:
            identity = (current.st_dev, current.st_ino)
            if (not stat.S_ISREG(current.st_mode) or current.st_nlink != 1
                    or self._state.database_identity is not None
                    and identity != self._state.database_identity):
                raise StoreWriterLeaseError("telemetry database target changed")

    def bind_database_identity(self, identity: tuple[int, int] | None) -> None:
        self.verify()
        self._state.bind_database_identity(identity)

    @property
    def database_missing_at_acquire(self) -> bool:
        return self._database_identity_at_acquire is None

    @property
    def held(self) -> bool:
        return not self._released

    def release_after_quiescence(self) -> bool:
        """Release one ref only after the caller proves Store quiescence."""
        if self._released:
            return True
        with _REGISTRY_LOCK:
            state = self._state
            if _REGISTRY.get(state.key) is not state:
                raise StoreWriterLeaseError("telemetry writer lease registry changed")
            if state.references > 1:
                state.references -= 1
                self._released = True
                return True
            # Keep the registry's strong reference until the descriptor is
            # closed. Closing is the actual process-lock release operation.
            try:
                os.close(state.fd)
            except OSError:
                # Do not drop the registry entry. The OS will release it when
                # this process exits; a caller may retry cleanup.
                return False
            _REGISTRY.pop(state.key, None)
            state.references = 0
            self._released = True
            return True


def retained_lease_count() -> int:
    """Diagnostic count of process-held lease states, including failed drains."""
    with _REGISTRY_LOCK:
        return len(_REGISTRY)
