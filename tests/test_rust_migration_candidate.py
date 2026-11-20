"""The offline Rust verifier accepts backups from actual Python stores."""

from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import unittest

from observatory.core import Store
from observatory.plugin_admission import activate_source
from observatory.saved_tasks import TaskStore
from observatory.workflow_store import WorkflowStore


class RustMigrationCandidateTests(unittest.TestCase):
    def test_real_store_backups_match_offline_contract(self):
        binary = os.environ.get("AGENTURM_RUST_MIGRATION_VERIFY_BIN")
        if not binary:
            self.skipTest("set AGENTURM_RUST_MIGRATION_VERIFY_BIN to an installed binary")
        with tempfile.TemporaryDirectory(prefix="agenturm-migration-real-") as temporary:
            root = Path(temporary)
            telemetry = root / "telemetry.db"
            workflow = root / "workflow.db"
            tasks = root / "tasks.db"
            store = Store(telemetry)
            try:
                activate_source(store, plugin_id="fixture.plugin",
                                source_id="real-source", generation="a" * 64)
                receiver_id = store.receiver_id
            finally:
                store.close()
            WorkflowStore(workflow)
            TaskStore(tasks).close()

            def backup(source: Path, name: str, schema_version: int) -> dict:
                target = root / name
                with closing(sqlite3.connect(source)) as original:
                    with closing(sqlite3.connect(target)) as copied:
                        original.backup(copied)
                target.chmod(0o600)
                info = target.stat()
                return {"path": str(target), "device": info.st_dev,
                        "inode": info.st_ino, "size_bytes": info.st_size,
                        "sha256": hashlib.sha256(target.read_bytes()).hexdigest(),
                        "schema_version": schema_version}

            telemetry_backup = backup(telemetry, "telemetry.backup.db", 1)
            workflow_backup = backup(workflow, "workflow.backup.db", 2)
            task_backup = backup(tasks, "tasks.backup.db", 1)
            with closing(sqlite3.connect(telemetry_backup["path"])) as db:
                rows = db.execute(
                    "SELECT source_id,cursor,lease_epoch,policy_epoch,dirty_seq,"
                    "last_batch_digest FROM plugin_source_cursor_v1 ORDER BY source_id"
                ).fetchall()
            self.assertEqual(len(rows), 1)
            sources = [dict(zip(("source_id", "cursor", "lease_epoch",
                                 "policy_epoch", "dirty_seq", "last_batch_digest"), row))
                       | {"content_receipt": None, "command_receipt": None}
                       for row in rows]
            manifest = {"format": "migration_manifest_v1", "receiver_id": receiver_id,
                        "telemetry": telemetry_backup, "workflow": workflow_backup,
                        "tasks": task_backup,
                        "snapshot": {"sources": sources,
                                     "workflow_notification_claims": 0,
                                     "workflow_native_events": 0,
                                     "saved_tasks": 0, "max_task_revision": 0}}
            manifest_path = root / "candidate.json"
            manifest_path.write_text(json.dumps(manifest))
            result = subprocess.run([binary, str(manifest_path)], text=True,
                                    capture_output=True, timeout=20)
            self.assertEqual(result.returncode, 0, result.stderr)
            reply = json.loads(result.stdout)
            self.assertEqual(reply["format"], "migration_candidate_verified_v1")
            self.assertEqual(reply["receiver_id"], receiver_id)


if __name__ == "__main__":
    unittest.main()
