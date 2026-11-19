"""Internal format callbacks for trusted JSONL session adapters.

This is deliberately not the external plugin SDK. Readers still run in the
collector process and receive a legacy Session context until the host-owned
batch/cursor contract and process protocol are implemented.
"""
from __future__ import annotations

from typing import Protocol


class NativeSessionAdapter(Protocol):
    def initial_task(self, row: dict) -> object: ...
    def parse(self, session: object, row: dict, stamp: int, prior_models: dict) -> list[dict]: ...
    def activity(self, session: object, row: dict, stamp: int) -> list[dict]: ...
    def observe_state(self, session: object, row: dict, stamp: int) -> tuple[str, object] | None: ...


def builtin_adapter(runtime: str) -> NativeSessionAdapter | None:
    if runtime == "Pi":
        from .adapters.pi import PiAdapter
        return PiAdapter()
    return None
