"""Install the standalone local HTTP connector with pinned executable identity."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
from pathlib import Path

from agenturm_fake_inbound import endpoint


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
    destination.mkdir(parents=True, exist_ok=True)
    artifact = destination / "fake-inbound"
    manifest = destination / "fake-inbound.toml"
    source = destination / "source.json"
    config = destination / "plugins.toml"
    if any(path.exists() for path in (artifact, manifest, source, config)):
        parser.error("destination already contains connector files")
    shutil.copyfile(Path(__file__).with_name("agenturm_fake_inbound.py"), artifact)
    artifact.chmod(0o700)
    source.write_text(json.dumps({"endpoint": url}, separators=(",", ":")))
    source.chmod(0o600)
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    manifest.write_text(
        'id = "example.fake-inbound"\nversion = "0.1.0"\nprotocol = "1"\n'
        f'entrypoint = [{json.dumps(str(artifact))}]\n'
        f'artifact_sha256 = "{digest}"\n'
        'provides = ["source-adapter/1", "configured-root/1", "connector.inbound/1"]\n'
        'requires = ["evidence-ingest/1"]\n'
        'depends_on = []\ngrants = ["configured-source-read", "loopback-http-read"]\n')
    config.write_text(
        'schema = 1\n[[sources]]\n'
        f'manifest = {json.dumps(str(manifest))}\n'
        f'source_id = {json.dumps(args.source_id)}\n'
        'grants = ["configured-source-read", "loopback-http-read"]\n'
        'capture = "metadata-only"\npoll_interval = 0.5\ntimeout = 5.0\n'
        f'root = {json.dumps(str(source))}\n')
    print(config)


if __name__ == "__main__":
    main()
