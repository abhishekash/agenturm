"""Test-only adapter for the Rust marked-clone metadata fixture boundary.

This helper deliberately implements only a subset of the Python admission
port's behavior. It is not imported by observatory runtime modules and does
not claim PluginAdmissionPort conformance.
"""
from __future__ import annotations

import hashlib
import json
import sqlite3
import subprocess
import uuid
from dataclasses import dataclass
from pathlib import Path

from observatory.plugin_admission import _validated
from observatory.plugin_reference_admission import DEFAULT_REFERENCE_ADMISSIONS
from observatory.plugin_wire import PollBatch


SUPPORTED_RECORD_KINDS = frozenset({
    "lifecycle", "message", "tool", "check", "usage", "diagnostic",
})


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

    Supported: the session-facing activation/read/commit shape for content-free
    lifecycle, typed fact, diagnostic, and one-member gap batches, plus fixture
    capture policy changes and source revoke. This remains a deliberately
    incomplete subset: a separate test-only façade exercises PluginSession's
    structural seam, but no runtime composition uses this adapter and it does
    not claim full PluginAdmissionPort conformance. Relationships, root gaps, references,
    correction revisions, and raw content stay outside this adapter.
    """

    def __init__(self, binary: str | Path, clone_path: str | Path):
        self.binary = str(Path(binary).resolve(strict=True))
        self.clone_path = Path(clone_path).resolve(strict=True)
        self._closed = False

    @staticmethod
    def _request_id() -> str:
        return uuid.uuid4().hex

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
                        generation: str, request_id: str | None = None,
                        content_fingerprint: str = "",
                        reference_admissions=DEFAULT_REFERENCE_ADMISSIONS
                        ) -> RustMetadataLease:
        if content_fingerprint:
            raise UnsupportedMetadataSubset("content fingerprints are not admitted")
        if reference_admissions is not DEFAULT_REFERENCE_ADMISSIONS:
            raise UnsupportedMetadataSubset("custom reference providers are not admitted")
        request_id = request_id or self._request_id()
        request = self._request(request_id, self._receiver_id(), {
            "op": "activate_source", "plugin_id": plugin_id,
            "source_id": source_id, "generation": generation,
        })
        result = self._run("--v2-activation-clone-path", request)["result"]
        return self._lease(result)

    def install_reference_schema(self, reference_admissions=DEFAULT_REFERENCE_ADMISSIONS) -> None:
        """The detached Store copy already has default schema; custom providers fail closed."""
        if reference_admissions is not DEFAULT_REFERENCE_ADMISSIONS:
            raise UnsupportedMetadataSubset("custom reference providers are not admitted")

    def _read(self, mode: str, operation: dict, request_id: str | None = None) -> dict:
        request = self._request(request_id or self._request_id(),
                                operation["lease"]["receiver_id"], operation)
        reply = self._run(mode, request)
        canonical = json.dumps(request, sort_keys=True, separators=(",", ":"),
                               ensure_ascii=False).encode("utf-8")
        if (reply.get("version") != 2
                or reply.get("request_id") != request["request_id"]
                or reply.get("receiver_id") != request["receiver_id"]
                or reply.get("request_digest") != hashlib.sha256(canonical).hexdigest()
                or set(reply) != {"version", "request_id", "receiver_id",
                                  "request_digest", "result"}):
            raise RustFixtureError("Rust fixture read reply is not request-bound")
        return reply["result"]

    def member_cursors(self, lease: RustMetadataLease,
                       *, request_id: str | None = None) -> dict[str, str]:
        result = self._read("--v2-clone-member-cursor-path", {
            "op": "read_source_member_cursors", "lease": lease.wire(),
        }, request_id)
        cursors = result.get("cursors") if isinstance(result, dict) else None
        if (not isinstance(result, dict) or set(result) != {"kind", "cursors"}
                or result["kind"] != "source_member_cursors_read"
                or not isinstance(cursors, list)):
            raise RustFixtureError("Rust fixture returned malformed member cursor result")
        output: dict[str, str] = {}
        for item in cursors:
            if (not isinstance(item, dict) or set(item) != {"member_id", "cursor"}
                    or not isinstance(item["member_id"], str)
                    or not isinstance(item["cursor"], str)
                    or item["member_id"] in output):
                raise RustFixtureError("Rust fixture returned malformed member cursor row")
            output[item["member_id"]] = item["cursor"]
        return output

    def last_source_diagnostic(self, lease: RustMetadataLease,
                               *, request_id: str | None = None) -> dict:
        result = self._read("--v2-clone-diagnostic-path", {
            "op": "read_last_source_diagnostic", "lease": lease.wire(),
        }, request_id)
        diagnostic = result.get("diagnostic") if isinstance(result, dict) else None
        if (diagnostic is None and isinstance(result, dict)
                and set(result) == {"kind", "diagnostic"}
                and result["kind"] == "source_diagnostic_read"):
            return {}
        if (not isinstance(result, dict) or set(result) != {"kind", "diagnostic"}
                or result["kind"] != "source_diagnostic_read"
                or not isinstance(diagnostic, dict)
                or set(diagnostic) != {"code", "severity", "time_ns"}):
            raise RustFixtureError("Rust fixture returned malformed diagnostic result")
        return diagnostic

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
                      reference_refs: tuple = (), allow_root_gap: bool = False) -> dict:
        # Python's existing validation and metadata minimization define the host
        # side of this limited fixture protocol before Rust receives any rows.
        digest, rows, checked = _validated(batch, lease.source_id)
        if reference_refs:
            raise UnsupportedMetadataSubset("reference payloads are not admitted")
        if checked.root_gap or allow_root_gap:
            raise UnsupportedMetadataSubset("discovery root gaps are not admitted")
        gap_rows = [row for row in rows if row[3] == "diagnostic"
                    and json.loads(row[5]).get("code") == "possible_source_gap"]
        if checked.member_id and not checked.member_cursor:
            if (len(rows) != 1 or len(gap_rows) != 1
                    or rows[0][7] or rows[0][8] or rows[0][9] or rows[0][10]):
                raise UnsupportedMetadataSubset(
                    "member removal requires one sessionless source-gap diagnostic")
        elif checked.member_cursor and not checked.member_id:
            raise UnsupportedMetadataSubset("member cursor requires a member identity")
        elif gap_rows:
            raise UnsupportedMetadataSubset(
                "possible_source_gap requires a single member removal batch")
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
        operation = {
            "op": "admit_batch", "lease": lease.wire(),
            "expected_cursor": expected_cursor, "cursor": checked.cursor,
            "batch_digest": digest, "records": records,
        }
        if checked.member_id:
            operation["member_update"] = {
                "member_id": checked.member_id, "cursor": checked.member_cursor,
            }
        return self._request(request_id, lease.receiver_id, operation)

    def commit_batch(self, lease: RustMetadataLease, expected_cursor: str,
                     batch: PollBatch, *, request_id: str | None = None,
                     reference_refs: tuple = (), allow_root_gap: bool = False,
                     reference_submission=None,
                     reference_admissions=DEFAULT_REFERENCE_ADMISSIONS) -> dict:
        if reference_submission is not None or reference_admissions is not DEFAULT_REFERENCE_ADMISSIONS:
            raise UnsupportedMetadataSubset("reference providers are not admitted")
        request = self.batch_request(lease, expected_cursor, batch,
                                     request_id=request_id or self._request_id(),
                                     reference_refs=reference_refs,
                                     allow_root_gap=allow_root_gap)
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

    def classify_lease_failure(self, error: BaseException,
                               source_id: str) -> str | None:
        if not isinstance(error, RustFixtureError):
            return None
        message = str(error).lower()
        if "capture" in message and ("disabled" in message or "revoked" in message):
            return "capture_revoked"
        if "stale" in message or "lease" in message:
            return ("capture_revoked" if self.capture_revoked(source_id)
                    else "source_fenced")
        return None

    def send_normal_dispatch(self, request: dict) -> None:
        """Use the service-facing activation clone dispatcher (expected to reject subset ops)."""
        self._run("--v2-activation-clone-path", request)

    def close(self) -> None:
        self._closed = True


class RustMarkedCloneSessionPortSubset:
    """Port-shaped test façade for PluginSession; not a runtime backend.

    Only the metadata subset implemented by the marked-clone adapter is
    accepted. Keep this class under tests/support and out of observatory.
    """

    def __init__(self, adapter: RustMarkedCloneMetadataAdapter):
        self.adapter = adapter

    def install_reference_schema(self, reference_admissions) -> None:
        self.adapter.install_reference_schema(reference_admissions)

    def activate_source(self, *, plugin_id: str, source_id: str, generation: str,
                        content_fingerprint: str,
                        reference_admissions) -> RustMetadataLease:
        return self.adapter.activate_source(
            plugin_id=plugin_id, source_id=source_id, generation=generation,
            content_fingerprint=content_fingerprint,
            reference_admissions=reference_admissions)

    def member_cursors(self, lease: RustMetadataLease) -> dict[str, str]:
        return self.adapter.member_cursors(lease)

    def last_source_diagnostic(self, lease: RustMetadataLease) -> dict:
        return self.adapter.last_source_diagnostic(lease)

    def commit_batch(self, lease: RustMetadataLease, expected_cursor: str,
                     batch: PollBatch, *, allow_root_gap: bool,
                     reference_submission,
                     reference_admissions) -> str:
        if reference_submission is not None:
            raise UnsupportedMetadataSubset("reference submissions are not admitted")
        result = self.adapter.commit_batch(
            lease, expected_cursor, batch, allow_root_gap=allow_root_gap,
            reference_admissions=reference_admissions)
        return result["cursor"]

    def capture_revoked(self, source_id: str) -> bool:
        return self.adapter.capture_revoked(source_id)

    def classify_lease_failure(self, error: BaseException,
                               source_id: str) -> str | None:
        return self.adapter.classify_lease_failure(error, source_id)

    def close(self) -> None:
        self.adapter.close()
