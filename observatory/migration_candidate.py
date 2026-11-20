"""Explicit offline producer for Rust-verifiable, detached migration candidates.

This is not called by the receiver and does not establish writer quiescence.
The operator must stop all writers before invoking it.
"""

from __future__ import annotations

from contextlib import ExitStack, closing, contextmanager
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import stat
import subprocess
from typing import Iterator


class CandidateError(ValueError):
    pass


_MAX_BACKUP_BYTES = 4 * 1024 * 1024 * 1024
_SOURCE_NAMES = ("telemetry", "workflow", "tasks")
_VERSIONS = (1, 2, 1)


def _private_directory(path: Path) -> None:
    try:
        info = path.lstat()
        if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid()
                or info.st_mode & 0o077 or path.resolve(strict=True) != path):
            raise CandidateError(f"private real directory required: {path}")
    except OSError as exc:
        raise CandidateError(f"directory unavailable: {path}") from exc


def _absolute(path: os.PathLike[str] | str) -> Path:
    result = Path(path)
    if not result.is_absolute() or result != result.resolve(strict=False):
        raise CandidateError("paths must be absolute with no symbolic-link components")
    return result


def _sidecars(path: Path) -> bool:
    return any(os.path.lexists(f"{path}{suffix}") for suffix in ("-wal", "-shm", "-journal"))


def _digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            hasher.update(block)
    return hasher.hexdigest()


def _identity(info: os.stat_result) -> tuple[int, ...]:
    return info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns


def _digest_fd(fd: int) -> str:
    hasher = hashlib.sha256()
    offset = 0
    while block := os.pread(fd, 1024 * 1024, offset):
        hasher.update(block)
        offset += len(block)
    return hasher.hexdigest()


@contextmanager
def _stopped_source(path: Path) -> Iterator[dict[str, int]]:
    """Hold the main file and extant WAL/SHM for read-only copy and recheck."""
    _private_directory(path.parent)
    if os.path.lexists(f"{path}-journal"):
        raise CandidateError(f"source has a rollback journal: {path}")
    flags = os.O_RDONLY | os.O_CLOEXEC | getattr(os, "O_NOFOLLOW", 0)
    with ExitStack() as stack:
        files: dict[str, int] = {}
        before: dict[str, tuple[tuple[int, ...], str]] = {}
        for suffix in ("", "-wal", "-shm"):
            named_path = Path(f"{path}{suffix}")
            if suffix and not os.path.lexists(named_path):
                continue
            fd = os.open(named_path, flags)
            stack.callback(os.close, fd)
            opened = os.fstat(fd)
            named = named_path.lstat()
            if (not stat.S_ISREG(opened.st_mode) or opened.st_uid != os.geteuid()
                    or opened.st_mode & 0o077 or opened.st_nlink != 1
                    or _identity(opened) != _identity(named)
                    or opened.st_size > _MAX_BACKUP_BYTES
                    or (not suffix and opened.st_size == 0)):
                raise CandidateError(f"private regular source required: {named_path}")
            files[suffix] = fd
            before[suffix] = _identity(opened), _digest_fd(fd)
        if ("-shm" in files) != ("-wal" in files):
            raise CandidateError(f"incomplete source WAL pair: {path}")
        yield files
        if os.path.lexists(f"{path}-journal"):
            raise CandidateError(f"source journal appeared during preparation: {path}")
        for suffix in ("", "-wal", "-shm"):
            named_path = Path(f"{path}{suffix}")
            if os.path.lexists(named_path) != (suffix in files):
                raise CandidateError(f"source sidecars changed during preparation: {path}")
            if suffix in files:
                opened = os.fstat(files[suffix])
                named = named_path.lstat()
                if (_identity(opened) != _identity(named)
                        or (_identity(opened), _digest_fd(files[suffix])) != before[suffix]):
                    raise CandidateError(f"source changed during preparation: {named_path}")


def _copy_fd(fd: int, destination: Path) -> None:
    copied = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC, 0o600)
    try:
        offset = 0
        while block := os.pread(fd, 1024 * 1024, offset):
            view = memoryview(block)
            while view:
                view = view[os.write(copied, view):]
            offset += len(block)
        os.fsync(copied)
    finally:
        os.close(copied)


