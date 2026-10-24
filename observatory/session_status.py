"""Conservative session health from lifecycle, collector, and process evidence."""
from __future__ import annotations

RECENT_NS = 35 * 1_000_000_000
STALL_NS = 5 * 60 * 1_000_000_000


def session_health(entry, now_ns):
    diagnostics = entry.get("diagnostics") or {}
    last = entry.get("last_activity") or {}
    stamp = max(last.get("time_unix_nano") or 0, entry.get("last_record_ns") or 0,
                diagnostics.get("source_mtime_ns") or 0)
    quiet_ns = max(0, now_ns - stamp) if stamp else STALL_NS
    recent = bool(stamp and quiet_ns < RECENT_NS)
    reported = str(entry.get("run_state") or "unknown").casefold()
    reason = str(entry.get("waiting_for") or "").casefold()
    process = entry.get("process") or {}
    live = bool(process.get("pids")) and now_ns - process.get("observed_at_ns", 0) < 15 * 1_000_000_000
    process_quiet_ns = now_ns - max(process.get("last_progress_ns", 0), process.get("observed_since_ns", now_ns))
    progressing = live and bool(process.get("last_progress_ns")) and now_ns - process["last_progress_ns"] < RECENT_NS
    evidence = entry.get("status_evidence") or "Status comes from recorded lifecycle events."
    active = False
    if reported in {"completed", "interrupted", "error", "failed", "incomplete", "idle"}:
        state = reported.title()
    elif reported == "waiting" and not reason.startswith("tool response:"):
        state = "Waiting"
        evidence = "An explicit approval or input wait remains pending until the agent reports a response or a new turn."
    elif reported in {"running", "waiting"}:
        read_ns = diagnostics.get("last_success_ns") or entry.get("collector_updated_ns") or 0
        if diagnostics.get("backlog_bytes", 0) > 0:
            state, evidence = "Catching up", "The collector has unread session records; live status is not yet reliable."
        elif read_ns and now_ns - read_ns > 15 * 1_000_000_000:
            state, evidence = "Monitor delayed", "The collector has not completed a read in over 15 seconds; agent health is unconfirmed."
        elif recent or progressing:
            state = "Tool running" if reason.startswith("tool response:") else "Working" if recent else "Processing"
            active = True
            evidence = ("The exact session's process or its children are using CPU / I/O." if progressing and not recent
                        else "Session records are advancing; the turn has not reported completion.")
        elif quiet_ns < STALL_NS or live and process_quiet_ns < STALL_NS:
            state, active = "Working (quiet)", True
            evidence = "The turn is unfinished. A quiet model or tool call is not evidence of a stall."
        elif live:
            state = "Possibly stalled"
            evidence = ("The session's process is alive, but no session records or observed CPU / I/O progress "
                        "have advanced for at least 5 minutes. This is a warning, not proof of a hang; check the agent terminal.")
        else:
            state = "Unconfirmed"
            evidence = ("The last turn is unfinished, but no new records arrived for at least 5 minutes and no live "
                        "process could be linked to this session. Cannot distinguish a slow request from an exited agent.")
    else:
        # A freshly created transcript/header is not an active turn.
        state = "Unknown"
    return {"state": state, "active": active, "recent": recent, "stamp": stamp,
            "status_evidence": evidence}
