"""Bounded, source-neutral collection coverage for terminal and plain output."""
from __future__ import annotations

import unicodedata
import re
from rich.cells import chop_cells

MAX_GROUPS = 24
MAX_COUNT = 1_000_000_000
MAX_LABEL_CELLS = 32


def _count(value):
    return value if type(value) is int and 0 <= value <= MAX_COUNT else None


def _label(value):
    if not isinstance(value, str):
        return ""
    value = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", value[:1024])
    clean = "".join(char for char in value if unicodedata.category(char) not in {"Cc", "Cf"})
    clean = clean.strip()
    return chop_cells(clean, MAX_LABEL_CELLS)[0] if clean else ""


def coverage_rows(local_status):
    """Accept only bounded host-owned descriptors; retain a legacy generic view."""
    if not isinstance(local_status, dict):
        return []
    declared = local_status.get("source_coverage")
    if not isinstance(declared, list):
        if not any(key in local_status for key in ("sources", "omitted_sources",
                                                 "opencode_discovered_sessions",
                                                 "opencode_omitted_sessions")):
            return []
        # Older receivers used separate counters for the directory collector
        # and its one native reader. Keep their known omissions visible without
        # carrying runtime-name dispatch into either UI.
        declared = [{"label": "Native JSONL", "unit": "files",
                     "active": local_status.get("sources"),
                     "omitted": local_status.get("omitted_sources"),
                     "limit": local_status.get("max_sources"), "state": "ready"}]
        if (local_status.get("opencode_discovered_sessions") or
                local_status.get("opencode_omitted_sessions")):
            declared.append({"label": "Legacy reader", "unit": "sessions",
                             "active": None,
                             "omitted": local_status.get("opencode_omitted_sessions"),
                             "limit": 50, "state": "partial"})
    rows = []
    for raw in declared[:MAX_GROUPS]:
        if not isinstance(raw, dict):
            continue
        label = _label(raw.get("label"))
        unit = raw.get("unit")
        if not label or not isinstance(unit, str) or unit not in {"files", "sessions"}:
            continue
        raw_state = raw.get("state")
        state = raw_state if isinstance(raw_state, str) and raw_state in {"ready", "partial", "unavailable"} else "unavailable"
        rows.append({"label": label, "unit": unit, "state": state,
                     "active": _count(raw.get("active")),
                     "omitted": _count(raw.get("omitted")),
                     "omitted_exact": raw.get("omitted_exact", True) is True,
                     "limit": _count(raw.get("limit")),
                     "pending": _count(raw.get("pending")) or 0})
    return rows


def coverage_summary(local_status):
    """Compact Host/CLI summary without naming any harness in rendering code."""
    rows = coverage_rows(local_status)
    if not rows:
        return "Collection coverage unavailable"
    units = []
    for unit in ("files", "sessions"):
        group = [row for row in rows if row["unit"] == unit]
        if group:
            if all(row["active"] is None for row in group):
                units.append(f"{unit} count unavailable")
                continue
            total = sum(row["active"] or 0 for row in group)
            qualifier = "at least " if any(row["active"] is None for row in group) else ""
            name = unit[:-1] if total == 1 and not qualifier else unit
            units.append(f"{qualifier}{total} {name} observed")
    count = ", ".join(units)
    omitted = sum(row["omitted"] or 0 for row in rows)
    unknown = any(row["omitted"] is None or not row["omitted_exact"] for row in rows)
    omission = f"{omitted} omitted" if not unknown else f"at least {omitted} omitted; some counts unavailable"
    return f"{len(rows)} source groups · {count} · {omission}"


def coverage_detail(local_status):
    rows = coverage_rows(local_status)
    if not rows:
        return "Source coverage unavailable"
    parts = []
    for row in rows[:6]:
        active = str(row["active"]) if row["active"] is not None else "unknown"
        part = f"{row['label']}: {active} {row['unit']}"
        if row["limit"] is not None:
            part += f" / {row['limit']} limit"
        if row["state"] != "ready":
            part += f" ({row['state']})"
        parts.append(part)
    if (len(rows) > 6 or _count(local_status.get("source_coverage_overflow")) or
            isinstance(local_status.get("source_coverage"), list) and
            len(local_status["source_coverage"]) > MAX_GROUPS):
        parts.append("More source groups available")
    return "; ".join(parts)


def coverage_warning(local_status):
    rows = coverage_rows(local_status)
    issues = []
    for row in rows:
        named_issue = False
        if row["omitted"]:
            limit = f" (limit {row['limit']})" if row["limit"] is not None else ""
            at_least = "at least " if not row["omitted_exact"] else ""
            unit = row["unit"][:-1] if row["omitted"] == 1 and not at_least else row["unit"]
            issues.append(f"{at_least}{row['omitted']} {row['label']} {unit} omitted{limit}")
            named_issue = True
        elif row["omitted"] is None and row["state"] == "partial":
            issues.append(f"{row['label']} omission count unavailable")
            named_issue = True
        if row["pending"]:
            issues.append(f"{row['pending']} {row['label']} sources pending")
            named_issue = True
        if row["state"] == "unavailable":
            issues.append(f"{row['label']} coverage unavailable")
        elif row["state"] == "partial" and not named_issue:
            issues.append(f"{row['label']} coverage partial")
    hierarchy = local_status.get("hierarchy") if isinstance(local_status, dict) else None
    hierarchy = hierarchy if isinstance(hierarchy, dict) else {}
    if hierarchy.get("coverage") == "partial":
        announced = _count(hierarchy.get("announced")) or 0
        issues.append(f"{announced} spawn announcements or partial child evidence; relationship coverage is incomplete")
    if (isinstance(local_status, dict) and
            (_count(local_status.get("source_coverage_overflow")) or
             isinstance(local_status.get("source_coverage"), list) and
             len(local_status["source_coverage"]) > MAX_GROUPS)):
        issues.append("additional source groups are not shown")
    if len(issues) > 8:
        issues = [*issues[:8], "additional coverage limits are not shown"]
    return "Coverage limited: " + "; ".join(issues) if issues else ""
