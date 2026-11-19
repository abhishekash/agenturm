"""Install a locally built Rust adapter with a pinned manifest and explicit source.

Build first: cargo build --locked --release -p agenturm-sample-harness
Then: python install.py --binary /absolute/target/release/agenturm-sample-harness
                        --destination /absolute/plugin-dir --source /absolute/events.jsonl
The installer never builds, downloads, starts, or activates executable code.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--destination", required=True, type=Path)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--source-id", default="rust-sample-source")
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("the Rust sample executable currently supports POSIX hosts")
    if not all(path.is_absolute() for path in (args.binary, args.destination, args.source)):
        parser.error("binary, destination and source must be absolute paths")
    if not args.binary.is_file() or not os.access(args.binary, os.X_OK):
        parser.error("binary must be an existing executable file")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,199}", args.source_id):
        parser.error("invalid source ID")
    destination = args.destination
    destination.mkdir(parents=True, exist_ok=True)
    artifact = destination / "rust-sample-harness"
    manifest = destination / "rust-sample-harness.toml"
    config = destination / "plugins.toml"
    if any(path.exists() for path in (artifact, manifest, config)):
        parser.error("destination already contains a Rust sample plugin file")
    shutil.copyfile(args.binary, artifact)
    artifact.chmod(0o700)
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    manifest.write_text(
        'id = "example.rust-sample-harness"\nversion = "0.1.0"\nprotocol = "1"\n'
        f'entrypoint = [{json.dumps(str(artifact))}]\n'
        f'artifact_sha256 = "{digest}"\n'
        'provides = ["source-adapter/1", "configured-root/1"]\n'
        'requires = ["evidence-ingest/1"]\n'
        'depends_on = []\ngrants = ["configured-source-read"]\n')
    config.write_text(
        'schema = 1\n[[sources]]\n'
        f'manifest = {json.dumps(str(manifest))}\n'
        f'source_id = {json.dumps(args.source_id)}\n'
        'grants = ["configured-source-read"]\n'
        'capture = "metadata-only"\npoll_interval = 0.5\ntimeout = 5.0\n'
        f'root = {json.dumps(str(args.source))}\n')
    print(config)


if __name__ == "__main__":
    main()
