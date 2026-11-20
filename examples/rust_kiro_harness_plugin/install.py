"""Install a locally built, pinned Rust Kiro reader for one named transcript.

Build: cargo build --release --locked -p agenturm-kiro-harness
Run: python install.py --binary /absolute/agenturm-kiro-harness
       --destination /absolute/plugin-dir --source /absolute/sess_*/messages.jsonl
The installer never builds, downloads, starts, or activates executable code.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
from pathlib import Path


def source_shape(path: Path) -> bool:
    if not path.is_absolute() or ".." in path.parts:
        return False
    if path.name == "messages.jsonl":
        session = path.parent.name
        worker = ""
    elif path.parent.name == "sub-executions" and path.suffix == ".jsonl":
        session = path.parent.parent.name
        worker = path.stem
    else:
        return False
    allowed = r"[A-Za-z0-9][A-Za-z0-9._-]{0,79}"
    return bool(session.startswith("sess_") and re.fullmatch(allowed, session)
                and (not worker or re.fullmatch(allowed, worker)))


def open_destination(path: Path) -> int:
    if not path.is_absolute() or ".." in path.parts:
        raise ValueError("invalid destination directory")
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    directory = os.open("/", flags)
    try:
        for component in path.parts[1:]:
            try:
                os.mkdir(component, mode=0o700, dir_fd=directory)
            except FileExistsError:
                pass
            child = os.open(component, flags, dir_fd=directory)
            os.close(directory)
            directory = child
        return directory
    except BaseException:
        os.close(directory)
        raise


def write_new(directory: int, name: str, raw: bytes, mode: int) -> None:
    descriptor = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                         mode, dir_fd=directory)
    with os.fdopen(descriptor, "wb") as stream:
        os.fchmod(stream.fileno(), mode)
        stream.write(raw)


def install(binary: Path, destination: Path, source: Path,
            source_id: str = "rust-kiro-source") -> Path:
    if os.name != "posix" or not all(path.is_absolute() for path in (binary, destination, source)):
        raise ValueError("POSIX and absolute binary, destination and source paths required")
    if not source_shape(source):
        raise ValueError("source must name Kiro messages.jsonl or a sub-execution JSONL")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,199}", source_id):
        raise ValueError("invalid source ID")
    fd = os.open(binary, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or not (info.st_mode & 0o111) or info.st_size > 64 * 1024 * 1024:
            raise ValueError("binary must be an executable regular file within the size limit")
        with os.fdopen(fd, "rb", closefd=False) as stream:
            raw = stream.read(64 * 1024 * 1024 + 1)
        if len(raw) > 64 * 1024 * 1024:
            raise ValueError("binary exceeds size limit")
    finally:
        os.close(fd)
    artifact = destination / "rust-kiro-harness"
    manifest = destination / "rust-kiro-harness.toml"
    config = destination / "plugins.toml"
    manifest_raw = (
        'id = "example.rust-kiro-harness"\nversion = "0.1.0"\nprotocol = "1"\n'
        f'entrypoint = [{json.dumps(str(artifact))}]\n'
        f'artifact_sha256 = "{hashlib.sha256(raw).hexdigest()}"\n'
        'provides = ["source-adapter/1", "configured-root/1"]\n'
        'requires = ["evidence-ingest/1"]\n'
        'depends_on = []\ngrants = ["configured-source-read"]\n').encode()
    config_raw = (
        'schema = 1\n[[sources]]\n'
        f'manifest = {json.dumps(str(manifest))}\n'
        f'source_id = {json.dumps(source_id)}\n'
        'grants = ["configured-source-read"]\n'
        'capture = "metadata-only"\npoll_interval = 0.5\ntimeout = 5.0\n'
        f'root = {json.dumps(str(source))}\n').encode()
    directory = open_destination(destination)
    try:
        for name in (artifact.name, manifest.name, config.name):
            try:
                os.stat(name, dir_fd=directory, follow_symlinks=False)
            except FileNotFoundError:
                continue
            raise ValueError("destination already contains Kiro plugin files")
        write_new(directory, artifact.name, raw, 0o700)
        write_new(directory, manifest.name, manifest_raw, 0o600)
        write_new(directory, config.name, config_raw, 0o600)
    finally:
        os.close(directory)
    return config


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--destination", required=True, type=Path)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--source-id", default="rust-kiro-source")
    args = parser.parse_args()
    try:
        print(install(args.binary, args.destination, args.source, args.source_id))
    except (OSError, ValueError) as exc:
        parser.error(str(exc))
