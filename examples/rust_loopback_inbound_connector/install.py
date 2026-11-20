"""Install an already-built Rust loopback connector; never invokes Cargo."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
from pathlib import Path
from urllib.parse import urlsplit

MAX_ARTIFACT = 64 * 1024 * 1024


def endpoint_port(value: str) -> int:
    if (not isinstance(value, str) or len(value) > 300 or not value.isascii()
            or any(ord(char) <= 32 or ord(char) == 127 for char in value)
            or "?" in value or "#" in value):
        raise ValueError("invalid loopback endpoint")
    try:
        parsed = urlsplit(value)
        port = parsed.port
    except ValueError as exc:
        raise ValueError("invalid loopback endpoint") from exc
    if (parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or port is None
            or not 1 <= port <= 65535 or parsed.username or parsed.password
            or parsed.path != "/events" or parsed.query or parsed.fragment
            or parsed.netloc != f"127.0.0.1:{port}"):
        raise ValueError("only http://127.0.0.1:<port>/events is permitted")
    return port


def _directory(path: Path) -> int:
    if not path.is_absolute() or ".." in path.parts:
        raise ValueError("destination must be an absolute path without traversal")
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    fd = os.open("/", flags)
    try:
        for component in path.parts[1:]:
            try:
                os.mkdir(component, mode=0o700, dir_fd=fd)
            except FileExistsError:
                pass
            child = os.open(component, flags, dir_fd=fd)
            os.close(fd)
            fd = child
        result, fd = fd, -1
        return result
    finally:
        if fd >= 0:
            os.close(fd)


def _open_source(path: Path) -> int:
    if not path.is_absolute() or ".." in path.parts or not path.name:
        raise ValueError("binary path must be absolute and contain no traversal")
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    parent = os.open("/", flags)
    try:
        for component in path.parts[1:-1]:
            child = os.open(component, flags, dir_fd=parent)
            os.close(parent)
            parent = child
        return os.open(path.name,
            os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC,
            dir_fd=parent)
    finally:
        os.close(parent)


def _copy_binary(binary: Path, directory: int, name: str) -> str:
    source_fd = _open_source(binary)
    output_fd = -1
    created = False
    try:
        before = os.fstat(source_fd)
        if not stat.S_ISREG(before.st_mode) or not before.st_mode & 0o111 or before.st_size == 0:
            raise ValueError("binary must be a nonempty executable regular file")
        if before.st_size > MAX_ARTIFACT:
            raise ValueError("binary exceeds host 64 MiB artifact limit")
        fingerprint = (before.st_dev, before.st_ino, before.st_size,
                       before.st_mtime_ns, before.st_ctime_ns)
        output_fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                            0o700, dir_fd=directory)
        created = True
        digest = hashlib.sha256()
        total = 0
        with os.fdopen(output_fd, "wb") as output:
            output_fd = -1
            os.fchmod(output.fileno(), 0o700)
            while True:
                chunk = os.read(source_fd, 1024 * 1024)
                if not chunk:
                    break
                if total + len(chunk) > MAX_ARTIFACT:
                    raise ValueError("binary exceeds host 64 MiB artifact limit")
                output.write(chunk)
                digest.update(chunk)
                total += len(chunk)
            output.flush()
            os.fsync(output.fileno())
            after = os.fstat(source_fd)
            current = (after.st_dev, after.st_ino, after.st_size,
                       after.st_mtime_ns, after.st_ctime_ns)
            if total != before.st_size or current != fingerprint:
                raise ValueError("binary changed while being copied")
        return digest.hexdigest()
    except BaseException:
        if created:
            try:
                os.unlink(name, dir_fd=directory)
            except FileNotFoundError:
                pass
        raise
    finally:
        if output_fd >= 0:
            os.close(output_fd)
        os.close(source_fd)


def _write_new(directory: int, name: str, raw: bytes, mode: int = 0o600) -> None:
    fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                 mode, dir_fd=directory)
    try:
        with os.fdopen(fd, "wb") as stream:
            fd = -1
            os.fchmod(stream.fileno(), mode)
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
    except BaseException:
        if fd >= 0:
            os.close(fd)
        try:
            os.unlink(name, dir_fd=directory)
        except FileNotFoundError:
            pass
        raise


def install(binary: Path, destination: Path, endpoint: str, source_id: str) -> Path:
    if os.name != "posix":
        raise ValueError("the Rust connector installer currently requires POSIX")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,199}", source_id):
        raise ValueError("invalid source ID")
    endpoint_port(endpoint)
    artifact = destination / "rust-loopback-inbound"
    source = destination / "source.json"
    manifest = destination / "rust-loopback-inbound.toml"
    config = destination / "plugins.toml"
    names = (artifact.name, source.name, manifest.name, config.name)
    directory = _directory(destination)
    created: list[str] = []
    try:
        for name in names:
            try:
                os.stat(name, dir_fd=directory, follow_symlinks=False)
            except FileNotFoundError:
                continue
            raise ValueError("destination already contains connector files")
        digest = _copy_binary(binary, directory, artifact.name)
        created.append(artifact.name)
        manifest_bytes = (
            'id = "example.rust-loopback-inbound"\nversion = "0.1.0"\nprotocol = "1"\n'
            f'entrypoint = [{json.dumps(str(artifact))}]\n'
            f'artifact_sha256 = "{digest}"\n'
            'provides = ["source-adapter/1", "configured-root/1", '
            '"connector.inbound/1", "connector-adapter/1"]\n'
            'requires = ["evidence-ingest/1"]\ndepends_on = []\n'
            'grants = ["configured-source-read", "loopback-http-read"]\n').encode()
        source_bytes = json.dumps({"endpoint": endpoint}, separators=(",", ":")).encode()
        config_bytes = (
            'schema = 1\n[[sources]]\n'
            f'manifest = {json.dumps(str(manifest))}\n'
            f'source_id = {json.dumps(source_id)}\n'
            'grants = ["configured-source-read", "loopback-http-read"]\n'
            'requires_features = ["connector-adapter/1"]\n'
            'capture = "metadata-only"\npoll_interval = 0.5\ntimeout = 5.0\n'
            f'root = {json.dumps(str(source))}\n').encode()
        _write_new(directory, source.name, source_bytes)
        created.append(source.name)
        _write_new(directory, manifest.name, manifest_bytes)
        created.append(manifest.name)
        _write_new(directory, config.name, config_bytes)
        created.append(config.name)
        os.fsync(directory)
        return config
    except BaseException:
        for name in reversed(created):
            try:
                os.unlink(name, dir_fd=directory)
            except FileNotFoundError:
                pass
        raise
    finally:
        os.close(directory)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True,
                        help="already-built executable; Cargo is not invoked")
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--source-id", default="rust-loopback-source")
    arguments = parser.parse_args()
    try:
        print(install(arguments.binary, arguments.destination,
                       arguments.endpoint, arguments.source_id))
    except (OSError, ValueError) as exc:
        parser.error(str(exc))
