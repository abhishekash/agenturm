"""Installed current-schema OpenCode reader through real Store admission."""
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory.opencode import OpenCodeReader
from observatory.plugin_admission import set_source_capture
from observatory.plugin_config import read_plugin_config
from observatory.plugin_projection import project_source
from observatory.plugin_runtime import PluginSession
from observatory.plugin_wire import ProtocolError
from tests.native_schema_fixture import load_current_opencode


ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "examples" / "rust_opencode_harness_plugin"


class RustOpenCodeHarnessTests(unittest.TestCase):
    def setUp(self):
        executable = os.environ.get("AGENTURM_RUST_OPENCODE_BIN")
        if not executable:
            self.skipTest("Rust OpenCode binary is built in the Rust CI job")
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.source = self.root / "opencode.db"
        self.fixture = load_current_opencode(self.source)
        output = subprocess.run(
            [sys.executable, str(PACKAGE / "install.py"), "--binary", executable,
             "--destination", str(self.root / "installed"), "--source", str(self.source)],
            capture_output=True, text=True, check=True)
        self.plugin = read_plugin_config(Path(output.stdout.strip()))[0]
        self.assertEqual(self.plugin.replaces, ())
        self.store = Store(self.root / "process.db")
        self.addCleanup(self.store.close)

    def worker(self):
        return PluginSession(self.store, self.plugin.manifest,
                             source_id=self.plugin.source_id,
                             allowed_grants=self.plugin.allowed_grants,
                             timeout=self.plugin.timeout,
                             source_root=self.plugin.source_root)

    def projected(self):
        project_source(self.store, self.plugin.source_id)
        return self.store.plugin_sessions()["sessions"]

    def drain(self, worker):
        batches = []
        for _ in range(10):
            batch = worker.poll_once()
            batches.append(batch)
            if not batch.records:
                break
        else:
            self.fail("OpenCode reader did not reach idle")
        return batches

    def test_fixture_matches_native_lifecycle_without_content(self):
        native = Store(self.root / "native.db")
        self.addCleanup(native.close)
        native_reader = OpenCodeReader(native, capture_details=False, path=self.source)
        with patch("observatory.opencode.time.time_ns", return_value=self.fixture["clock_ns"]):
            native_reader.poll()
        self.assertEqual(native_reader.sessions[0]["run_state"], "completed")
        with self.worker() as worker:
            batches = self.drain(worker)
            sessions = self.projected()
            self.assertEqual(len(sessions), 1)
            self.assertEqual(sessions[0]["run_state"], native_reader.sessions[0]["run_state"])
            self.assertEqual(sessions[0]["reason_code"], "opencode_idle_succeeded")
            self.assertEqual(sessions[0]["metadata_usage"],
                             {"input_tokens": 117, "output_tokens": 109,
                              "cache_read_tokens": 0, "cache_write_tokens": 0,
                              "total_tokens": 226})
            activity = self.store.activity_page()["activity"]
            self.assertEqual(len([x for x in activity if x["domain"] == "message"]), 6)
            self.assertEqual(len([x for x in activity if x["domain"] == "tool"]), 2)
            self.assertTrue(all(not x["turn_id"] for x in activity))
            admitted = ""
            with self.store._connect() as db:
                admitted = "".join(row[0] for row in db.execute("SELECT payload_json FROM plugin_evidence_v1"))
            for private in ("Implement current", "pending request", "Synthetic recorded", "/synthetic/project",
                            "pytest tests", "oldString", "corrected"):
                self.assertNotIn(private, json.dumps(activity) + admitted)
            last = batches[-1].cursor
        with self.worker() as resumed:
            self.assertEqual(resumed.cursor, last)
            self.assertEqual(resumed.poll_once().records, ())
            set_source_capture(self.store, source_id=self.plugin.source_id, enabled=False)
            self.assertEqual(self.store.activity_page()["activity"], [])
            with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
                resumed.poll_once()

    def test_assistant_finish_does_not_complete_until_idle_and_part_correction_revises(self):
        with sqlite3.connect(self.source) as db:
            db.execute("DELETE FROM session_message WHERE seq>=50")
            db.execute("DELETE FROM session_part WHERE message_id='msg_step_other'")
        with self.worker() as worker:
            self.drain(worker)
            self.assertEqual(self.projected()[0]["run_state"], "running")
            with sqlite3.connect(self.source) as db:
                db.execute("UPDATE session_part SET data=json_set(data,'$.state.status','error'),time_updated=time_updated+1 WHERE message_id='msg_step_two' AND ordinal=1")
            self.drain(worker)
            self.projected()
            tools = [x for x in self.store.activity_page()["activity"] if x["domain"] == "tool"]
            self.assertEqual({x["result"] for x in tools}, {"succeeded", "failed"})
            with sqlite3.connect(self.source) as db:
                db.execute("INSERT INTO session_message VALUES(?,?,?,?,?,?,?)",
                           ("msg_idle_one", "ses_schema_fixture", "idle", 50, 1794650400050,
                            1794650400050, json.dumps({"outcome":"succeeded"})))
            self.drain(worker)
            self.assertEqual(self.projected()[0]["run_state"], "completed")

    def test_old_layout_fails_before_ack_and_db_does_not_change(self):
        with sqlite3.connect(self.source) as db:
            db.execute("DROP TABLE session_part")
        before = self.source.read_bytes()
        with self.worker() as worker:
            with self.assertRaises(ProtocolError):
                worker.poll_once()
        self.assertEqual(self.source.read_bytes(), before)
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 0)

    def test_database_replacement_reports_gap_and_new_epoch(self):
        with self.worker() as worker:
            self.drain(worker)
            self.source.rename(self.root / "original.db")
            replacement = self.root / "replacement.db"
            load_current_opencode(replacement)
            with sqlite3.connect(replacement) as db:
                db.execute("UPDATE session_v2 SET id='ses_replacement'")
                db.execute("UPDATE session_message SET session_id='ses_replacement'")
                db.execute("UPDATE session_part SET session_id='ses_replacement'")
            replacement.rename(self.source)
            batch = worker.poll_once()
            self.assertEqual(batch.records[0]["payload"]["code"], "possible_source_gap")
            self.drain(worker)
            self.assertEqual({s["native_session_id"] for s in self.projected()},
                             {"ses_schema_fixture", "ses_replacement"})

    def test_deleted_terminal_idle_revises_status_and_backfill_does_not_wedge(self):
        with self.worker() as worker:
            self.drain(worker)
            self.assertEqual(self.projected()[0]["run_state"], "completed")
            with sqlite3.connect(self.source) as db:
                db.execute("DELETE FROM session_message WHERE id='msg_idle_two'")
            batches = self.drain(worker)
            self.assertEqual(batches[0].records[0]["payload"]["code"], "possible_source_gap")
            self.assertEqual(self.projected()[0]["run_state"], "running")
            self.assertEqual(self.projected()[0]["reason_code"], "opencode_latest_assistant")
            with sqlite3.connect(self.source) as db:
                db.execute("INSERT INTO session_message VALUES(?,?,?,?,?,?,?)",
                           ("msg_backfilled_idle", "ses_schema_fixture", "idle", 25,
                            1794650400025, 1794650400025, '{"outcome":"succeeded"}'))
            self.drain(worker)
            self.assertEqual(self.projected()[0]["run_state"], "running")

    def test_huge_identifiers_are_skipped_before_materialization(self):
        huge = "x" * (2 * 1024 * 1024)
        with sqlite3.connect(self.source) as db:
            db.execute("INSERT INTO session_message VALUES(?,?,?,?,?,?,?)",
                       (huge, "ses_schema_fixture", "assistant", 90,
                        1794650400090, 1794650400090, "{}"))
        with self.worker() as worker:
            self.drain(worker)
            with self.store._connect() as db:
                payloads = "".join(row[0] for row in db.execute("SELECT payload_json FROM plugin_evidence_v1"))
            self.assertIn("oversized_opencode_identity", payloads)
            self.assertNotIn(huge[:1000], payloads)

    def test_row_ceiling_fails_before_ack(self):
        with sqlite3.connect(self.source) as db:
            db.executemany("INSERT INTO session_part VALUES(?,?,?,?,?,?,?,?)",
                (("msg_step_one", "ses_schema_fixture", n + 10, "text", None,
                  1794650400020, 1794650400020, "{}") for n in range(50_000)))
        with self.worker() as worker:
            with self.assertRaises(ProtocolError):
                worker.poll_once()
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0], "")

    def test_installer_rejects_symlinked_source_and_destination_ancestor(self):
        target = self.root / "target"
        target.mkdir()
        linked = self.root / "linked"
        linked.symlink_to(target, target_is_directory=True)
        command = [sys.executable, str(PACKAGE / "install.py"), "--binary",
                   os.environ["AGENTURM_RUST_OPENCODE_BIN"], "--source", str(self.source)]
        denied = subprocess.run(command + ["--destination", str(linked / "nested")],
                                capture_output=True, text=True)
        self.assertNotEqual(denied.returncode, 0)
        self.assertEqual(list(target.iterdir()), [])
        source_link = self.root / "source-link.db"
        source_link.symlink_to(self.source)
        denied = subprocess.run(command[:-1] + [str(source_link), "--destination", str(self.root / "other")],
                                capture_output=True, text=True)
        self.assertNotEqual(denied.returncode, 0)

    def test_source_path_retarget_to_symlink_fails_before_ack(self):
        self.source.rename(self.root / "original.db")
        other = self.root / "other.db"
        load_current_opencode(other)
        self.source.symlink_to(other)
        with self.worker() as worker:
            with self.assertRaises(ProtocolError):
                worker.poll_once()
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0], "")

    def test_continuous_append_does_not_starve_parts(self):
        # Force more than one bounded scan window, then append between polls.
        with sqlite3.connect(self.source) as db:
            self.assertEqual(db.execute("PRAGMA journal_mode=WAL").fetchone()[0], "wal")
            db.executemany("INSERT INTO session_message VALUES(?,?,?,?,?,?,?)",
                [(f"msg_extra_{n}", "ses_schema_fixture", "assistant", 100+n,
                  1794650400100+n, 1794650400100+n, json.dumps({"content":[]}))
                 for n in range(300)])
            db.commit()
            sidecars = {p.name for p in self.root.glob("opencode.db-*")}
            self.assertEqual(sidecars, {"opencode.db-wal", "opencode.db-shm"})
            with self.worker() as worker:
                first = worker.poll_once()
                self.assertTrue(first.records)
                self.assertEqual({p.name for p in self.root.glob("opencode.db-*")}, sidecars)
                db.execute("INSERT INTO session_message VALUES(?,?,?,?,?,?,?)",
                           ("msg_late", "ses_schema_fixture", "assistant", 500,
                            1794650400500, 1794650400500, "{}"))
                db.commit()
                for _ in range(12):
                    worker.poll_once()
                    self.projected()
                    tools = self.store.activity_page(domain="tool")["activity"]
                    if len(tools) == 2:
                        break
                self.assertEqual(len(tools), 2)


if __name__ == "__main__":
    unittest.main()
