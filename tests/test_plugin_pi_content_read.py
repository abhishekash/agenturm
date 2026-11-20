"""Internal Pi content delivery gate: exact proof and changing-state closure."""
import threading
import json
import unittest
from unittest.mock import patch

from observatory.plugin_admission import activate_source, set_source_capture
from observatory.plugin_pi_content import PiContentGrant
from observatory.plugin_pi_content_read import Available, Unavailable, read_exact_pi_user_text
from observatory.plugin_projection import project_source
from observatory.plugin_supervisor import PluginSupervisor
from tests.test_plugin_pi_content import PiContentTests


class PiContentReadTests(unittest.TestCase):
    def setUp(self):
        self.fixture = PiContentTests("runTest")
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
        self.fixture.admit(batch, ref)
        project_source(self.store, "pi-directory")
        with self.store._read() as db:
            return db.execute("SELECT event_key FROM activity_index_v1 WHERE kind='plugin'").fetchone()[0]

    def test_exact_redacted_text_and_normal_lease_restart(self):
        key = self.publish()
        first = read_exact_pi_user_text(self.store, key)
        self.assertIsInstance(first, Available)
        self.assertEqual(first.redacted_text, "password=[REDACTED]")
        self.assertNotIn("highly-private", repr(first))
        activate_source(self.store, plugin_id="fixture.pi", source_id="pi-directory",
                        generation="a" * 64,
                        content_fingerprint=PiContentGrant.fingerprint_for_source(self.fixture.source))
        second = read_exact_pi_user_text(self.store, key)
        self.assertIsInstance(second, Available)
        self.assertEqual(second.redacted_text, first.redacted_text)
        self.assertNotEqual(second.proof.lease_epoch, first.proof.lease_epoch)

    def test_missing_member_symlink_and_rewritten_row_close(self):
        key = self.publish()
        file = self.fixture.file
        original = file.read_bytes()
        file.unlink()
        self.assertIsInstance(read_exact_pi_user_text(self.store, key), Unavailable)
        file.symlink_to(self.fixture.sources / "elsewhere.jsonl")
        self.assertIsInstance(read_exact_pi_user_text(self.store, key), Unavailable)
        file.unlink()
        file.write_bytes(original.replace(b"highly-private", b"another-secret"))
        self.assertIsInstance(read_exact_pi_user_text(self.store, key), Unavailable)

    def test_revocation_gap_and_pending_projection_close(self):
        key = self.publish()
        with self.store._connect() as db:
            db.execute("INSERT INTO plugin_source_gap_invalidated_v1 VALUES(?,?,?)",
                       ("pi-directory", "pi-session", 1))
        self.assertIsInstance(read_exact_pi_user_text(self.store, key), Unavailable)
        with self.store._connect() as db:
            db.execute("DELETE FROM plugin_source_gap_invalidated_v1")
            db.execute("UPDATE plugin_source_cursor_v1 SET dirty_seq=dirty_seq+1 "
                       "WHERE source_id='pi-directory'")
        self.assertIsInstance(read_exact_pi_user_text(self.store, key), Unavailable)
        set_source_capture(self.store, source_id="pi-directory", enabled=False)
        self.assertIsInstance(read_exact_pi_user_text(self.store, key), Unavailable)

    def test_revoke_during_file_proof_closes_without_text(self):
        key = self.publish()
        from observatory import plugin_pi_content_read as reader
        original = reader._prove_file

        def revoke(*args):
            original(*args)
            set_source_capture(self.store, source_id="pi-directory", enabled=False)

        with patch.object(reader, "_prove_file", side_effect=revoke):
            result = read_exact_pi_user_text(self.store, key)
        self.assertIsInstance(result, Unavailable)
        self.assertNotIn("highly-private", repr(result))
        self.assertNotIn("password=", repr(result))

    def test_path_swap_after_file_proof_closes(self):
        key = self.publish()
        from observatory import plugin_pi_content_read as reader
        original = reader._prove_file
        file = self.fixture.file

        def replace(*args):
            relative = original(*args)
            file.rename(file.with_suffix(".held"))
            file.write_bytes(self.fixture.header + self.fixture.user)
            return relative

        with patch.object(reader, "_prove_file", side_effect=replace):
            result = read_exact_pi_user_text(self.store, key)
        self.assertIsInstance(result, Unavailable)

    def test_same_inode_rewrite_after_first_file_proof_closes(self):
        key = self.publish()
        from observatory import plugin_pi_content_read as reader
        original = reader._prove_file
        file = self.fixture.file
        calls = 0

        def rewrite(*args):
            nonlocal calls
            relative = original(*args)
            calls += 1
            if calls == 1:
                with file.open("r+b") as stream:
                    stream.seek(len(self.fixture.header))
                    stream.write(self.fixture.user.replace(b"highly-private", b"another-secret"))
            return relative

        with patch.object(reader, "_prove_file", side_effect=rewrite):
            result = read_exact_pi_user_text(self.store, key)
        self.assertIsInstance(result, Unavailable)
        self.assertEqual(calls, 1)

    def test_supervisor_detached_during_proof_closes(self):
        key = self.publish()
        from observatory import plugin_pi_content_read as reader
        original = reader._prove_file

        def detach(*args):
            relative = original(*args)
            self.store.plugin_supervisor = None
            return relative

        with patch.object(reader, "_prove_file", side_effect=detach):
            self.assertEqual(read_exact_pi_user_text(self.store, key),
                             Unavailable("source_unavailable"))

    def test_corrupt_stored_text_budget_closes(self):
        key = self.publish()
        with self.store._connect() as db:
            db.execute("UPDATE plugin_content_v1 SET redacted_text=?",
                       ("x" * (1024 * 1024 + 1),))
        self.assertEqual(read_exact_pi_user_text(self.store, key),
                         Unavailable("content_budget"))

    def test_forged_activity_payload_closes(self):
        key = self.publish()
        with self.store._connect() as db:
            row = db.execute("SELECT payload FROM activity_index_v1 WHERE event_key=?",
                             (key,)).fetchone()
            payload = json.loads(row[0])
            payload["session_id"] = "plugin:other"
            db.execute("UPDATE activity_index_v1 SET payload=? WHERE event_key=?",
                       (json.dumps(payload), key))
        self.assertEqual(read_exact_pi_user_text(self.store, key),
                         Unavailable("activity_changed"))

    def test_empty_user_text_is_available(self):
        self.fixture.user = self.fixture.user_line("user-1", "")
        self.fixture.file.write_bytes(self.fixture.header + self.fixture.user)
        key = self.publish()
        result = read_exact_pi_user_text(self.store, key)
        self.assertIsInstance(result, Available)
        self.assertEqual(result.redacted_text, "")

    def test_no_supervisor_and_wrong_activity_are_closed(self):
        key = self.publish()
        self.assertEqual(read_exact_pi_user_text(self.store, "invalid"),
                         Unavailable("invalid_activity"))
        self.store.plugin_supervisor = None
        self.assertEqual(read_exact_pi_user_text(self.store, key),
                         Unavailable("source_unavailable"))


if __name__ == "__main__":
    unittest.main()
