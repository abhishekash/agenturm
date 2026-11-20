"""The explicit offline producer consumes real stopped stores without changing them."""

import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

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


if __name__ == "__main__":
    unittest.main()
