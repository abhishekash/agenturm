"""Install the opt-in declarative count capability fixture."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path


def _directory(path: Path) -> int:
    if not path.is_absolute() or ".." in path.parts:
        raise ValueError("invalid destination directory")
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    fd = os.open("/", flags)
    try:
        for component in path.parts[1:]:
            try:
                os.mkdir(component, mode=0o700, dir_fd=fd)
            except FileExistsError:
                pass
            child = os.open(component, flags, dir_fd=fd)
            os.close(fd)
            fd = child
        result, fd = fd, -1
        return result
    finally:
        if fd >= 0:
            os.close(fd)


def _new(directory: int, name: str, raw: bytes, mode: int) -> None:
    fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                 mode, dir_fd=directory)
    with os.fdopen(fd, "wb") as stream:
        os.fchmod(stream.fileno(), mode)
        stream.write(raw)


def install(destination: Path) -> Path:
    if os.name != "posix" or not destination.is_absolute():
        raise ValueError("fixture requires a POSIX absolute destination")
    artifact = destination / "count-observations"
    manifest = destination / "count-observations.toml"
    config = destination / "capability.json"
    raw = Path(__file__).with_name("agenturm_count_observations.py").read_bytes()
    manifest_raw = (
        'id = "example.count-observations-process"\nversion = "0.1.0"\nprotocol = "1"\n'
        f'entrypoint = [{json.dumps(str(artifact))}]\n'
        f'artifact_sha256 = "{hashlib.sha256(raw).hexdigest()}"\n'
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
    directory = _directory(destination)
    try:
        for name in (artifact.name, manifest.name, config.name):
            try:
                os.stat(name, dir_fd=directory, follow_symlinks=False)
            except FileNotFoundError:
                continue
            raise ValueError("destination already contains capability files")
        _new(directory, artifact.name, raw, 0o700)
        _new(directory, manifest.name, manifest_raw, 0o600)
        _new(directory, config.name,
             json.dumps(descriptor, separators=(",", ":")).encode(), 0o600)
    finally:
        os.close(directory)
    return config


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    try:
        print(install(args.destination))
    except (OSError, ValueError) as exc:
        parser.error(str(exc))
