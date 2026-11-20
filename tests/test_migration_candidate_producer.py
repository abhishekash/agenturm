"""The explicit offline producer consumes real stopped stores without changing them."""

from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch
import observatory.migration_candidate as producer

from observatory.core import Store
from observatory.migration_candidate import CandidateError, prepare_candidate
from observatory.plugin_admission import activate_source
from observatory.saved_tasks import TaskStore
from observatory.workflow_store import WorkflowStore


class CandidateProducerTests(unittest.TestCase):
    def setUp(self):
        binary = os.environ.get("AGENTURM_RUST_MIGRATION_VERIFY_BIN")
        if not binary:
            self.skipTest("set AGENTURM_RUST_MIGRATION_VERIFY_BIN to a built verifier")
        self.binary = Path(binary).resolve(strict=True)
        self.temporary = tempfile.TemporaryDirectory(
            prefix="agenturm-preparation-", dir="/dev/shm" if Path("/dev/shm").is_dir() else None)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.telemetry = self.root / "telemetry.db"
        self.workflow = self.root / "workflow.db"
        self.tasks = self.root / "tasks.db"
        store = Store(self.telemetry)
        try:
            activate_source(store, plugin_id="fixture.plugin",
                            source_id="real-source", generation="a" * 64)
            self.receiver = store.receiver_id
        finally:
            store.close()
        WorkflowStore(self.workflow)
        TaskStore(self.tasks).close()
        self.paths = (self.telemetry, self.workflow, self.tasks)

    def prepare(self, output=None):
        return prepare_candidate(telemetry=self.telemetry, workflow=self.workflow,
                                 tasks=self.tasks, output=output or self.root / "candidate",
                                 verifier=self.binary)

    def source_state(self):
        return {str(path): tuple((part.stat().st_ino, part.stat().st_mtime_ns,
                                  hashlib.sha256(part.read_bytes()).hexdigest())
                                 if part.exists() else None
                                 for part in (path, Path(f"{path}-wal"), Path(f"{path}-shm"),
                                              Path(f"{path}-journal")))
                for path in self.paths}

    def test_real_stopped_stores_produce_verified_detached_candidate(self):
        with closing(sqlite3.connect(self.telemetry.as_uri() + "?mode=ro&immutable=1",
                                     uri=True)) as main_only:
            self.assertEqual(main_only.execute(
                "SELECT COUNT(*) FROM plugin_source_cursor_v1").fetchone()[0], 0,
                "fixture source row must still reside only in WAL")
        before = self.source_state()
        manifest_path = self.prepare()
        self.assertEqual(self.source_state(), before)
        manifest = json.loads(manifest_path.read_text())
        self.assertEqual(manifest["receiver_id"], self.receiver)
        self.assertEqual([row["source_id"] for row in manifest["snapshot"]["sources"]],
                         ["real-source"])
        self.assertEqual(json.loads((manifest_path.parent / "state.json").read_text()),
                         {"format": "migration_candidate_preparation_v1", "state": "verified",
                          "manifest_sha256": hashlib.sha256(manifest_path.read_bytes()).hexdigest()})
        for name in ("telemetry", "workflow", "tasks"):
            descriptor = manifest[name]
            path = Path(descriptor["path"])
            self.assertEqual(path.parent, manifest_path.parent)
            self.assertEqual(path.stat().st_mode & 0o077, 0)
            self.assertEqual(descriptor["sha256"], hashlib.sha256(path.read_bytes()).hexdigest())

    def test_verifier_failure_leaves_incomplete_candidate_and_unchanged_sources(self):
        before = self.source_state()
        false = Path("/usr/bin/false")
        if not false.is_file():
            self.skipTest("false executable unavailable")
        with self.assertRaisesRegex(CandidateError, "verification failed"):
            prepare_candidate(telemetry=self.telemetry, workflow=self.workflow,
                              tasks=self.tasks, output=self.root / "candidate", verifier=false)
        self.assertEqual(self.source_state(), before)
        self.assertEqual(json.loads((self.root / "candidate" / "state.json").read_text())["state"],
                         "incomplete")

    def test_rollback_journal_and_symlink_are_rejected_before_output(self):
        journal = Path(f"{self.telemetry}-journal")
        journal.write_bytes(b"incomplete transaction")
        with self.assertRaisesRegex(CandidateError, "rollback journal"):
            self.prepare()
        self.assertFalse((self.root / "candidate").exists())
        journal.unlink()
        alias = self.root / "alias.db"
        alias.symlink_to(self.telemetry)
        with self.assertRaisesRegex(CandidateError, "symbolic-link"):
            prepare_candidate(telemetry=alias, workflow=self.workflow, tasks=self.tasks,
                              output=self.root / "candidate", verifier=self.binary)
        self.assertFalse((self.root / "candidate").exists())

    def test_interrupted_backup_leaves_incomplete_marker(self):
        before = self.source_state()
        with patch("observatory.migration_candidate._backup", side_effect=RuntimeError("interrupted")):
            with self.assertRaisesRegex(RuntimeError, "interrupted"):
                self.prepare()
        self.assertEqual(self.source_state(), before)
        state = json.loads((self.root / "candidate" / "state.json").read_text())
        self.assertEqual(state["state"], "incomplete")
        self.assertFalse((self.root / "candidate" / "manifest.json").exists())

    def test_final_directory_fsync_failure_restores_incomplete_marker(self):
        before = self.source_state()
        original = producer._sync_directory
        calls = 0

        def fail_final_sync(path):
            nonlocal calls
            calls += 1
            if calls == 4:
                raise OSError("injected final fsync failure")
            return original(path)

        with patch.object(producer, "_sync_directory", side_effect=fail_final_sync):
            with self.assertRaisesRegex(CandidateError, "publish failed"):
                self.prepare()
        self.assertEqual(self.source_state(), before)
        state = json.loads((self.root / "candidate" / "state.json").read_text())
        self.assertEqual(state["state"], "incomplete")

    def test_repeated_publish_sync_failure_removes_verified_marker(self):
        original = producer._sync_directory
        calls = 0

        def fail_publish_and_recovery(path):
            nonlocal calls
            calls += 1
            if calls in (4, 5):
                raise OSError("injected publish fsync failure")
            return original(path)

        with patch.object(producer, "_sync_directory", side_effect=fail_publish_and_recovery):
            with self.assertRaisesRegex(CandidateError, "publish failed"):
                self.prepare()
        self.assertFalse((self.root / "candidate" / "state.json").exists())

    def test_copy_stops_at_initial_size_even_if_reader_keeps_returning_bytes(self):
        source = self.root / "growing-wal"
        source.write_bytes(b"abcd")
        destination = self.root / "bounded-copy"
        fd = os.open(source, os.O_RDONLY)
        calls = 0

        def endless_read(_fd, amount, _offset):
            nonlocal calls
            calls += 1
            if calls > 2:
                raise AssertionError("copy read beyond initial file size")
            return b"x" * amount

        try:
            with patch.object(producer.os, "pread", side_effect=endless_read):
                producer._copy_fd(fd, destination)
        finally:
            os.close(fd)
        self.assertEqual(destination.read_bytes(), b"xxxx")
        self.assertEqual(calls, 1)


if __name__ == "__main__":
    unittest.main()
