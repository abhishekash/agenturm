"""Install the pinned fake outbound adapter and explicit destination descriptor."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path

from agenturm_fake_outbound import endpoint


def open_destination(path: Path) -> int:
    """Walk from `/` with no-follow directory handles, creating missing parts."""
    if len(path.parts) < 2 or ".." in path.parts:
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
    """Never follow or overwrite an existing entry, including a dangling link."""
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
    descriptor = os.open(name, flags, mode, dir_fd=directory)
    with os.fdopen(descriptor, "wb") as stream:
        os.fchmod(stream.fileno(), mode)
        stream.write(raw)


def ensure_empty_entries(directory: int, names: tuple[str, ...]) -> None:
    for name in names:
        try:
            os.stat(name, dir_fd=directory, follow_symlinks=False)
        except FileNotFoundError:
            continue
        raise ValueError("destination already contains adapter files")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", required=True, type=Path)
    parser.add_argument("--endpoint", required=True)
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("the example executable currently supports POSIX hosts")
    if not args.destination.is_absolute():
        parser.error("destination must be an absolute path")
    try:
        url = endpoint(args.endpoint)
    except ValueError as exc:
        parser.error(str(exc))
    root = args.destination
    artifact = root / "fake-outbound"
    manifest = root / "fake-outbound.toml"
    descriptor = root / "destination.json"
    artifact_bytes = Path(__file__).with_name("agenturm_fake_outbound.py").read_bytes()
    digest = hashlib.sha256(artifact_bytes).hexdigest()
    manifest_bytes = (
        'id = "example.fake-outbound"\nversion = "0.1.0"\nprotocol = "1"\n'
        f'entrypoint = [{json.dumps(str(artifact))}]\n'
        f'artifact_sha256 = "{digest}"\n'
        'provides = ["action-adapter/1"]\nrequires = ["delivery-attempt/1"]\n'
        'depends_on = []\ngrants = ["loopback-sink-write"]\n').encode()
    descriptor_bytes = json.dumps(
        {"schema": 1, "destination_id": "fake-sink", "action": "write_fixture_metric",
         "endpoint": url, "manifest": str(manifest), "grants": ["loopback-sink-write"]},
        separators=(",", ":")).encode()
    try:
        directory = open_destination(root)
        try:
            ensure_empty_entries(directory, (artifact.name, manifest.name, descriptor.name))
            write_new(directory, artifact.name, artifact_bytes, 0o700)
            write_new(directory, manifest.name, manifest_bytes, 0o600)
            write_new(directory, descriptor.name, descriptor_bytes, 0o600)
        finally:
            os.close(directory)
    except (OSError, ValueError) as exc:
        parser.error(f"cannot install adapter: {exc}")
    print(descriptor)


if __name__ == "__main__":
    main()
