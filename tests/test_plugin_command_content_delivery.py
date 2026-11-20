"""The local requested-command route delivers only a current exact Pi Activity proof."""
import json
import os
import shutil
import threading
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

from observatory.plugin_admission import commit_batch, set_source_capture
from observatory.plugin_pi_command_read import Available
from observatory.plugin_projection import project_source
from observatory.plugin_supervisor import PluginSupervisor
from observatory.server import BoundedHTTPServer, handler_for
from tests import test_plugin_pi_command


class PiCommandHTTPDeliveryTests(unittest.TestCase):
    def setUp(self):
        fixture = test_plugin_pi_command.PiCommandAdmissionTests("runTest")
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture = fixture
        self.store = fixture.store
        supervisor = object.__new__(PluginSupervisor)
        supervisor.store = self.store
        supervisor.sources = (fixture.source,)
        supervisor.stop = threading.Event()
        # The UI integration also polls /api/health, which snapshots worker
        # state. Keep this synthetic supervisor complete for that read path.
        supervisor.lock = threading.Lock()
        supervisor.statuses = {fixture.source.source_id: {
            "source_id": fixture.source.source_id,
            "state": "configured",
            "last_success_ns": 0,
            "last_failure_ns": 0,
            "last_failure_code": "",
        }}
        self.store.plugin_supervisor = supervisor
        batch, ref = fixture.bundle()
        commit_batch(self.store, fixture.lease, "", batch,
                     command_refs=(ref,), command_grant=fixture.grant)
        project_source(self.store, fixture.source.source_id)
        with self.store._read() as db:
            self.event_id = db.execute(
                "SELECT event_key FROM activity_index_v1 WHERE kind='plugin'").fetchone()[0]
        self.handler = handler_for(self.store, "secret", quiet=True)
        self.addCleanup(self.handler.response_cache.close)
        self.server = BoundedHTTPServer(("127.0.0.1", 0), self.handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)

    def path(self, event_id=None):
        return "/api/plugin-command-content?" + urlencode({
            "event_id": self.event_id if event_id is None else event_id})

    def get(self, path=None, *, token="secret", host=None, server=None):
        server = self.server if server is None else server
        headers = {} if token is None else {"Authorization": "Bearer " + token}
        if host is not None:
            headers["Host"] = host
        request = Request(f"http://127.0.0.1:{server.server_port}" +
                          (self.path() if path is None else path), headers=headers)
        try:
            with urlopen(request, timeout=5) as response:
                return response.status, json.load(response), response.headers
        except HTTPError as exc:
            return exc.code, json.loads(exc.read()), exc.headers

    def test_exact_redacted_requested_text_and_no_cache(self):
        self.assertEqual(self.get(token=None)[0], 401)
        self.assertEqual(self.get(token="wrong")[0], 401)
        from observatory import plugin_pi_command_read as reader
        with patch.object(reader, "read_exact_pi_requested_command",
                          wraps=reader.read_exact_pi_requested_command) as read:
            first_status, first, first_headers = self.get()
            second_status, second, second_headers = self.get()
        self.assertEqual((first_status, second_status), (200, 200))
        self.assertEqual(read.call_count, 4)  # registry before/after binding + pre-header gate
        self.assertEqual(first, second)
        self.assertEqual(first["event_id"], self.event_id)
        self.assertEqual(first["redacted_text"], "password=[REDACTED] npm test")
        self.assertEqual(first["revision"], [self.fixture.lease.epoch,
                                              self.fixture.lease.policy_epoch, 1])
        self.assertFalse(first["clipped"])
        self.assertLessEqual(len(first["redacted_text"].encode("utf-8")), 4096)
        self.assertEqual(first_headers["Cache-Control"], "no-store")
        self.assertEqual(second_headers["Cache-Control"], "no-store")
        self.assertEqual(self.handler.response_cache.entries, {})
        self.assertNotIn("highly-private", json.dumps(first))
        self.assertNotIn("private assistant", json.dumps(first))
        self.assertNotIn(str(self.fixture.sources), json.dumps(first))

    def test_exact_id_host_and_loopback_even_without_authentication(self):
        for path in ("/api/plugin-command-content",
                     self.path() + "&event_id=duplicate",
                     self.path() + "&extra=1",
                     self.path("invalid"),
                     self.path("plugin-event:" + "A" * 64)):
            with self.subTest(path=path):
                self.assertEqual(self.get(path)[0], 400)
        self.assertEqual(self.get(self.path("plugin-event:" + "a" * 64))[0], 404)
        self.assertEqual(self.get(host="example.com")[0], 403)
        unauth_handler = handler_for(self.store, None, quiet=True)
        self.addCleanup(unauth_handler.response_cache.close)
        unauth_server = BoundedHTTPServer(("127.0.0.1", 0), unauth_handler)
        thread = threading.Thread(target=unauth_server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(unauth_server.server_close)
        self.addCleanup(unauth_server.shutdown)
        self.assertEqual(self.get(token=None, server=unauth_server)[0], 200)
        # The first loopback call validates Host; the next two validate bind
        # address and client address. Neither may be bypassed by token=None.
        with patch("observatory.receiver_state.loopback_host",
                   side_effect=[True, False]):
            self.assertEqual(self.get(token=None, server=unauth_server)[0], 403)
        with patch("observatory.receiver_state.loopback_host",
                   side_effect=[True, True, False]):
            self.assertEqual(self.get(token=None, server=unauth_server)[0], 403)

    def test_revoked_or_detached_source_returns_no_command(self):
        set_source_capture(self.store, source_id=self.fixture.source.source_id,
                           enabled=False)
        status, body, _ = self.get()
        self.assertEqual(status, 404)
        self.assertNotIn("npm test", json.dumps(body))
        self.store.plugin_supervisor = None
        self.assertEqual(self.get()[0], 404)

    def test_unavailable_activity_revision_returns_service_unavailable(self):
        with patch.object(self.store, "_source_file_identity",
                          side_effect=OSError("synthetic unavailable revision")):
            status, body, _ = self.get()
        self.assertEqual(status, 503)
        self.assertNotIn("npm test", json.dumps(body))

    def test_revocation_between_read_and_headers_withholds_command(self):
        from observatory import plugin_pi_command_read as reader
        original = reader.read_exact_pi_requested_command
        calls = 0

        def revoke(store, event_id):
            nonlocal calls
            result = original(store, event_id)
            calls += 1
            if calls == 1 and isinstance(result, Available):
                set_source_capture(store, source_id=self.fixture.source.source_id,
                                   enabled=False)
            return result

        with patch.object(reader, "read_exact_pi_requested_command", side_effect=revoke):
            status, body, _ = self.get()
        self.assertEqual(status, 409)
        self.assertEqual(calls, 1)  # registry post-read binding catches the revocation
        self.assertNotIn("npm test", json.dumps(body))

    def test_source_rewrite_between_read_and_headers_withholds_command(self):
        from observatory import plugin_pi_command_read as reader
        original = reader.read_exact_pi_requested_command
        calls = 0
        inode = self.fixture.file.stat().st_ino

        def rewrite(store, event_id):
            nonlocal calls
            result = original(store, event_id)
            calls += 1
            if calls == 1 and isinstance(result, Available):
                with self.fixture.file.open("r+b") as stream:
                    stream.seek(len(self.fixture.header + self.fixture.user))
                    stream.write(self.fixture.row.replace(b"npm test", b"npm lint"))
                self.assertEqual(self.fixture.file.stat().st_ino, inode)
            return result

        with patch.object(reader, "read_exact_pi_requested_command", side_effect=rewrite):
            status, body, _ = self.get()
        self.assertEqual(status, 409)
        self.assertEqual(calls, 2)  # provider proof and the pre-header capability guard
        self.assertNotIn("npm test", json.dumps(body))

    def test_store_replacement_between_read_and_headers_withholds_command(self):
        from observatory import plugin_pi_command_read as reader
        original = reader.read_exact_pi_requested_command
        calls = 0

        def replace_store(store, event_id):
            nonlocal calls
            result = original(store, event_id)
            calls += 1
            if calls == 1 and isinstance(result, Available):
                replacement = self.fixture.root / "replacement.db"
                shutil.copy2(store.path, replacement)
                os.replace(replacement, store.path)
            return result

        with patch.object(reader, "read_exact_pi_requested_command",
                          side_effect=replace_store):
            status, body, _ = self.get()
        self.assertEqual(status, 409)
        self.assertEqual(calls, 1)  # inode check precedes the second read
        self.assertNotIn("npm test", json.dumps(body))

    def test_store_replacement_at_header_boundary_withholds_command(self):
        original = self.handler._json_bytes
        replaced = False

        def replace_before_headers(handler, code, data, **options):
            nonlocal replaced
            if code == 200 and not replaced:
                replaced = True
                replacement = self.fixture.root / "replacement-at-headers.db"
                shutil.copy2(self.store.path, replacement)
                os.replace(replacement, self.store.path)
            return original(handler, code, data, **options)

        with patch.object(self.handler, "_json_bytes", replace_before_headers):
            status, body, _ = self.get()
        self.assertTrue(replaced)
        self.assertEqual(status, 409)
        self.assertNotIn("npm test", json.dumps(body))

    def test_receiver_change_at_header_boundary_withholds_command(self):
        original = self.handler._json_bytes
        changed = False

        def change_receiver(handler, code, data, **options):
            nonlocal changed
            if code == 200 and not changed:
                changed = True
                with self.store._connect() as db:
                    db.execute("UPDATE schema_meta SET value='different-receiver' "
                               "WHERE key='receiver_id'")
            return original(handler, code, data, **options)

        with patch.object(self.handler, "_json_bytes", change_receiver):
            status, body, _ = self.get()
        self.assertTrue(changed)
        self.assertEqual(status, 409)
        self.assertNotIn("npm test", json.dumps(body))


if __name__ == "__main__":
    unittest.main()
