"""A Pi source fixture compared with the strict process-derived boundary."""
import json
import tempfile
import unittest
from collections import Counter
from pathlib import Path

from observatory.core import Store
from observatory.local_agents import Session
from observatory.plugin_admission import activate_source, commit_batch
from observatory.plugin_projection import project_source, sessions
from observatory.plugin_wire import PollBatch


class PiProcessBoundaryTests(unittest.TestCase):
    def test_native_pi_and_process_facts_preserve_observed_turn_and_tool_outcome(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root)
            native = Store(path / "native.db")
            process = Store(path / "process.db")
            self.addCleanup(native.close)
            self.addCleanup(process.close)
            source = path / "pi.jsonl"
            rows = [
                {"type": "session", "id": "pi-session", "cwd": "/private/work"},
                {"type": "message", "id": "user-1", "timestamp": "2026-10-24T12:00:00Z",
                 "message": {"role": "user", "content": [{"type": "text", "text": "private request"}]}},
                {"type": "message", "id": "assistant-1", "timestamp": "2026-10-24T12:00:01Z",
                 "message": {"role": "assistant", "stopReason": "toolUse",
                             "usage": {"input": 8, "output": 3},
                             "content": [{"type": "toolCall", "id": "call-1", "name": "read",
                                          "arguments": {"path": "/private/work"}}]}},
                {"type": "message", "id": "tool-1", "timestamp": "2026-10-24T12:00:02Z",
                 "message": {"role": "toolResult", "toolCallId": "call-1", "toolName": "read",
                             "isError": True, "content": [{"type": "text", "text": "private error"}]}},
                {"type": "message", "id": "assistant-2", "timestamp": "2026-10-24T12:00:03Z",
                 "message": {"role": "assistant", "stopReason": "error", "content": []}},
            ]
            source.write_text("".join(json.dumps(row) + "\n" for row in rows))
            reader = Session(native, source, "Pi", capture_details=False)
            reader.poll()
            self.assertEqual(reader.run_state, "interrupted")
            native_activity = native.activity_page()["activity"]
            self.assertEqual({(row["domain"], row["action"], row["result"])
                              for row in native_activity},
                             {("tool", "result", "error"), ("tool", "call", "started"),
                              ("model", "usage", "reported"),
                              ("message", "user", "observed")})

            lease = activate_source(process, plugin_id="fixture.pi-rust",
                                    source_id="pi-file", generation="a" * 64)
            def fact(event, kind, stamp, payload, *, operation=None):
                identity = {"session_id": "pi-session", "turn_id": "user-1"}
                if operation:
                    identity["operation_id"] = operation
                return {"event_id": event, "revision": 0, "kind": kind,
                        "time_ns": str(stamp), "identity": identity, "payload": payload}
            process_facts = [
                fact("user-1:message", "message", 100, {"role": "user"}),
                fact("user-1:state", "lifecycle", 100, {"state": "running"}),
                fact("assistant-1:message", "message", 101, {"role": "assistant"}),
                fact("assistant-1:usage", "usage", 101,
                     {"input_tokens": 8, "output_tokens": 3}),
                fact("assistant-1:call-1", "tool", 101,
                     {"category": "file", "result": "started"}, operation="call-1"),
                fact("tool-1:result", "tool", 102,
                     {"category": "file", "result": "failed"}, operation="call-1"),
                fact("assistant-2:state", "lifecycle", 103,
                     {"state": "interrupted", "reason_code": "pi_stop_error"}),
            ]
            commit_batch(process, lease, "", PollBatch("pi-file", "one", tuple(process_facts)))
            project_source(process, "pi-file")
            projected = sessions(process)["sessions"]
            self.assertEqual(len(projected), 1)
            self.assertEqual(projected[0]["run_state"], "interrupted")
            self.assertEqual(projected[0]["metadata_usage"],
                             {"input_tokens": 8, "output_tokens": 3})
            self.assertEqual(projected[0]["coverage"], "partial")
            activity = process.activity_page()["activity"]
            self.assertEqual(Counter((row["domain"], row["result"]) for row in activity),
                             Counter({("tool", "failed"): 1, ("tool", "started"): 1,
                                      ("message", "observed"): 2}))
            tool_rows = [row for row in activity if row["domain"] == "tool"]
            self.assertEqual(tool_rows[0]["operation_id"], tool_rows[1]["operation_id"])
            self.assertEqual({row["turn_id"] for row in activity},
                             {activity[0]["turn_id"]})
            self.assertEqual(len(process.activity_page(review=True)["activity"]), 3)
            self.assertNotIn("private", json.dumps(activity))
            with process._connect() as db:
                self.assertNotIn("private", "".join(row[0] for row in db.execute(
                    "SELECT payload_json FROM plugin_evidence_v1")))


if __name__ == "__main__":
    unittest.main()