def _backup(source_path: Path, destination: Path) -> None:
    """Use SQLite online backup on a private clone, never on the canonical files."""
    created = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC, 0o600)
    os.close(created)
    uri = source_path.as_uri() + "?mode=ro"
    with closing(sqlite3.connect(uri, uri=True, timeout=5)) as source:
        with closing(sqlite3.connect(destination, timeout=5)) as target:
            source.backup(target)
    if _sidecars(destination):
        raise CandidateError("detached backup has SQLite sidecars")
    with destination.open("rb") as stream:
        os.fsync(stream.fileno())


def _descriptor(path: Path, version: int) -> dict:
    info = path.lstat()
    if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid()
            or info.st_nlink != 1 or info.st_mode & 0o077
            or not 0 < info.st_size <= _MAX_BACKUP_BYTES or _sidecars(path)):
        raise CandidateError("invalid detached backup")
    return dict(path=str(path), device=info.st_dev, inode=info.st_ino,
                size_bytes=info.st_size, sha256=_digest(path), schema_version=version)


@contextmanager
def _read_backup(path: Path) -> Iterator[sqlite3.Connection]:
    with closing(sqlite3.connect(path.as_uri() + "?mode=ro&immutable=1", uri=True)) as db:
        db.row_factory = sqlite3.Row
        db.execute("PRAGMA query_only=ON")
        yield db


def _receipt(cursor: str | None, digest: str | None) -> dict | None:
    return dict(cursor=cursor, refs_digest=digest) if cursor is not None and digest is not None else None


def _snapshot(paths: dict[str, Path]) -> tuple[str, dict]:
    with _read_backup(paths["telemetry"]) as db:
        receiver = db.execute("SELECT value FROM schema_meta WHERE key='receiver_id'").fetchone()[0]
        schema = db.execute("SELECT value FROM schema_meta WHERE key='plugin_sessions_read_schema'").fetchone()[0]
        if schema != "1":
            raise CandidateError("unsupported telemetry read schema")
        rows = db.execute("""SELECT s.source_id,s.receiver_id,s.cursor,s.lease_epoch,s.policy_epoch,
                s.dirty_seq,s.last_batch_digest,c.cursor AS content_cursor,c.refs_digest AS content_digest,
                k.cursor AS command_cursor,k.refs_digest AS command_digest
                FROM plugin_source_cursor_v1 s
                LEFT JOIN plugin_content_batch_v1 c USING(source_id)
                LEFT JOIN plugin_command_batch_v1 k USING(source_id)
                ORDER BY s.source_id LIMIT 4097""").fetchall()
        if len(rows) > 4096 or any(row["receiver_id"] != receiver for row in rows):
            raise CandidateError("source cursor receiver or count mismatch")
        sources = [dict(source_id=row["source_id"], cursor=row["cursor"],
                        lease_epoch=row["lease_epoch"], policy_epoch=row["policy_epoch"],
                        dirty_seq=row["dirty_seq"], last_batch_digest=row["last_batch_digest"],
                        content_receipt=_receipt(row["content_cursor"], row["content_digest"]),
                        command_receipt=_receipt(row["command_cursor"], row["command_digest"]))
                   for row in rows]
    with _read_backup(paths["workflow"]) as db:
        workflow_notification_claims = db.execute(
            "SELECT COUNT(*) FROM workflow_notification_claims_v1").fetchone()[0]
        workflow_native_events = db.execute(
            "SELECT COUNT(*) FROM workflow_native_events_v1").fetchone()[0]
    with _read_backup(paths["tasks"]) as db:
        saved_tasks = db.execute("SELECT COUNT(*) FROM saved_tasks_v1").fetchone()[0]
        max_task_revision = db.execute(
            "SELECT COALESCE(MAX(revision),0) FROM saved_tasks_v1").fetchone()[0]
    snapshot = dict(sources=sources, workflow_notification_claims=workflow_notification_claims,
                    workflow_native_events=workflow_native_events, saved_tasks=saved_tasks,
                    max_task_revision=max_task_revision)
    return receiver, snapshot


