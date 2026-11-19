"""Keep ordinary installs pure; bundle a prebuilt Rust host only when requested."""
from __future__ import annotations

import hashlib
import json
import os
import platform
import re
import stat
import struct
from pathlib import Path

from setuptools import setup
from setuptools.command.build_py import build_py
from setuptools.command.bdist_wheel import bdist_wheel


_BUNDLE = os.environ.get("AGENTURM_BUNDLE_RUST_HOST")
_MAX_BYTES = 64 * 1024 * 1024


def _validate_bundle() -> tuple[bytes, str]:
    if platform.system() != "Linux" or platform.machine().lower() not in {"x86_64", "amd64"}:
        raise RuntimeError("native host wheel is currently built only on Linux x86_64")
    libc, version = platform.libc_ver()
    if libc != "glibc" or not re.fullmatch(r"[0-9]+\.[0-9]+", version):
        raise RuntimeError("native host wheel requires a glibc build host")
    path = Path(_BUNDLE or "")
    if not path.is_absolute() or path.is_symlink():
        raise RuntimeError("native host build requires an absolute, non-symlink binary path")
    try:
        info = path.stat()
        if not stat.S_ISREG(info.st_mode) or not 0 < info.st_size <= _MAX_BYTES:
            raise RuntimeError("invalid native host binary type or size")
        data = path.read_bytes()
    except OSError as exc:
        raise RuntimeError("cannot read native host binary") from exc
    # Match a 64-bit little-endian x86_64 ELF before applying a Linux wheel
    # tag. This is not a glibc portability certificate; that remains a gate.
    if (len(data) > _MAX_BYTES or len(data) < 20 or data[:6] != b"\x7fELF\x02\x01"
            or struct.unpack_from("<H", data, 18)[0] != 62):
        raise RuntimeError("native host is not a Linux x86_64 ELF executable")
    return data, hashlib.sha256(data).hexdigest()


class _SafeBuildPy(build_py):
    def run(self) -> None:
        super().run()
        native = Path(self.build_lib) / "observatory" / "_native"
        if not _BUNDLE:
            # Reusing a build tree must never smuggle a previously bundled
            # executable into a py3-none-any compatibility wheel.
            for name in ("agenturm-plugin-host", "host.json"):
                (native / name).unlink(missing_ok=True)
            return
        binary, digest = _validate_bundle()
        native.mkdir(parents=True, exist_ok=True)
        artifact = native / "agenturm-plugin-host"
        artifact.write_bytes(binary)
        (native / "host.json").write_text(json.dumps({
            "schema": 1, "platform": "linux_x86_64", "glibc_floor": platform.libc_ver()[1],
            "sha256": digest,
        }, sort_keys=True, separators=(",", ":")))


class _NativeWheel(bdist_wheel):
    def finalize_options(self) -> None:
        super().finalize_options()
        self.root_is_pure = False

    def get_tag(self) -> tuple[str, str, str]:
        return "py3", "none", "linux_x86_64"


_commands = {"build_py": _SafeBuildPy}
if _BUNDLE:
    _commands["bdist_wheel"] = _NativeWheel
setup(cmdclass=_commands)
