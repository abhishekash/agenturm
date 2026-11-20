"""Exact, source-owned capture decisions for receiver capability reads."""
from __future__ import annotations


def capture_sources(local: dict, *, service: str, session_id: str,
                    agent_id: str) -> tuple[str, ...] | None:
    """Return every approved evidence source for one exact actor, or deny.

    Health rows describe a session; ``evidence_source_id`` identifies the
    source that actually wrote its Activity. A display-only session ID is not
    sufficient authority to consume another source's records.
    """
    if not isinstance(local, dict) or local.get("capture_content") is not True:
        return None
    rows = local.get("sessions")
    if not isinstance(rows, list):
        return None
    matches = [row for row in rows if isinstance(row, dict)
               and row.get("service") == service
               and row.get("session_id") == session_id
               and row.get("agent_id") == agent_id]
    if not matches or len(matches) > 32 or any(
            row.get("capture_content") is not True or
            not isinstance(row.get("evidence_source_id", row.get("source_id")), str) or
            not 0 < len(row.get("evidence_source_id", row.get("source_id"))) <= 200
            for row in matches):
        return None
    return tuple(sorted({row.get("evidence_source_id", row.get("source_id"))
                         for row in matches}))
