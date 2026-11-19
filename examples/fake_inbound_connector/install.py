"""Install the standalone local HTTP connector with pinned executable identity."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
from pathlib import Path

from agenturm_fake_inbound import endpoint


def open_destination(path: Path) -> int:
    """Create and open each absolute path component without following links."""
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
    descriptor = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                         mode, dir_fd=directory)
    with os.fdopen(descriptor, "wb") as stream:
        os.fchmod(stream.fileno(), mode)
        stream.write(raw)


def ensure_empty_entries(directory: int, names: tuple[str, ...]) -> None:
    for name in names:
        try:
            os.stat(name, dir_fd=directory, follow_symlinks=False)
        except FileNotFoundError:
            continue
        raise ValueError("destination already contains connector files")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", required=True, type=Path)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--source-id", default="fake-inbound-source")
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("the sample executable currently supports POSIX hosts")
    if not args.destination.is_absolute():
        parser.error("destination must be an absolute path")
    try:
        url = endpoint(args.endpoint)
    except ValueError as exc:
        parser.error(str(exc))
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,199}", args.source_id):
        parser.error("invalid source ID")
    destination = args.destination
    artifact = destination / "fake-inbound"
    manifest = destination / "fake-inbound.toml"
    source = destination / "source.json"
    config = destination / "plugins.toml"
    artifact_bytes = Path(__file__).with_name("agenturm_fake_inbound.py").read_bytes()
    digest = hashlib.sha256(artifact_bytes).hexdigest()
    source_bytes = json.dumps({"endpoint": url}, separators=(",", ":")).encode()
    manifest_bytes = (
        'id = "example.fake-inbound"\nversion = "0.1.0"\nprotocol = "1"\n'
        f'entrypoint = [{json.dumps(str(artifact))}]\n'
        f'artifact_sha256 = "{digest}"\n'
        'provides = ["source-adapter/1", "configured-root/1", '
        '"connector.inbound/1", "connector-adapter/1"]\n'
        'requires = ["evidence-ingest/1"]\n'
        'depends_on = []\ngrants = ["configured-source-read", "loopback-http-read"]\n').encode()
    config_bytes = (
        'schema = 1\n[[sources]]\n'
        f'manifest = {json.dumps(str(manifest))}\n'
        f'source_id = {json.dumps(args.source_id)}\n'
        'grants = ["configured-source-read", "loopback-http-read"]\n'
        'requires_features = ["connector-adapter/1"]\n'
        'capture = "metadata-only"\npoll_interval = 0.5\ntimeout = 5.0\n'
        f'root = {json.dumps(str(source))}\n').encode()
    try:
        directory = open_destination(destination)
        try:
            ensure_empty_entries(directory, (artifact.name, manifest.name, source.name, config.name))
            write_new(directory, artifact.name, artifact_bytes, 0o700)
            write_new(directory, source.name, source_bytes, 0o600)
            write_new(directory, manifest.name, manifest_bytes, 0o600)
            write_new(directory, config.name, config_bytes, 0o600)
        finally:
            os.close(directory)
    except (OSError, ValueError) as exc:
        parser.error(f"cannot install connector: {exc}")
    print(config)


if __name__ == "__main__":
    main()
