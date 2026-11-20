"""Feature-off receiver proof for requested Pi bash command text."""
import hashlib
import json
import os
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
from observatory.plugin_manifest import PluginManifest
from observatory.plugin_pi_command import PiCommandGrant, PiCommandRef
from observatory.plugin_supervisor import PluginSource
from observatory.plugin_wire import PollBatch, ProtocolError


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def line(value):
    return (json.dumps(value, separators=(",", ":"), ensure_ascii=False) + "\n").encode()


class PiCommandAdmissionTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.sources = self.root / "sessions"
        self.sources.mkdir()
        self.relative = "one.jsonl"
        self.file = self.sources / self.relative
        self.header = line({"type": "session", "id": "pi-session"})
        self.user = line({"type": "message", "id": "user-1",
                          "timestamp": "2026-10-24T12:00:00Z",
                          "message": {"role": "user", "content": "private request"}})
        self.row = line({
            "type": "message", "id": "assistant-1",
            "timestamp": "2026-10-24T12:00:01Z",
            "message": {"role": "assistant", "content": [
                {"type": "text", "text": "private assistant text"},
                {"type": "toolCall", "id": "call-1", "name": "bash",
                 "arguments": {"command": "password=highly-private npm test"}},
                {"type": "toolCall", "id": "call-2", "name": "read",
                 "arguments": {"path": "/secret"}},
            ]}})
        self.file.write_bytes(self.header + self.user + self.row)
        self.store = Store(self.root / "store.db")
        self.addCleanup(self.store.close)
        features = ("harness.pi/1", "source-discovery/1",
                    "source-discovery-root-gap/1", "configured-root/1",
                    "pi-command-ref/1")
        manifest = PluginManifest(
            "fixture.pi-command", "1.0.0", 1, 1, ("/unused",), "a" * 64,
            features, (), (), ("configured-source-read",), self.root)
        self.source = PluginSource(
            manifest, "pi-directory", allowed_grants=("configured-source-read",),
            replaces=("pi",), source_root=str(self.sources),
            required_features=features, capture_mode="pi-review-text-command-v1")
        self.lease = activate_source(
            self.store, plugin_id=manifest.id, source_id=self.source.source_id,
            generation=manifest.artifact_sha256,
            content_fingerprint=PiCommandGrant.fingerprint_for_source(self.source))
        self.grant = PiCommandGrant.from_source(self.source, self.lease)

    def bundle(self, *, cursor="d1", block=1, command_row=None):
        row = self.row if command_row is None else command_row
        info = self.file.stat()
        origin = sha(f"{info.st_dev}:{info.st_ino}:{sha(self.header)}".encode())
        checkpoint = (f"v2:{info.st_dev}:{info.st_ino}:{len(self.header + self.user + row)}:"
                      f"{sha(self.header + self.user + row)}:{sha(self.header)}:{origin}:user-1")
        event_id = f"pi:{origin}:assistant-1:tool-{block}"
        fact = {"event_id": event_id, "revision": 0, "kind": "tool",
                "time_ns": "1792843201000000000",
                "identity": {"session_id": "pi-session", "turn_id": "user-1",
                             "operation_id": "call-1"},
                "payload": {"category": "shell", "result": "started"}}
        member = sha(self.relative.encode())
        batch = PollBatch(self.source.source_id, cursor, (fact,), member, checkpoint)
        ref = PiCommandRef(1, self.source.source_id, member, self.relative,
                           len(self.header + self.user), len(row), sha(row), event_id, 0,
                           "pi-session", "user-1", "assistant-1", block,
                           "call-1", "pi.shell_command_text")
        return batch, ref

    def rows(self):
        with self.store._connect() as db:
            return [tuple(row) for row in db.execute(
                "SELECT * FROM plugin_command_content_v1 ORDER BY event_id_hash")]

    def test_exact_selected_block_is_redacted_and_replay_is_durable(self):
        batch, ref = self.bundle()
        self.assertEqual(commit_batch(self.store, self.lease, "", batch,
                                      command_refs=(ref,), command_grant=self.grant), "d1")
        rows = self.rows()
        self.assertEqual(len(rows), 1)
        self.assertIn("[REDACTED]", rows[0][15])
        self.assertNotIn("highly-private", str(rows))
        self.assertNotIn("private assistant", str(rows))
        self.assertEqual(commit_batch(self.store, self.lease, "", batch,
                                      command_refs=(ref,), command_grant=self.grant), "d1")
        self.assertEqual(self.rows(), rows)
        with self.assertRaises(ProtocolError):
            commit_batch(self.store, self.lease, "", batch)
        with self.assertRaises(ProtocolError):
            commit_batch(self.store, self.lease, "", batch,
                         command_refs=(replace(ref, row_sha256="0" * 64),),
                         command_grant=self.grant)

    def test_forged_ref_and_non_bash_block_never_ack(self):
        batch, ref = self.bundle()
        for changed in (replace(ref, block_index=2),
                        replace(ref, tool_call_id="call-2"),
                        replace(ref, row_offset=1),
                        replace(ref, row_sha256="0" * 64)):
            with self.subTest(changed=changed):
                with self.assertRaises(ProtocolError):
                    commit_batch(self.store, self.lease, "", batch,
                                 command_refs=(changed,), command_grant=self.grant)
        self.assertEqual(self.rows(), [])
        with self.store._connect() as db:
            self.assertEqual(db.execute(
                "SELECT cursor FROM plugin_source_cursor_v1 WHERE source_id=?",
                (self.source.source_id,)).fetchone()[0], "")

    def test_source_change_and_symlink_refuse_before_ack(self):
        batch, ref = self.bundle()
        self.file.write_bytes(self.header + self.user +
                              self.row.replace(b"npm test", b"npm lint"))
        with self.assertRaises(ProtocolError):
            commit_batch(self.store, self.lease, "", batch,
                         command_refs=(ref,), command_grant=self.grant)
        self.file.unlink()
        target = self.sources / "target.jsonl"
        target.write_bytes(self.header + self.user + self.row)
        self.file.symlink_to(target)
        with self.assertRaises((ProtocolError, OSError)):
            commit_batch(self.store, self.lease, "", batch,
                         command_refs=(ref,), command_grant=self.grant)
        self.assertEqual(self.rows(), [])

    def test_revocation_removes_command_and_fences_old_lease(self):
        batch, ref = self.bundle()
        commit_batch(self.store, self.lease, "", batch,
                     command_refs=(ref,), command_grant=self.grant)
        set_source_capture(self.store, source_id=self.source.source_id, enabled=False)
        self.assertEqual(self.rows(), [])
        with self.assertRaises(ProtocolError):
            commit_batch(self.store, self.lease, "", batch,
                         command_refs=(ref,), command_grant=self.grant)

    def test_command_requires_explicit_internal_grant(self):
        batch, ref = self.bundle()
        with self.assertRaises(ProtocolError):
            commit_batch(self.store, self.lease, "", batch, command_refs=(ref,))
        with self.assertRaises(ProtocolError):
            commit_batch(self.store, self.lease, "", batch,
                         command_refs=(ref,), command_grant=replace(
                             self.grant, policy_epoch=self.grant.policy_epoch + 1))

    def test_only_same_batch_started_shell_fact_can_authorize_text(self):
        batch, ref = self.bundle()
        for fact in ({**batch.records[0], "payload": {"category": "shell", "result": "succeeded"}},
                     {**batch.records[0], "payload": {"category": "file", "result": "started"}},
                     {**batch.records[0], "time_ns": "1792843202000000000"},
                     {**batch.records[0], "identity": {
                         "session_id": "pi-session", "turn_id": "user-1",
                         "operation_id": "call-2"}}):
            changed = replace(batch, records=(fact,))
            with self.subTest(fact=fact), self.assertRaises(ProtocolError):
                commit_batch(self.store, self.lease, "", changed,
                             command_refs=(ref,), command_grant=self.grant)
        self.assertEqual(self.rows(), [])

    def test_forged_turn_cannot_borrow_preceding_user(self):
        batch, ref = self.bundle()
        fact = {**batch.records[0],
                "identity": {**batch.records[0]["identity"],
                             "turn_id": "user-fake"}}
        changed = replace(batch, records=(fact,))
        with self.assertRaises(ProtocolError):
            commit_batch(self.store, self.lease, "", changed,
                         command_refs=(replace(ref, native_turn_id="user-fake"),),
                         command_grant=self.grant)
        self.assertEqual(self.rows(), [])

    def test_ambiguous_member_owner_withholds_command_but_commits_metadata(self):
        batch, ref = self.bundle()
        with self.store._source_write(), self.store._connect() as db:
            db.execute("BEGIN IMMEDIATE")
            db.execute("INSERT INTO plugin_session_member_v1 "
                       "(source_id,native_session_id,member_id) VALUES(?,?,?)",
                       (self.source.source_id, "pi-session", ""))
        self.assertEqual(commit_batch(self.store, self.lease, "", batch,
                                      command_refs=(ref,), command_grant=self.grant), "d1")
        self.assertEqual(self.rows(), [])
        with self.store._connect() as db:
            evidence = db.execute(
                "SELECT member_id FROM plugin_evidence_v1 WHERE source_id=?",
                (self.source.source_id,)).fetchone()
            self.assertEqual(evidence[0], "")

    def test_gap_and_retention_invalidation_remove_command(self):
        batch, ref = self.bundle()
        commit_batch(self.store, self.lease, "", batch,
                     command_refs=(ref,), command_grant=self.grant)
        diagnostic = {"event_id": "gap-1", "revision": 0, "kind": "diagnostic",
                      "time_ns": "1792843203000000000",
                      "payload": {"code": "possible_source_gap", "severity": "warning"}}
        gap = PollBatch(self.source.source_id, "d2", (diagnostic,),
                        batch.member_id, "")
        commit_batch(self.store, self.lease, "d1", gap)
        self.assertEqual(self.rows(), [])

        # A second source proves the retention tombstone cleanup without
        # relying on the prior member-gap disposition.
        self.file.write_bytes(self.header + self.user + self.row)
        self.store.close()
        self.store = Store(self.root / "retention.db")
        self.lease = activate_source(
            self.store, plugin_id=self.source.manifest.id,
            source_id=self.source.source_id,
            generation=self.source.manifest.artifact_sha256,
            content_fingerprint=PiCommandGrant.fingerprint_for_source(self.source))
        self.grant = PiCommandGrant.from_source(self.source, self.lease)
        batch, ref = self.bundle()
        commit_batch(self.store, self.lease, "", batch,
                     command_refs=(ref,), command_grant=self.grant)
        with self.store._source_write(), self.store._connect() as db:
            db.execute("BEGIN IMMEDIATE")
            db.execute("INSERT INTO plugin_retention_invalidated_v1 "
                       "(source_id,native_session_id,last_record_ns) VALUES(?,?,?)",
                       (self.source.source_id, "pi-session", 1))
        commit_batch(self.store, self.lease, "d1",
                     PollBatch(self.source.source_id, "d2", ()))
        self.assertEqual(self.rows(), [])

    def test_config_downgrade_removes_command_and_replay_receipt(self):
        batch, ref = self.bundle()
        commit_batch(self.store, self.lease, "", batch,
                     command_refs=(ref,), command_grant=self.grant)
        activate_source(self.store, plugin_id=self.source.manifest.id,
                        source_id=self.source.source_id,
                        generation=self.source.manifest.artifact_sha256,
                        content_fingerprint="")
        self.assertEqual(self.rows(), [])
        with self.store._connect() as db:
            self.assertIsNone(db.execute(
                "SELECT * FROM plugin_command_batch_v1 WHERE source_id=?",
                (self.source.source_id,)).fetchone())

    def test_utf8_command_over_byte_limit_refuses_without_cursor_ack(self):
        row = line({"type": "message", "id": "assistant-1",
                    "timestamp": "2026-10-24T12:00:01Z",
                    "message": {"role": "assistant", "content": [
                        {"type": "toolCall", "id": "call-1", "name": "bash",
                         "arguments": {"command": "é" * 2049}}]}})
        self.file.write_bytes(self.header + self.user + row)
        batch, ref = self.bundle(block=0, command_row=row)
        with self.assertRaises(ProtocolError):
            commit_batch(self.store, self.lease, "", batch,
                         command_refs=(ref,), command_grant=self.grant)
        self.assertEqual(self.rows(), [])

    def test_duplicate_occurrence_cannot_replace_first_command(self):
        batch, ref = self.bundle()
        commit_batch(self.store, self.lease, "", batch,
                     command_refs=(ref,), command_grant=self.grant)
        original = self.rows()
        self.file.write_bytes(self.header + self.user + self.row + self.row)
        info = self.file.stat()
        origin = sha(f"{info.st_dev}:{info.st_ino}:{sha(self.header)}".encode())
        raw = self.file.read_bytes()
        checkpoint = (f"v2:{info.st_dev}:{info.st_ino}:{len(raw)}:"
                      f"{sha(raw)}:{sha(self.header)}:{origin}:user-1")
        duplicate = replace(batch, cursor="d2", member_cursor=checkpoint)
        later_ref = replace(ref, row_offset=len(self.header + self.user + self.row))
        self.assertEqual(commit_batch(self.store, self.lease, "d1", duplicate,
                                      command_refs=(later_ref,),
                                      command_grant=self.grant), "d2")
        self.assertEqual(self.rows(), original)


if __name__ == "__main__":
    unittest.main()
