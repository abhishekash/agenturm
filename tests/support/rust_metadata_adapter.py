"""Test-only adapter for the Rust marked-clone metadata fixture boundary.

This helper deliberately implements only a subset of the Python admission
port's behavior. It is not imported by observatory runtime modules and does
not claim PluginAdmissionPort conformance.
"""
from __future__ import annotations

import json
import sqlite3
import subprocess
from dataclasses import dataclass
from pathlib import Path

from observatory.plugin_admission import _validated
from observatory.plugin_wire import PollBatch


SUPPORTED_RECORD_KINDS = frozenset({"lifecycle", "message", "tool", "check", "usage"})


def logical_clone_snapshot(path: str | Path) -> tuple:
    """Read every schema object and table row for no-write dispatch checks."""
    with sqlite3.connect(path) as db:
        schema = tuple(tuple(row) for row in db.execute(
            "SELECT type,name,tbl_name,sql FROM sqlite_master "
            "WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name"))
        tables = [row[0] for row in db.execute(
            "SELECT name FROM sqlite_master WHERE type='table' "
            "AND name NOT LIKE 'sqlite_%' ORDER BY name")]
        contents = []
        for table in tables:
            quoted = '"' + table.replace('"', '""') + '"'
            rows = tuple(sorted((tuple(row) for row in db.execute(
                f"SELECT * FROM {quoted}")), key=repr))
            contents.append((table, rows))
        return schema, tuple(contents)


class RustFixtureError(RuntimeError):
    """A marked-clone fixture operation failed in the Rust executable."""


class UnsupportedMetadataSubset(ValueError):
    """The request is valid Python wire data outside the Rust fixture subset."""


@dataclass(frozen=True)
class RustMetadataLease:
    receiver_id: str
    plugin_id: str
    source_id: str
    generation: str
    epoch: int
    policy_epoch: int
    cursor: str

    def wire(self) -> dict:
        return {"receiver_id": self.receiver_id, "plugin_id": self.plugin_id,
                "source_id": self.source_id, "generation": self.generation,
                "epoch": self.epoch, "policy_epoch": self.policy_epoch}


