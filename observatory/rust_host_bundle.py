"""Resolve an explicitly selected, wheel-bundled Rust plugin host."""
from __future__ import annotations

import json
import platform
import re
from pathlib import Path


_DIGEST = re.compile(r"[0-9a-f]{64}\Z")
_VERSION = re.compile(r"[0-9]+\.[0-9]+\Z")


def bundled_rust_host() -> tuple[str, str]:
    """Return the installed artifact and pinned digest, or fail closed.

    The caller passes both to the existing pre-Store verifier. This function
    performs no download, install or process launch.
    """
    if platform.system() != "Linux" or platform.machine().lower() not in {"x86_64", "amd64"}:
        raise ValueError("bundled Rust plugin host is available only on Linux x86_64")
    native = Path(__file__).resolve().parent / "_native"
    metadata = native / "host.json"
    artifact = native / "agenturm-plugin-host"
    try:
        data = json.loads(metadata.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise ValueError("this Agenturm installation has no bundled Rust plugin host") from exc
    if (not isinstance(data, dict) or set(data) != {"schema", "platform", "glibc_floor", "sha256"}
            or type(data["schema"]) is not int or data["schema"] != 1
            or data["platform"] != "linux_x86_64"
            or not isinstance(data["glibc_floor"], str)
            or not _VERSION.fullmatch(data["glibc_floor"])
            or not isinstance(data["sha256"], str) or not _DIGEST.fullmatch(data["sha256"])
            or not artifact.is_file() or artifact.is_symlink()):
        raise ValueError("installed Rust plugin host metadata is invalid")
    runtime_libc, runtime_version = platform.libc_ver()
    if (runtime_libc != "glibc" or not _VERSION.fullmatch(runtime_version)
            or tuple(map(int, runtime_version.split("."))) <
            tuple(map(int, data["glibc_floor"].split(".")))):
        raise ValueError("bundled Rust plugin host requires a newer Linux glibc")
    return str(artifact), data["sha256"]
