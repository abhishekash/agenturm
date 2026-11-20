"""Pi-owned exact-reference admission and receipts within the host transaction.

This module keeps Pi's source proof, redacted text, and content tables outside
the generic plugin cursor/evidence writer. The child never calls these hooks.
"""
from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass

from . import plugin_pi_command, plugin_pi_content
from .plugin_reference_admission import (PreparedReference, ReferenceAdmissionProvider,
                                         ReferenceSubmission, ReferenceWriteContext)
from .plugin_wire import ProtocolError, decode_message, encode_message


_EMPTY_CONTENT_DIGEST = hashlib.sha256(encode_message({"refs": []}).encode()).hexdigest()
_EMPTY_COMMAND_DIGEST = hashlib.sha256(b"[]").hexdigest()


def legacy_submission(content_refs, content_grant, command_refs, command_grant, batch):
    """Keep direct callers' two legacy keywords behind a Pi-owned adapter."""
    content_checked = plugin_pi_content.validate_refs(content_refs, batch)
    command_checked = plugin_pi_command.validate_refs(command_refs, batch)
    if content_checked and command_checked:
        raise ProtocolError("Pi content reference families cannot share one batch")
    if content_checked or content_grant is not None:
        return ReferenceSubmission("pi-content", content_refs, content_grant)
    if command_checked or command_grant is not None:
        return ReferenceSubmission("pi-command", command_refs, command_grant)
    return None


def _grant_matches(grant, grant_type, lease) -> bool:
    return (isinstance(grant, grant_type)
            and (grant.source_id, grant.plugin_id, grant.generation,
                 grant.lease_epoch, grant.policy_epoch, grant.fingerprint) ==
            (lease.source_id, lease.plugin_id, lease.generation,
             lease.epoch, lease.policy_epoch, lease.content_fingerprint))


@dataclass(frozen=True)
class _ContentOutcome:
    omitted_duplicates: int = 0
    missing_new_content: bool = False
    capacity_victims: int = 0


