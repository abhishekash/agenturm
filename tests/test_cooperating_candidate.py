"""Only supplied cooperating instances may be claimed by this offline drain."""

from contextlib import contextmanager
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import threading
import unittest
from unittest.mock import patch

from observatory.cooperating_candidate import prepare_cooperating_candidate
from observatory.core import Store
from observatory.migration_candidate import CandidateError
from observatory.outbound_delivery import OutboundQueue
from observatory.saved_tasks import TaskError, TaskStore
from observatory.workflow_store import WorkflowStore, WorkflowStoreError
from observatory.plugin_wire import ProtocolError
import observatory.cooperating_candidate as coordinator


class CooperatingCandidateTests(unittest.TestCase):
    def setUp(self):
        binary = os.environ.get("AGENTURM_RUST_MIGRATION_VERIFY_BIN")
        if not binary:
            self.skipTest("set AGENTURM_RUST_MIGRATION_VERIFY_BIN to a built verifier")
        self.binary = Path(binary).resolve(strict=True)
        self.temporary = tempfile.TemporaryDirectory(
            prefix="agenturm-cooperating-", dir="/dev/shm" if Path("/dev/shm").is_dir() else None)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.telemetry = Store(self.root / "telemetry.db")
        self.addCleanup(self.telemetry.close)
        self.workflow = WorkflowStore(self.root / "workflow.db")
        self.tasks = TaskStore(self.root / "tasks.db")
        self.outbound = OutboundQueue(self.telemetry)
        self.output = self.root / "candidate"

    def prepare(self, *, timeout=3):
        return prepare_cooperating_candidate(
            telemetry=self.telemetry, workflow=self.workflow, tasks=self.tasks,
            outbound=self.outbound, output=self.output, verifier=self.binary,
            timeout=timeout)

    def test_real_instance_drain_produces_verified_candidate_and_keeps_fences(self):
        manifest = self.prepare()
        self.assertEqual(manifest, self.output / "manifest.json")
        self.assertEqual(json.loads((self.output / "state.json").read_text())["state"], "verified")
        with self.assertRaises(WorkflowStoreError):
            with self.workflow._transaction():
                pass
        with self.assertRaises(TaskError):
            with self.tasks._connect():
                pass
        with self.assertRaises(sqlite3.ProgrammingError):
            with self.telemetry._connect():
                pass

    def test_active_dispatch_blocks_before_fencing_any_store(self):
        with self.outbound._dispatch_lease(exclusive=False):
            with self.assertRaisesRegex(ProtocolError, "dispatch or recovery is active"):
                self.prepare(timeout=0.05)
        self.assertFalse(self.output.exists())
        with self.workflow._transaction(write=False) as db:
            self.assertEqual(db.execute("SELECT 1").fetchone()[0], 1)
        with self.telemetry._connect() as db:
            self.assertEqual(db.execute("SELECT 1").fetchone()[0], 1)

    def test_held_auxiliary_connection_blocks_candidate_without_releasing_fence(self):
        entered = threading.Event()
        release = threading.Event()
        failures = []

        def hold_workflow():
            try:
                with self.workflow._transaction(write=False):
                    entered.set()
                    release.wait(2)
            except Exception as exc:
                failures.append(exc)

        worker = threading.Thread(target=hold_workflow)
        worker.start()
        self.assertTrue(entered.wait(2))
        try:
            with self.assertRaisesRegex(CandidateError, "workflow store did not quiesce"):
                self.prepare(timeout=0.05)
            self.assertFalse(self.output.exists())
            with self.assertRaises(WorkflowStoreError):
                with self.workflow._transaction(write=False):
                    pass
            with self.telemetry._connect() as db:
                self.assertEqual(db.execute("SELECT 1").fetchone()[0], 1)
        finally:
            release.set()
            worker.join(2)
        self.assertFalse(worker.is_alive())
        self.assertEqual(failures, [])

    def test_held_telemetry_connection_blocks_after_auxiliary_drain(self):
        entered = threading.Event()
        release = threading.Event()
        failures = []

        def hold_telemetry():
            try:
                with self.telemetry._connect():
                    entered.set()
                    release.wait(2)
            except Exception as exc:
                failures.append(exc)

        worker = threading.Thread(target=hold_telemetry)
        worker.start()
        self.assertTrue(entered.wait(2))
        try:
            with self.assertRaisesRegex(CandidateError, "telemetry store did not quiesce"):
                self.prepare(timeout=0.05)
            self.assertFalse(self.output.exists())
            with self.assertRaises(WorkflowStoreError):
                with self.workflow._transaction(write=False):
                    pass
            with self.assertRaises(TaskError):
                with self.tasks._connect():
                    pass
        finally:
            release.set()
            worker.join(2)
        self.assertFalse(worker.is_alive())
        self.assertEqual(failures, [])

    def test_rejects_unrelated_outbound_before_fencing(self):
        other = Store(self.root / "other.db")
        self.addCleanup(other.close)
        with self.assertRaisesRegex(CandidateError, "supplied telemetry Store"):
            prepare_cooperating_candidate(
                telemetry=self.telemetry, workflow=self.workflow, tasks=self.tasks,
                outbound=OutboundQueue(other), output=self.output, verifier=self.binary,
                timeout=1)
        self.assertFalse(self.output.exists())
        with self.telemetry._connect() as db:
            self.assertEqual(db.execute("SELECT 1").fetchone()[0], 1)

    def test_guard_failure_after_verified_backup_invalidates_marker(self):
        class FailingGuard:
            def __init__(self):
                self.calls = 0

            def verify(self):
                self.calls += 1
                if self.calls == 2:
                    raise RuntimeError("injected guard path change")

        @contextmanager
        def guard(_timeout):
            yield FailingGuard()

        with patch.object(self.outbound, "dispatch_handoff_guard", side_effect=guard):
            with self.assertRaisesRegex(RuntimeError, "guard path change"):
                self.prepare()
        state = json.loads((self.output / "state.json").read_text())
        self.assertEqual(state["state"], "incomplete")

    def test_invalid_verifier_and_output_fail_before_one_way_fences(self):
        for output, verifier, message in (
            (self.output, self.root / "missing-verifier", "binary unavailable"),
            (Path("relative-candidate"), self.binary, "absolute"),
        ):
            with self.subTest(message=message):
                with self.assertRaisesRegex(CandidateError, message):
                    prepare_cooperating_candidate(
                        telemetry=self.telemetry, workflow=self.workflow, tasks=self.tasks,
                        outbound=self.outbound, output=output, verifier=verifier, timeout=1)
                with self.workflow._transaction(write=False) as db:
                    self.assertEqual(db.execute("SELECT 1").fetchone()[0], 1)
                with self.tasks._connect() as db:
                    self.assertEqual(db.execute("SELECT 1").fetchone()[0], 1)
                with self.telemetry._connect() as db:
                    self.assertEqual(db.execute("SELECT 1").fetchone()[0], 1)
        self.assertFalse(self.output.exists())

    def test_failed_incomplete_marker_sync_unlinks_and_syncs_directory(self):
        class FailingGuard:
            def __init__(self):
                self.calls = 0

            def verify(self):
                self.calls += 1
                if self.calls == 2:
                    raise RuntimeError("late guard failure")

        @contextmanager
        def guard(_timeout):
            yield FailingGuard()

        original_sync = coordinator._sync_directory
        sync_calls = 0

        def fail_first_sync(path):
            nonlocal sync_calls
            sync_calls += 1
            if sync_calls == 1:
                raise OSError("injected incomplete marker sync failure")
            return original_sync(path)

        with patch.object(self.outbound, "dispatch_handoff_guard", side_effect=guard):
            with patch.object(coordinator, "_sync_directory", side_effect=fail_first_sync):
                with self.assertRaisesRegex(CandidateError, "incomplete state may need quarantine"):
                    self.prepare()
        self.assertEqual(sync_calls, 2)
        self.assertFalse((self.output / "state.json").exists())

    def test_dispatch_guard_is_held_through_backup_and_verification(self):
        entered = threading.Event()
        release = threading.Event()
        result = []
        original_prepare = coordinator.prepare_candidate

        def blocked_prepare(**kwargs):
            entered.set()
            if not release.wait(2):
                raise RuntimeError("backup release timed out")
            return original_prepare(**kwargs)

        def run():
            try:
                result.append(self.prepare())
            except Exception as exc:
                result.append(exc)

        with patch.object(coordinator, "prepare_candidate", side_effect=blocked_prepare):
            worker = threading.Thread(target=run)
            worker.start()
            try:
                self.assertTrue(entered.wait(2))
                with self.assertRaisesRegex(ProtocolError, "dispatch or recovery is active"):
                    with self.outbound._dispatch_lease(exclusive=False):
                        pass
            finally:
                release.set()
                worker.join(3)
        self.assertFalse(worker.is_alive())
        self.assertEqual(result, [self.output / "manifest.json"])


if __name__ == "__main__":
    unittest.main()
