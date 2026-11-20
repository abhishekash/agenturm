"""Trusted native turn review contributions behind one Store query port.

Only bundled adapter modules may register callbacks. Process adapters publish
typed evidence through the admission contract; they do not execute Store SQL.
"""
from __future__ import annotations

from dataclasses import dataclass
import re
from typing import Callable

from .native_sources import bundled_adapter_modules


@dataclass(frozen=True)
class NativeTurnReviewSpec:
    source_id: str
    review: Callable[[object, str], dict | None]

    def __post_init__(self) -> None:
        if (not isinstance(self.source_id, str) or
                not re.fullmatch(r"[a-z][a-z0-9_.-]*", self.source_id)):
            raise ValueError("invalid native turn review source")
        if not callable(self.review):
            raise ValueError("native turn review requires a callback")


def review_turn(db: object, trace_id: str) -> dict | None:
    """Use registered bundled reviewers; reject conflicting exact ownership."""
    specs = []
    source_ids: set[str] = set()
    for module in bundled_adapter_modules():
        factory = getattr(module, "turn_review_spec", None)
        if factory is None:
            continue
        if not callable(factory):
            raise ValueError("invalid bundled turn review factory")
        spec = factory()
        if not isinstance(spec, NativeTurnReviewSpec):
            raise ValueError("invalid bundled turn review declaration")
        if spec.source_id in source_ids:
            raise ValueError("duplicate bundled turn review source")
        source_ids.add(spec.source_id)
        specs.append(spec)
    result = None
    for spec in specs:
        candidate = spec.review(db, trace_id)
        if candidate is not None:
            if result is not None:
                raise ValueError("multiple native turn reviewers claim one trace")
            result = candidate
    return result
