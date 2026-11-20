"""Trusted, declarative MCP config inventory contributions.

Only modules in Agenturm's installed ``observatory.adapters`` package are
imported. This is an in-process compatibility registry, not process-plugin
discovery and not an authority to execute project-local Python code.
"""
from __future__ import annotations

import importlib
import json
import pkgutil
import re
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Callable


MAX_CONFIG_BYTES = 1024 * 1024
MAX_CONTRIBUTIONS = 32
MAX_PATHS_PER_CONTRIBUTION = 32


@dataclass(frozen=True)
class McpConfigContribution:
    id: str
    client: str
    selectors: tuple[tuple[str, ...], ...]
    global_paths: Callable[[], tuple]
    project_paths: Callable[[Path], tuple]
    priority: int = 100

    def __post_init__(self) -> None:
        if not re.fullmatch(r"[a-z][a-z0-9_.-]*", self.id):
            raise ValueError("invalid MCP config contribution ID")
        if not self.client or len(self.client) > 80 or not self.client.strip():
            raise ValueError("invalid MCP config client")
        if (not isinstance(self.selectors, tuple) or not self.selectors
                or any(not isinstance(path, tuple) or not path
                       or any(not isinstance(key, str) or not key or len(key) > 80
                              for key in path) for path in self.selectors)):
            raise ValueError("invalid MCP config selectors")
        if not callable(self.global_paths) or not callable(self.project_paths):
            raise ValueError("MCP config contribution needs path factories")
        if type(self.priority) is not int or not 0 <= self.priority <= 1000:
            raise ValueError("invalid MCP config priority")


def bundled_contributions(on_error=None) -> tuple[McpConfigContribution, ...]:
    """Load only trusted installed adapter declarations, isolating bad ones."""
    from . import adapters

    found = []
    for item in sorted(pkgutil.iter_modules(adapters.__path__), key=lambda item: item.name):
        try:
            module = importlib.import_module(f"{adapters.__name__}.{item.name}")
            factory = getattr(module, "mcp_config_contributions", None)
            if factory is None:
                continue
            if not callable(factory):
                raise ValueError("invalid MCP config factory")
            declared = factory()
            if not isinstance(declared, tuple) or len(declared) > MAX_CONTRIBUTIONS:
                raise ValueError("invalid MCP config declarations")
            if not all(isinstance(spec, McpConfigContribution) for spec in declared):
                raise ValueError("invalid MCP config declaration")
            found.extend(declared)
        except Exception:
            if on_error:
                on_error()
    found.sort(key=lambda spec: (spec.priority, spec.id))
    unique = []
    ids, clients = set(), set()
    for spec in found:
        if spec.id in ids or spec.client in clients or len(unique) == MAX_CONTRIBUTIONS:
            if on_error:
                on_error()
            continue
        ids.add(spec.id)
        clients.add(spec.client)
        unique.append(spec)
    return tuple(unique)


def _decode_json_or_jsonc(raw: bytes, *, jsonc: bool = False):
    """Parse JSONC while preserving comment markers inside quoted strings."""
    text = raw.decode("utf-8")
    if jsonc:
        output = []
        index = 0
        quoted = escaped = False
        while index < len(text):
            char = text[index]
            following = text[index + 1] if index + 1 < len(text) else ""
            if quoted:
                output.append(char)
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == '"':
                    quoted = False
                index += 1
                continue
            if char == '"':
                quoted = True
                output.append(char)
                index += 1
            elif char == "/" and following == "/":
                index += 2
                while index < len(text) and text[index] not in "\r\n":
                    index += 1
            elif char == "/" and following == "*":
                end = text.find("*/", index + 2)
                if end < 0:
                    raise ValueError("unterminated JSONC comment")
                output.extend(" " for item in text[index:end + 2] if item in "\r\n")
                index = end + 2
            else:
                output.append(char)
                index += 1
        text = "".join(output)
        output = []
        index = 0
        quoted = escaped = False
        while index < len(text):
            char = text[index]
            if quoted:
                output.append(char)
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == '"':
                    quoted = False
            elif char == '"':
                quoted = True
                output.append(char)
            elif char == ",":
                lookahead = index + 1
                while lookahead < len(text) and text[lookahead].isspace():
                    lookahead += 1
                if lookahead == len(text) or text[lookahead] not in "]}":
                    output.append(char)
            else:
                output.append(char)
            index += 1
        text = "".join(output)
    return json.loads(text)


def _server_names(data: object, selectors: tuple[tuple[str, ...], ...]) -> list[str]:
    servers = None
    for selector in selectors:
        value = data
        for key in selector:
            value = value.get(key) if isinstance(value, dict) else None
        if isinstance(value, dict):
            servers = value
            break
    if servers is None:
        return []
    return [str(name).strip()[:128] for name, value in servers.items()
            if isinstance(name, str) and name.strip() and isinstance(value, dict)][:500]


def _paths(factory, default_scope: str, *args) -> tuple[tuple[Path, str], ...]:
    paths = factory(*args)
    if not isinstance(paths, tuple) or len(paths) > MAX_PATHS_PER_CONTRIBUTION:
        raise ValueError("invalid MCP config paths")
    locations = []
    for item in paths:
        path, scope = item if isinstance(item, tuple) and len(item) == 2 else (item, default_scope)
        if not isinstance(path, Path) or scope not in {"User", "System", "Project"}:
            raise ValueError("invalid MCP config path")
        locations.append((path, scope))
    return tuple(locations)


def discover(global_paths=None, project_directories=(), on_error=None):
    """Return bounded names only; a bad provider cannot hide sibling names."""
    specs = bundled_contributions(on_error=on_error)
    by_client = {spec.client: spec for spec in specs}
    default_selectors = (("mcpServers",),)
    candidates = []
    if global_paths is None:
        for spec in specs:
            try:
                candidates.extend((path, spec.client, scope, spec.selectors)
                                  for path, scope in _paths(spec.global_paths, "User"))
            except Exception:
                if on_error:
                    on_error()
    else:
        for path, client, scope in global_paths:
            selectors = by_client[client].selectors if client in by_client else default_selectors
            candidates.append((Path(path), client, scope, selectors))
    for directory in sorted({str(Path(directory)) for directory in project_directories if directory}):
        root = Path(directory)
        for spec in specs:
            try:
                candidates.extend((path, spec.client, scope, spec.selectors)
                                  for path, scope in _paths(spec.project_paths, "Project", root))
            except Exception:
                if on_error:
                    on_error()
    found, seen = [], set()
    for path, client, scope, selectors in candidates:
        try:
            if not path.is_file():
                continue
            with path.open("rb") as handle:
                raw = handle.read(MAX_CONFIG_BYTES + 1)
            if len(raw) > MAX_CONFIG_BYTES:
                if on_error:
                    on_error()
                continue
            data = (tomllib.loads(raw.decode("utf-8")) if path.suffix.lower() == ".toml"
                    else _decode_json_or_jsonc(raw, jsonc=path.suffix.lower() == ".jsonc"))
            for name in _server_names(data, selectors):
                key = (client, scope, name)
                if key in seen:
                    continue
                seen.add(key)
                found.append({"client": client, "scope": scope, "server": name,
                              "config": path.name})
        except (OSError, UnicodeError, ValueError, tomllib.TOMLDecodeError):
            if on_error:
                on_error()
    return sorted(found, key=lambda item: (item["client"].casefold(), item["scope"], item["server"].casefold()))
