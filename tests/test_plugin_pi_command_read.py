"""Internal Pi requested-command read gate and changing-state closure."""
import json
import threading
import unittest
from dataclasses import replace
from unittest.mock import patch

from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
from observatory.plugin_pi_command import PiCommandGrant
from observatory.plugin_pi_command_read import (
    Available, Unavailable, read_exact_pi_requested_command,
)
from observatory.plugin_projection import project_source
from observatory.plugin_supervisor import PluginSupervisor
from tests import test_plugin_pi_command


class PiCommandReadTests(unittest.TestCase):
    def setUp(self):
        self.fixture = test_plugin_pi_command.PiCommandAdmissionTests("runTest")
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.store = self.fixture.store
        supervisor = object.__new__(PluginSupervisor)
        supervisor.store = self.store
        supervisor.sources = (self.fixture.source,)
        supervisor.stop = threading.Event()
        self.store.plugin_supervisor = supervisor
        self.supervisor = supervisor

    def publish(self):
        batch, ref = self.fixture.bundle()
        commit_batch(self.store, self.fixture.lease, "", batch,
                     command_refs=(ref,), command_grant=self.fixture.grant)
        project_source(self.store, self.fixture.source.source_id)
        with self.store._read() as db:
            return db.execute("SELECT event_key FROM activity_index_v1 WHERE kind='plugin'").fetchone()[0]

    def read(self, key):
        return read_exact_pi_requested_command(self.store, key)

    def test_exact_requested_command_and_normal_lease_restart(self):
        key = self.publish()
        first = self.read(key)
        self.assertIsInstance(first, Available)
        self.assertEqual(first.redacted_command, "password=[REDACTED] npm test")
        self.assertNotIn("highly-private", repr(first))
        self.assertNotIn("private assistant", repr(first))
        activate_source(self.store, plugin_id=self.fixture.source.manifest.id,
                        source_id=self.fixture.source.source_id,
                        generation=self.fixture.source.manifest.artifact_sha256,
                        content_fingerprint=PiCommandGrant.fingerprint_for_source(self.fixture.source))
        second = self.read(key)
        self.assertIsInstance(second, Available)
        self.assertEqual(second.redacted_command, first.redacted_command)
        self.assertNotEqual(second.proof.lease_epoch, first.proof.lease_epoch)

    def test_wrong_activity_and_detached_supervisor_close(self):
        key = self.publish()
        self.assertEqual(self.read("invalid"), Unavailable("invalid_activity"))
        self.supervisor.sources = (replace(self.fixture.source,
                                           capture_mode="metadata-only"),)
        self.assertIsInstance(self.read(key), Unavailable)
        self.supervisor.sources = (self.fixture.source,)
        self.store.plugin_supervisor = None
        self.assertEqual(self.read(key), Unavailable("source_unavailable"))

    def test_revocation_gap_retention_and_pending_projection_close(self):
        key = self.publish()
        with self.store._connect() as db:
            db.execute("INSERT INTO plugin_source_gap_invalidated_v1 VALUES(?,?,?)",
                       (self.fixture.source.source_id, "pi-session", 1))
        self.assertIsInstance(self.read(key), Unavailable)
        with self.store._connect() as db:
            db.execute("DELETE FROM plugin_source_gap_invalidated_v1")
            db.execute("INSERT INTO plugin_retention_invalidated_v1 "
                       "(source_id,native_session_id,last_record_ns) VALUES(?,?,?)",
                       (self.fixture.source.source_id, "pi-session", 1))
        self.assertIsInstance(self.read(key), Unavailable)
        with self.store._connect() as db:
            db.execute("DELETE FROM plugin_retention_invalidated_v1")
            db.execute("UPDATE plugin_source_cursor_v1 SET dirty_seq=dirty_seq+1 "
                       "WHERE source_id=?", (self.fixture.source.source_id,))
        self.assertIsInstance(self.read(key), Unavailable)
        set_source_capture(self.store, source_id=self.fixture.source.source_id, enabled=False)
        self.assertIsInstance(self.read(key), Unavailable)

    def test_correction_or_wrong_stored_identity_closes(self):
        key = self.publish()
        with self.store._connect() as db:
            db.execute("UPDATE plugin_command_content_v1 SET block_index=2")
        self.assertIsInstance(self.read(key), Unavailable)
        with self.store._connect() as db:
            db.execute("UPDATE plugin_command_content_v1 SET block_index=1, native_turn_id='user-fake'")
        self.assertIsInstance(self.read(key), Unavailable)
        with self.store._connect() as db:
            db.execute("UPDATE plugin_command_content_v1 SET native_turn_id='user-1', "
                       "tool_call_id='call-2'")
        self.assertIsInstance(self.read(key), Unavailable)
        with self.store._connect() as db:
            db.execute("UPDATE plugin_command_content_v1 SET tool_call_id='call-1'")
            db.execute("UPDATE plugin_evidence_v1 SET revision=1")
        self.assertIsInstance(self.read(key), Unavailable)

    def test_source_symlink_rewrite_and_member_mapping_close(self):
        key = self.publish()
        original = self.fixture.file.read_bytes()
        self.fixture.file.unlink()
        self.assertIsInstance(self.read(key), Unavailable)
        target = self.fixture.sources / "target.jsonl"
        target.write_bytes(original)
        self.fixture.file.symlink_to(target)
        self.assertIsInstance(self.read(key), Unavailable)
        self.fixture.file.unlink()
        self.fixture.file.write_bytes(original.replace(b"npm test", b"npm lint"))
        self.assertIsInstance(self.read(key), Unavailable)
        self.fixture.file.write_bytes(original)
        with self.store._connect() as db:
            db.execute("UPDATE plugin_session_member_v1 SET member_id='' "
                       "WHERE source_id=?", (self.fixture.source.source_id,))
        self.assertIsInstance(self.read(key), Unavailable)

    def test_corrupt_stored_command_and_activity_close(self):
        key = self.publish()
        with self.store._connect() as db:
            db.execute("UPDATE plugin_command_content_v1 SET redacted_command=?",
                       ("x" * (1024 * 1024 + 1),))
        self.assertEqual(self.read(key), Unavailable("command_budget"))
        with self.store._connect() as db:
            db.execute("UPDATE plugin_command_content_v1 SET redacted_command='faked'")
        self.assertIsInstance(self.read(key), Unavailable)
        with self.store._connect() as db:
            db.execute("UPDATE plugin_command_content_v1 "
                       "SET redacted_command='password=[REDACTED] npm test'")
            row = db.execute("SELECT payload FROM activity_index_v1 WHERE event_key=?",
                             (key,)).fetchone()
            payload = json.loads(row[0])
            payload["operation_id"] = "plugin-operation:fake"
            db.execute("UPDATE activity_index_v1 SET payload=? WHERE event_key=?",
                       (json.dumps(payload), key))
        self.assertEqual(self.read(key), Unavailable("activity_changed"))

    def test_revoke_during_proof_closes_without_text(self):
        key = self.publish()
        from observatory import plugin_pi_command_read as reader
        original = reader._prove_file

        def revoke(*args):
            result = original(*args)
            set_source_capture(self.store, source_id=self.fixture.source.source_id,
                               enabled=False)
            return result

        with patch.object(reader, "_prove_file", side_effect=revoke):
            result = self.read(key)
        self.assertIsInstance(result, Unavailable)
        self.assertNotIn("npm test", repr(result))

    def test_path_swap_after_first_proof_closes(self):
        key = self.publish()
        from observatory import plugin_pi_command_read as reader
        original = reader._prove_file

        def swap(*args):
            result = original(*args)
            self.fixture.file.rename(self.fixture.file.with_suffix(".held"))
            self.fixture.file.write_bytes(self.fixture.header + self.fixture.user + self.fixture.row)
            return result

        with patch.object(reader, "_prove_file", side_effect=swap):
            self.assertIsInstance(self.read(key), Unavailable)

    def test_same_inode_rewrite_after_first_proof_closes(self):
        key = self.publish()
        from observatory import plugin_pi_command_read as reader
        original = reader._prove_file
        calls = 0

        def rewrite(*args):
            nonlocal calls
            result = original(*args)
            calls += 1
            if calls == 1:
                with self.fixture.file.open("r+b") as stream:
                    stream.seek(len(self.fixture.header + self.fixture.user))
                    stream.write(self.fixture.row.replace(b"npm test", b"npm lint"))
            return result

        with patch.object(reader, "_prove_file", side_effect=rewrite):
            self.assertIsInstance(self.read(key), Unavailable)
        self.assertEqual(calls, 1)

    def test_supervisor_detach_during_proof_closes(self):
        key = self.publish()
        from observatory import plugin_pi_command_read as reader
        original = reader._prove_file

        def detach(*args):
            result = original(*args)
            self.store.plugin_supervisor = None
            return result

        with patch.object(reader, "_prove_file", side_effect=detach):
            self.assertEqual(self.read(key), Unavailable("source_unavailable"))


if __name__ == "__main__":
    unittest.main()