class RustMarkedCloneMetadataAdapter:
    """Exercise feature-off v2 metadata operations on one marked clone path.

    Supported: source activation, content-free lifecycle and typed fact batches,
    capture policy changes, source revoke, and exact durable replay. Diagnostics,
    relationships, discovery cursors/root gaps, reference providers, and
    reference payloads are deliberately outside this test adapter.
    """

    def __init__(self, binary: str | Path, clone_path: str | Path):
        self.binary = str(Path(binary).resolve(strict=True))
        self.clone_path = Path(clone_path).resolve(strict=True)
        self._closed = False

    @staticmethod
    def _lease(result: dict, *, cursor: str | None = None) -> RustMetadataLease:
        lease = result.get("lease", result)
        cursor_value = result.get("cursor", cursor)
        if cursor_value is None:
            cursor_value = ""
        return RustMetadataLease(
            receiver_id=lease["receiver_id"], plugin_id=lease["plugin_id"],
            source_id=lease["source_id"], generation=lease["generation"],
            epoch=lease["epoch"], policy_epoch=lease["policy_epoch"],
            cursor=cursor_value)

    def _run(self, mode: str, request: dict) -> dict:
        if self._closed:
            raise RustFixtureError("metadata fixture adapter is closed")
        raw = json.dumps(request, sort_keys=True, separators=(",", ":"),
                         ensure_ascii=False)
        result = subprocess.run(
            [self.binary, mode, str(self.clone_path)], input=raw, text=True,
            capture_output=True, timeout=10)
        if result.returncode != 0:
            raise RustFixtureError(result.stderr.strip())
        try:
            return json.loads(result.stdout)
        except json.JSONDecodeError as error:
            raise RustFixtureError("Rust fixture returned malformed JSON") from error

    @staticmethod
    def _request(request_id: str, receiver_id: str, operation: dict) -> dict:
        return {"version": 2, "request_id": request_id,
                "receiver_id": receiver_id, "operation": operation}

    def activate_source(self, *, plugin_id: str, source_id: str,
                        generation: str, request_id: str) -> RustMetadataLease:
        request = self._request(request_id, self._receiver_id(), {
            "op": "activate_source", "plugin_id": plugin_id,
            "source_id": source_id, "generation": generation,
        })
        result = self._run("--v2-activation-clone-path", request)["result"]
        return self._lease(result)

    def _receiver_id(self) -> str:
        import sqlite3

        with sqlite3.connect(self.clone_path) as db:
            row = db.execute(
                "SELECT value FROM schema_meta WHERE key='receiver_id'").fetchone()
        if row is None:
            raise RustFixtureError("marked clone has no receiver identity")
        return row[0]

    def batch_request(self, lease: RustMetadataLease, expected_cursor: str,
                      batch: PollBatch, *, request_id: str,
                      reference_refs: tuple = ()) -> dict:
        # Python's existing validation and metadata minimization define the host
        # side of this limited fixture protocol before Rust receives any rows.
        digest, rows, checked = _validated(batch, lease.source_id)
        if reference_refs:
            raise UnsupportedMetadataSubset("reference payloads are not admitted")
        if checked.member_id or checked.member_cursor:
            raise UnsupportedMetadataSubset("discovery member cursors are not admitted")
        if checked.root_gap:
            raise UnsupportedMetadataSubset("discovery root gaps are not admitted")
        unsupported = sorted({row[3] for row in rows} - SUPPORTED_RECORD_KINDS)
        if unsupported:
            raise UnsupportedMetadataSubset(
                f"unsupported metadata record kinds: {', '.join(unsupported)}")
        if any(row[2] != 0 for row in rows):
            raise UnsupportedMetadataSubset("correction revisions are not admitted")
        records = [{
            "event_id_hash": row[1], "revision": row[2], "kind": row[3],
            "time_ns": row[4], "payload_json": row[5], "record_digest": row[6],
            "native_session_id": row[7], "native_worker_id": row[8],
            "native_turn_id": row[9], "native_operation_id": row[10],
        } for row in rows]
        return self._request(request_id, lease.receiver_id, {
            "op": "admit_batch", "lease": lease.wire(),
            "expected_cursor": expected_cursor, "cursor": checked.cursor,
            "batch_digest": digest, "records": records,
        })

    def commit_batch(self, lease: RustMetadataLease, expected_cursor: str,
                     batch: PollBatch, *, request_id: str,
                     reference_refs: tuple = ()) -> dict:
        request = self.batch_request(lease, expected_cursor, batch,
                                     request_id=request_id,
                                     reference_refs=reference_refs)
        return self._run("--v2-mixed-clone-batch", request)["result"]

    def set_capture_enabled(self, lease: RustMetadataLease, *, enabled: bool,
                            expected_enabled: bool, expected_cursor: str,
                            expected_dirty_seq: int, request_id: str) -> dict:
        request = self._request(request_id, lease.receiver_id, {
            "op": "set_source_capture", "lease": lease.wire(),
            "enabled": enabled, "expected_capture_enabled": expected_enabled,
            "expected_cursor": expected_cursor,
            "expected_dirty_seq": expected_dirty_seq,
        })
        result = self._run("--v2-clone-policy-path", request)["result"]
        next_lease = self._lease({"lease": result["lease"], "cursor": result["cursor"]})
        return {**result, "lease": next_lease}

    def revoke_source(self, lease: RustMetadataLease, *, cursor: str,
                      expected_dirty_seq: int, request_id: str) -> dict:
        request = self._request(request_id, lease.receiver_id, {
            "op": "revoke_source", "lease": lease.wire(), "cursor": cursor,
            "expected_dirty_seq": expected_dirty_seq,
        })
        return self._run("--v2-clone-policy-path", request)["result"]

    def capture_revoked(self, source_id: str) -> bool:
        import sqlite3

        with sqlite3.connect(self.clone_path) as db:
            row = db.execute(
                "SELECT capture_enabled FROM plugin_source_cursor_v1 "
                "WHERE source_id=? AND receiver_id=?",
                (source_id, self._receiver_id())).fetchone()
        return row is not None and not bool(row[0])

    def send_normal_dispatch(self, request: dict) -> None:
        """Use the service-facing activation clone dispatcher (expected to reject subset ops)."""
        self._run("--v2-activation-clone-path", request)

    def close(self) -> None:
        self._closed = True
