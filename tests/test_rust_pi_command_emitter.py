"""Installed opt-in Pi requested-command references through the real receiver."""
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
from observatory.plugin_projection import project_source
from observatory.plugin_wire import CommandPollBatch, ProtocolError, validate_command_batch


INSTALLER = Path(__file__).resolve().parents[1] / "examples/rust_pi_harness_plugin/install.py"


def row(value):
    return (json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\n").encode()


class RustPiCommandEmitterTests(unittest.TestCase):
    def setUp(self):
        self.binary = os.environ.get("AGENTURM_RUST_PI_BIN")
        if not self.binary:
            self.skipTest("installed Rust Pi binary required")
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
                             {"type": "text", "text": "Please inspect"}]}})
        self.assistant = row({"type": "message", "id": "assistant-1",
                              "timestamp": "2026-10-24T12:00:01Z",
                              "message": {"role": "assistant", "stopReason": "toolUse",
                                          "content": [{"type": "toolCall", "id": "call-1",
                                                       "name": "bash", "arguments": {
                                                           "command": "password=highly-private"}}]}})
        self.file.write_bytes(self.header + self.user + self.assistant)
        self.store = Store(self.root / "store.db")
        self.addCleanup(self.store.close)

    def install(self, *flags):
        target = self.root / ("install-" + str(len(list(self.root.glob("install-*")))))
        command = [sys.executable, str(INSTALLER), "--binary", self.binary,
                   "--destination", str(target), "--source", str(self.sources),
                   "--directory", *flags]
        result = subprocess.run(command, capture_output=True, text=True)
        if result.returncode:
            return result, None
        return result, read_plugin_config(result.stdout.strip())[0]

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
                             command_source=(source if source.capture_mode ==
                                             "pi-review-text-command-v1" else None),
                             **options)

    def command_rows(self):
        with self.store._connect() as db:
            return [tuple(row) for row in db.execute(
                "SELECT redacted_command FROM plugin_command_content_v1")]

    def test_exact_command_reference_is_admitted_without_text_on_wire(self):
        result, source = self.install("--command-refs")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(source.capture_mode, "pi-review-text-command-v1")
        self.assertEqual(source.source_id, "rust-pi-command-source")
        self.assertIn("pi-command-ref/1", source.runtime_features)
        with self.worker(source) as worker:
            raw_batches = []
            selected = worker.process._batch_validator
            def inspect(message, source_id):
                raw_batches.append(message)
                return selected(message, source_id)
            worker.process._batch_validator = inspect
            batch = worker.poll_once()
            self.assertIsInstance(batch, CommandPollBatch)
            self.assertEqual(len(batch.command_refs), 1)
            ref = batch.command_refs[0]
            self.assertEqual(ref.relative_member_path, "projekt/sitzung-ä.jsonl")
            self.assertEqual(ref.row_offset, len(self.header) + len(self.user))
            self.assertEqual(ref.row_length, len(self.assistant))
            self.assertEqual(ref.row_sha256, hashlib.sha256(self.assistant).hexdigest())
            self.assertEqual(ref.native_session_id, "pi-one")
            self.assertEqual(ref.native_turn_id, "user-1")
            self.assertEqual(ref.tool_call_id, "call-1")
            self.assertEqual(ref.assistant_row_id, "assistant-1")
            self.assertEqual(ref.block_index, 0)
            self.assertNotIn("highly-private", repr(batch))
            self.assertEqual(worker.poll_once().command_refs, ())
            self.assertEqual(len(raw_batches), 2)
            self.assertTrue(all("command_refs" in raw and "content_refs" not in raw
                                for raw in raw_batches))
            self.assertNotIn("highly-private", json.dumps(raw_batches))
        rows = self.command_rows()
        self.assertEqual(len(rows), 1)
        self.assertIn("password=[REDACTED]", rows[0][0])
        self.assertNotIn("highly-private", rows[0][0])
        with self.store._connect() as db:
            evidence = "".join(row[0] for row in db.execute(
                "SELECT payload_json FROM plugin_evidence_v1"))
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_content_v1").fetchone()[0], 0)
        self.assertNotIn("highly-private", evidence)
        project_source(self.store, source.source_id)
        activity = self.store.activity_page()["activity"]
        from observatory.terminal_ui import copy_event_text
        self.assertNotIn("highly-private", json.dumps(activity))
        self.assertNotIn("highly-private", "\n".join(copy_event_text(item)
                                                   for item in activity))
        with self.worker(source) as worker:
            self.assertEqual(worker.poll_once().records, ())
        self.assertEqual(self.command_rows(), rows)
        set_source_capture(self.store, source_id=source.source_id, enabled=False)
        self.assertEqual(self.command_rows(), [])

    def test_root_gap_has_only_empty_command_envelope_and_revokes_row(self):
        _, source = self.install("--command-refs")
        with self.worker(source) as worker:
            self.assertEqual(len(worker.poll_once().command_refs), 1)
            self.assertEqual(len(self.command_rows()), 1)
            raw_batches = []
            selected = worker.process._batch_validator
            def inspect(message, source_id):
                raw_batches.append(message)
                return selected(message, source_id)
            worker.process._batch_validator = inspect
            self.sources.rename(self.root / "sessions-away")
            gap = worker.poll_once()
            self.assertTrue(gap.root_gap)
            self.assertEqual(gap.command_refs, ())
            self.assertEqual(len(raw_batches), 1)
            self.assertEqual(raw_batches[0]["command_refs"],
                             {"schema_version": 1, "refs": []})
            self.assertNotIn("content_refs", raw_batches[0])
        self.assertEqual(self.command_rows(), [])

    def test_rust_process_host_forwards_command_batch_feature(self):
        _, source = self.install("--command-refs")
        with self.worker(source, rust_host=True) as worker:
            batch = worker.poll_once()
            self.assertIsInstance(batch, CommandPollBatch)
            self.assertEqual(len(batch.command_refs), 1)
        self.assertEqual(len(self.command_rows()), 1)

    def test_non_bash_call_and_tool_result_cannot_nominate_command(self):
        _, source = self.install("--command-refs")
        self.file.write_bytes(self.header + self.user + self.assistant.replace(
            b'"name":"bash"', b'"name":"read"'))
        with self.worker(source) as worker:
            batch = worker.poll_once()
            self.assertEqual(batch.command_refs, ())
            self.assertTrue(any(record["kind"] == "tool" for record in batch.records))
        self.assertEqual(self.command_rows(), [])

    def test_tool_result_text_cannot_nominate_missing_requested_command(self):
        _, source = self.install("--command-refs")
        call = row({"type": "message", "id": "assistant-1",
                    "timestamp": "2026-10-24T12:00:01Z",
                    "message": {"role": "assistant", "content": [
                        {"type": "toolCall", "id": "call-1", "name": "bash",
                         "arguments": {}}]}})
        result = row({"type": "message", "id": "result-1",
                      "timestamp": "2026-10-24T12:00:02Z",
                      "message": {"role": "toolResult", "toolCallId": "call-1",
                                  "toolName": "bash", "isError": False,
                                  "content": "private output"}})
        self.file.write_bytes(self.header + self.user + call + result)
        with self.worker(source) as worker:
            batch = worker.poll_once()
            self.assertEqual(batch.command_refs, ())
            self.assertTrue(any(record["kind"] == "tool"
                                and record["payload"]["result"] == "started"
                                for record in batch.records))
        self.assertEqual(self.command_rows(), [])

    def test_metadata_default_and_invalid_install_modes_stay_closed(self):
        result, source = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(source.capture_mode, "metadata-only")
        with self.worker(source) as worker:
            batch = worker.poll_once()
            self.assertNotIsInstance(batch, CommandPollBatch)
        self.assertEqual(self.command_rows(), [])
        bad = subprocess.run([sys.executable, str(INSTALLER), "--binary", self.binary,
                              "--destination", str(self.root / "no-directory"),
                              "--source", str(self.sources), "--command-refs"],
                             capture_output=True, text=True)
        self.assertNotEqual(bad.returncode, 0)
        both = subprocess.run([sys.executable, str(INSTALLER), "--binary", self.binary,
                               "--destination", str(self.root / "both"),
                               "--source", str(self.sources), "--directory",
                               "--content-refs", "--command-refs"],
                              capture_output=True, text=True)
        self.assertNotEqual(both.returncode, 0)

    def test_command_feature_requires_trusted_source_and_exact_wire_shape(self):
        _, source = self.install("--command-refs")
        with self.assertRaisesRegex(ProtocolError, "trusted configuration"):
            PluginSession(self.store, source.manifest,
                source_id=source.source_id, allowed_grants=source.allowed_grants,
                source_root=source.source_root, required_features=source.runtime_features)
        ordinary = {"type": "batch", "protocol": 1, "source_id": source.source_id,
                    "cursor": "", "records": []}
        with self.assertRaisesRegex(ProtocolError, "envelope is required"):
            validate_command_batch(ordinary, source_id=source.source_id)
        with self.assertRaisesRegex(ProtocolError, "invalid Pi command reference shape"):
            validate_command_batch({**ordinary, "command_refs": {"schema_version": 1,
                                    "refs": [{"command": "private"}]}},
                                   source_id=source.source_id)


if __name__ == "__main__":
    unittest.main()
