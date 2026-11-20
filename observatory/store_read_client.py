"""A narrow, query-only client for an already initialized Agenturm Store.

This is an opt-in authority seam, not a replacement for the writer. It never
constructs ``Store`` or installs schema, and exposes only a bounded plugin
session query. SQLite may use existing WAL shared-memory sidecars while
serving a live read; the client never opens a write connection to the Store.
"""
from __future__ import annotations

import re
import sqlite3
import stat
import threading
from pathlib import Path

from .plugin_projection import render_session_view, session_rows
from .plugin_wire import ProtocolError
from .read_connections import ReadConnections


_RECEIVER = re.compile(r"[0-9a-f]{32}\Z")
_SCHEMA_MARKER = "plugin_sessions_read_schema"
_REQUIRED = {
    "schema_meta": {"key": "TEXT", "value": "TEXT"},
    "plugin_source_cursor_v1": {
        "source_id": "TEXT", "receiver_id": "TEXT", "plugin_id": "TEXT",
        "dirty_seq": "INTEGER", "policy_epoch": "INTEGER",
        "capture_enabled": "INTEGER",
    },
    "plugin_projection_cursor_v1": {
        "source_id": "TEXT", "projected_seq": "INTEGER",
        "policy_epoch": "INTEGER",
    },
    "plugin_session_projection_v1": {
        "source_id": "TEXT", "native_session_id": "TEXT", "session_key": "TEXT",
        "state": "TEXT", "reason_code": "TEXT", "state_time_ns": "INTEGER",
        "last_record_ns": "INTEGER", "policy_epoch": "INTEGER",
        "source_dirty_seq": "INTEGER", "usage_json": "TEXT",
        "parent_native_session_id": "TEXT", "relationship_kind": "TEXT",
        "relationship_state": "TEXT",
    },
    "plugin_retention_v1": {
        "source_id": "TEXT", "policy_epoch": "INTEGER",
        "watermark_ns": "INTEGER", "dropped_events": "INTEGER",
    },
}


def _receiver(db) -> str:
    """Reject an unknown read contract before interpreting projection rows."""
    for table, required in _REQUIRED.items():
        found = db.execute("SELECT type FROM sqlite_master WHERE name=?", (table,)).fetchone()
        if found is None or found[0] != "table":
            raise ProtocolError("unsupported Store read schema")
        columns = {row[1]: row[2].upper() for row in db.execute(
            f'PRAGMA table_info("{table}")')}
        if any(columns.get(name) != declaration for name, declaration in required.items()):
            raise ProtocolError("unsupported Store read schema")
    marker = db.execute("SELECT value FROM schema_meta WHERE key=?",
                        (_SCHEMA_MARKER,)).fetchone()
    if marker is None or marker[0] != "1":
        raise ProtocolError("unsupported Store read schema version")
    row = db.execute("SELECT value FROM schema_meta WHERE key='receiver_id'").fetchone()
    if row is None or not isinstance(row[0], str) or not _RECEIVER.fullmatch(row[0]):
        raise ProtocolError("Store receiver identity unavailable")
    return row[0]


def _target(path: Path) -> tuple[int, int]:
    """Bind the regular target, including an explicitly configured symlink."""
    try:
        info = path.stat()
    except OSError as exc:
        raise ProtocolError("Store read client file unavailable") from exc
    if not stat.S_ISREG(info.st_mode):
        raise ProtocolError("Store read client requires a regular database")
    return info.st_dev, info.st_ino


class StoreReadClient:
    """Read one versioned Store query without Store initialization or write APIs."""

    __slots__ = ("_path", "_reads", "_bound", "_lock")

    def __init__(self, path: str | Path):
        self._path = Path(path).absolute()
        self._reads = ReadConnections(self._path)
        self._bound: tuple[tuple[int, int], str] | None = None
        self._lock = threading.Lock()

    @property
    def path(self) -> Path:
        return self._path

    def plugin_sessions(self, limit: int = 200) -> dict:
        """Return the exact policy-current Store plugin session view."""
        if type(limit) is not int or not 1 <= limit <= 1_000:
            raise ValueError("invalid plugin session limit")
        with self._lock:
            before = _target(self.path)
            if self._bound is not None and self._bound[0] != before:
                raise ProtocolError("Store read client target changed")
            try:
                with self._reads.read() as db:
                    db.execute("BEGIN")
                    receiver_id = _receiver(db)
                    if self._bound is not None and self._bound[1] != receiver_id:
                        raise ProtocolError("Store read client receiver changed")
                    snapshot = session_rows(db, receiver_id, limit=limit)
            except ProtocolError:
                raise
            except (OSError, sqlite3.Error, ValueError, TypeError, KeyError,
                    IndexError, OverflowError) as exc:
                raise ProtocolError("Store read client unavailable") from exc
            try:
                result = render_session_view(snapshot, limit=limit)
            except (ValueError, TypeError, KeyError, IndexError, OverflowError) as exc:
                raise ProtocolError("Store read client unavailable") from exc
            if _target(self.path) != before:
                raise ProtocolError("Store read client target changed")
            if self._bound is None:
                self._bound = (before, receiver_id)
            return result

    def close(self) -> None:
        with self._lock:
            self._reads.close()

    def __enter__(self):
        return self

    def __exit__(self, _kind, _value, _traceback):
        self.close()