class PiContentAdmission(ReferenceAdmissionProvider):
    id = "pi-content"

    def initialize_schema(self, db) -> None:
        plugin_pi_content.initialize(db)
        db.execute("INSERT OR IGNORE INTO plugin_reference_owner_v1(source_id,provider_id) "
                   "SELECT source_id,'pi-content' FROM plugin_content_v1 UNION "
                   "SELECT source_id,'pi-content' FROM plugin_content_batch_v1 UNION "
                   "SELECT source_id,'pi-content' FROM plugin_content_status_v1")

    def erase_source(self, db, source_id: str) -> None:
        for table in ("plugin_content_v1", "plugin_content_batch_v1",
                      "plugin_content_status_v1"):
            db.execute(f"DELETE FROM {table} WHERE source_id=?", (source_id,))

    def inherited_withheld_reason(self, selected_provider, reason: str) -> str:
        # Preserve the original shared Pi status row for an unanchored command
        # source. An unrelated trusted provider must not affect Pi diagnostics.
        return (reason if isinstance(selected_provider, PiCommandAdmission)
                and reason == "unanchored_origin" else "")

    def prepare(self, refs, grant, batch, lease) -> PreparedReference:
        plugin_pi_content.validate_refs(refs, batch)
        # Direct callers may supply mutable dictionaries. Bind the digest and
        # subsequent exact-source proof to the same bounded immutable snapshot.
        canonical = encode_message({"refs": list(refs)})
        snapshot = tuple(decode_message(canonical)["refs"])
        checked = plugin_pi_content.validate_refs(snapshot, batch)
        if checked:
            if not _grant_matches(grant, plugin_pi_content.PiContentGrant, lease):
                raise ProtocolError("Pi content grant unavailable")
        elif grant is not None:
            raise ProtocolError("Pi content grant without references")
        digest = hashlib.sha256(canonical.encode()).hexdigest()
        return PreparedReference(self.id, tuple(checked), digest, grant)

    def verify(self, prepared, batch, prior_cursor):
        withheld = plugin_pi_content.withheld_reason(batch)
        try:
            verified = plugin_pi_content.verify_refs(
                prepared.checked, prepared.grant, batch, prior_cursor)
        except plugin_pi_content.UnanchoredOrigin:
            return (), "unanchored_origin"
        return verified, withheld

    def replay(self, db, source_id, cursor, prepared):
        receipt = db.execute("SELECT cursor,refs_digest FROM plugin_content_batch_v1 "
                             "WHERE source_id=?", (source_id,)).fetchone()
        requested = bool(prepared and prepared.requested)
        digest = prepared.digest if prepared is not None else _EMPTY_CONTENT_DIGEST
        if ((receipt is None and requested)
                or (receipt is not None and
                    (receipt["cursor"], receipt["refs_digest"]) != (cursor, digest))):
            raise ProtocolError("Pi content replay mismatch")

    def commit(self, context: ReferenceWriteContext) -> _ContentOutcome:
        db, lease, batch = context.db, context.lease, context.batch
        omitted_duplicates = 0
        inserted_content = []
        for ref, file_dev, file_ino, redacted_text in context.verified:
            event_hash = hashlib.sha256(
                (lease.source_id + "\0" + ref["event_id"]).encode()).hexdigest()
            if event_hash not in context.inserted_hashes:
                # A repeated v1 fact is not a content correction, even if
                # another source row reused its native Pi ID.
                omitted_duplicates += 1
                continue
            db.execute("DELETE FROM plugin_content_v1 WHERE source_id=? AND event_id_hash=?",
                       (lease.source_id, event_hash))
            db.execute("INSERT INTO plugin_content_v1 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                       (lease.source_id, event_hash, ref["revision"], ref["kind"],
                        ref["native_session_id"], ref["native_turn_id"], batch.member_id,
                        ref["row_offset"], ref["row_length"], ref["row_sha256"],
                        file_dev, file_ino, redacted_text, lease.policy_epoch, lease.epoch,
                        context.dirty_seq))
            inserted_content.append((event_hash, ref["revision"]))
        plugin_pi_content.prune_orphans(db, lease.source_id)
        missing = any(db.execute(
            "SELECT 1 FROM plugin_content_v1 WHERE source_id=? AND event_id_hash=? AND revision=?",
            (lease.source_id, event_hash, revision)).fetchone() is None
            for event_hash, revision in inserted_content)
        victims = (plugin_pi_content.prune_capacity(db, lease.source_id)
                   if context.verified else 0)
        return _ContentOutcome(omitted_duplicates, missing, victims)

    def finalize(self, context: ReferenceWriteContext, outcome: _ContentOutcome) -> None:
        prepared = context.prepared
        checked_count = len(prepared.checked) if prepared is not None else 0
        reason = context.withheld_reason or (
            "invalidated_session" if outcome.missing_new_content else
            "capacity" if outcome.capacity_victims else "")
        if not reason:
            reason = "duplicate_occurrence" if outcome.omitted_duplicates else (
                "not_requested" if not checked_count else "none")
        db, lease, batch = context.db, context.lease, context.batch
        db.execute("INSERT INTO plugin_content_status_v1(source_id,cursor,reason,refs_count,policy_epoch) "
                   "VALUES(?,?,?,?,?) ON CONFLICT(source_id) DO UPDATE SET "
                   "cursor=excluded.cursor,reason=excluded.reason,"
                   "refs_count=excluded.refs_count,policy_epoch=excluded.policy_epoch",
                   (lease.source_id, batch.cursor, reason, checked_count, lease.policy_epoch))
        db.execute("INSERT INTO plugin_content_batch_v1(source_id,cursor,refs_digest) VALUES(?,?,?) "
                   "ON CONFLICT(source_id) DO UPDATE SET cursor=excluded.cursor,"
                   "refs_digest=excluded.refs_digest",
                   (lease.source_id, batch.cursor,
                    prepared.digest if prepared is not None else _EMPTY_CONTENT_DIGEST))


