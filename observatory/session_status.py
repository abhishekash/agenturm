"""Conservative session health from lifecycle, collector, and process evidence."""
from __future__ import annotations
from datetime import datetime

RECENT_NS = 35 * 1_000_000_000
STALL_NS = 5 * 60 * 1_000_000_000


def session_phase(item):
    """Three scan labels; never changes lifecycle, process or incident evidence.

    Attention includes unavailable status that merits inspection. It does not
    imply an approval request, an error, or a notification-worthy incident.
    """
    state = item.get('state')
    if state == 'Completed':
        return 'review'
    if state in {'Working', 'Processing', 'Tool running', 'Working (quiet)'}:
        return 'running'
    return 'attention'


def task_outcome(root):
    """Read lifecycle evidence from the run itself, never from child errors.

    A successful tool/model span does not prove task completion. Unknown is
    intentional when a producer has supplied only low-level telemetry.
    """
    attrs = root.get('attributes') or {}
    state = str(attrs.get('agent.run.state') or '').strip().casefold()
    stopped = str(attrs.get('agent.stopped_reason') or '').strip().casefold()
    aliases = {'error': 'failed', 'cancelled': 'interrupted', 'canceled': 'interrupted'}
    if state in {'completed', 'failed', 'error', 'interrupted', 'cancelled', 'canceled', 'incomplete'}:
        return aliases.get(state, state)
    if stopped:
        return aliases.get(stopped, stopped) if stopped in {'completed', 'failed', 'error', 'interrupted', 'cancelled', 'canceled'} else 'incomplete'
    if state in {'running', 'waiting'}:
        return state
    if any(isinstance(attrs.get(key),str) and attrs[key].strip()
           for key in ('agent.waiting_for', 'agent.wait.reason', 'waiting_for', 'wait_reason')):
        return 'waiting'
    is_run = root.get('name') == 'agent.run' or attrs.get('gen_ai.operation.name') == 'invoke_agent'
    if is_run and root.get('status') == 'ERROR':
        return 'failed'
    return 'unknown'


def local_day_start_ns(now_ns):
    """Local calendar day, including local daylight-saving transitions."""
    return int(datetime.fromtimestamp(now_ns / 1e9).replace(
        hour=0, minute=0, second=0, microsecond=0).timestamp() * 1e9)


def today_bucket(item, now_ns=None):
    """Current work of any age; today's outcomes; older outcomes in History."""
    if now_ns is None:
        import time
        now_ns = time.time_ns()
    state = item.get('state')
    if session_phase(item) == 'running':
        return 'running'
    reason = str(item.get('waiting_for') or '').casefold()
    pending_input = state == 'Waiting' and any(
        marker in reason for marker in ('approval', 'permission', 'user input', 'input request'))
    reviewed = item.get('_workflow_reviewed', False)
    if pending_input or state == 'Possibly stalled':
        return '' if reviewed else 'attention'
    stamp = item.get('stamp') or 0
    # File mtimes and late token reports must not resurrect a completed task.
    if state in {'Completed', 'Error', 'Failed', 'Interrupted', 'Incomplete'}:
        stamp = item.get('task_outcome_ns') or item.get('state_since_ns') or stamp
    if stamp < local_day_start_ns(now_ns):
        return ''
    if state == 'Completed':
        return '' if reviewed else 'review'
    if state in {'Error', 'Failed', 'Interrupted', 'Incomplete'}:
        return '' if reviewed else 'attention'
    return '' if reviewed else 'attention'


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
        evidence = "Task outcome was not reported. Tool/span results remain available in Activity and Traces."
    return {"state": state, "active": active, "recent": recent, "stamp": stamp,
            "status_evidence": evidence}
