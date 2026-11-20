"""Bounded, source-neutral collection coverage for terminal and plain output."""
from __future__ import annotations

import unicodedata
import re
from rich.cells import chop_cells

MAX_GROUPS = 24
MAX_COUNT = 1_000_000_000
MAX_LABEL_CELLS = 32
MAX_PROCESS_SOURCES = 4
MAX_PROCESS_STATUS = 204  # Four configured workers plus 200 retained session owners.


def _timestamp(value):
    return value if type(value) is int and 0 <= value <= 2**63 - 1 else 0


def process_source_availability(workers, sessions=()):
    """Describe exact process-source freshness without changing run outcomes.

    Supervisor entries are host-owned and capped at four sources. A restart is
    still unavailable until a successful poll follows its latest failure.
    """
    workers = workers if isinstance(workers, list) else []
    retained = {}
    for session in sessions[:200] if isinstance(sessions, list) else ():
        if isinstance(session, dict):
            source = session.get("plugin_source_id")
            if isinstance(source, str) and re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9._:-]{0,255}", source):
                retained[source] = retained.get(source, 0) + 1
    result = []
    seen = set()
    for worker in workers[:MAX_PROCESS_SOURCES]:
        if not isinstance(worker, dict):
            continue
        source = worker.get("source_id")
        if not isinstance(source, str) or not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9._:-]{0,255}", source):
            continue
        if source in seen:
            continue
        seen.add(source)
        success = _timestamp(worker.get("last_success_ns"))
        failure = _timestamp(worker.get("last_failure_ns"))
        state = worker.get("state")
        if state in {"failed", "degraded", "fenced", "quarantined", "revoked", "stopped"} or failure and failure >= success:
            availability = "unavailable"
        elif state == "running" and success:
            availability = "ready"
        else:
            availability = "pending"
        code = worker.get("last_failure_code") if availability == "unavailable" else ""
        code = code if isinstance(code, str) and re.fullmatch(r"[a-z_]{1,32}", code) else ""
        result.append({"source_id": source, "availability": availability,
                       "reason_code": code, "retained_sessions": retained.get(source, 0)})
    # An earlier process source can still have policy-current projected rows
    # after the receiver restarts without its plugin configuration. Its last
    # recorded outcomes are inspectable, but current source coverage is not.
    for source, count in retained.items():
        if source not in seen and len(result) < MAX_PROCESS_STATUS:
            result.append({"source_id": source, "availability": "unavailable",
                           "reason_code": "unmonitored", "retained_sessions": count})
    return result


def _process_statuses(local_status):
    raw = local_status.get("process_source_availability") if isinstance(local_status, dict) else None
    if not isinstance(raw, list):
        return []
    rows = []
    for entry in raw[:MAX_PROCESS_STATUS]:
        if not isinstance(entry, dict):
            continue
        source = entry.get("source_id")
        state = entry.get("availability")
        if (not isinstance(source, str) or
                not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9._:-]{0,255}", source) or
                state not in {"ready", "pending", "unavailable"}):
            continue
        display = source if len(source) <= MAX_LABEL_CELLS else source[:MAX_LABEL_CELLS - 1] + "…"
        rows.append({"source_id": source, "display": display, "availability": state,
                     "retained_sessions": _count(entry.get("retained_sessions")) or 0})
    rank = {"unavailable": 0, "pending": 1, "ready": 2}
    return sorted(rows, key=lambda item: (rank[item["availability"]], item["source_id"]))


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
    statuses = [source for source in _process_statuses(local_status)
                if source["availability"] != "ready"]
    for source in statuses[:MAX_PROCESS_SOURCES]:
        if source["availability"] != "ready":
            parts.append(f"Process source {source['display']}: {source['availability']}")
    if len(statuses) > MAX_PROCESS_SOURCES:
        parts.append(f"{len(statuses) - MAX_PROCESS_SOURCES} more process sources unavailable")
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
    source_issues = []
    statuses = [source for source in _process_statuses(local_status)
                if source["availability"] != "ready"]
    for source in statuses[:MAX_PROCESS_SOURCES]:
        if source["availability"] == "unavailable":
            source_issues.append(f"Process source {source['display']} unavailable; "
                                 "session outcomes are last recorded")
        elif source["availability"] == "pending" and source["retained_sessions"]:
            source_issues.append(f"Process source {source['display']} has not refreshed "
                                 "its retained sessions yet")
    if len(statuses) > MAX_PROCESS_SOURCES:
        source_issues.append(f"{len(statuses) - MAX_PROCESS_SOURCES} additional process sources have unconfirmed coverage")
    issues = source_issues + issues
    if len(issues) > 8:
        issues = [*issues[:8], "additional coverage limits are not shown"]
    return "Coverage limited: " + "; ".join(issues) if issues else ""
