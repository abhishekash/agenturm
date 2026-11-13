"""Recorded parent relationships join user turns without merging step traces."""

import json
import sqlite3
import threading
import unittest
from tests import test_opencode as fixture
from observatory.opencode import OpenCodeReader
from observatory.local_agents import identity
from observatory.review_cli import collect_review
from observatory.server import BoundedHTTPServer, handler_for
from observatory.terminal import Api


class OpenCodeTurnTests(unittest.TestCase):
    setUp = fixture.OpenCodeReaderTests.setUp

    def test_user_prompt_two_steps_and_second_turn_remain_disjoint_through_http(self):
        with sqlite3.connect(self.database) as db:
            db.execute("DELETE FROM todo")
            original = json.loads(
                db.execute("SELECT data FROM message WHERE id='msg_test'").fetchone()[0]
            )
            stamp = original["time"]["created"]
            original["parentID"] = "user-one"
            original["content"] = [{"type": "text", "text": "First assistant step"}]
            db.execute(
                "UPDATE message SET data=? WHERE id='msg_test'", (json.dumps(original),)
            )
            for mid, offset, data in [
                (
                    "user-one",
                    -1,
                    {
                        "role": "user",
                        "content": [
                            {"type": "text", "text": "Implement the validation fix"}
                        ],
                    },
                ),
                (
                    "step-two",
                    1,
                    {
                        **original,
                        "content": [{"type": "text", "text": "Validation implemented"}],
                        "tokens": {"input": 5, "output": 3},
                    },
                ),
                (
                    "user-two",
                    2,
                    {
                        "role": "user",
                        "content": [{"type": "text", "text": "Unrelated next request"}],
                    },
                ),
                (
                    "step-other",
                    3,
                    {
                        **original,
                        "parentID": "user-two",
                        "tokens": {"input": 100, "output": 99},
                    },
                ),
            ]:
                db.execute(
                    "INSERT INTO message VALUES(?,?,?,?,?)",
                    (mid, "ses_test", stamp + offset, stamp + offset, json.dumps(data)),
                )
            db.execute(
                "UPDATE part SET data=? WHERE id='prt_test'",
                (
                    json.dumps(
                        {
                            "type": "tool",
                            "tool": "bash",
                            "state": {
                                "status": "completed",
                                "input": {"command": "pytest tests -q"},
                                "output": "2 passed",
                                "metadata": {"exit": 0},
                            },
                        }
                    ),
                ),
            )
        reader = OpenCodeReader(self.store, path=self.database)
        reader.poll()
        trace = identity("opencode-trace", "ses_test", "msg_test")
        rollup = self.store.review_turn(trace)
        self.assertEqual(len(rollup["step_trace_ids"]), 2)
        self.assertEqual(rollup["input_tokens"], 17)
        self.assertEqual(rollup["output_tokens"], 10)
        self.assertTrue(rollup["turn_usage_complete"])
        server = BoundedHTTPServer(
            ("127.0.0.1", 0), handler_for(self.store, "synthetic", quiet=True)
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        projection = collect_review(
            Api(f"http://127.0.0.1:{server.server_port}", "synthetic"),
            service="opencode",
            agent_id=rollup["review_owner"]["agent_id"],
            session_id="ses_test",
            trace_id=trace,
            pages=5,
            whole_turn=True,
        )
        self.assertEqual(projection["request"]["text"], "Implement the validation fix")
        self.assertEqual(projection["agent_update"]["text"], "Validation implemented")
        self.assertEqual(projection["checks"][0]["command"], "pytest tests -q")
        self.assertEqual(projection["checks"][0]["exit_code"], 0)
        self.assertEqual(projection["usage"]["values"]["input_tokens"], 17)
        self.assertNotIn("Unrelated next request", json.dumps(projection))
        self.assertEqual(
            self.store.activity_page(trace_id=trace, domain="message", action="user")[
                "activity"
            ],
            [],
        )
        self.assertEqual(reader.poll(), 0)
        self.assertEqual(self.store.review_turn(trace)["input_tokens"], 17)

    def test_missing_foreign_or_contradictory_parents_never_widen_to_session(self):
        with sqlite3.connect(self.database) as db:
            db.execute("DELETE FROM todo")
            original = json.loads(
                db.execute("SELECT data FROM message WHERE id='msg_test'").fetchone()[0]
            )
            stamp = original["time"]["created"]
            original.update(parentID="missing", parent_id="different")
            db.execute(
                "UPDATE message SET data=? WHERE id='msg_test'", (json.dumps(original),)
            )
        reader = OpenCodeReader(self.store, path=self.database)
        reader.poll()
        trace = identity("opencode-trace", "ses_test", "msg_test")
        self.assertIsNone(self.store.review_turn(trace))
        with sqlite3.connect(self.database) as db:
            original.pop("parent_id")
            original["parentID"] = "late-user"
            db.execute(
                "UPDATE message SET data=?,time_updated=time_updated+1 WHERE id='msg_test'",
                (json.dumps(original),),
            )
        reader.poll()
        self.assertIsNone(self.store.review_turn(trace))
        with sqlite3.connect(self.database) as db:
            db.execute(
                "INSERT INTO message VALUES(?,?,?,?,?)",
                (
                    "late-user",
                    "ses_test",
                    stamp - 1,
                    stamp - 1,
                    json.dumps(
                        {
                            "role": "user",
                            "content": [{"type": "text", "text": "Late parent"}],
                        }
                    ),
                ),
            )
        # Parent arrival is membership evidence even if the assistant payload
        # itself did not change since the prior poll.
        reader.poll()
        self.assertIsNotNone(self.store.review_turn(trace))

    def test_progress_does_not_hide_turn_membership_or_turn_evidence(self):
        with sqlite3.connect(self.database) as db:
            original = json.loads(
                db.execute("SELECT data FROM message WHERE id='msg_test'").fetchone()[0]
            )
            stamp = original["time"]["created"]
            original["parentID"] = "user-progress"
            db.execute(
                "UPDATE message SET data=? WHERE id='msg_test'", (json.dumps(original),)
            )
            db.execute(
                "INSERT INTO message VALUES(?,?,?,?,?)",
                (
                    "user-progress",
                    "ses_test",
                    stamp - 1,
                    stamp - 1,
                    json.dumps(
                        {
                            "role": "user",
                            "content": [
                                {
                                    "type": "text",
                                    "text": "A turn with recorded plan progress",
                                }
                            ],
                        }
                    ),
                ),
            )
        reader = OpenCodeReader(self.store, path=self.database)
        reader.poll()
        trace = identity("opencode-trace", "ses_test", "msg_test")
        with self.store._connect() as db:
            db.execute(
                "INSERT INTO progress(trace_id,time_ns,stage,message) VALUES(?,?,?,?)",
                (trace, 1, "plan", "synthetic progress"),
            )
        self.store._refresh_summaries([trace])
        turn = self.store.review_turn(trace)
        self.assertTrue(turn["scope_owner"])
        self.assertFalse(turn["turn_usage_complete"])
        session = self.store.sessions()[0]
        self.assertEqual(session["latest_run"]["turn_id"], turn["turn_id"])
        self.assertEqual(
            session["latest_run"]["turn_usage"]["turn_id"], turn["turn_id"]
        )
        rows = self.store.activity_page(turn_id=turn["turn_id"])["activity"]
        self.assertIn(
            "A turn with recorded plan progress", {row["subject"] for row in rows}
        )

    def test_part_only_streaming_updates_are_bounded_and_idempotent(self):
        from observatory.opencode import MAX_PAYLOAD_BYTES

        with sqlite3.connect(self.database) as db:
            db.execute("DELETE FROM todo")
            db.execute(
                "INSERT INTO part VALUES(?,?,?,?,?)",
                (
                    "text-reply",
                    "ses_test",
                    "msg_test",
                    1,
                    json.dumps({"type": "text", "text": "First partial reply"}),
                ),
            )
        reader = OpenCodeReader(self.store, path=self.database)
        reader.poll()
        trace = identity("opencode-trace", "ses_test", "msg_test")
        self.assertIn(
            "First partial reply",
            {
                row["subject"]
                for row in self.store.activity_page(trace_id=trace)["activity"]
            },
        )
        with sqlite3.connect(self.database) as db:
            db.execute(
                "UPDATE part SET data=? WHERE id='text-reply'",
                (json.dumps({"type": "text", "text": "Final complete reply"}),),
            )
        self.assertGreater(reader.poll(), 0)
        subjects = {
            row["subject"]
            for row in self.store.activity_page(trace_id=trace)["activity"]
        }
        self.assertIn("Final complete reply", subjects)
        self.assertNotIn("First partial reply", subjects)
        self.assertEqual(reader.poll(), 0)
        with sqlite3.connect(self.database) as db:
            db.execute(
                "UPDATE part SET data=? WHERE id='text-reply'",
                (
                    json.dumps(
                        {
                            "type": "text",
                            "text": "OVERSIZED_REPLY" + ("x" * MAX_PAYLOAD_BYTES),
                        }
                    ),
                ),
            )
        reader.poll()
        self.assertTrue(reader.skipped_oversized)
        self.assertNotIn(
            "OVERSIZED_REPLY", json.dumps(self.store.activity_page(trace_id=trace))
        )

    def test_missing_step_usage_is_not_summed_as_zero(self):
        with sqlite3.connect(self.database) as db:
            db.execute("DELETE FROM todo")
            original = json.loads(
                db.execute("SELECT data FROM message WHERE id='msg_test'").fetchone()[0]
            )
            stamp = original["time"]["created"]
            original["parentID"] = "user-partial"
            db.execute(
                "UPDATE message SET data=? WHERE id='msg_test'", (json.dumps(original),)
            )
            db.execute(
                "INSERT INTO message VALUES(?,?,?,?,?)",
                (
                    "user-partial",
                    "ses_test",
                    stamp - 1,
                    stamp - 1,
                    json.dumps(
                        {
                            "role": "user",
                            "content": [{"type": "text", "text": "Partial usage turn"}],
                        }
                    ),
                ),
            )
            other = {**original, "tokens": {}, "cost": None}
            db.execute(
                "INSERT INTO message VALUES(?,?,?,?,?)",
                ("step-partial", "ses_test", stamp + 1, stamp + 1, json.dumps(other)),
            )
        reader = OpenCodeReader(self.store, path=self.database)
        reader.poll()
        turn = self.store.review_turn(
            identity("opencode-trace", "ses_test", "msg_test")
        )
        self.assertTrue(turn["turn_usage_complete"])
        self.assertIsNone(turn["input_tokens"])
        self.assertFalse(turn["input_tokens_reported"])
        self.assertFalse(turn["usage_complete"])
        self.assertFalse(turn["cost_complete"])
