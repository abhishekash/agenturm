"""Install a locally built Rust adapter with a pinned manifest and explicit source.

Build first: cargo build --locked --release -p agenturm-claude-harness
Then: python install.py --binary /absolute/target/release/agenturm-claude-harness
                        --destination /absolute/plugin-dir --source /absolute/events.jsonl
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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--destination", required=True, type=Path)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--source-id", default="rust-claude-source")
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("the Rust Claude Code executable currently supports POSIX hosts")
    if not all(path.is_absolute() for path in (args.binary, args.destination, args.source)):
        parser.error("binary, destination and source must be absolute paths")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,199}", args.source_id):
        parser.error("invalid source ID")
    destination = args.destination
    artifact = destination / "rust-claude-harness"
    manifest = destination / "rust-claude-harness.toml"
    config = destination / "plugins.toml"
    try:
        with os.fdopen(os.open(args.binary, os.O_RDONLY | os.O_NOFOLLOW), "rb") as source:
            info = os.fstat(source.fileno())
            if not stat.S_ISREG(info.st_mode) or not info.st_mode & 0o111 or info.st_size > 64 * 1024 * 1024:
                raise ValueError("binary must be a bounded executable regular file")
            raw = source.read(64 * 1024 * 1024 + 1)
            if len(raw) > 64 * 1024 * 1024:
                raise ValueError("binary exceeds artifact budget")
        digest = hashlib.sha256(raw).hexdigest()
        manifest_raw = (
            'id = "example.rust-claude-harness"\nversion = "0.1.0"\nprotocol = "1"\n'
            f'entrypoint = [{json.dumps(str(artifact))}]\n'
            f'artifact_sha256 = "{digest}"\n'
            'provides = ["source-adapter/1", "configured-root/1"]\n'
            'requires = ["evidence-ingest/1"]\n'
            'depends_on = []\ngrants = ["configured-source-read"]\n').encode()
        config_raw = (
            'schema = 1\n[[sources]]\n'
            f'manifest = {json.dumps(str(manifest))}\n'
            f'source_id = {json.dumps(args.source_id)}\n'
            'grants = ["configured-source-read"]\n'
            'capture = "metadata-only"\npoll_interval = 0.5\ntimeout = 5.0\n'
            f'root = {json.dumps(str(args.source))}\n').encode()
        directory = open_destination(destination)
        try:
            for name in (artifact.name, manifest.name, config.name):
                try:
                    os.stat(name, dir_fd=directory, follow_symlinks=False)
                except FileNotFoundError:
                    continue
                raise ValueError("destination already contains a Rust Claude plugin file")
            write_new(directory, artifact.name, raw, 0o700)
            write_new(directory, manifest.name, manifest_raw, 0o600)
            write_new(directory, config.name, config_raw, 0o600)
        finally:
            os.close(directory)
    except (OSError, ValueError) as exc:
        parser.error(str(exc))
    print(config)


if __name__ == "__main__":
    main()
