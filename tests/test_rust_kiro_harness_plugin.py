"""Installed Rust Kiro process reader over headerless Kiro session JSONL."""
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from examples.rust_kiro_harness_plugin.install import install
from observatory.core import Store
from observatory.local_agents import Session, identity
from observatory.plugin_admission import set_source_capture, last_source_diagnostic
from observatory.plugin_config import read_plugin_config
from observatory.plugin_projection import project_source
from observatory.plugin_runtime import PluginSession
from observatory.plugin_wire import ProtocolError


def row(record_id, second, payload):
    return {"id": record_id, "timestamp": f"2026-11-18T12:00:{second:02d}.000Z",
            "payload": payload}


class RustKiroHarnessTests(unittest.TestCase):
    def setUp(self):
        binary = os.environ.get("AGENTURM_RUST_KIRO_BIN")
        if not binary:
            self.skipTest("Rust Kiro binary is built in the Rust CI job")
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.home = self.root / "kiro-home"
        patcher = patch.dict(os.environ, {"KIRO_HOME": str(self.home)})
        patcher.start()
        self.addCleanup(patcher.stop)
        self.session_dir = self.home / "sessions" / "workspace" / "sess_11111111-2222-3333-4444-555555555555"
        self.session_dir.mkdir(parents=True)
        self.source = self.session_dir / "messages.jsonl"
        self.source.write_text("")
        self.binary = Path(binary)
        self.config = install(self.binary, self.root / "installed", self.source)
        self.plugin = read_plugin_config(self.config)[0]
        self.assertEqual(self.plugin.replaces, ())
        self.store = Store(self.root / "plugin.db")
        self.addCleanup(self.store.close)

    def append(self, *rows, path=None):
        with (path or self.source).open("a") as stream:
            for item in rows:
                stream.write(json.dumps(item, separators=(",", ":")) + "\n")

    def worker(self):
        return PluginSession(self.store, self.plugin.manifest,
                             source_id=self.plugin.source_id,
                             allowed_grants=self.plugin.allowed_grants,
                             timeout=self.plugin.timeout,
                             source_root=self.plugin.source_root)

    def projected(self):
        project_source(self.store, self.plugin.source_id)
        return self.store.plugin_sessions()["sessions"]

    def test_actual_kiro_lifecycle_activity_and_unrepresented_credits(self):
        self.append(
            row("user-one", 0, {"type": "user", "content": "private request"}),
            row("start-one", 1, {"type": "turn_start", "turnId": "turn-one"}),
            row("approval-one", 2, {"type": "pending_interaction", "toolCallId": "call-one",
                                    "interactionType": "tool_approval", "question": "private command"}),
            row("call-one", 3, {"type": "tool_call", "toolCallId": "call-one",
                                "toolName": "run_command", "args": {"command": "private command"}}),
            row("resolve-one", 4, {"type": "interaction_resolved", "toolCallId": "call-one",
                                   "selectedOption": "accept"}),
            row("result-one", 5, {"type": "tool_result", "toolCallId": "call-one",
                                  "success": False, "content": "private output"}),
            row("usage-one", 6, {"type": "usage_summary",
                                 "promptTurnSummaries": [{"unit": "credit", "usage": 0.75}]}),
            row("assistant-one", 7, {"type": "assistant", "operationType": "Say",
                                     "content": "private answer"}),
            row("end-one", 8, {"type": "turn_end", "stopReason": "cancelled"}),
        )
        native = Store(self.root / "native.db")
        self.addCleanup(native.close)
        native_reader = Session(native, self.source, "Kiro", capture_details=False)
        native_reader.poll()
        self.assertEqual(native_reader.run_state, "interrupted")
        with native._connect() as db:
            native_attributes = [json.loads(item[0]) for item in db.execute(
                "SELECT attributes FROM spans WHERE service='kiro'")]
        self.assertTrue(any(item.get("kiro.usage.credits") == 0.75
                            for item in native_attributes))
        with self.worker() as process:
            admitted = process.poll_once()
            self.assertGreaterEqual(len(admitted.records), 9)
            self.assertEqual(process.poll_once().records, ())
            sessions = self.projected()
            self.assertEqual(len(sessions), 1)
            self.assertEqual(sessions[0]["native_session_id"], self.session_dir.name)
            self.assertEqual(sessions[0]["run_state"], native_reader.run_state)
            self.assertEqual(sessions[0]["reason_code"], "kiro_cancelled")
            self.assertEqual(sessions[0]["metadata_usage"], {})
            activity = self.store.activity_page()["activity"]
            self.assertEqual(len([item for item in activity if item["domain"] == "message"]), 2)
            tools = [item for item in activity if item["domain"] == "tool"]
            self.assertEqual(len(tools), 2)
            self.assertEqual({item["result"] for item in tools}, {"started", "failed"})
            self.assertEqual(len({item["operation_id"] for item in tools}), 1)
            self.assertEqual(len({item["turn_id"] for item in activity}), 1)
            evidence = json.dumps(activity)
            with self.store._connect() as db:
                evidence += "".join(value[0] for value in db.execute(
                    "SELECT payload_json FROM plugin_evidence_v1"))
            for secret in ("private request", "private command", "private output",
                           "private answer", str(self.home)):
                self.assertNotIn(secret, evidence)
            diagnostic = last_source_diagnostic(self.store, process.lease)
            self.assertEqual(diagnostic["code"], "kiro_credit_usage_unrepresented")
        with self.worker() as restarted:
            self.assertEqual(restarted.cursor, admitted.cursor)
            self.assertEqual(restarted.poll_once().records, ())
            set_source_capture(self.store, source_id=self.plugin.source_id, enabled=False)
            self.assertEqual(self.store.activity_page()["activity"], [])
            with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
                restarted.poll_once()

    def test_queued_user_turn_is_preserved_across_restart(self):
        self.append(row("user-one", 0, {"type": "user", "content": "one"}),
                    row("user-two", 1, {"type": "user", "content": "queued"}),
                    row("end-one", 2, {"type": "turn_end", "stopReason": "end_turn"}))
        with self.worker() as first:
            first.poll_once()
        self.append(row("start-two", 3, {"type": "turn_start", "turnId": "turn-two"}),
                    row("assistant-two", 4, {"type": "assistant", "operationType": "Say",
                                         "content": "second"}),
                    row("end-two", 5, {"type": "turn_end", "stopReason": "end_turn"}))
        with self.worker() as second:
            second.poll_once()
            self.projected()
            activity = self.store.activity_page()["activity"]
            users = sorted((item for item in activity
                            if item["domain"] == "message" and item["action"] == "user"),
                           key=lambda item: item["time_unix_nano"])
            self.assertEqual(len(users), 2)
            self.assertNotEqual(users[0]["turn_id"], users[1]["turn_id"])
            self.assertEqual(len(self.store.activity_page(turn_id=users[1]["turn_id"])["activity"]), 2)

    def test_single_permission_wait_and_matching_resolution(self):
        self.append(row("user-one", 0, {"type": "user"}),
                    row("start-one", 1, {"type": "turn_start"}),
                    row("pending-one", 2, {"type": "pending_interaction",
                                           "toolCallId": "call-one", "interactionType": "tool_approval"}))
        with self.worker() as process:
            process.poll_once()
            self.assertEqual(self.projected()[0]["run_state"], "waiting")
            self.assertEqual(self.projected()[0]["reason_code"], "kiro_interaction_pending")
            self.append(row("resolved-one", 3, {"type": "interaction_resolved",
                                                "toolCallId": "call-one", "selectedOption": "accept"}))
            process.poll_once()
            self.assertEqual(self.projected()[0]["run_state"], "running")
            self.assertEqual(self.projected()[0]["reason_code"], "")

    def test_consent_round_resolution_keeps_newer_request_waiting(self):
        self.append(row("user-one", 0, {"type": "user"}),
                    row("start-one", 1, {"type": "turn_start"}),
                    row("pending-round-one", 2, {"type": "pending_interaction",
                                                 "toolCallId": "call-one", "consentRound": 1}),
                    row("pending-round-two", 3, {"type": "pending_interaction",
                                                 "toolCallId": "call-one", "consentRound": 2}))
        native = Store(self.root / "native-rounds.db")
        self.addCleanup(native.close)
        native_reader = Session(native, self.source, "Kiro", capture_details=False)
        native_reader.poll()
        self.assertEqual(native_reader.run_state, "waiting")
        with self.worker() as first:
            first.poll_once()
            self.assertEqual(self.projected()[0]["run_state"], "waiting")
        self.append(row("resolved-round-one", 4, {"type": "interaction_resolved",
                                                 "toolCallId": "call-one", "consentRound": 1}))
        native_reader.poll()
        self.assertEqual(native_reader.run_state, "waiting")
        with self.worker() as second:
            second.poll_once()
            self.assertEqual(self.projected()[0]["run_state"], native_reader.run_state)
        self.append(row("resolved-round-two", 5, {"type": "interaction_resolved",
                                                 "toolCallId": "call-one", "consentRound": 2}))
        native_reader.poll()
        self.assertEqual(native_reader.run_state, "running")
        with self.worker() as third:
            third.poll_once()
            self.assertEqual(self.projected()[0]["run_state"], native_reader.run_state)

    def test_call_status_overrides_success_and_survives_restart(self):
        self.append(row("user-one", 0, {"type": "user"}),
                    row("start-one", 1, {"type": "turn_start"}),
                    row("denied-call", 2, {"type": "tool_call", "toolCallId": "denied",
                                           "toolName": "run_command", "status": "denied"}))
        with self.worker() as first:
            first.poll_once()
        self.append(row("denied-result", 3, {"type": "tool_result", "toolCallId": "denied",
                                             "success": True}),
                    row("failed-call", 4, {"type": "tool_call", "toolCallId": "failed",
                                           "toolName": "run_command", "status": "failed"}),
                    row("failed-result", 5, {"type": "tool_result", "toolCallId": "failed",
                                             "content": "private failed output"}))
        native = Store(self.root / "native-call-status.db")
        self.addCleanup(native.close)
        Session(native, self.source, "Kiro", capture_details=False).poll()
        with native._connect() as db:
            native_results = [json.loads(item[0])["agent.activity.result"] for item in db.execute(
                "SELECT attributes FROM logs WHERE service='kiro'")
                if json.loads(item[0]).get("tool.name") == "run_command"]
        self.assertEqual(native_results, ["denied", "error"])
        with self.worker() as second:
            second.poll_once()
        self.projected()
        tool_activity = [item for item in self.store.activity_page()["activity"]
                         if item["domain"] == "tool"]
        self.assertEqual([item["result"] for item in sorted(tool_activity,
                           key=lambda item: item["time_unix_nano"])],
                         ["denied", "denied", "failed", "failed"])

    def test_new_turn_with_unresolved_call_fails_before_wrong_attribution(self):
        self.append(row("user-one", 0, {"type": "user"}),
                    row("start-one", 1, {"type": "turn_start"}),
                    row("call-one", 2, {"type": "tool_call", "toolCallId": "call-one",
                                         "toolName": "run_command"}),
                    row("end-one", 3, {"type": "turn_end", "stopReason": "end_turn"}))
        with self.worker() as first:
            admitted = first.poll_once()
        self.append(row("user-two", 4, {"type": "user"}),
                    row("start-two", 5, {"type": "turn_start"}),
                    row("result-one", 6, {"type": "tool_result", "toolCallId": "call-one",
                                           "success": True}))
        native = Store(self.root / "native-late-result.db")
        self.addCleanup(native.close)
        native_reader = Session(native, self.source, "Kiro", capture_details=False)
        native_reader.poll()
        with native._connect() as db:
            native_tools = [(item[0], json.loads(item[1])) for item in db.execute(
                "SELECT trace_id,attributes FROM logs WHERE service='kiro'")
                if json.loads(item[1]).get("tool.name") == "run_command"]
        self.assertEqual(len(native_tools), 1)
        self.assertEqual(native_tools[0][0], identity(native_reader.source_id, "user-one"))
        with self.worker() as second:
            with self.assertRaises(ProtocolError):
                second.poll_once()
            self.assertEqual(second.cursor, admitted.cursor)

    def test_worker_path_identity_and_missing_result_flag_refusal(self):
        worker_dir = self.session_dir / "sub-executions"
        worker_dir.mkdir()
        worker_source = worker_dir / "child-one.jsonl"
        worker_source.write_text("")
        config = install(self.binary, self.root / "worker-installed", worker_source,
                         source_id="rust-kiro-worker")
        plugin = read_plugin_config(config)[0]
        self.append(row("assistant", 0, {"type": "assistant", "operationType": "Say",
                                         "content": "private worker"}), path=worker_source)
        with PluginSession(self.store, plugin.manifest, source_id=plugin.source_id,
                           allowed_grants=plugin.allowed_grants, timeout=plugin.timeout,
                           source_root=plugin.source_root) as worker:
            initial = worker.poll_once()
        project_source(self.store, plugin.source_id)
        sessions = [item for item in self.store.plugin_sessions()["sessions"]
                    if item["plugin_source_id"] == plugin.source_id]
        self.assertEqual(len(sessions), 1)
        self.assertEqual(sessions[0]["native_session_id"], "child-one")
        self.assertEqual(sessions[0]["run_state"], "running")
        self.append(row("bad-result", 1, {"type": "tool_result", "toolCallId": "call-one"}),
                    path=worker_source)
        with PluginSession(self.store, plugin.manifest, source_id=plugin.source_id,
                           allowed_grants=plugin.allowed_grants, timeout=plugin.timeout,
                           source_root=plugin.source_root) as worker:
            with self.assertRaises(ProtocolError):
                worker.poll_once()
            self.assertEqual(worker.cursor, initial.cursor)

    def test_same_path_rotation_and_symlink_retarget_report_gap(self):
        self.append(row("user-one", 0, {"type": "user"}))
        with self.worker() as first:
            initial = first.poll_once()
        replacement = self.root / "replacement.jsonl"
        replacement.write_text(json.dumps(row("user-one", 1, {"type": "user"})) + "\n")
        os.replace(replacement, self.source)
        with self.worker() as second:
            changed = second.poll_once()
            self.assertNotEqual(initial.cursor, changed.cursor)
            self.assertTrue(any(item["payload"].get("code") == "possible_source_gap"
                                for item in changed.records))
            self.assertTrue(any(item["kind"] == "message" for item in changed.records))
        target_a = self.root / "target-a.jsonl"
        os.replace(self.source, target_a)
        self.source.symlink_to(target_a)
        with self.worker() as third:
            linked = third.poll_once()
            self.assertEqual(linked.records, ())
        target_b = self.root / "target-b.jsonl"
        target_b.write_text(json.dumps(row("user-one", 2, {"type": "user"})) + "\n")
        replacement_link = self.root / "next-link"
        replacement_link.symlink_to(target_b)
        os.replace(replacement_link, self.source)
        with self.worker() as fourth:
            retargeted = fourth.poll_once()
            self.assertTrue(any(item["payload"].get("code") == "possible_source_gap"
                                for item in retargeted.records))

    def test_installer_rejects_symlink_destination_and_bad_source_shape(self):
        outside = self.root / "outside"
        outside.mkdir()
        link = self.root / "link"
        link.symlink_to(outside, target_is_directory=True)
        with self.assertRaises(OSError):
            install(self.binary, link, self.source)
        self.assertEqual(list(outside.iterdir()), [])
        with self.assertRaises(ValueError):
            install(self.binary, self.root / "bad", self.root / "other.jsonl")


if __name__ == "__main__":
    unittest.main()
