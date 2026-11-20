"""Install a locally built Rust adapter with a pinned manifest and explicit source.

Build first: cargo build --locked --release -p agenturm-codex-harness
Then: python install.py --binary /absolute/target/release/agenturm-codex-harness
                        --destination /absolute/plugin-dir --source /absolute/events.jsonl
The installer never builds, downloads, starts, or activates executable code.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--destination", required=True, type=Path)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--directory", action="store_true",
                        help="read up to 48 JSONL files below an existing directory")
    parser.add_argument("--source-id", default="rust-codex-source")
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("the Rust Codex executable currently supports POSIX hosts")
    if not all(path.is_absolute() for path in (args.binary, args.destination, args.source)):
        parser.error("binary, destination and source must be absolute paths")
    for path in (args.binary, args.destination, args.source):
        if any(component.is_symlink() for component in (path, *path.parents)):
            parser.error("binary, destination and source must not traverse a symlink")
    if not args.binary.is_file() or not os.access(args.binary, os.X_OK):
        parser.error("binary must be an existing executable file")
    if args.directory and not args.source.is_dir():
        parser.error("directory source must already exist")
    if not args.directory and args.source.is_dir():
        parser.error("use --directory for a directory source")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,199}", args.source_id):
        parser.error("invalid source ID")
    destination = args.destination
    if destination.is_symlink():
        parser.error("destination must not be a symlink")
    destination.mkdir(parents=True, exist_ok=True)
    artifact = destination / "rust-codex-harness"
    manifest = destination / "rust-codex-harness.toml"
    config = destination / "plugins.toml"
    if any(os.path.lexists(path) for path in (artifact, manifest, config)):
        parser.error("destination already contains a Rust Codex plugin file")
    with args.binary.open("rb") as source, artifact.open("xb") as output:
        while chunk := source.read(1024 * 1024):
            output.write(chunk)
    artifact.chmod(0o700)
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    provides = ('["source-adapter/1", "configured-root/1", "source-discovery/1"]'
                if args.directory else '["source-adapter/1", "configured-root/1"]')
    with manifest.open("x") as output:
        output.write(
            'id = "example.rust-codex-harness"\nversion = "0.1.0"\nprotocol = "1"\n'
            f'entrypoint = [{json.dumps(str(artifact))}]\n'
            f'artifact_sha256 = "{digest}"\n'
            f'provides = {provides}\n'
            'requires = ["evidence-ingest/1"]\n'
            'depends_on = []\ngrants = ["configured-source-read"]\n')
    with config.open("x") as output:
        output.write(
            'schema = 1\n[[sources]]\n'
            f'manifest = {json.dumps(str(manifest))}\n'
            f'source_id = {json.dumps(args.source_id)}\n'
            'grants = ["configured-source-read"]\n'
            'capture = "metadata-only"\npoll_interval = 0.5\ntimeout = 5.0\n'
            f'root = {json.dumps(str(args.source))}\n'
            + ('requires_features = ["source-discovery/1"]\n' if args.directory else ''))
    print(config)


if __name__ == "__main__":
    main()
