"""Installed Pi directory reader through durable per-file Store admission."""
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import member_cursors
from observatory.plugin_config import read_plugin_config
from observatory.plugin_projection import project_source
from observatory.plugin_runtime import PluginSession
from observatory.plugin_wire import ProtocolError


INSTALLER = Path(__file__).resolve().parents[1] / "examples" / "rust_pi_harness_plugin" / "install.py"


def session(path, session_id, *, stop="stop"):
    rows = [
        {"type": "session", "id": session_id},
        {"type": "message", "id": f"{session_id}-user",
         "timestamp": "2026-10-24T12:00:00Z", "message": {"role": "user",
         "content": [{"type": "text", "text": "private request"}]}},
        {"type": "message", "id": f"{session_id}-assistant",
         "timestamp": "2026-10-24T12:00:01Z", "message": {"role": "assistant",
         "stopReason": stop, "content": []}},
    ]
    path.write_text("".join(json.dumps(row, separators=(",", ":")) + "\n" for row in rows))


def gap_id(evidence):
    return "pi-discovery-gap:" + hashlib.sha256(evidence.encode()).hexdigest()


class PiDirectoryTests(unittest.TestCase):
    def setUp(self):
        binary = os.environ.get("AGENTURM_RUST_PI_BIN")
        if not binary:
            self.skipTest("installed Rust Pi binary required")
        self.binary = binary
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.sources = self.root / "sessions"
        (self.sources / "project").mkdir(parents=True)
        self.first = self.sources / "project" / "one.jsonl"
        self.second = self.sources / "project" / "two.jsonl"
        session(self.first, "pi-one")
        session(self.second, "pi-two")
        result = subprocess.run(
            [sys.executable, str(INSTALLER), "--binary", binary,
             "--destination", str(self.root / "installed"), "--source", str(self.sources),
             "--directory"], check=True, capture_output=True, text=True)
        self.plugin = read_plugin_config(result.stdout.strip())[0]
        self.assertEqual(self.plugin.source_id, "rust-pi-directory-source")
        self.assertEqual(self.plugin.required_features,
                         ("source-discovery/1", "source-discovery-root-gap/1"))
        self.store = Store(self.root / "store.db")
        self.addCleanup(self.store.close)

    def worker(self):
        return PluginSession(self.store, self.plugin.manifest,
                             source_id=self.plugin.source_id,
                             allowed_grants=self.plugin.allowed_grants,
                             source_root=self.plugin.source_root,
                             required_features=self.plugin.runtime_features)

    def sessions(self):
        project_source(self.store, self.plugin.source_id)
        return {row["native_session_id"]: row for row in
                self.store.plugin_sessions()["sessions"]}

    def admit_two(self):
        with self.worker() as worker:
            batches = [worker.poll_once(), worker.poll_once()]
            self.assertEqual(len(worker.members), 2)
            self.assertTrue(all(batch.member_id and batch.member_cursor for batch in batches))
            self.assertTrue(all(batch.cursor.startswith("d1:") for batch in batches))
            self.assertEqual(worker.poll_once().records, ())
            return dict(worker.members), worker.cursor

    def test_multi_session_restart_removal_and_replacement_are_scoped(self):
        checkpoints, cursor = self.admit_two()
        self.assertEqual({name: row["run_state"] for name, row in self.sessions().items()},
                         {"pi-one": "completed", "pi-two": "completed"})
        self.assertEqual(len(self.store.activity_page()["activity"]), 4)
        with self.worker() as worker:
            self.assertEqual((worker.members, worker.cursor), (checkpoints, cursor))
            self.assertEqual(worker.poll_once().records, ())
            self.first.unlink()
            previous = worker.cursor
            removed = worker.poll_once()
            self.assertEqual(removed.records[0]["event_id"],
                             gap_id(f"{self.plugin.source_id}:{removed.member_id}:{previous}"))
            self.assertEqual(removed.records[0]["payload"]["code"], "possible_source_gap")
            self.assertEqual(removed.member_cursor, "")
            self.assertEqual(len(member_cursors(self.store, worker.lease)), 1)
            rows = self.sessions()
            self.assertEqual((rows["pi-one"]["run_state"], rows["pi-one"]["reason_code"]),
                             ("unknown", "source_gap"))
            self.assertEqual(rows["pi-two"]["run_state"], "completed")
            third = self.sources / "project" / "three.jsonl"
            session(third, "pi-three")
            worker.poll_once()
            self.assertEqual(self.sessions()["pi-three"]["run_state"], "completed")
        with self.store._connect() as db:
            retained = "".join(row[0] for row in db.execute(
                "SELECT payload_json FROM plugin_evidence_v1"))
        self.assertNotIn("private request", retained)
        self.assertNotIn(str(self.sources), retained)

    def test_acknowledged_removal_then_same_path_reappearance_keeps_old_session_unknown(self):
        self.admit_two()
        self.first.unlink()
        with self.worker() as worker:
            removed = worker.poll_once()
            self.assertEqual(removed.member_cursor, "")
            self.assertEqual(len(worker.members), 1)
        self.assertEqual(self.sessions()["pi-one"]["reason_code"], "source_gap")
        self.assertEqual(self.sessions()["pi-two"]["run_state"], "completed")

        # The removed member has no local checkpoint. Re-reading identical
        # bytes must not turn the already gapped session back to Completed.
        session(self.first, "pi-one")
        with self.worker() as worker:
            restored = worker.poll_once()
            self.assertTrue(restored.member_cursor)
            self.assertEqual(len(worker.members), 2)
        self.assertEqual((self.sessions()["pi-one"]["run_state"],
                          self.sessions()["pi-one"]["reason_code"]),
                         ("unknown", "source_gap"))
        with self.worker() as worker:
            self.assertEqual(len(worker.members), 2)
            self.assertEqual(worker.poll_once().records, ())
        self.assertEqual(self.sessions()["pi-one"]["run_state"], "unknown")

        # A genuinely new session in that path can be admitted from its own
        # complete header, while the old tombstone and healthy sibling remain.
        self.first.unlink()
        with self.worker() as worker:
            self.assertEqual(worker.poll_once().member_cursor, "")
        session(self.first, "pi-new")
        with self.worker() as worker:
            self.assertTrue(worker.poll_once().member_cursor)
        states = self.sessions()
        self.assertEqual((states["pi-one"]["run_state"],
                          states["pi-one"]["reason_code"]),
                         ("unknown", "source_gap"))
        self.assertEqual(states["pi-two"]["run_state"], "completed")
        self.assertEqual(states["pi-new"]["run_state"], "completed")
        with self.store._connect() as db:
            self.assertEqual(db.execute(
                "SELECT COUNT(*) FROM plugin_source_gap_invalidated_v1 "
                "WHERE source_id=? AND native_session_id='pi-one'",
                (self.plugin.source_id,)).fetchone()[0], 1)

    def test_root_outage_is_one_durable_source_wide_gap(self):
        checkpoints, cursor = self.admit_two()
        moved = self.root / "unavailable"
        self.sources.rename(moved)
        try:
            with self.worker() as worker:
                self.assertEqual((worker.members, worker.cursor), (checkpoints, cursor))
                gap = worker.poll_once()
                self.assertTrue(gap.root_gap)
                self.assertFalse(gap.member_id)
                self.assertTrue(gap.cursor.startswith("d2:"))
                self.assertEqual(gap.records[0]["event_id"],
                                 gap_id(f"{self.plugin.source_id}:{gap.cursor}:root"))
                self.assertEqual(dict(worker.members), checkpoints)
                self.assertEqual({row["run_state"] for row in self.sessions().values()},
                                 {"unknown"})
                outage = worker.cursor
            with self.worker() as worker:
                self.assertEqual(worker.cursor, outage)
                with self.assertRaises(ProtocolError):
                    worker.poll_once()
        finally:
            moved.rename(self.sources)
        with self.worker() as worker:
            recovered = worker.poll_once()
            self.assertFalse(recovered.records)
            self.assertTrue(recovered.cursor.startswith("d1:"))
            self.assertEqual(dict(worker.members), checkpoints)
            self.assertEqual({row["run_state"] for row in self.sessions().values()},
                             {"unknown"})

    def test_rust_host_preserves_directory_negotiation_and_root_gap(self):
        host = os.environ.get("AGENTURM_RUST_PLUGIN_HOST_BIN")
        if not host:
            self.skipTest("installed Rust plugin host required")
        kwargs = {"rust_host_executable": host,
                  "rust_host_sha256": hashlib.sha256(Path(host).read_bytes()).hexdigest()}
        with PluginSession(self.store, self.plugin.manifest,
                           source_id=self.plugin.source_id,
                           allowed_grants=self.plugin.allowed_grants,
                           source_root=self.plugin.source_root,
                           required_features=self.plugin.runtime_features,
                           **kwargs) as worker:
            worker.poll_once()
            worker.poll_once()
            self.assertEqual(set(self.sessions()), {"pi-one", "pi-two"})
        moved = self.root / "host-unavailable"
        self.sources.rename(moved)
        try:
            with PluginSession(self.store, self.plugin.manifest,
                               source_id=self.plugin.source_id,
                               allowed_grants=self.plugin.allowed_grants,
                               source_root=self.plugin.source_root,
                               required_features=self.plugin.runtime_features,
                               **kwargs) as worker:
                gap = worker.poll_once()
                self.assertTrue(gap.root_gap)
                self.assertTrue(gap.cursor.startswith("d2:"))
                self.assertEqual({row["run_state"] for row in self.sessions().values()},
                                 {"unknown"})
        finally:
            moved.rename(self.sources)

    @unittest.skipIf(hasattr(os, "geteuid") and os.geteuid() == 0,
                     "root can read a permission-denied test file")
    def test_unreadable_member_keeps_checkpoint_and_recovers(self):
        checkpoints, _ = self.admit_two()
        self.first.chmod(0)
        try:
            with self.worker() as worker:
                gap = None
                for _ in range(3):
                    previous = worker.cursor
                    batch = worker.poll_once()
                    if batch.cursor.startswith("d3:") and batch.records:
                        gap = batch
                        break
                self.assertIsNotNone(gap)
                self.assertEqual(gap.records[0]["event_id"],
                                 gap_id(f"{self.plugin.source_id}:{gap.member_id}:{previous}"))
                self.assertEqual(gap.member_cursor, checkpoints[gap.member_id])
                self.assertEqual(dict(worker.members), checkpoints)
                self.assertEqual(len(member_cursors(self.store, worker.lease)), 2)
                self.assertEqual(self.sessions()["pi-one"]["run_state"], "unknown")
                self.assertEqual(self.sessions()["pi-two"]["run_state"], "completed")
                with self.assertRaises(ProtocolError):
                    worker.poll_once()
        finally:
            self.first.chmod(0o600)
        with self.worker() as worker:
            self.assertTrue(worker.cursor.startswith("d3:"))
            recovered = worker.poll_once()
            self.assertTrue(recovered.cursor.startswith("d1:"))
            self.assertEqual(dict(worker.members), checkpoints)
            self.assertEqual({row["run_state"] for row in self.sessions().values()},
                             {"unknown", "completed"})

    def test_invalid_pi_format_does_not_commit_a_source_gap(self):
        self.admit_two()
        (self.sources / "project" / "invalid.jsonl").write_text(
            '{"type":"not-a-pi-session"}\n')
        with self.worker() as worker:
            old_cursor = worker.cursor
            for _ in range(4):
                try:
                    batch = worker.poll_once()
                except ProtocolError:
                    break
                self.assertFalse(batch.records)
            else:
                self.fail("malformed Pi member never failed")
        with self.worker() as worker:
            self.assertEqual(worker.cursor, old_cursor)
        self.assertEqual({row["run_state"] for row in self.sessions().values()},
                         {"completed"})

    def test_installer_requires_explicit_safe_directory(self):
        options = [sys.executable, str(INSTALLER), "--binary", self.binary,
                   "--destination", str(self.root / "invalid"), "--source"]
        for source, directory in ((self.sources, False), (self.root / "missing", True),
                                  (self.first, True)):
            args = [*options, str(source), *(["--directory"] if directory else [])]
            failed = subprocess.run(args, capture_output=True, text=True)
            self.assertNotEqual(failed.returncode, 0)
        linked = self.root / "linked"
        linked.symlink_to(self.sources, target_is_directory=True)
        failed = subprocess.run([*options, str(linked), "--directory"],
                                capture_output=True, text=True)
        self.assertNotEqual(failed.returncode, 0)
        self.assertFalse((self.root / "invalid").exists())
        default_single = subprocess.run(
            [sys.executable, str(INSTALLER), "--binary", self.binary,
             "--destination", str(self.root / "default-single"),
             "--source", str(self.first)], check=True, capture_output=True, text=True)
        self.assertEqual(read_plugin_config(default_single.stdout.strip())[0].source_id,
                         "rust-pi-source")

    def test_mode_switch_with_explicit_reused_source_id_keeps_checkpoint(self):
        result = subprocess.run(
            [sys.executable, str(INSTALLER), "--binary", self.binary,
             "--destination", str(self.root / "single-installed"),
             "--source", str(self.first), "--source-id", self.plugin.source_id],
            check=True, capture_output=True, text=True)
        single = read_plugin_config(result.stdout.strip())[0]
        self.assertEqual(single.source_id, self.plugin.source_id)
        self.assertEqual(single.required_features, ())

        def single_worker(store):
            return PluginSession(store, single.manifest, source_id=single.source_id,
                                 allowed_grants=single.allowed_grants,
                                 source_root=single.source_root,
                                 required_features=single.runtime_features)

        def durable_state(store):
            with store._connect() as db:
                row = db.execute("SELECT cursor,dirty_seq,policy_epoch "
                                 "FROM plugin_source_cursor_v1 WHERE source_id=?",
                                 (self.plugin.source_id,)).fetchone()
                evidence = db.execute("SELECT COUNT(*) FROM plugin_evidence_v1 "
                                      "WHERE source_id=?", (self.plugin.source_id,)).fetchone()[0]
                return (*row, evidence)

        # A directory checkpoint cannot be consumed by the single-file parser.
        self.admit_two()
        directory_state = durable_state(self.store)
        self.assertTrue(directory_state[0].startswith("d1:"))
        with single_worker(self.store) as worker:
            with self.assertRaises(ProtocolError):
                worker.poll_once()
        self.assertEqual(durable_state(self.store), directory_state)

        # The reverse switch also fails visibly without acknowledging a cursor.
        other_store = Store(self.root / "single-first.db")
        self.addCleanup(other_store.close)
        with single_worker(other_store) as worker:
            worker.poll_once()
        single_state = durable_state(other_store)
        self.assertTrue(single_state[0].startswith("v2:"))
        with PluginSession(other_store, self.plugin.manifest,
                           source_id=self.plugin.source_id,
                           allowed_grants=self.plugin.allowed_grants,
                           source_root=self.plugin.source_root,
                           required_features=self.plugin.runtime_features) as worker:
            with self.assertRaises(ProtocolError):
                worker.poll_once()
        self.assertEqual(durable_state(other_store), single_state)


if __name__ == "__main__":
    unittest.main()
