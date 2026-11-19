"""Inert, explicitly configured plugin manifests and deterministic activation.

This internal schema is not the external SDK. Parsing TOML never imports a
module, resolves an entrypoint, or launches a child process. The future host
must verify artifact identity again immediately before execution.
"""
from __future__ import annotations

import heapq
import re
import stat
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


MAX_MANIFEST_BYTES = 64 * 1024
PLUGIN_ID = re.compile(r"[a-z][a-z0-9]*(?:[.-][a-z0-9]+)*\Z")
FEATURE = re.compile(r"[a-z][a-z0-9.-]*/[1-9][0-9]*\Z")
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\Z")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
PROTOCOL = re.compile(r"([1-9][0-9]*)(?:\.\.([1-9][0-9]*))?\Z")
FIELDS = frozenset({"id", "version", "protocol", "entrypoint", "artifact_sha256",
                    "provides", "requires", "depends_on", "grants"})


class ManifestError(ValueError):
    """A manifest or activation plan is invalid; no plugin was started."""


def _strings(data: dict, key: str, pattern: re.Pattern[str]) -> tuple[str, ...]:
    values = data.get(key, [])
    if not isinstance(values, list) or any(not isinstance(value, str) or
            not pattern.fullmatch(value) for value in values):
        raise ManifestError(f"invalid {key}")
    if len(values) > 128 or len(set(values)) != len(values):
        raise ManifestError(f"duplicate or excessive {key}")
    return tuple(values)


@dataclass(frozen=True)
class PluginManifest:
    id: str
    version: str
    protocol_min: int
    protocol_max: int
    entrypoint: tuple[str, ...]
    artifact_sha256: str
    provides: tuple[str, ...]
    requires: tuple[str, ...]
    depends_on: tuple[str, ...]
    grants: tuple[str, ...]
    origin: Path

    @classmethod
    def read(cls, path: str | Path) -> PluginManifest:
        path = Path(path)
        try:
            if not stat.S_ISREG(path.stat().st_mode):
                raise ManifestError(f"manifest {path.name} is not a regular file")
            with path.open("rb") as stream:
                raw = stream.read(MAX_MANIFEST_BYTES + 1)
        except OSError as exc:
            raise ManifestError(f"cannot read manifest {path.name}") from exc
        if len(raw) > MAX_MANIFEST_BYTES:
            raise ManifestError(f"manifest {path.name} exceeds byte limit")
        try:
            data = tomllib.loads(raw.decode("utf-8"))
        except (UnicodeError, tomllib.TOMLDecodeError) as exc:
            raise ManifestError(f"invalid TOML in {path.name}") from exc
        if not isinstance(data, dict) or set(data) != FIELDS:
            raise ManifestError(f"manifest {path.name} has missing or unknown fields")
        plugin_id = data["id"]
        version = data["version"]
        protocol = data["protocol"]
        digest = data["artifact_sha256"]
        if not isinstance(plugin_id, str) or not PLUGIN_ID.fullmatch(plugin_id):
            raise ManifestError("invalid plugin ID")
        if not isinstance(version, str) or not VERSION.fullmatch(version):
            raise ManifestError(f"invalid version for {plugin_id}")
        match = PROTOCOL.fullmatch(protocol) if isinstance(protocol, str) else None
        if match is None or len(match[1]) > 4 or len(match[2] or "") > 4:
            raise ManifestError(f"invalid protocol range for {plugin_id}")
        low, high = int(match[1]), int(match[2] or match[1])
        if low > high or high - low > 8:
            raise ManifestError(f"invalid protocol range for {plugin_id}")
        if not isinstance(digest, str) or not SHA256.fullmatch(digest):
            raise ManifestError(f"invalid artifact hash for {plugin_id}")
        entrypoint = data["entrypoint"]
        if (not isinstance(entrypoint, list) or not 1 <= len(entrypoint) <= 16 or
                any(not isinstance(arg, str) or not arg or len(arg) > 1024 or
                    any(ord(char) < 32 or ord(char) == 127 for char in arg)
                    for arg in entrypoint)):
            raise ManifestError(f"invalid entrypoint for {plugin_id}")
        if (not Path(entrypoint[0]).is_absolute() and
                ("/" in entrypoint[0] or "\\" in entrypoint[0] or entrypoint[0] in {".", ".."})):
            raise ManifestError(f"entrypoint for {plugin_id} must be a command name or absolute path")
        provides = _strings(data, "provides", FEATURE)
        requires = _strings(data, "requires", FEATURE)
        depends_on = _strings(data, "depends_on", PLUGIN_ID)
        grants = _strings(data, "grants", PLUGIN_ID)
        if plugin_id in depends_on:
            raise ManifestError(f"plugin {plugin_id} depends on itself")
        return cls(plugin_id, version, low, high, tuple(entrypoint), digest,
                   provides, requires, depends_on, grants, path.resolve())


