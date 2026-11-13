"""Resumable source indexing and explicit event ownership survive native history."""

import json
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch
from observatory.core import Store, normalize_native
from observatory.storage_activity import drain


class SourceIndexTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / "source.db"
        self.store = Store(self.path)
        self.addCleanup(lambda: self.store.close())

    def test_all_source_span_events_survive_bounded_summary_preview_and_deletion(self):
        records = [
            normalize_native(
                dict(
                    trace_id="a" * 32,
                    span_id=f"{i + 1:016x}",
                    name="tool",
                    start_unix_nano=i + 1,
                    end_unix_nano=i + 2,
                    attributes={"tool.name": "fixture", "session.id": "s"},
                    events=[
                        dict(
                            name="agent.activity",
                            time_unix_nano=i + 2,
                            attributes={
                                "agent.activity.domain": "file",
                                "agent.activity.action": "update",
                                "agent.activity.subject": f"file-{i}.py",
                                "agent.activity.result": "success",
                            },
                        )
                    ],
                ),
                "fixture",
            )
            for i in range(300)
        ]
        self.store.ingest(records)
        rows = self.store.activity_page(limit=500)["activity"]
        self.assertEqual(len(rows), 300)
        with self.store._connect() as db:
            db.execute(
                "DELETE FROM spans WHERE trace_id=? AND span_id=?",
                ("a" * 32, records[0]["span_id"]),
            )
        self.assertEqual(len(self.store.activity_page(limit=500)["activity"]), 299)
        self.store.close()
        self.store = Store(self.path)
        self.assertEqual(len(self.store.activity_page(limit=500)["activity"]), 299)

    def test_event_turn_overrides_span_turn_and_unknown_is_not_invented(self):
        events = [
            dict(
                name="agent.activity",
                time_unix_nano=i + 1,
                attributes={
                    "agent.turn.id": turn,
                    "agent.activity.domain": "message",
                    "agent.activity.action": "assistant",
                    "agent.activity.subject": str(i),
                },
            )
            for i, turn in enumerate(("b" * 32, "c" * 32, None))
        ]
        self.store.ingest(
            [
                normalize_native(
                    dict(
                        trace_id="a" * 32,
                        span_id="1" * 16,
                        name="agent.run",
                        start_unix_nano=1,
                        end_unix_nano=4,
                        attributes={"agent.turn.id": "b" * 32, "session.id": "s"},
                        events=events,
                    ),
                    "fixture",
                )
            ]
        )
        self.assertEqual(
            [
                row["subject"]
                for row in self.store.activity_page(turn_id="b" * 32)["activity"]
            ],
            ["0"],
        )
        self.assertEqual(
            [
                row["subject"]
                for row in self.store.activity_page(turn_id="c" * 32)["activity"]
            ],
            ["1"],
        )
        self.assertIsNone(
            next(
                row
                for row in self.store.activity_page()["activity"]
                if row["subject"] == "2"
            )["turn_id"]
        )

    def test_interrupted_source_rebuild_resumes_and_never_reports_ready_early(self):
        with self.store._connect() as db:
            db.executemany(
                "INSERT INTO logs VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                [
                    (
                        f"{i:064x}",
                        None,
                        None,
                        i,
                        "fixture",
                        9,
                        "INFO",
                        "agent.activity",
                        json.dumps("synthetic source"),
                        json.dumps(
                            {
                                "session.id": "s",
                                "agent.activity.domain": "message",
                                "agent.activity.action": "user",
                            }
                        ),
                        "{}",
                        "fixture",
                        None,
                    )
                    for i in range(1200)
                ],
            )
        self.store.close()

        def interrupted(db, limit=200):
            drain(db, limit)
            db.commit()
            raise RuntimeError("synthetic interrupted migration")

        with patch("observatory.storage_activity.drain", side_effect=interrupted):
            self.store = Store(self.path, async_derived_refresh=True)
            deadline = time.monotonic() + 3
            while (
                "error" not in self.store.migration_status
                and time.monotonic() < deadline
            ):
                time.sleep(0.01)
            self.assertFalse(self.store.index_status()["ready"])
            self.assertIn("error", self.store.migration_status)
            self.store.close()
        self.store = Store(self.path)
        self.assertTrue(self.store.index_status()["ready"])
        seen = []
        cursor = None
        while True:
            page = self.store.activity_page(limit=500, cursor=cursor)
            seen.extend(row["id"] for row in page["activity"])
            cursor = page["next_cursor"]
            if not cursor:
                break
        self.assertEqual(len(seen), 1200)
        self.assertEqual(len(set(seen)), 1200)

    def test_pointer_hydration_uses_the_same_snapshot_as_scoped_metadata(self):
        from observatory.storage_activity import hydrate

        for kind in ("log", "span"):
            with self.subTest(kind=kind):
                if kind == "log":
                    self.store.ingest_logs(
                        [
                            dict(
                                trace_id="a" * 32,
                                span_id=None,
                                time_unix_nano=1,
                                service="fixture",
                                severity_number=9,
                                severity_text="INFO",
                                event_name="agent.activity",
                                body="Session A original detail",
                                attributes={
                                    "session.id": "session-A",
                                    "agent.activity.domain": "message",
                                    "agent.activity.action": "user",
                                },
                                resource_attributes={},
                                scope_name="fixture",
                            )
                        ]
                    )
                else:
                    self.store.ingest(
                        [
                            normalize_native(
                                dict(
                                    trace_id="b" * 32,
                                    span_id="2" * 16,
                                    name="tool",
                                    start_unix_nano=1,
                                    end_unix_nano=2,
                                    attributes={"session.id": "session-A"},
                                    events=[
                                        dict(
                                            name="agent.activity",
                                            time_unix_nano=2,
                                            attributes={
                                                "agent.activity.domain": "message",
                                                "agent.activity.action": "assistant",
                                                "agent.activity.detail": "Session A original detail",
                                            },
                                        )
                                    ],
                                ),
                                "fixture",
                            )
                        ]
                    )
                mutated = []

                def concurrent_write(db, rows):
                    self.assertTrue(db.in_transaction)
                    if not mutated:
                        with self.store._connect() as writer:
                            if kind == "log":
                                writer.execute(
                                    "UPDATE logs SET body=?,attributes=?",
                                    (
                                        json.dumps("Private session B detail"),
                                        json.dumps(
                                            {
                                                "session.id": "session-B",
                                                "agent.activity.domain": "message",
                                                "agent.activity.action": "user",
                                            }
                                        ),
                                    ),
                                )
                            else:
                                writer.execute(
                                    "UPDATE spans SET attributes=?,events=? WHERE trace_id=?",
                                    (
                                        json.dumps({"session.id": "session-B"}),
                                        json.dumps(
                                            [
                                                dict(
                                                    name="agent.activity",
                                                    time_unix_nano=2,
                                                    attributes={
                                                        "agent.activity.domain": "message",
                                                        "agent.activity.action": "assistant",
                                                        "agent.activity.detail": "Private session B detail",
                                                    },
                                                )
                                            ]
                                        ),
                                        "b" * 32,
                                    ),
                                )
                        mutated.append(True)
                    return hydrate(db, rows)

                with patch(
                    "observatory.storage_activity.hydrate", side_effect=concurrent_write
                ):
                    response = self.store.activity_page(session_id="session-A")
                self.assertTrue(response["activity"])
                self.assertIn("Session A original detail", json.dumps(response))
                self.assertNotIn("Private session B detail", json.dumps(response))
                self.assertIn(
                    "Private session B detail",
                    json.dumps(self.store.activity_page(session_id="session-B")),
                )
