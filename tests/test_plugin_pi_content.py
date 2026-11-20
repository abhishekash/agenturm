"""Receiver-owned, feature-off Pi content reference admission corpus."""
import hashlib
import json
import os
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
from observatory.plugin_manifest import PluginManifest
from observatory.plugin_pi_content import PiContentGrant
from observatory.plugin_supervisor import PluginSource
from observatory.plugin_wire import PollBatch, ProtocolError


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def line(value):
    return (json.dumps(value, separators=(",", ":"), ensure_ascii=False) + "\n").encode()


class PiContentTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.sources = self.root / "sessions"
        (self.sources / "project").mkdir(parents=True)
        self.relative = "project/one.jsonl"
        self.file = self.sources / self.relative
        self.header = line({"type": "session", "id": "pi-session"})
        self.user = self.user_line("user-1", "password=highly-private")
        self.file.write_bytes(self.header + self.user)
        self.store = Store(self.root / "store.db")
        self.addCleanup(self.store.close)
        manifest = PluginManifest(
            "fixture.pi", "1.0.0", 1, 1, ("/unused",), "a" * 64,
            ("harness.pi/1", "source-discovery/1", "configured-root/1",
             "pi-content-ref/1"), (), (), ("configured-source-read",), self.root)
        self.source = PluginSource(manifest, "pi-directory",
                              allowed_grants=("configured-source-read",),
                              source_root=str(self.sources),
                              required_features=("source-discovery/1", "configured-root/1",
                                                 "harness.pi/1", "pi-content-ref/1"),
                              capture_mode="pi-review-text-v1")
        self.lease = activate_source(self.store, plugin_id="fixture.pi",
                                     source_id="pi-directory", generation="a" * 64,
                                     content_fingerprint=PiContentGrant.fingerprint_for_source(self.source))
        self.grant = PiContentGrant.from_source(self.source, self.lease)
        self.member_id = sha(self.relative.encode())

    def user_line(self, row_id, content, timestamp="2026-10-24T12:00:00Z"):
        return line({"type": "message", "id": row_id, "timestamp": timestamp,
                     "message": {"role": "user", "content": [{"type": "text", "text": content}]}})

    def checkpoint(self, *, offset=None, generation=None):
        raw = self.file.read_bytes()
        info = self.file.stat()
        offset = len(raw) if offset is None else offset
        generation = generation or sha(
            f"{info.st_dev}:{info.st_ino}:{sha(self.header)}".encode())
        return (f"v2:{info.st_dev}:{info.st_ino}:{offset}:"
                f"{sha(raw[:offset])}:{sha(self.header)}:{generation}:user-1")

    def bundle(self, *, row=None, row_id="user-1", event_id=None, revision=0,
               member_cursor=None, global_cursor=None, relative=None):
        row = self.user if row is None else row
        member_cursor = member_cursor or self.checkpoint()
        origin = member_cursor.split(":")[6]
        event_id = event_id or f"pi:{origin}:{row_id}:message"
        stamp = "1792843200000000000"
        record = {"event_id": event_id, "revision": revision, "kind": "message",
                  "time_ns": stamp, "identity": {"session_id": "pi-session",
                                                  "turn_id": row_id},
                  "payload": {"role": "user"}}
        batch = PollBatch("pi-directory", global_cursor or "d1:1:" + self.member_id,
                          (record,), self.member_id, member_cursor)
        relative = relative or self.relative
        ref = {"schema": 1, "source_id": "pi-directory", "member_id": self.member_id,
               "relative_member_path": relative, "row_offset": len(self.header),
               "row_length": len(row), "row_sha256": sha(row), "event_id": event_id,
               "revision": revision, "native_session_id": "pi-session",
               "native_turn_id": row_id, "kind": "pi.user_text"}
        return batch, ref

    def admit(self, batch, ref, expected=""):
        return commit_batch(self.store, self.lease, expected, batch,
                            content_refs=(ref,), content_grant=self.grant)

    def snapshot(self):
        with self.store._connect() as db:
            cursor = db.execute("SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 "
                                "WHERE source_id='pi-directory'").fetchone()
            evidence = db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0]
            content = db.execute("SELECT redacted_text FROM plugin_content_v1").fetchall()
        return tuple(cursor), evidence, [row[0] for row in content]

    def test_exact_reference_redacts_and_replays_without_source(self):
        batch, ref = self.bundle()
        self.assertEqual(self.admit(batch, ref), batch.cursor)
        original = self.snapshot()
        self.assertEqual(original[1], 1)
        self.assertEqual(len(original[2]), 1)
        self.assertIn("password=[REDACTED]", original[2][0])
        self.assertNotIn("highly-private", str(original))
        self.file.unlink()
        self.assertEqual(self.admit(batch, ref), batch.cursor)
        self.assertEqual(self.snapshot(), original)
        forged = {**ref, "row_sha256": "0" * 64}
        with self.assertRaisesRegex(ProtocolError, "replay mismatch"):
            self.admit(batch, forged)
        self.assertEqual(self.snapshot(), original)

    def test_same_configuration_restart_keeps_content_but_fences_old_lease(self):
        batch, ref = self.bundle()
        self.admit(batch, ref)
        previous = self.snapshot()
        new_lease = activate_source(
            self.store, plugin_id="fixture.pi", source_id="pi-directory",
            generation="a" * 64,
            content_fingerprint=PiContentGrant.fingerprint_for_source(self.source))
        self.assertEqual(self.snapshot(), previous)
        with self.assertRaises(ProtocolError):
            self.admit(batch, ref)
        self.lease = new_lease
        self.grant = PiContentGrant.from_source(self.source, new_lease)
        self.file.unlink()
        self.assertEqual(self.admit(batch, ref), batch.cursor)
        self.assertEqual(self.snapshot(), previous)

    def test_two_references_verify_one_member_prefix_once(self):
        from observatory import plugin_pi_content
        second = self.user_line("user-2", "second")
        with self.file.open("ab") as stream:
            offset = stream.tell()
            stream.write(second)
        batch, ref = self.bundle()
        current = batch.member_cursor
        origin = current.split(":")[6]
        second_event = f"pi:{origin}:user-2:message"
        second_record = {**batch.records[0], "event_id": second_event,
                         "identity": {"session_id": "pi-session", "turn_id": "user-2"}}
        batch = PollBatch(batch.source_id, batch.cursor,
                          (batch.records[0], second_record), batch.member_id, current)
        second_ref = {**ref, "event_id": second_event, "native_turn_id": "user-2",
                      "row_offset": offset, "row_length": len(second),
                      "row_sha256": sha(second)}
        original = plugin_pi_content._prefix
        with patch.object(plugin_pi_content, "_prefix", wraps=original) as prefix:
            self.assertEqual(commit_batch(self.store, self.lease, "", batch,
                                          content_refs=(ref, second_ref),
                                          content_grant=self.grant), batch.cursor)
            self.assertEqual(prefix.call_count, 1)
        self.assertEqual(len(self.snapshot()[2]), 2)

    def test_forged_occurrence_revision_time_and_changed_source_are_atomic(self):
        batch, ref = self.bundle()
        before = self.snapshot()
        for altered in ({**ref, "event_id": "pi:fake:user-1:message"},
                        {**ref, "revision": 1},
                        {**ref, "row_offset": len(self.header) - 1}):
            with self.assertRaises(ProtocolError):
                self.admit(batch, altered)
            self.assertEqual(self.snapshot(), before)
        fake_time = PollBatch(batch.source_id, batch.cursor,
                              ({**batch.records[0], "time_ns": "1"},),
                              batch.member_id, batch.member_cursor)
        with self.assertRaises(ProtocolError):
            self.admit(fake_time, ref)
        self.assertEqual(self.snapshot(), before)
        self.file.write_bytes(self.header + self.user.replace(b"highly-private", b"another-secret"))
        with self.assertRaises(ProtocolError):
            self.admit(batch, ref)
        self.assertEqual(self.snapshot(), before)

    def test_path_traversal_symlink_and_fifo_are_atomic(self):
        batch, ref = self.bundle()
        before = self.snapshot()
        with self.assertRaises(ProtocolError):
            self.admit(batch, {**ref, "relative_member_path": "../one.jsonl"})
        link = self.sources / "project" / "link.jsonl"
        link.symlink_to(self.file)
        relative = "project/link.jsonl"
        spoofed_batch, spoofed_ref = self.bundle(relative=relative)
        spoofed_batch = PollBatch(spoofed_batch.source_id, spoofed_batch.cursor,
                                  spoofed_batch.records, sha(relative.encode()),
                                  spoofed_batch.member_cursor)
        spoofed_ref = {**spoofed_ref, "member_id": sha(relative.encode())}
        with self.assertRaises(ProtocolError):
            self.admit(spoofed_batch, spoofed_ref)
        link.unlink()
        os.mkfifo(link)
        result = []
        thread = threading.Thread(target=lambda: result.append(self._try_admit(spoofed_batch, spoofed_ref)))
        thread.start()
        thread.join(2)
        self.assertFalse(thread.is_alive(), "FIFO source open blocked")
        self.assertEqual(result, ["ProtocolError"])
        self.assertEqual(self.snapshot(), before)

    def _try_admit(self, batch, ref):
        try:
            self.admit(batch, ref)
        except Exception as exc:
            return type(exc).__name__
        return "admitted"

    def test_revocation_erases_and_stale_grant_cannot_commit(self):
        batch, ref = self.bundle()
        self.admit(batch, ref)
        self.assertEqual(len(self.snapshot()[2]), 1)
        set_source_capture(self.store, source_id="pi-directory", enabled=False)
        self.assertEqual(self.snapshot()[2], [])
        with self.assertRaises(ProtocolError):
            self.admit(batch, ref)

    def test_capture_configuration_downgrade_erases_content(self):
        batch, ref = self.bundle()
        self.admit(batch, ref)
        resumed = activate_source(self.store, plugin_id="fixture.pi",
                                  source_id="pi-directory", generation="a" * 64)
        self.assertEqual(resumed.cursor, batch.cursor)
        self.assertEqual(resumed.content_fingerprint, "")
        self.assertEqual(self.snapshot()[2], [])
        with self.store._connect() as db:
            self.assertIsNone(db.execute("SELECT 1 FROM plugin_content_batch_v1").fetchone())

    def test_replaced_root_fails_before_content_admission(self):
        batch, ref = self.bundle()
        original_root = self.root / "old-sessions"
        self.sources.rename(original_root)
        (self.sources / "project").mkdir(parents=True)
        (self.sources / self.relative).write_bytes(self.header + self.user)
        before = self.snapshot()
        with self.assertRaisesRegex(ProtocolError, "root identity changed"):
            self.admit(batch, ref)
        self.assertEqual(self.snapshot(), before)
        new_fingerprint = PiContentGrant.fingerprint_for_source(self.source)
        self.assertNotEqual(new_fingerprint, self.lease.content_fingerprint)
        activate_source(self.store, plugin_id="fixture.pi", source_id="pi-directory",
                        generation="a" * 64, content_fingerprint=new_fingerprint)
        self.assertEqual(self.snapshot()[2], [])

    def test_duplicate_native_id_cannot_retroactively_change_content(self):
        batch, ref = self.bundle()
        self.admit(batch, ref)
        first = self.snapshot()
        duplicate = self.user_line("user-1", "secret=different-value")
        offset = self.file.stat().st_size
        with self.file.open("ab") as stream:
            stream.write(duplicate)
        next_cursor = self.checkpoint()
        next_batch = PollBatch(batch.source_id, "d1:2:" + self.member_id,
                               batch.records, self.member_id, next_cursor)
        next_ref = {**ref, "row_offset": offset, "row_length": len(duplicate),
                    "row_sha256": sha(duplicate)}
        self.assertEqual(self.admit(next_batch, next_ref, batch.cursor), next_batch.cursor)
        after = self.snapshot()
        self.assertEqual(after[1:], first[1:])
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT reason FROM plugin_content_status_v1").fetchone()[0],
                             "duplicate_occurrence")

    def test_already_acknowledged_row_cannot_gain_content_later(self):
        batch, ref = self.bundle()
        commit_batch(self.store, self.lease, "", batch)
        next_user = self.user_line("user-2", "later")
        with self.file.open("ab") as stream:
            stream.write(next_user)
        later = PollBatch(batch.source_id, "d1:2:" + self.member_id,
                          batch.records, self.member_id, self.checkpoint())
        before = self.snapshot()
        with self.assertRaisesRegex(ProtocolError, "outside checkpoint"):
            self.admit(later, ref, batch.cursor)
        self.assertEqual(self.snapshot(), before)

    def test_forged_prior_origin_is_withheld_without_blocking_metadata(self):
        forged_origin = "f" * 64
        first_cursor = self.checkpoint(generation=forged_origin)
        first_event = f"pi:{forged_origin}:user-1:message"
        first = {"event_id": first_event, "revision": 0, "kind": "message",
                 "time_ns": "1792843200000000000",
                 "identity": {"session_id": "pi-session", "turn_id": "user-1"},
                 "payload": {"role": "user"}}
        first_batch = PollBatch("pi-directory", "d1:1:" + self.member_id,
                                (first,), self.member_id, first_cursor)
        commit_batch(self.store, self.lease, "", first_batch)
        second = self.user_line("user-2", "secret=forged-origin")
        with self.file.open("ab") as stream:
            offset = stream.tell()
            stream.write(second)
        current = self.checkpoint(generation=forged_origin)
        event = f"pi:{forged_origin}:user-2:message"
        second_batch = PollBatch("pi-directory", "d1:2:" + self.member_id,
                                 ({**first, "event_id": event,
                                   "identity": {"session_id": "pi-session",
                                                "turn_id": "user-2"}},),
                                 self.member_id, current)
        _, ref = self.bundle(row=second, row_id="user-2", event_id=event,
                             member_cursor=current)
        ref["row_offset"] = offset
        self.assertEqual(self.admit(second_batch, ref, first_batch.cursor), second_batch.cursor)
        self.assertEqual(self.snapshot()[1:], (2, []))
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT reason FROM plugin_content_status_v1").fetchone()[0],
                             "unanchored_origin")

    def test_invalid_header_and_source_timestamp_are_not_content(self):
        self.header = line({"type": "session", "id": "../invalid"})
        self.file.write_bytes(self.header + self.user)
        batch, ref = self.bundle()
        with self.assertRaisesRegex(ProtocolError, "header identity unavailable"):
            self.admit(batch, ref)
        self.assertEqual(self.snapshot(), (("", 0), 0, []))
        self.header = line({"type": "session", "id": "pi-session"})
        invalid = self.user_line("user-1", "hello", timestamp="not-a-time")
        self.file.write_bytes(self.header + invalid)
        batch, ref = self.bundle(row=invalid)
        with self.assertRaisesRegex(ProtocolError, "timestamp"):
            self.admit(batch, ref)
        self.assertEqual(self.snapshot(), (("", 0), 0, []))

    def test_capacity_prunes_oldest_content_without_stalling_metadata(self):
        from observatory import plugin_pi_content
        batch, ref = self.bundle()
        self.admit(batch, ref)
        second = self.user_line("user-2", "new content")
        with self.file.open("ab") as stream:
            offset = stream.tell()
            stream.write(second)
        cursor = self.checkpoint()
        event = f"pi:{cursor.split(':')[6]}:user-2:message"
        record = {"event_id": event, "revision": 0, "kind": "message",
                  "time_ns": "1792843200000000000",
                  "identity": {"session_id": "pi-session", "turn_id": "user-2"},
                  "payload": {"role": "user"}}
        next_batch = PollBatch(batch.source_id, "d1:2:" + self.member_id,
                               (record,), self.member_id, cursor)
        next_ref = {**ref, "event_id": event, "native_turn_id": "user-2",
                    "row_offset": offset, "row_length": len(second), "row_sha256": sha(second)}
        with patch.object(plugin_pi_content, "MAX_CONTENT_ROWS", 1):
            self.assertEqual(self.admit(next_batch, next_ref, batch.cursor), next_batch.cursor)
        self.assertEqual(self.snapshot()[1:], (2, ["new content"]))
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT reason FROM plugin_content_status_v1").fetchone()[0],
                             "capacity")

    def test_gap_batch_commits_metadata_and_omits_content(self):
        batch, ref = self.bundle()
        self.admit(batch, ref)
        replacement = self.sources / "project" / "replacement.jsonl"
        second = self.user_line("user-2", "secret=new-value")
        replacement.write_bytes(self.header + second)
        os.replace(replacement, self.file)
        new_cursor = self.checkpoint(generation=sha((batch.member_cursor.split(":")[6] + ":gap").encode()))
        new_event = f"pi:{new_cursor.split(':')[6]}:user-2:message"
        new_record = {"event_id": new_event, "revision": 0, "kind": "message",
                      "time_ns": "1792843200000000000",
                      "identity": {"session_id": "pi-session", "turn_id": "user-2"},
                      "payload": {"role": "user"}}
        gap = {"event_id": "pi-gap:unique", "revision": 0, "kind": "diagnostic",
               "time_ns": "0", "payload": {"code": "possible_source_gap", "severity": "warning"}}
        next_batch = PollBatch(batch.source_id, "d1:2:" + self.member_id,
                               (gap, new_record), self.member_id, new_cursor)
        next_ref = {**ref, "event_id": new_event, "native_turn_id": "user-2",
                    "row_length": len(second), "row_sha256": sha(second)}
        self.assertEqual(self.admit(next_batch, next_ref, batch.cursor), next_batch.cursor)
        self.assertEqual(self.snapshot()[2], [])
        self.assertEqual(self.admit(next_batch, next_ref, batch.cursor), next_batch.cursor)
        third = self.user_line("user-3", "secret=still-invalidated")
        with self.file.open("ab") as stream:
            offset = stream.tell()
            stream.write(third)
        clean_cursor = self.checkpoint(generation=new_cursor.split(":")[6])
        third_event = f"pi:{clean_cursor.split(':')[6]}:user-3:message"
        third_batch = PollBatch(batch.source_id, "d1:3:" + self.member_id,
                                ({**new_record, "event_id": third_event,
                                  "identity": {"session_id": "pi-session",
                                               "turn_id": "user-3"}},),
                                self.member_id, clean_cursor)
        third_ref = {**ref, "event_id": third_event, "native_turn_id": "user-3",
                     "row_offset": offset, "row_length": len(third), "row_sha256": sha(third)}
        self.assertEqual(self.admit(third_batch, third_ref, next_batch.cursor), third_batch.cursor)
        self.assertEqual(self.snapshot()[2], [])
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT reason FROM plugin_content_status_v1").fetchone()[0],
                             "unanchored_origin")

    def test_file_verification_does_not_hold_writer_lock(self):
        batch, ref = self.bundle()
        entered = threading.Event()
        resume = threading.Event()
        from observatory import plugin_pi_content
        original = plugin_pi_content.verify_refs
        result = []
        def pause(*args):
            entered.set()
            self.assertTrue(resume.wait(2))
            return original(*args)
        with patch.object(plugin_pi_content, "verify_refs", side_effect=pause):
            worker = threading.Thread(target=lambda: result.append(self._try_admit(batch, ref)))
            worker.start()
            self.assertTrue(entered.wait(2))
            # A separate writer can make progress while source verification waits.
            set_source_capture(self.store, source_id="pi-directory", enabled=False)
            resume.set()
            worker.join(2)
        self.assertFalse(worker.is_alive())
        self.assertEqual(result, ["ProtocolError"])
        self.assertEqual(self.snapshot()[0][0], "")

    def test_prefix_proof_budget_does_not_stop_metadata_cursor(self):
        # Nine valid but unsupported Pi rows push the member checkpoint past
        # this primitive's 8 MiB proof budget. The v1 metadata path advances.
        with self.file.open("ab") as stream:
            for index in range(9):
                stream.write(line({"type": "model_change", "id": f"m-{index}",
                                   "padding": "x" * 999_000}))
            late = self.user_line("user-late", "late content")
            offset = stream.tell()
            stream.write(late)
        cursor = self.checkpoint()
        origin = cursor.split(":")[6]
        event = f"pi:{origin}:user-late:message"
        record = {"event_id": event, "revision": 0, "kind": "message",
                  "time_ns": "1792843200000000000",
                  "identity": {"session_id": "pi-session", "turn_id": "user-late"},
                  "payload": {"role": "user"}}
        batch = PollBatch("pi-directory", "d1:1:" + self.member_id,
                          (record,), self.member_id, cursor)
        ref = {"schema": 1, "source_id": "pi-directory", "member_id": self.member_id,
               "relative_member_path": self.relative, "row_offset": offset,
               "row_length": len(late), "row_sha256": sha(late), "event_id": event,
               "revision": 0, "native_session_id": "pi-session",
               "native_turn_id": "user-late", "kind": "pi.user_text"}
        with self.assertRaises(ProtocolError):
            self.admit(batch, {**ref, "relative_member_path": "../forged.jsonl"})
        self.assertEqual(self.snapshot(), (("", 0), 0, []))
        self.assertEqual(self.admit(batch, ref), batch.cursor)
        self.assertEqual(self.snapshot()[0][0], batch.cursor)
        self.assertEqual(self.snapshot()[1:], (1, []))
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT reason FROM plugin_content_status_v1 "
                                        "WHERE source_id='pi-directory'").fetchone()[0],
                             "proof_budget")
        self.assertEqual(self.admit(batch, ref), batch.cursor)


if __name__ == "__main__":
    unittest.main()
