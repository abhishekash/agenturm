"""Opt-in attestation of one historical Rust backup query against live Store.

This module does not create a backup, start a Rust child, or serve production
queries. Its result is current only at the final Store check. A later write
requires another check before publication.
"""
from __future__ import annotations

import json
import os
import re
import stat

from .plugin_wire import ProtocolError
from .rust_shadow_snapshot import capture, current


MAX_RESULT_BYTES = 128 * 1024
_NATIVE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,255}\Z")
_EVENT = re.compile(r"plugin-event:[0-9a-f]{64}\Z")
_RESULT_KEYS = frozenset({"schema_version", "receiver_id", "source_id", "state",
                          "revision", "coverage", "query"})
_REVISION_KEYS = frozenset({"lease_epoch", "policy_epoch", "dirty_seq",
                            "capture_enabled", "projected_seq", "projected_policy"})


def _unique_object(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ProtocolError("duplicate Rust query result key")
        result[key] = value
    return result


def _reject_constant(_value):
    raise ProtocolError("non-finite Rust query result number")


def _reject_float(_value):
    raise ProtocolError("floating point Rust query result number")


def _query(query):
    if not isinstance(query, dict) or query.get("kind") not in ("session", "activity"):
        raise ProtocolError("invalid exact Rust query")
    kind = query["kind"]
    identity_key = "native_session_id" if kind == "session" else "event_id"
    if set(query) != {"kind", "source_id", identity_key}:
        raise ProtocolError("invalid exact Rust query fields")
    source, identity = query["source_id"], query[identity_key]
    if (not isinstance(source, str) or _NATIVE.fullmatch(source) is None
            or not isinstance(identity, str)
            or (kind == "session" and _NATIVE.fullmatch(identity) is None)
            or (kind == "activity" and _EVENT.fullmatch(identity) is None)):
        raise ProtocolError("invalid exact Rust query identity")
    return kind, source, identity


def _result(raw):
    if not isinstance(raw, (str, bytes)):
        raise ProtocolError("Rust query result must be JSON text")
    try:
        if len(raw) > MAX_RESULT_BYTES:
            raise ProtocolError("Rust query result exceeds bound")
        encoded = raw.encode("utf-8", "strict") if isinstance(raw, str) else raw
        if len(encoded) > MAX_RESULT_BYTES:
            raise ProtocolError("Rust query result exceeds bound")
        result = json.loads(encoded.decode("utf-8", "strict"),
                            object_pairs_hook=_unique_object,
                            parse_constant=_reject_constant, parse_float=_reject_float)
    except (UnicodeError, ValueError, RecursionError) as exc:
        raise ProtocolError("invalid Rust query result JSON") from exc
    if not isinstance(result, dict):
        raise ProtocolError("invalid Rust query result")
    return result


def _same_json(left, right):
    """Compare JSON values without Python's bool/int or int/float coercion."""
    return json.dumps(left, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=True, allow_nan=False) == json.dumps(
                          right, sort_keys=True, separators=(",", ":"),
                          ensure_ascii=True, allow_nan=False)


def _store_file(store):
    """Bind the Store pathname to one regular inode across this check."""
    try:
        info = os.stat(store.path, follow_symlinks=False)
    except (AttributeError, OSError) as exc:
        raise ProtocolError("live Store file is unavailable") from exc
    if not stat.S_ISREG(info.st_mode):
        raise ProtocolError("live Store path is not a regular file")
    return info.st_dev, info.st_ino


def attest_exact(store, query: dict, rust_result: str | bytes) -> dict:
    """Return one exact row/absence attested at the final ``current`` check.

    The Rust result is historical and untrusted. Every field is checked against
    a fresh Python snapshot of this source. The returned row comes from that
    snapshot, never from the backup. Coverage remains partial because exact
    absence does not prove the source is complete. No production path calls
    this helper.
    """
    kind, source, identity = _query(query)
    result = _result(rust_result)
    if (set(result) != _RESULT_KEYS | {kind} or type(result["schema_version"]) is not int
            or result["schema_version"] != 1 or result["state"] != "ready"
            or result["coverage"] != "partial" or result["source_id"] != source
            or not isinstance(result["receiver_id"], str)
            or not _same_json(result["query"], query)
            or not isinstance(result["revision"], dict)
            or set(result["revision"]) != _REVISION_KEYS):
        raise ProtocolError("Rust query result is not an exact ready projection")
    file_identity = _store_file(store)
    snapshot = capture(store, source)
    if snapshot is None:
        raise ProtocolError("plugin projection is unavailable")
    if _store_file(store) != file_identity:
        raise ProtocolError("live Store file changed during capture")
    revision = snapshot.revision
    expected_revision = {
        "lease_epoch": str(revision[1]),
        "policy_epoch": str(revision[2]),
        "dirty_seq": str(revision[3]),
        "capture_enabled": True,
        "projected_seq": str(revision[5]),
        "projected_policy": str(revision[6]),
    }
    if (result["receiver_id"] != revision[0]
            or not _same_json(result["revision"], expected_revision)):
        raise ProtocolError("Rust query revision differs from live Store")
    rows = snapshot.expected["sessions" if kind == "session" else "activities"]
    row_key = "native_session_id" if kind == "session" else "id"
    matching = [row for row in rows if row.get(row_key) == identity]
    if len(matching) > 1:
        raise ProtocolError("duplicate live exact projection")
    expected_row = matching[0] if matching else None
    if not _same_json(result[kind], expected_row):
        raise ProtocolError("Rust query row differs from live Store")
    if not current(store, snapshot):
        raise ProtocolError("live Store changed during Rust query attestation")
    if _store_file(store) != file_identity:
        raise ProtocolError("live Store file changed during Rust query attestation")
    return {"schema_version": 1, "receiver_id": revision[0],
            "source_id": source, "revision": expected_revision,
            "query": dict(query), "coverage": "partial", "row": expected_row}
