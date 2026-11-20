"""Install a locally built, pinned Rust OpenCode reader for one SQLite DB.

Build: cargo build --release --locked -p agenturm-opencode-harness
Run: python install.py --binary /absolute/agenturm-opencode-harness
       --destination /absolute/plugin-dir --source /absolute/opencode.db
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


def components(path: Path) -> tuple[str, ...]:
    if not path.is_absolute() or ".." in path.parts or len(str(path)) > 1024:
        raise ValueError("absolute path without parent traversal required")
    return path.parts[1:]


def open_file(path: Path) -> int:
    parts = components(path)
    if not parts:
        raise ValueError("regular file path required")
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    directory = os.open("/", flags)
    try:
        for component in parts[:-1]:
            child = os.open(component, flags, dir_fd=directory)
            os.close(directory)
            directory = child
        file_descriptor = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW, dir_fd=directory)
        if not stat.S_ISREG(os.fstat(file_descriptor).st_mode):
            os.close(file_descriptor)
            raise ValueError("source must be a regular file")
        return file_descriptor
    finally:
        os.close(directory)


def open_destination(path: Path) -> int:
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    directory = os.open("/", flags)
    try:
        for component in components(path):
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
            source_id: str = "rust-opencode-source") -> Path:
    if os.name != "posix":
        raise ValueError("Rust OpenCode reader requires POSIX")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,199}", source_id):
        raise ValueError("invalid source ID")
    source_fd = open_file(source)
    os.close(source_fd)
    binary_fd = open_file(binary)
    try:
        info = os.fstat(binary_fd)
        if not (info.st_mode & 0o111) or info.st_size > 64 * 1024 * 1024:
            raise ValueError("binary must be an executable regular file within the size limit")
        with os.fdopen(binary_fd, "rb", closefd=False) as stream:
            raw = stream.read(64 * 1024 * 1024 + 1)
        if len(raw) > 64 * 1024 * 1024:
            raise ValueError("binary exceeds size limit")
    finally:
        os.close(binary_fd)
    artifact = destination / "rust-opencode-harness"
    manifest = destination / "rust-opencode-harness.toml"
    config = destination / "plugins.toml"
    manifest_raw = (
        'id = "example.rust-opencode-harness"\nversion = "0.1.0"\nprotocol = "1"\n'
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
            raise ValueError("destination already contains OpenCode plugin files")
        write_new(directory, artifact.name, raw, 0o700)
        write_new(directory, manifest.name, manifest_raw, 0o600)
        write_new(directory, config.name, config_raw, 0o600)
        stat_path = os.stat(destination, follow_symlinks=False)
        stat_fd = os.fstat(directory)
        if (stat_path.st_dev, stat_path.st_ino) != (stat_fd.st_dev, stat_fd.st_ino):
            raise ValueError("destination path changed during installation")
    finally:
        os.close(directory)
    return config


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--destination", required=True, type=Path)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--source-id", default="rust-opencode-source")
    args = parser.parse_args()
    try:
        print(install(args.binary, args.destination, args.source, args.source_id))
    except (OSError, ValueError) as exc:
        parser.error(str(exc))
