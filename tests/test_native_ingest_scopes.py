"""Trusted scope declarations must fence native capture across Store upgrades."""
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
import json
import sqlite3
import tempfile
import unittest

from observatory.core import Store, normalize_native
from observatory.native_ingest_scopes import NativeIngestScope, NativeIngestScopes


SOURCE = "a" * 32
TRACE = "b" * 32
SPAN = "c" * 16


def span(scope="observatory.local_agents"):
    return normalize_native({
        "trace_id": TRACE, "span_id": SPAN, "name": "agent.run",
        "start_unix_nano": 10, "end_unix_nano": 20, "status": "OK",
        "scope_name": scope, "attributes": {"session.id": "session-one"},
        "resource_attributes": {"observatory.source_id": SOURCE},
        "events": []}, "fixture")


def log(scope="observatory.opencode"):
    return {
        "trace_id": TRACE, "span_id": SPAN, "time_unix_nano": 21,
        "service": "fixture", "severity_number": 9, "severity_text": "INFO",
        "event_name": "agent.activity", "body": "", "attributes": {"session.id": "session-one"},
        "resource_attributes": {"observatory.source_id": SOURCE},
        "scope_name": scope}


def owners(store):
    with store._connect() as db:
        return {(row[0], row[1], row[2]) for row in db.execute(
            "SELECT kind,owner_key,source_id FROM native_capture_owner_v1")}


class NativeIngestScopeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / "store.db"

    def test_bundled_registry_rejects_duplicates_and_sql_syntax(self):
        self.assertEqual(NativeIngestScopes.bundled().names,
                         ("observatory.local_agents", "observatory.opencode"))
        with self.assertRaisesRegex(ValueError, "invalid native ingest scope"):
            NativeIngestScope("scope'); DELETE FROM logs; --")
        duplicate = SimpleNamespace(native_ingest_scopes=lambda: (
            NativeIngestScope("observatory.local_agents"),
            NativeIngestScope("observatory.local_agents")))
        with patch("observatory.native_sources.bundled_adapter_modules",
                   return_value=(duplicate,)):
            with self.assertRaisesRegex(ValueError, "registry"):
                Store(self.path)
        self.assertFalse(self.path.exists())

    def test_legacy_upgrade_keeps_existing_claims_and_raw_writes_revoke(self):
        store = Store(self.path)
        self.addCleanup(store.close)
        store.ingest([span()], _trusted_native=True)
        store.ingest_logs([log()], _trusted_native=True)
        original = owners(store)
        self.assertEqual({row[0] for row in original}, {"span", "log"})
        # Earlier Stores had the same trigger set but no registry marker.
        with store._connect() as db:
            db.execute("DELETE FROM schema_meta WHERE key='native_capture_scopes_v1'")
        store.close()
        reopened = Store(self.path)
        self.addCleanup(reopened.close)
        self.assertEqual(owners(reopened), original)
        reopened.close()
        with sqlite3.connect(self.path) as db:
            before_schema = db.execute("PRAGMA schema_version").fetchone()[0]
        again = Store(self.path)
        self.addCleanup(again.close)
        self.assertEqual(owners(again), original)
        with sqlite3.connect(self.path) as db:
            self.assertEqual(db.execute("PRAGMA schema_version").fetchone()[0],
                             before_schema)
        with again._connect() as db:
            db.execute("UPDATE spans SET name='changed' WHERE trace_id=? AND span_id=?",
                       (TRACE, SPAN))
            db.execute("UPDATE logs SET event_name='changed' WHERE scope_name='observatory.opencode'")
        self.assertEqual(owners(again), set())

    def test_new_scope_clears_old_claims_and_fences_new_raw_updates(self):
        store = Store(self.path)
        self.addCleanup(store.close)
        store.ingest([span()], _trusted_native=True)
        self.assertEqual(len(owners(store)), 1)
        contributed = SimpleNamespace(native_ingest_scopes=lambda: (
            NativeIngestScope("observatory.local_agents"),
            NativeIngestScope("observatory.opencode"),
            NativeIngestScope("observatory.synthetic")))
        with patch("observatory.native_sources.bundled_adapter_modules",
                   return_value=(contributed,)):
            upgraded = Store(self.path)
            self.addCleanup(upgraded.close)
            self.assertEqual(owners(upgraded), set())
            # The older Store must not restore an owner claim after a newer
            # process has installed a different revocation trigger set.
            with self.assertRaisesRegex(ValueError, "registry changed"):
                store.ingest([span()], _trusted_native=True)
            with self.assertRaisesRegex(ValueError, "registry changed"):
                store.ingest_logs([log()], _trusted_native=True)
            with self.assertRaisesRegex(ValueError, "registry changed"):
                store.activity_page(service="fixture", session_id="session-one",
                                    agent_id="actor-one", include_reviews=False,
                                    _capture_source_ids=(SOURCE,))
            upgraded.ingest([span("observatory.synthetic")], _trusted_native=True)
            owned = owners(upgraded)
            self.assertEqual(len(owned), 1)
            self.assertEqual(upgraded._native_ingest_scopes.names[-1], "observatory.synthetic")
            with upgraded._connect() as db:
                db.execute("UPDATE spans SET name='raw replacement' WHERE trace_id=? AND span_id=?",
                           (TRACE, SPAN))
            self.assertEqual(owners(upgraded), set())
            upgraded.ingest_logs([log("observatory.synthetic")], _trusted_native=True)
            self.assertEqual({row[0] for row in owners(upgraded)}, {"log"})
            with upgraded._connect() as db:
                db.execute("DELETE FROM logs WHERE scope_name='observatory.synthetic'")
            self.assertEqual(owners(upgraded), set())
            upgraded.close()
        restored = Store(self.path)
        self.addCleanup(restored.close)
        with restored._connect() as db:
            recorded = db.execute(
                "SELECT value FROM schema_meta WHERE key='native_capture_scopes_v1'").fetchone()[0]
        self.assertEqual(tuple(json.loads(recorded)), NativeIngestScopes.bundled().names)


if __name__ == "__main__":
    unittest.main()
