"""Closed, host-verifiable display formulas for external capability panes.

The child receives only host-owned aggregate facts. Its model is accepted only
when it exactly matches these configured formulas; labels and coverage are
rendered by the host, not supplied as arbitrary process output.
"""
from __future__ import annotations

import re
from dataclasses import dataclass

from .capability_facts import PromptSignalFacts
from .ui_contributions import PaneModel, PaneRow


_KEY = re.compile(r"[a-z][a-z0-9-]{0,31}\Z")
_LABELS = {
    "observed_events": "Scoped Activity events",
    "user_messages": "Recorded user messages",
    "shell_actions": "Recorded shell actions",
    "first_shell_after_user_ms": "First shell action after a user message",
}
_RATIO_LABELS = {
    "observed_events": "Activity events",
    "user_messages": "user messages",
    "shell_actions": "shell actions",
}


@dataclass(frozen=True)
class ModelRowSpec:
    key: str
    fact: str = ""
    numerator: str = ""
    denominator: str = ""


def parse_model_rows(raw: object) -> tuple[ModelRowSpec, ...]:
    """Validate a bounded v2 descriptor without evaluating plugin code."""
    if not isinstance(raw, dict) or set(raw) != {"rows"}:
        raise ValueError("invalid capability model declaration")
    rows = raw["rows"]
    if not isinstance(rows, list) or not 1 <= len(rows) <= 6:
        raise ValueError("capability needs one to six declared rows")
    parsed = []
    seen = set()
    for row in rows:
        if not isinstance(row, dict) or set(row) != {"key", "value"}:
            raise ValueError("invalid capability row declaration")
        key, value = row["key"], row["value"]
        if not isinstance(key, str) or not _KEY.fullmatch(key) or key in seen:
            raise ValueError("invalid or duplicate capability row key")
        if not isinstance(value, dict):
            raise ValueError("invalid capability value formula")
        if set(value) == {"fact"} and type(value["fact"]) is str and value["fact"] in _LABELS:
            spec = ModelRowSpec(key, fact=value["fact"])
        elif (set(value) == {"ratio"} and isinstance(value["ratio"], list)
              and len(value["ratio"]) == 2
              and all(type(part) is str and part in _RATIO_LABELS for part in value["ratio"])):
            spec = ModelRowSpec(key, numerator=value["ratio"][0],
                                denominator=value["ratio"][1])
        else:
            raise ValueError("unsupported capability value formula")
        seen.add(key)
        parsed.append(spec)
    return tuple(parsed)


def model_from_facts(facts: PromptSignalFacts,
                     rows: tuple[ModelRowSpec, ...], *, include_coverage: bool) -> PaneModel:
    """Build the only model a schema-v2 child may return for these facts."""
    if facts.state not in {"ready", "degraded"}:
        raise ValueError("unavailable facts cannot form a pane model")
    coverage = "complete recent window" if facts.window_complete else "partial recent window"
    result = []
    for spec in rows:
        if spec.fact:
            value = getattr(facts, spec.fact)
            rendered = ("unavailable" if value is None else
                        f"{value} ms" if spec.fact == "first_shell_after_user_ms" else str(value))
            label = _LABELS[spec.fact]
        else:
            numerator = getattr(facts, spec.numerator)
            denominator = getattr(facts, spec.denominator)
            if denominator == 0:
                rendered = "unavailable"
            else:
                # Half-up to two decimals using bounded integer arithmetic.
                scaled = (numerator * 100 + denominator // 2) // denominator
                rendered = f"{scaled // 100}.{scaled % 100:02d}"
            label = f"{_RATIO_LABELS[spec.numerator]} / {_RATIO_LABELS[spec.denominator]}"
        cells = (label, rendered, coverage) if include_coverage else (label, rendered)
        result.append(PaneRow(spec.key, cells))
    return PaneModel(facts.state,
                     f"{facts.observed_events} scoped events · {coverage} · observational signals only",
                     tuple(result))
