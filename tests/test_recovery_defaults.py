"""Local safety contracts, using synthetic state and isolated HTTP authorities."""

import concurrent.futures
from email.message import Message
import json
import os
import sqlite3
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import urllib.error
from observatory.core import Store
from observatory.receiver_state import (
    telemetry_path,
    local_token,
    request_authority,
    retention_policy,
    migrate_database,
)
from observatory.local_activity import safe
from observatory.terminal import Api


class LocalDefaultsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = patch.dict(os.environ, {"XDG_STATE_HOME": str(self.root)})
        self.env.start()
        self.addCleanup(self.env.stop)

    def test_concurrent_token_creation_publishes_one_complete_private_value(self):
        with concurrent.futures.ThreadPoolExecutor(max_workers=12) as executor:
            values = list(
                executor.map(lambda _: local_token(14318, create=True), range(60))
            )
        self.assertEqual(len(set(values)), 1)
        self.assertEqual(local_token(14318), values[0])
        self.assertEqual(telemetry_path(), self.root / "agenturm" / "observatory.db")
        self.assertEqual(
            (self.root / "agenturm" / "receiver-14318.token").stat().st_mode & 0o777,
            0o600,
        )

    def test_host_and_origin_are_validated_independently_of_authentication(self):
        for host, origin in [
            ("localhost:4318", None),
            ("127.0.0.1:4318", "http://127.0.0.1:4318"),
            ("[::1]:4318", "http://[::1]:4318"),
        ]:
            headers = Message()
            headers["Host"] = host
            if origin:
                headers["Origin"] = origin
            request_authority(headers)
        for host, origin in [
            ("evil.example", None),
            ("localhost.evil", None),
            ("localhost:4318", "null"),
            ("localhost:4318", "http://evil.example"),
            ("localhost:4318", "http://localhost:4319"),
            ("local host", None),
            ("localhost@evil", None),
        ]:
            headers = Message()
            headers["Host"] = host
            if origin:
                headers["Origin"] = origin
            with self.assertRaises(ValueError, msg=(host, origin)):
                request_authority(headers)
        duplicate = Message()
        duplicate["Host"] = "localhost"
        duplicate["Host"] = "localhost"
        with self.assertRaises(ValueError):
            request_authority(duplicate)

    def test_only_new_managed_stores_enrol_in_retention_and_explicit_policy_persists(
        self,
    ):
        path = self.root / "existing.db"
        store = Store(path)
        self.assertIsNone(retention_policy(store))
        store.close()
        store = Store(path)
        self.assertIsNone(retention_policy(store, managed=True))
        self.assertEqual(retention_policy(store, requested=7), 7)
        store.close()
        store = Store(path)
        self.addCleanup(store.close)
        self.assertEqual(retention_policy(store), 7)
        fresh = Store(self.root / "fresh.db")
        self.addCleanup(fresh.close)
        self.assertEqual(retention_policy(fresh, managed=True), 14)

    def test_consistent_wal_migration_preserves_source_and_refuses_overwrite(self):
        source = self.root / "source.db"
        store = Store(source)
        self.addCleanup(store.close)
        with store._connect() as db:
            db.execute(
                "INSERT OR REPLACE INTO schema_meta VALUES('synthetic','committed')"
            )
        original = store.receiver_id
        destination = self.root / "private" / "copied.db"
        # Keep a real writer open so committed bytes remain in WAL, rather
        # than accidentally testing only an already-checkpointed database.
        writer = sqlite3.connect(source)
        self.addCleanup(writer.close)
        writer.execute(
            "INSERT OR REPLACE INTO schema_meta VALUES('synthetic','committed')"
        )
        writer.commit()
        wal = Path(str(source) + "-wal")
        self.assertTrue(wal.exists())
        self.assertGreater(wal.stat().st_size, 0)
        before = (source.read_bytes(), wal.read_bytes())
        result = migrate_database(source, destination)
        self.assertEqual((source.read_bytes(), wal.read_bytes()), before)
        self.assertTrue(result["source_unchanged"])
        self.assertTrue(source.exists())
        copy = Store(destination)
        self.addCleanup(copy.close)
        self.assertNotEqual(copy.receiver_id, original)
        self.assertEqual(Store(source).receiver_id, original)
        with copy._connect() as db:
            self.assertEqual(
                db.execute(
                    "SELECT value FROM schema_meta WHERE key='synthetic'"
                ).fetchone()[0],
                "committed",
            )
        self.assertEqual(destination.stat().st_mode & 0o777, 0o600)
        with self.assertRaises(FileExistsError):
            migrate_database(source, destination)
        with self.assertRaises(ValueError):
            migrate_database(source, self.root / "identity.db", preserve_identity=True)
        sidecar = self.root / "blocked.db-wal"
        sidecar.write_text("untouched")
        with self.assertRaises(FileExistsError):
            migrate_database(source, self.root / "blocked.db")
        self.assertEqual(sidecar.read_text(), "untouched")

    def test_common_credentials_are_redacted_without_destroying_usage_primitives(self):
        from observatory.findings import _safe_text

        secrets = [
            "AWS_SECRET_ACCESS_KEY=synthetic_key",
            "CLOUDFLARE_API_TOKEN=synthetic_token",
            "DB_PASS=pw",
            "postgres://admin:synthetic_password@host/db",
            "redis://:synthetic_password@host/0",
            "sk_live_" + "x" * 24,
            "mysql -P3306 -psynthetic_password db",
            "mysql -psynthetic_password -P3306 db",
        ]
        for text in secrets:
            sanitized = _safe_text(text)
            self.assertIn("REDACTED", sanitized, text)
            self.assertNotIn("synthetic", sanitized, text)
        original = {
            "input_tokens": 123,
            "llm.input_token_count": 123,
            "passed": True,
            "exit_code": 0,
            "absent": None,
            "API_TOKEN": "synthetic",
            "AWS_SESSION_TOKEN": "synthetic",
            "nested": {"count": 2, "output": "readable"},
        }
        result = json.loads(safe(original))
        for key in (
            "input_tokens",
            "llm.input_token_count",
            "passed",
            "exit_code",
            "absent",
            "nested",
        ):
            self.assertEqual(result[key], original[key])
        self.assertEqual(result["API_TOKEN"], "[REDACTED]")
        self.assertEqual(result["AWS_SESSION_TOKEN"], "[REDACTED]")
        self.assertEqual(_safe_text("pytest -p plugin"), "pytest -p plugin")

    def test_redirects_never_forward_local_tokens(self):
        received = []

        class Destination(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                received.append(self.headers.get("Authorization"))
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b"{}")

        target = ThreadingHTTPServer(("127.0.0.1", 0), Destination)

        class Redirect(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                self.send_response(302)
                self.send_header(
                    "Location", f"http://127.0.0.1:{target.server_port}/api/health"
                )
                self.end_headers()

        source = ThreadingHTTPServer(("127.0.0.1", 0), Redirect)
        for server in (target, source):
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            self.addCleanup(server.server_close)
            self.addCleanup(server.shutdown)
        with self.assertRaises(urllib.error.HTTPError):
            Api(f"http://127.0.0.1:{source.server_port}", "synthetic").get(
                "/api/health"
            )
        self.assertEqual(received, [])

    def test_non_strict_embedded_client_contains_token_configuration_failure(self):
        from observatory.client import Client

        directory = self.root / "agenturm"
        directory.mkdir()
        (directory / "receiver-4318.token").write_text("malformed")
        client = Client()
        self.assertIsInstance(client.last_error, ValueError)
        with patch("observatory.client.receiver_urlopen") as network:
            self.assertFalse(client._post("/api/spans", {}))
            network.assert_not_called()
        with self.assertRaises(ValueError):
            Client(strict=True)
