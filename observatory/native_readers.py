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

    def __post_init__(self) -> None:
        if not isinstance(self.id, str) or not re.fullmatch(r"[a-z][a-z0-9_.-]*", self.id):
            raise ValueError("invalid native reader ID")
        if not callable(self.factory) or (self.mcp_client is not None and
                (not isinstance(self.mcp_client, str) or not self.mcp_client)):
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
    def opencode(store: object, capture_details: bool) -> NativeReader:
        from .opencode import OpenCodeReader
        return OpenCodeReader(store, capture_details=capture_details)

    return (NativeReaderSpec("opencode", opencode, mcp_client="OpenCode"),)