def discover_manifests(paths: Iterable[str | Path]) -> dict[str, PluginManifest]:
    """Read only paths explicitly supplied by the host configuration."""
    manifests = {}
    for path in paths:
        manifest = PluginManifest.read(path)
        if manifest.id in manifests:
            raise ManifestError(f"duplicate plugin ID {manifest.id}")
        manifests[manifest.id] = manifest
    return manifests


def plan_activation(manifests: dict[str, PluginManifest], enabled: Iterable[str], *,
                    host_features: Iterable[str], allowed_grants: Iterable[str],
                    protocol_major: int = 1) -> tuple[PluginManifest, ...]:
    """Return a stable startup order, or fail before any plugin starts."""
    enabled_ids = tuple(enabled)
    if (any(not isinstance(plugin_id, str) or not PLUGIN_ID.fullmatch(plugin_id)
            for plugin_id in enabled_ids) or len(set(enabled_ids)) != len(enabled_ids)):
        raise ManifestError("invalid or duplicate enabled plugin ID")
    selected = set(enabled_ids)
    unknown = selected - manifests.keys()
    if unknown:
        raise ManifestError(f"unknown enabled plugins: {', '.join(sorted(unknown))}")
    features = set(host_features)
    grants = set(allowed_grants)
    if (any(not isinstance(feature, str) or not FEATURE.fullmatch(feature)
            for feature in features) or
            any(not isinstance(grant, str) or not PLUGIN_ID.fullmatch(grant)
                for grant in grants)):
        raise ManifestError("invalid host features or grants")
    if type(protocol_major) is not int or protocol_major < 1:
        raise ManifestError("invalid host protocol")
    providers: dict[str, set[str]] = {}
    for plugin_id in selected:
        manifest = manifests[plugin_id]
        if not manifest.protocol_min <= protocol_major <= manifest.protocol_max:
            raise ManifestError(f"incompatible protocol for {plugin_id}")
        denied = set(manifest.grants) - grants
        if denied:
            raise ManifestError(f"ungranted access for {plugin_id}: {', '.join(sorted(denied))}")
        for feature in manifest.provides:
            providers.setdefault(feature, set()).add(plugin_id)
    edges = {}
    for plugin_id in selected:
        manifest = manifests[plugin_id]
        deps = set(manifest.depends_on)
        missing = deps - selected
        if missing:
            raise ManifestError(f"disabled dependency for {plugin_id}: {', '.join(sorted(missing))}")
        for feature in manifest.requires:
            if feature in features:
                continue
            candidates = providers.get(feature, set()) - {plugin_id}
            if not candidates:
                raise ManifestError(f"missing required feature {feature} for {plugin_id}")
            explicit = candidates & deps
            if len(candidates) > 1 and len(explicit) != 1:
                raise ManifestError(f"ambiguous provider for {feature} required by {plugin_id}")
            deps.add(next(iter(explicit or candidates)))
        edges[plugin_id] = deps
    ready = [plugin_id for plugin_id, deps in edges.items() if not deps]
    heapq.heapify(ready)
    ordered = []
    while ready:
        plugin_id = heapq.heappop(ready)
        ordered.append(plugin_id)
        for other in sorted(edges):
            if plugin_id in edges[other]:
                edges[other].remove(plugin_id)
                if not edges[other]:
                    heapq.heappush(ready, other)
    if len(ordered) != len(selected):
        raise ManifestError("plugin dependency cycle")
    return tuple(manifests[plugin_id] for plugin_id in ordered)
