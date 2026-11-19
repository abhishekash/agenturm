"""Textual-free declarations for trusted, opt-in capability panes.

The shell owns layout, navigation and rendering. A provider answers a scoped
query with bounded view data; no provider receives a Textual widget or a raw
receiver snapshot through this interface.
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Callable


_PANE_ID = re.compile(r"[a-z][a-z0-9-]{0,31}\Z")
_RESERVED = frozenset({"agents", "activity", "mcp", "traces", "host", "metrics", "findings"})
MAX_PANES = 8
MAX_ROWS = 500


@dataclass(frozen=True)
class PaneScope:
    agent_id: str = ""
    service: str = ""
    session_id: str = ""


@dataclass(frozen=True)
class PaneRow:
    key: str
    cells: tuple[str, ...]
    detail: str = ""


@dataclass(frozen=True)
class PaneModel:
    state: str
    summary: str
    rows: tuple[PaneRow, ...] = ()


@dataclass(frozen=True)
class PaneContribution:
    id: str
    title: str
    columns: tuple[tuple[str, int, int], ...]
    query: Callable[[PaneScope], PaneModel]


def _safe_text(value: object, limit: int) -> bool:
    return isinstance(value, str) and len(value) <= limit and not any(
        ord(char) < 32 or ord(char) == 127 for char in value)


class PaneRegistry:
    def __init__(self, contributions: tuple[PaneContribution, ...] = ()):
        if len(contributions) > MAX_PANES:
            raise ValueError("too many contributed panes")
        self._panes: dict[str, PaneContribution] = {}
        for pane in contributions:
            self.register(pane)

    def register(self, pane: PaneContribution) -> None:
        if not isinstance(pane, PaneContribution):
            raise TypeError("pane declaration required")
        if not isinstance(pane.id, str) or not _PANE_ID.fullmatch(pane.id) or pane.id in _RESERVED:
            raise ValueError("invalid or reserved pane ID")
        if pane.id in self._panes or len(self._panes) >= MAX_PANES:
            raise ValueError("duplicate or excessive pane")
        if not _safe_text(pane.title, 40) or not pane.title or not callable(pane.query):
            raise ValueError("invalid pane title or query")
        if not isinstance(pane.columns, tuple) or not 1 <= len(pane.columns) <= 6:
            raise ValueError("pane needs one to six columns")
        for column in pane.columns:
            if (not isinstance(column, tuple) or len(column) != 3
                    or not _safe_text(column[0], 32) or not column[0]
                    or type(column[1]) is not int or not 4 <= column[1] <= 80
                    or type(column[2]) is not int or not 0 <= column[2] <= 4):
                raise ValueError("invalid pane column")
        self._panes[pane.id] = pane

    def get(self, pane_id: str) -> PaneContribution | None:
        return self._panes.get(pane_id)

    def all(self) -> tuple[PaneContribution, ...]:
        return tuple(self._panes.values())


def validate_model(pane: PaneContribution, model: PaneModel) -> PaneModel:
    if (not isinstance(model, PaneModel) or not isinstance(model.state, str)
            or model.state not in {"ready", "unavailable", "degraded"}
            or not _safe_text(model.summary, 240) or not isinstance(model.rows, tuple)
            or len(model.rows) > MAX_ROWS):
        raise ValueError("invalid pane view model")
    seen = set()
    for row in model.rows:
        if (not isinstance(row, PaneRow) or not _safe_text(row.key, 128) or not row.key
                or row.key in seen or not isinstance(row.cells, tuple)
                or len(row.cells) != len(pane.columns) or not _safe_text(row.detail, 2048)
                or any(not _safe_text(cell, 240) for cell in row.cells)):
            raise ValueError("invalid pane row")
        seen.add(row.key)
    return model
