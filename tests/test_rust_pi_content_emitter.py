"""Installed opt-in Rust Pi emitter through the real receiver and durable Store."""
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import set_source_capture
from observatory.plugin_config import read_plugin_config
from observatory.plugin_runtime import PluginSession
from observatory.plugin_wire import ContentPollBatch, ProtocolError


INSTALLER = Path(__file__).resolve().parents[1] / "examples/rust_pi_harness_plugin/install.py"


def row(value):
    return (json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\n").encode()


class RustPiContentEmitterTests(unittest.TestCase):
    def setUp(self):
        binary = os.environ.get("AGENTURM_RUST_PI_BIN")
        if not binary:
            self.skipTest("installed Rust Pi binary required")
        self.binary = binary
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.sources = self.root / "sessions"
        (self.sources / "projekt").mkdir(parents=True)
        self.file = self.sources / "projekt" / "sitzung-ä.jsonl"
        self.header = row({"type": "session", "id": "pi-one"})
        self.user = row({"type": "message", "id": "user-1",
                         "timestamp": "2026-10-24T12:00:00Z",
                         "message": {"role": "user", "content": [
                             {"type": "text", "text": "password=highly-private"}]}})
        self.assistant = row({"type": "message", "id": "assistant-1",
                              "timestamp": "2026-10-24T12:00:01Z",
                              "message": {"role": "assistant", "stopReason": "stop", "content": []}})
        self.file.write_bytes(self.header + self.user + self.assistant)
        self.store = Store(self.root / "store.db")
        self.addCleanup(self.store.close)

    def install(self, *, content=True, directory=True):
        target = self.root / ("content" if content else "metadata")
        command = [sys.executable, str(INSTALLER), "--binary", self.binary,
                   "--destination", str(target), "--source", str(self.sources)]
        if directory:
            command.append("--directory")
        if content:
            command.append("--content-refs")
        result = subprocess.run(command, capture_output=True, text=True)
        if not directory:
            self.assertNotEqual(result.returncode, 0)
            return None
        self.assertEqual(result.returncode, 0, result.stderr)
        return read_plugin_config(result.stdout.strip())[0]

    def worker(self, source, *, rust_host=False):
        options = {}
        if rust_host:
            host = os.environ.get("AGENTURM_RUST_PLUGIN_HOST_BIN")
            if not host:
                self.skipTest("installed Rust plugin host required")
            options = {"rust_host_executable": host,
                       "rust_host_sha256": hashlib.sha256(Path(host).read_bytes()).hexdigest()}
        return PluginSession(self.store, source.manifest, source_id=source.source_id,
                             allowed_grants=source.allowed_grants,
                             source_root=source.source_root,
                             required_features=source.runtime_features,
                             content_source=source if source.capture_mode == "pi-review-text-v1" else None,
                             **options)

    def content(self):
        with self.store._connect() as db:
            rows = db.execute("SELECT redacted_text FROM plugin_content_v1").fetchall()
            status = db.execute("SELECT reason,refs_count FROM plugin_content_status_v1").fetchone()
        return [row[0] for row in rows], tuple(status) if status else None

    def test_installed_content_refs_are_exact_and_replay_is_durable(self):
        source = self.install()
        self.assertEqual(source.capture_mode, "pi-review-text-v1")
        self.assertIn("pi-content-ref/1", source.runtime_features)
        with self.worker(source) as worker:
            batch = worker.poll_once()
            self.assertIsInstance(batch, ContentPollBatch)
            self.assertEqual(len(batch.content_refs), 1)
            ref = batch.content_refs[0]
            self.assertEqual(ref["relative_member_path"], "projekt/sitzung-ä.jsonl")
            self.assertEqual(ref["row_offset"], len(self.header))
            self.assertEqual(ref["row_length"], len(self.user))
            self.assertEqual(ref["row_sha256"], hashlib.sha256(self.user).hexdigest())
            self.assertEqual(ref["native_session_id"], "pi-one")
            self.assertEqual(ref["native_turn_id"], "user-1")
            self.assertEqual(ref["event_id"], next(record["event_id"] for record in batch.records
                                                    if record["kind"] == "message"
                                                    and record["payload"]["role"] == "user"))
            rows, status = self.content()
            self.assertEqual((len(rows), status), (1, ("none", 1)))
            self.assertEqual(worker.poll_once().content_refs, ())
        rows, status = self.content()
        self.assertEqual((len(rows), status), (1, ("none", 1)))
        self.assertIn("password=[REDACTED]", rows[0])
        self.assertNotIn("highly-private", rows[0])
        with self.worker(source) as worker:
            self.assertEqual(worker.poll_once().records, ())
        self.assertEqual(self.content(), (rows, status))
        with self.store._connect() as db:
            evidence = "".join(row[0] for row in db.execute(
                "SELECT payload_json FROM plugin_evidence_v1"))
        self.assertNotIn("highly-private", evidence)

    def test_default_directory_stays_metadata_only(self):
        source = self.install(content=False)
        self.assertEqual(source.capture_mode, "metadata-only")
        self.assertNotIn("pi-content-ref/1", source.runtime_features)
        with self.worker(source) as worker:
            batch = worker.poll_once()
            self.assertNotIsInstance(batch, ContentPollBatch)
            self.assertTrue(batch.records)
        self.assertEqual(self.content(), ([], ("not_requested", 0)))

    def test_gap_withholds_refs_and_invalidates_content(self):
        source = self.install()
        with self.worker(source) as worker:
            self.assertEqual(len(worker.poll_once().content_refs), 1)
        self.assertEqual(len(self.content()[0]), 1)
        replacement = self.root / "replacement"
        replacement.write_bytes(self.header + self.user + self.assistant)
        os.replace(replacement, self.file)
        with self.worker(source) as worker:
            gap = worker.poll_once()
            self.assertEqual(gap.content_refs, ())
            self.assertTrue(any(record["kind"] == "diagnostic" and
                                record["payload"]["code"] == "possible_source_gap"
                                for record in gap.records))
        self.assertEqual(self.content()[0], [])

    def test_revocation_erases_installed_content(self):
        source = self.install()
        with self.worker(source) as worker:
            self.assertEqual(len(worker.poll_once().content_refs), 1)
        self.assertEqual(len(self.content()[0]), 1)
        set_source_capture(self.store, source_id=source.source_id, enabled=False)
        self.assertEqual(self.content()[0], [])
        with self.assertRaises(ProtocolError):
            with self.worker(source) as worker:
                worker.poll_once()

    def test_reference_cap_does_not_drop_metadata(self):
        users = []
        for index in range(50):
            users.append(row({"type": "message", "id": f"user-{index}",
                              "timestamp": "2026-10-24T12:00:00Z",
                              "message": {"role": "user", "content": [
                                  {"type": "text", "text": f"private-{index}"}]}}))
        self.file.write_bytes(self.header + b"".join(users))
        source = self.install()
        with self.worker(source) as worker:
            batch = worker.poll_once()
            emitted = [record for record in batch.records if record["kind"] == "message"
                       and record["payload"]["role"] == "user"]
            self.assertEqual(len(emitted), 50)
            self.assertEqual(len(batch.content_refs), 48)
            self.assertEqual(len(self.content()[0]), 48)
            self.assertEqual(worker.poll_once().records, ())

    def test_root_gap_carries_empty_envelope_and_commits(self):
        source = self.install()
        moved = self.root / "moved-sessions"
        with self.worker(source) as worker:
            self.assertEqual(len(worker.poll_once().content_refs), 1)
            self.sources.rename(moved)
            try:
                gap = worker.poll_once()
                self.assertIsInstance(gap, ContentPollBatch)
                self.assertTrue(gap.root_gap)
                self.assertEqual(gap.content_refs, ())
                self.assertTrue(gap.cursor.startswith("d2:"))
                self.assertEqual(self.content()[0], [])
            finally:
                moved.rename(self.sources)

    def test_content_requires_directory_and_rust_host_forwards_feature(self):
        self.install(directory=False)
        source = self.install()
        with self.worker(source, rust_host=True) as worker:
            self.assertEqual(len(worker.poll_once().content_refs), 1)
        self.assertEqual(len(self.content()[0]), 1)


if __name__ == "__main__":
    unittest.main()
