"""Install an already-built Rust Count capability binary; never invokes Cargo."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
from pathlib import Path

from examples.count_observations_process.install import _directory


def _metadata(destination: Path, artifact_sha256: str) -> tuple[bytes, bytes]:
    artifact = destination / "count-observations"
    manifest = destination / "count-observations.toml"
    config = destination / "capability.json"
    manifest_raw = (
        'id = "example.count-observations-process"\nversion = "0.1.0"\nprotocol = "1"\n'
        f'entrypoint = [{json.dumps(str(artifact))}]\n'
        f'artifact_sha256 = "{artifact_sha256}"\n'
        'provides = ["capability-adapter/1"]\nrequires = ["pane-model/1"]\n'
        'depends_on = []\ngrants = ["scoped-aggregate-read"]\n').encode()
    descriptor = {"schema": 3, "manifest": str(manifest),
        "pane": {"id": "count-observations", "title": "Count observations",
                 "columns": [["Signal", 34, 2], ["Observed", 12, 0]]},
        "facts": {"counts": [
            {"key": "users", "label": "User messages", "domain": "message", "action": "user"},
            {"key": "shell", "label": "Shell actions", "domain": "shell", "action": "execute"},
            {"key": "checks", "label": "Failed checks", "domain": "check", "action": "failed"}]},
        "model": {"rows": [
            {"key": "users", "value": {"count": "users"}},
            {"key": "checks", "value": {"count": "checks"}},
            {"key": "shell-per-user", "value": {"ratio": ["shell", "users"]}}]},
        "grants": ["scoped-aggregate-read"]}
    return manifest_raw, json.dumps(descriptor, separators=(",", ":")).encode()


def _write_new(directory: int, name: str, raw: bytes) -> None:
    fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                 0o600, dir_fd=directory)
    try:
        with os.fdopen(fd, "wb") as output:
            fd = -1
            os.fchmod(output.fileno(), 0o600)
            output.write(raw)
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        if fd >= 0:
            os.close(fd)
        try:
            os.unlink(name, dir_fd=directory)
        except FileNotFoundError:
            pass
        raise


def _open_source(source: Path) -> int:
    if not source.is_absolute() or ".." in source.parts or not source.name:
        raise ValueError("binary path must be absolute and contain no parent traversal")
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    parent_fd = os.open("/", flags)
    try:
        for component in source.parts[1:-1]:
            child_fd = os.open(component, flags, dir_fd=parent_fd)
            os.close(parent_fd)
            parent_fd = child_fd
        return os.open(source.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC,
                       dir_fd=parent_fd)
    finally:
        os.close(parent_fd)


def _copy_executable(source: Path, directory: int, name: str) -> str:
    source_fd = _open_source(source)
    output_fd = -1
    created = False
    digest = hashlib.sha256()
    copied = 0
    try:
        source_stat = os.fstat(source_fd)
        if (not stat.S_ISREG(source_stat.st_mode) or source_stat.st_size == 0
                or not source_stat.st_mode & 0o111):
            raise ValueError("binary must be a nonempty executable regular file")
        output_fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                            0o700, dir_fd=directory)
        created = True
        with os.fdopen(output_fd, "wb") as output:
            output_fd = -1
            os.fchmod(output.fileno(), 0o700)
            while True:
                chunk = os.read(source_fd, 1024 * 1024)
                if not chunk:
                    break
                output.write(chunk)
                digest.update(chunk)
                copied += len(chunk)
            if copied != source_stat.st_size:
                raise ValueError("binary changed while it was being copied")
            output.flush()
            os.fsync(output.fileno())
        return digest.hexdigest()
    except BaseException:
        if created:
            try:
                os.unlink(name, dir_fd=directory)
            except FileNotFoundError:
                pass
        raise
    finally:
        if output_fd >= 0:
            os.close(output_fd)
        os.close(source_fd)


def install(binary: Path, destination: Path) -> Path:
    """Copy a built executable and create its pinned manifest and descriptor."""
    if os.name != "posix" or not destination.is_absolute():
        raise ValueError("fixture requires a POSIX absolute destination")
    artifact = destination / "count-observations"
    manifest = destination / "count-observations.toml"
    config = destination / "capability.json"
    directory = _directory(destination)
    created: list[str] = []
    try:
        for name in (artifact.name, manifest.name, config.name):
            try:
                os.stat(name, dir_fd=directory, follow_symlinks=False)
            except FileNotFoundError:
                continue
            raise ValueError("destination already contains capability files")

        digest = _copy_executable(binary, directory, artifact.name)
        created.append(artifact.name)
        manifest_raw, config_raw = _metadata(destination, digest)
        _write_new(directory, manifest.name, manifest_raw)
        created.append(manifest.name)
        _write_new(directory, config.name, config_raw)
        created.append(config.name)
        os.fsync(directory)
        return config
    except BaseException:
        for name in reversed(created):
            try:
                os.unlink(name, dir_fd=directory)
            except FileNotFoundError:
                pass
        raise
    finally:
        os.close(directory)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True,
                        help="already-built executable; Cargo is not invoked")
    parser.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    try:
        print(install(args.binary, args.destination))
    except (OSError, ValueError) as exc:
        parser.error(str(exc))
