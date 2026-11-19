"""Explicit, inert source-plugin activation configuration."""
from __future__ import annotations

import math
import stat
import tomllib
from pathlib import Path

from .plugin_manifest import PLUGIN_ID, PluginManifest
from .plugin_process import MAX_PROCESSES
from .plugin_supervisor import PluginSource

MAX_CONFIG_BYTES = 64 * 1024


def read_plugin_config(path: str | Path) -> tuple[PluginSource, ...]:
    """Parse only a named file; never search project directories for code."""
    try:
        path = Path(path).resolve(strict=True)
    except OSError as exc:
        raise ValueError("cannot find plugin configuration") from exc
    try:
        if not stat.S_ISREG(path.stat().st_mode):
            raise ValueError("plugin configuration must be a regular file")
        with path.open("rb") as stream:
            raw = stream.read(MAX_CONFIG_BYTES + 1)
    except OSError as exc:
        raise ValueError("cannot read plugin configuration") from exc
    if len(raw) > MAX_CONFIG_BYTES:
        raise ValueError("plugin configuration exceeds byte limit")
    try:
        data = tomllib.loads(raw.decode("utf-8"))
    except (UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise ValueError("invalid plugin configuration TOML") from exc
    if (not isinstance(data, dict) or set(data) != {"schema", "sources"}
            or type(data["schema"]) is not int or data["schema"] != 1):
        raise ValueError("invalid plugin configuration schema")
    rows = data["sources"]
    if not isinstance(rows, list) or not 1 <= len(rows) <= MAX_PROCESSES:
        raise ValueError("plugin configuration needs 1 to 4 sources")
    sources = []
    seen = set()
    replaced = set()
    for row in rows:
        required = {"manifest", "source_id", "grants", "capture", "poll_interval", "timeout"}
        if not isinstance(row, dict) or not required <= set(row) or set(row) - required - {"replaces", "root"}:
            raise ValueError("invalid plugin source configuration fields")
        source_id = row["source_id"]
        if not isinstance(source_id, str) or not source_id or source_id in seen:
            raise ValueError("invalid or duplicate plugin source ID")
        seen.add(source_id)
        if row["capture"] != "metadata-only":
            raise ValueError("plugin capture must be metadata-only")
        grants = row["grants"]
        if (not isinstance(grants, list) or len(grants) > 16 or
                any(not isinstance(value, str) or not PLUGIN_ID.fullmatch(value) for value in grants)
                or len(set(grants)) != len(grants)):
            raise ValueError("invalid plugin grants")
        for key, low, high in (("poll_interval", 0.05, 60.0), ("timeout", 0.1, 60.0)):
            value = row[key]
            if type(value) not in (int, float) or not math.isfinite(value) or not low <= value <= high:
                raise ValueError(f"invalid plugin {key}")
        manifest_path = row["manifest"]
        if not isinstance(manifest_path, str) or not manifest_path or len(manifest_path) > 1024:
            raise ValueError("invalid plugin manifest path")
        location = Path(manifest_path)
        if not location.is_absolute():
            location = path.parent / location
        manifest = PluginManifest.read(location)
        replaces = row.get("replaces", [])
        if (not isinstance(replaces, list) or len(replaces) > 1
                or any(not isinstance(value, str) or not PLUGIN_ID.fullmatch(value)
                       or f"harness.{value}/1" not in manifest.provides for value in replaces)
                or len(set(replaces)) != len(replaces) or replaced.intersection(replaces)):
            raise ValueError("invalid or unclaimed built-in harness replacement")
        replaced.update(replaces)
        source_root = row.get("root")
        if source_root is not None and (not isinstance(source_root, str)
                                        or not Path(source_root).is_absolute()
                                        or len(source_root) > 1024
                                        or any(ord(char) < 32 or ord(char) == 127
                                               for char in source_root)):
            raise ValueError("plugin source root must be an absolute path")
        sources.append(PluginSource(manifest, source_id, tuple(grants),
                                    float(row["poll_interval"]), float(row["timeout"]),
                                    tuple(replaces), source_root))
    return tuple(sources)
