"""Closed, host-computed Activity counts for declarative capability panes.

Selectors compare only bounded domain/action labels. Raw Activity payloads,
identifiers and revision proofs never cross the capability process pipe.
"""
from __future__ import annotations

import hashlib
import json
import re
from dataclasses import dataclass

from .agent_hierarchy import AGENT_ID
from .ui_contributions import PaneModel, PaneRow, PaneScope


MAX_COUNT_EVENTS = 200
_KEY = re.compile(r"[a-z][a-z0-9-]{0,31}\Z")
_ACTION = re.compile(r"[a-z][a-z0-9_.-]{0,31}\Z")
_LABEL = re.compile(r"[A-Za-z0-9][A-Za-z0-9 .:-]{0,13}\Z")
_REVISION = re.compile(r"[0-9a-f]{64}\Z")


@dataclass(frozen=True)
class CountSelector:
    key: str
    label: str
    domain: str
    action: str


@dataclass(frozen=True)
class CountRow:
    key: str
    count: str = ""
    numerator: str = ""
    denominator: str = ""


@dataclass(frozen=True)
class CountProfile:
    selectors: tuple[CountSelector, ...]
    rows: tuple[CountRow, ...]


@dataclass(frozen=True)
class CountFacts:
    state: str
    observed_events: int
    counts: tuple[tuple[str, int], ...]
    window_complete: bool
    capture_revision: str
    event_signature: str

    def wire(self) -> dict:
        return {"state": self.state, "observed_events": self.observed_events,
                "counts": dict(self.counts), "window_complete": self.window_complete}


def _label(raw: object) -> str:
    if not isinstance(raw, str) or not _LABEL.fullmatch(raw) or raw != raw.strip():
        raise ValueError("invalid declarative count label")
    return raw


def parse_count_profile(facts: object, model: object) -> CountProfile:
    """Accept only named exact count selectors and count/ratio display rows."""
    if not isinstance(facts, dict) or set(facts) != {"counts"}:
        raise ValueError("invalid declarative facts")
    raw = facts["counts"]
    if not isinstance(raw, list) or not 1 <= len(raw) <= 6:
        raise ValueError("invalid declarative count limit")
    selectors = []
    seen = set()
    for item in raw:
        if not isinstance(item, dict) or set(item) != {"key", "label", "domain", "action"}:
            raise ValueError("invalid declarative count selector")
        key, domain, action = item["key"], item["domain"], item["action"]
        if (not isinstance(key, str) or not _KEY.fullmatch(key) or key in seen
                or not isinstance(domain, str) or not _ACTION.fullmatch(domain)
                or not isinstance(action, str) or not _ACTION.fullmatch(action)):
            raise ValueError("invalid declarative count selector")
        seen.add(key)
        selectors.append(CountSelector(key, _label(item["label"]), domain, action))
    if not isinstance(model, dict) or set(model) != {"rows"}:
        raise ValueError("invalid declarative count model")
    raw_rows = model["rows"]
    if not isinstance(raw_rows, list) or not 1 <= len(raw_rows) <= 6:
        raise ValueError("invalid declarative count rows")
    rows = []
    used = set()
    for item in raw_rows:
        if not isinstance(item, dict) or set(item) != {"key", "value"}:
            raise ValueError("invalid declarative count row")
        key, value = item["key"], item["value"]
        if not isinstance(key, str) or not _KEY.fullmatch(key) or key in used:
            raise ValueError("invalid declarative count row key")
        if not isinstance(value, dict):
            raise ValueError("invalid declarative count formula")
        if set(value) == {"count"} and type(value["count"]) is str and value["count"] in seen:
            row = CountRow(key, count=value["count"])
        elif (set(value) == {"ratio"} and isinstance(value["ratio"], list)
              and len(value["ratio"]) == 2
              and all(type(part) is str and part in seen for part in value["ratio"])):
            row = CountRow(key, numerator=value["ratio"][0], denominator=value["ratio"][1])
        else:
            raise ValueError("invalid declarative count formula")
        used.add(key)
        rows.append(row)
    return CountProfile(tuple(selectors), tuple(rows))


def count_facts(reader, scope: PaneScope, profile: CountProfile, *, capture_allowed: bool) -> CountFacts | None:
    """Read one exact scoped page; require a reusable host policy token."""
    if (capture_allowed is not True or not isinstance(scope, PaneScope)
            or not scope.service or not scope.session_id
            or not AGENT_ID.fullmatch(scope.agent_id)):
        return None
    page = reader.activity_page(limit=MAX_COUNT_EVENTS, service=scope.service,
        session_id=scope.session_id, agent_id=scope.agent_id,
        subtree=False, include_reviews=False)
    revision = page.get("capture_revision") if isinstance(page, dict) else None
    if not isinstance(revision, str) or not _REVISION.fullmatch(revision):
        return None
    events = page.get("activity")
    if not isinstance(events, list) or len(events) > MAX_COUNT_EVENTS:
        return None
    cursor = page.get("next_cursor")
    if cursor is not None and (not isinstance(cursor, str) or len(cursor) > 512):
        return None
    exact = []
    signature = []
    seen = set()
    for event in events:
        if (not isinstance(event, dict) or event.get("service") != scope.service
                or event.get("session_id") != scope.session_id
                or event.get("agent_id") != scope.agent_id):
            return None
        event_id, stamp = event.get("id"), event.get("time_unix_nano")
        if (not isinstance(event_id, str) or not 1 <= len(event_id) <= 256
                or event_id in seen or type(stamp) is not int or stamp < 0
                or not isinstance(event.get("domain"), str)
                or not isinstance(event.get("action"), str)):
            return None
        seen.add(event_id)
        exact.append(event)
        signature.append((event_id, stamp, event["domain"], event["action"]))
    complete = cursor is None and len(exact) < MAX_COUNT_EVENTS
    counts = tuple((item.key, sum(event.get("domain") == item.domain and
                                   event.get("action") == item.action for event in exact))
                   for item in profile.selectors)
    digest = hashlib.sha256(json.dumps((signature, cursor), sort_keys=True,
        separators=(",", ":"), ensure_ascii=True).encode()).hexdigest()
    return CountFacts("ready" if complete else "degraded", len(exact), counts,
                      complete, revision, digest)


def model_from_count_facts(facts: CountFacts, profile: CountProfile,
                           *, include_coverage: bool) -> PaneModel:
    if facts.state not in ("ready", "degraded"):
        raise ValueError("unavailable declarative facts")
    values = dict(facts.counts)
    labels = {item.key: item.label for item in profile.selectors}
    coverage = ("page complete; source may be partial" if facts.window_complete
                else "recent page partial")
    rows = []
    for spec in profile.rows:
        if spec.count:
            label, rendered = labels[spec.count], str(values[spec.count])
        else:
            top, bottom = values[spec.numerator], values[spec.denominator]
            label = f"{labels[spec.numerator]} / {labels[spec.denominator]}"
            if bottom == 0:
                rendered = "unavailable"
            else:
                scaled = (top * 100 + bottom // 2) // bottom
                rendered = f"{scaled // 100}.{scaled % 100:02d}"
        cells = (label, rendered, coverage) if include_coverage else (label, rendered)
        rows.append(PaneRow(spec.key, cells))
    return PaneModel(facts.state,
        f"{facts.observed_events} scoped events · {coverage} · observations only",
        tuple(rows))