def _write_private(path: Path, payload: bytes) -> None:
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(payload)
        stream.flush()
        os.fsync(stream.fileno())


def _sync_directory(path: Path) -> None:
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def prepare_candidate(*, telemetry: os.PathLike[str] | str,
                      workflow: os.PathLike[str] | str,
                      tasks: os.PathLike[str] | str,
                      output: os.PathLike[str] | str,
                      verifier: os.PathLike[str] | str) -> Path:
    """Produce and verify a historical candidate, never a cutover certificate.

    All sources must be stopped and private. Any failure leaves
    the new output directory marked incomplete rather than a valid candidate.
    """
    if os.name != "posix" or not Path("/proc/self/fd").is_dir():
        raise CandidateError("descriptor-bound offline preparation requires Linux procfs")
    sources = dict(zip(_SOURCE_NAMES, map(_absolute, (telemetry, workflow, tasks))))
    destination = _absolute(output)
    binary = _absolute(verifier)
    _private_directory(destination.parent)
    if destination.exists() or destination.is_symlink():
        raise CandidateError("output directory already exists")
    if len(set(sources.values())) != 3 or destination in sources.values():
        raise CandidateError("source and output paths must be distinct")
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise CandidateError("trusted Rust verifier binary unavailable")
    # Preflight every source before creating any output. The per-source guard
    # checks again while main/WAL bytes are copied from held descriptors.
    for path in sources.values():
        with _stopped_source(path):
            pass
    destination.mkdir(mode=0o700)
    _sync_directory(destination.parent)
    state = destination / "state.json"
    _write_private(state, b'{"format":"migration_candidate_preparation_v1","state":"incomplete"}\n')
    _sync_directory(destination)
    paths: dict[str, Path] = {}
    descriptors: dict[str, dict] = {}
    for name, version in zip(_SOURCE_NAMES, _VERSIONS):
        path = destination / f"{name}.sqlite3"
        clone = destination / f".{name}.source.sqlite3"
        with _stopped_source(sources[name]) as files:
            try:
                _copy_fd(files[""], clone)
                if "-wal" in files:
                    _copy_fd(files["-wal"], Path(f"{clone}-wal"))
                _backup(clone, path)
            finally:
                for suffix in ("", "-wal", "-shm", "-journal"):
                    Path(f"{clone}{suffix}").unlink(missing_ok=True)
        paths[name] = path
        descriptors[name] = _descriptor(path, version)
    receiver, snapshot = _snapshot(paths)
    manifest = dict(format="migration_manifest_v1", receiver_id=receiver,
                    telemetry=descriptors["telemetry"], workflow=descriptors["workflow"],
                    tasks=descriptors["tasks"], snapshot=snapshot)
    manifest_path = destination / "manifest.json"
    _write_private(manifest_path, (json.dumps(manifest, separators=(",", ":")) + "\n").encode())
    _sync_directory(destination)
    result = subprocess.run((str(binary), str(manifest_path)), text=True,
                            capture_output=True, timeout=300, check=False)
    if result.returncode != 0:
        raise CandidateError("Rust migration candidate verification failed: " + result.stderr[:400])
    try:
        reply = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise CandidateError("invalid Rust verifier reply") from exc
    if reply != {"format": "migration_candidate_verified_v1", "receiver_id": receiver}:
        raise CandidateError("Rust verifier returned an unexpected receiver or format")
    next_state = destination / ".verified-state.tmp"
    _write_private(next_state,
                   b'{"format":"migration_candidate_preparation_v1","state":"verified"}\n')
    os.replace(next_state, state)
    _sync_directory(destination)
    return manifest_path


def main() -> None:
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("telemetry", "workflow", "tasks", "output", "verifier"):
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    try:
        path = prepare_candidate(**vars(args))
    except (CandidateError, OSError, sqlite3.Error, subprocess.TimeoutExpired) as exc:
        parser.exit(1, f"offline candidate preparation failed: {exc}\n")
    print(path)


if __name__ == "__main__":
    main()