class PiCommandAdmission(ReferenceAdmissionProvider):
    id = "pi-command"

    def initialize_schema(self, db) -> None:
        plugin_pi_command.initialize(db)
        db.execute("INSERT OR IGNORE INTO plugin_reference_owner_v1(source_id,provider_id) "
                   "SELECT source_id,'pi-command' FROM plugin_command_content_v1 UNION "
                   "SELECT source_id,'pi-command' FROM plugin_command_batch_v1")

    def erase_source(self, db, source_id: str) -> None:
        for table in ("plugin_command_content_v1", "plugin_command_batch_v1"):
            db.execute(f"DELETE FROM {table} WHERE source_id=?", (source_id,))

    def prepare(self, refs, grant, batch, lease) -> PreparedReference:
        checked = plugin_pi_command.validate_refs(refs, batch)
        if checked:
            if not _grant_matches(grant, plugin_pi_command.PiCommandGrant, lease):
                raise ProtocolError("Pi command grant unavailable")
        elif grant is not None:
            raise ProtocolError("Pi command grant without references")
        digest = hashlib.sha256(json.dumps(
            [vars(ref) for ref in refs], sort_keys=True,
            separators=(",", ":")).encode()).hexdigest()
        return PreparedReference(self.id, tuple(checked), digest, grant)

    def verify(self, prepared, batch, prior_cursor):
        try:
            verified = plugin_pi_command.verify_refs(
                prepared.checked, prepared.grant, batch, prior_cursor)
        except plugin_pi_content.UnanchoredOrigin:
            return (), "unanchored_origin"
        return verified, ""

    def replay(self, db, source_id, cursor, prepared):
        receipt = db.execute("SELECT cursor,refs_digest FROM plugin_command_batch_v1 "
                             "WHERE source_id=?", (source_id,)).fetchone()
        requested = bool(prepared and prepared.requested)
        digest = prepared.digest if prepared is not None else _EMPTY_COMMAND_DIGEST
        if ((receipt is None and requested)
                or (receipt is not None and
                    (receipt["cursor"], receipt["refs_digest"]) != (cursor, digest))):
            raise ProtocolError("Pi command replay mismatch")

    def commit(self, context: ReferenceWriteContext):
        db, lease, batch = context.db, context.lease, context.batch
        for ref, file_dev, file_ino, redacted_command in context.verified:
            event_hash = hashlib.sha256(
                (lease.source_id + "\0" + ref.event_id).encode()).hexdigest()
            if (event_hash not in context.inserted_hashes
                    or ref.native_session_id in context.ambiguous_ids):
                continue
            db.execute("DELETE FROM plugin_command_content_v1 "
                       "WHERE source_id=? AND event_id_hash=?",
                       (lease.source_id, event_hash))
            db.execute("INSERT INTO plugin_command_content_v1 VALUES("
                       "?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                       (lease.source_id, event_hash, ref.revision, ref.kind,
                        ref.native_session_id, ref.native_turn_id,
                        ref.tool_call_id, ref.assistant_row_id, ref.block_index,
                        batch.member_id, ref.row_offset, ref.row_length,
                        ref.row_sha256, file_dev, file_ino, redacted_command,
                        lease.policy_epoch, lease.epoch, context.dirty_seq))
        if context.verified:
            plugin_pi_command.prune_capacity(db, lease.source_id)
        plugin_pi_command.prune_orphans(db, lease.source_id)
        return None

    def finalize(self, context: ReferenceWriteContext, outcome) -> None:
        db, lease, batch = context.db, context.lease, context.batch
        prepared = context.prepared
        if prepared is not None and prepared.requested:
            db.execute("INSERT INTO plugin_command_batch_v1(source_id,cursor,refs_digest) "
                       "VALUES(?,?,?) ON CONFLICT(source_id) DO UPDATE SET "
                       "cursor=excluded.cursor,refs_digest=excluded.refs_digest",
                       (lease.source_id, batch.cursor, prepared.digest))
        else:
            db.execute("DELETE FROM plugin_command_batch_v1 WHERE source_id=?",
                       (lease.source_id,))
