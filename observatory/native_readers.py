"""Registration contract for trusted, in-process native readers.

Registration is inert. The host constructs only enabled readers, and owns the
poll loop and status aggregation. A process plugin protocol is a later phase.
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Callable, Protocol


class NativeReader(Protocol):
    def poll(self) -> object: ...
    def status_sessions(self) -> list[dict]: ...
    def diagnostics(self) -> dict: ...


@dataclass(frozen=True)
class NativeReaderSpec:
    id: str
    factory: Callable[[object, bool], NativeReader]
    mcp_client: str | None = None
    label: str | None = None
    coverage: Callable[[NativeReader], dict] | None = None
    status_extension: Callable[[NativeReader | None, dict], dict] | None = None
    status_keys: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        if not isinstance(self.id, str) or not re.fullmatch(r"[a-z][a-z0-9_.-]*", self.id):
            raise ValueError("invalid native reader ID")
        if (not callable(self.factory) or
                (self.label is not None and (not isinstance(self.label, str) or
                                             not self.label.strip() or len(self.label) > 40)) or
                (self.coverage is not None and not callable(self.coverage)) or
                (self.status_extension is not None and not callable(self.status_extension)) or
                type(self.status_keys) is not tuple or
                (self.status_extension is None and self.status_keys) or
                (self.status_extension is not None and not self.status_keys) or
                len(set(self.status_keys)) != len(self.status_keys) or
                any(type(key) is not str or not key or len(key) > 64 for key in self.status_keys) or
                (self.mcp_client is not None and
                 (not isinstance(self.mcp_client, str) or not self.mcp_client))):
            raise ValueError("invalid native reader specification")

    def build(self, store: object, capture_details: bool) -> NativeReader:
        reader = self.factory(store, capture_details)
        if not all(callable(getattr(reader, name, None)) for name in
                   ("poll", "status_sessions", "diagnostics")):
            raise TypeError(f"native reader {self.id} lacks required methods")
        if self.mcp_client and not callable(getattr(reader, "set_mcp_servers", None)):
            raise TypeError(f"native reader {self.id} lacks MCP configuration method")
        return reader


def builtin_readers() -> tuple[NativeReaderSpec, ...]:
    """Load trusted reader factories from the same bundled adapter package."""
    from .native_sources import bundled_adapter_modules

    declared = []
    for module in bundled_adapter_modules():
        factory = getattr(module, "reader_spec", None)
        if factory is None:
            continue
        if not callable(factory):
            raise ValueError(f"invalid bundled reader factory in {module.__name__}")
        spec = factory()
        if not isinstance(spec, NativeReaderSpec):
            raise ValueError(f"invalid bundled reader declaration in {module.__name__}")
        declared.append(spec)
    declared.sort(key=lambda spec: spec.id)
    if len({spec.id for spec in declared}) != len(declared):
        raise ValueError("duplicate bundled reader ID")
    return tuple(declared)
