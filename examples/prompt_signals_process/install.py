"""Explicitly install the pinned, fixture-only Prompt signals process."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
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


def install(destination: Path) -> Path:
    if os.name != "posix":
        raise ValueError("the fixture process requires POSIX")
    if not destination.is_absolute():
        raise ValueError("destination must be an absolute path")
    artifact = destination / "prompt-signals"
    manifest = destination / "prompt-signals.toml"
    config = destination / "capability.json"
    raw = Path(__file__).with_name("agenturm_prompt_signals.py").read_bytes()
    manifest_raw = (
        'id = "example.prompt-signals-process"\nversion = "0.1.0"\nprotocol = "1"\n'
        f'entrypoint = [{json.dumps(str(artifact))}]\n'
        f'artifact_sha256 = "{hashlib.sha256(raw).hexdigest()}"\n'
        'provides = ["capability-adapter/1"]\nrequires = ["pane-model/1"]\n'
        'depends_on = []\ngrants = ["scoped-aggregate-read"]\n').encode()
    config_raw = json.dumps({"schema": 1, "manifest": str(manifest),
        "pane": {"id": "prompt-signals", "title": "Prompt signals",
                 "columns": [["Signal", 40, 2], ["Observed", 14, 0], ["Coverage", 23, 1]]},
        "grants": ["scoped-aggregate-read"]}, separators=(",", ":")).encode()
    directory = open_destination(destination)
    try:
        for name in (artifact.name, manifest.name, config.name):
            try:
                os.stat(name, dir_fd=directory, follow_symlinks=False)
            except FileNotFoundError:
                continue
            raise ValueError("destination already contains capability files")
        write_new(directory, artifact.name, raw, 0o700)
        write_new(directory, manifest.name, manifest_raw, 0o600)
        write_new(directory, config.name, config_raw, 0o600)
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
