"""Pure validation for one explicitly requested process Activity text preview.

The text is deliberately not an Activity field or a ReviewHistory event. UI
callers keep it only in transient inspector state; copy and export continue to
use the content-free projected row.
"""
from __future__ import annotations

from .plugin_activity import _EVENT_KEY
from .workflow_evidence import safe_text


def eligible(record) -> bool:
    return (isinstance(record, dict) and record.get('source') == 'plugin'
            and record.get('domain') == 'message' and record.get('action') == 'user'
            and isinstance(record.get('id'), str)
            and bool(_EVENT_KEY.fullmatch(record['id'])))


def accepted_preview(response, record, receiver_id, before, after):
    """Require exact selected row and unchanged receiver revision around HTTP I/O."""
    if not eligible(record) or before is None or before != after or not receiver_id:
        return None
    if not isinstance(response, dict) or response.get('receiver_id') != receiver_id \
            or response.get('event_id') != record['id']:
        return None
    revision = response.get('revision')
    text = response.get('redacted_text')
    clipped = response.get('clipped')
    if (not isinstance(revision, list) or len(revision) != 3
            or any(type(value) is not int or value < 0 for value in revision)
            or tuple(revision) != before or type(text) is not str
            or len(text.encode('utf-8')) > 4096 or safe_text(text) != text
            or type(clipped) is not bool):
        return None
    return text, clipped
