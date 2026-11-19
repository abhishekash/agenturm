"""Internal format callbacks for trusted JSONL session adapters.

This is deliberately not the external plugin SDK. Readers still run in the
collector process and receive a legacy Session context until the host-owned
batch/cursor contract and process protocol are implemented.
"""
from __future__ import annotations

from typing import Protocol


class NativeSessionAdapter(Protocol):
    # Format adapters may additionally implement initialize,
    # reset_after_rotation, bounded_tail, continuation_root, finish_poll and
    # status_metadata, link_sessions and status_extension. The generic collector calls only hooks present on the
    # registered adapter; new formats need no runtime-name branch here.
    def initial_task(self, row: dict) -> object: ...
    def parse(self, session: object, row: dict, stamp: int, prior_models: dict) -> list[dict]: ...
    def activity(self, session: object, row: dict, stamp: int) -> list[dict]: ...
    def observe_state(self, session: object, row: dict, stamp: int) -> tuple[str, object] | None: ...


def builtin_adapter(runtime: str) -> NativeSessionAdapter | None:
    # Legacy direct Session construction uses the same declarations as the
    # collector. New source formats need no second runtime-name branch.
    from .native_sources import builtin_sources

    matching = [spec for spec in builtin_sources() if spec.runtime == runtime]
    if len(matching) > 1:
        raise ValueError(f"multiple bundled adapters for {runtime}")
    return matching[0].build_adapter() if matching else None
