"""Install a locally built Rust adapter with a pinned manifest and explicit source.

Build first: cargo build --locked --release -p agenturm-pi-harness
Then: python install.py --binary /absolute/target/release/agenturm-pi-harness
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
                        help="read up to 48 Pi session JSONL files below an existing directory")
    parser.add_argument("--content-refs", action="store_true",
                        help="explicit Pi directory user-text references (no public Review reader)")
    parser.add_argument("--source-id", help="stable source ID (defaults to a distinct ID per mode)")
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("the Rust Pi executable currently supports POSIX hosts")
    if not all(path.is_absolute() for path in (args.binary, args.destination, args.source)):
        parser.error("binary, destination and source must be absolute paths")
    if args.content_refs and not args.directory:
        parser.error("Pi content references require --directory")
    if not args.binary.is_file() or not os.access(args.binary, os.X_OK):
        parser.error("binary must be an existing executable file")
    if args.directory:
        if any(component.is_symlink() for component in (args.source, *args.source.parents)):
            parser.error("directory source must not traverse a symlink")
        if not args.source.is_dir():
            parser.error("directory source must already exist")
    elif args.source.is_dir():
        parser.error("use --directory for a directory source")
    source_id = (args.source_id if args.source_id is not None else
                 ("rust-pi-content-source" if args.content_refs else
                  "rust-pi-directory-source" if args.directory else "rust-pi-source"))
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,199}", source_id):
        parser.error("invalid source ID")
    destination = args.destination
    if destination.is_symlink():
        parser.error("destination must not be a symlink")
    destination.mkdir(parents=True, exist_ok=True)
    artifact = destination / "rust-pi-harness"
    manifest = destination / "rust-pi-harness.toml"
    config = destination / "plugins.toml"
    if any(os.path.lexists(path) for path in (artifact, manifest, config)):
        parser.error("destination already contains a Rust Pi plugin file")
    with args.binary.open("rb") as source, artifact.open("xb") as output:
        while chunk := source.read(1024 * 1024):
            output.write(chunk)
    artifact.chmod(0o700)
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    provides = ('["source-adapter/1", "configured-root/1", "source-discovery/1", '
                '"source-discovery-root-gap/1", "harness.pi/1", "pi-content-ref/1"]'
                if args.content_refs else
                '["source-adapter/1", "configured-root/1", "source-discovery/1", '
                '"source-discovery-root-gap/1"]'
                if args.directory else '["source-adapter/1", "configured-root/1"]')
    with manifest.open("x") as output:
        output.write(
            'id = "example.rust-pi-harness"\nversion = "0.1.0"\nprotocol = "1"\n'
            f'entrypoint = [{json.dumps(str(artifact))}]\n'
            f'artifact_sha256 = "{digest}"\n'
            f'provides = {provides}\n'
            'requires = ["evidence-ingest/1"]\n'
            'depends_on = []\ngrants = ["configured-source-read"]\n')
    with config.open("x") as output:
        output.write(
            'schema = 1\n[[sources]]\n'
            f'manifest = {json.dumps(str(manifest))}\n'
            f'source_id = {json.dumps(source_id)}\n'
            'grants = ["configured-source-read"]\n'
            f'capture = {json.dumps("pi-review-text-v1" if args.content_refs else "metadata-only")}\n'
            'poll_interval = 0.5\ntimeout = 5.0\n'
            f'root = {json.dumps(str(args.source))}\n'
            + ('replaces = ["pi"]\n' if args.content_refs else '')
            + ('requires_features = ["source-discovery/1", '
               '"source-discovery-root-gap/1", "configured-root/1", '
               '"harness.pi/1", "pi-content-ref/1"]\n' if args.content_refs else
               'requires_features = ["source-discovery/1", '
               '"source-discovery-root-gap/1"]\n' if args.directory else ''))
    print(config)


if __name__ == "__main__":
    main()
