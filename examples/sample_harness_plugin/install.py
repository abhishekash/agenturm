"""Create a pinned, explicit sample plugin manifest and source configuration.

Usage: python install.py --destination /absolute/plugin-dir --source /absolute/events.jsonl
The installer never starts the adapter or modifies Agenturm's receiver state.
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
    parser.add_argument("--destination", required=True, type=Path)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--source-id", default="sample-source")
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("the sample executable currently supports POSIX hosts")
    if not args.destination.is_absolute() or not args.source.is_absolute():
        parser.error("destination and source must be absolute paths")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,199}", args.source_id):
        parser.error("invalid source ID")
    destination = args.destination
    destination.mkdir(parents=True, exist_ok=True)
    artifact = destination / "sample-harness"
    manifest = destination / "sample-harness.toml"
    config = destination / "plugins.toml"
    if any(path.exists() for path in (artifact, manifest, config)):
        parser.error("destination already contains a sample plugin file")
    shutil.copyfile(Path(__file__).with_name("agenturm_sample_harness.py"), artifact)
    artifact.chmod(0o700)
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    manifest.write_text(
        'id = "example.sample-harness"\nversion = "0.1.0"\nprotocol = "1"\n'
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
