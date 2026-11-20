"""Installed capability activation with live receiver policy and CLI routing."""
from __future__ import annotations

import sys
import json
import sqlite3
import threading
import time
import unittest
import urllib.error
import urllib.request
import observatory.server as server_module
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from unittest.mock import patch

from examples.prompt_signals_process.install import install
from observatory.agent_hierarchy import canonical_agent_id
from observatory.capability_activation import (LiveCaptureGate,
    ReceiverActivityReader, _exact_capture, activate_capabilities)
from observatory.capability_policy import capture_sources
from observatory.core import Store, normalize_native
from observatory.local_agents import LocalAgents, Session
from observatory.native_ports import NativeEvidenceFacade
from observatory.server import BoundedHTTPServer, handler_for, _capability_revision_watch
from observatory.terminal import Api, Snapshot, run_cockpit
from observatory.ui_contributions import PaneScope


class FakeFeed:
    def __init__(self, scope):
        self._capture_lock = threading.RLock()
        self.stop = threading.Event()
        self.snapshot = Snapshot(receiver_id="receiver-one", updated=time.time(),
            local_status={"capture_content": True, "sessions": [
                {"service": scope.service, "session_id": scope.session_id,
                 "agent_id": scope.agent_id, "capture_content": True}]})


class FakeApi:
    def __init__(self, feed):
        self.feed = feed
        self.allowed = True
        self.disconnected = False
        self.calls = []

    def get(self, path):
        self.calls.append(path)
        if self.disconnected:
            raise OSError("receiver disconnected")
        if path == "/api/health":
            row = dict(self.feed.snapshot.local_status["sessions"][0])
            row["capture_content"] = self.allowed
            return {"receiver_id": "receiver-one", "local_agents":
                    {"capture_content": self.allowed, "sessions": [row]}}
        if path.startswith("/api/activity?"):
            return {"receiver_id": "receiver-one", "activity": [
                {"service": "svc", "session_id": "session-one", "agent_id":
                 self.feed.snapshot.local_status["sessions"][0]["agent_id"],
                 "domain": "message", "action": "user", "time_unix_nano": 1_000_000_000}],
                "next_cursor": None}
        raise AssertionError(path)


class CapabilityCockpitActivationTests(unittest.TestCase):
    def setUp(self):
        temp = TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.config_path = install(self.root / "installed")
        self.scope = PaneScope(canonical_agent_id("SDK", "svc", "owner"),
                               "svc", "session-one")
        self.feed = FakeFeed(self.scope)
        self.api = FakeApi(self.feed)

    def wait_until(self, predicate):
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(0.02)
        self.fail("capability policy did not reach expected state")

    def test_live_health_allows_exact_scope_and_http_activity_is_bounded(self):
        gate = LiveCaptureGate(self.api, self.feed)
        self.addCleanup(gate.close)
        self.assertFalse(gate(self.scope))
        self.wait_until(lambda: gate(self.scope))
        reader = ReceiverActivityReader(self.api, gate)
        page = reader.activity_page(limit=200, service=self.scope.service,
            session_id=self.scope.session_id, agent_id=self.scope.agent_id,
            subtree=False, include_reviews=False)
        self.assertEqual(len(page["activity"]), 1)
        self.assertTrue(any("subtree=0" in call and "include_reviews=0" in call
                            for call in self.api.calls))
        self.assertTrue(any("capture_scope=1" in call for call in self.api.calls))
        self.assertFalse(gate(PaneScope(self.scope.agent_id, "svc", "other-session")))

    def test_receiver_conflict_expires_but_does_not_permanently_deny_pane(self):
        gate = LiveCaptureGate(self.api, self.feed)
        self.addCleanup(gate.close)
        self.assertFalse(gate(self.scope))
        self.wait_until(lambda: gate(self.scope))
        original = self.api.get
        def one_conflict(path):
            if path.startswith('/api/activity?'):
                raise urllib.error.HTTPError(path, 409, 'policy changed', {}, None)
            return original(path)
        with patch.object(self.api, 'get', side_effect=one_conflict):
            with self.assertRaises(urllib.error.HTTPError):
                ReceiverActivityReader(self.api, gate).activity_page(limit=200,
                    service=self.scope.service, session_id=self.scope.session_id,
                    agent_id=self.scope.agent_id, subtree=False, include_reviews=False)
        self.assertFalse(gate._denied)
        self.wait_until(lambda: gate(self.scope))

    def test_process_pane_recovers_after_one_receiver_policy_conflict(self):
        original = self.api.get
        conflicts = [1]
        def one_conflict(path):
            if path.startswith('/api/activity?') and conflicts[0]:
                conflicts[0] -= 1
                raise urllib.error.HTTPError(path, 409, 'policy changed', {}, None)
            return original(path)
        with patch.object(self.api, 'get', side_effect=one_conflict):
            activations, panes = activate_capabilities((self.config_path,), self.api, self.feed)
            try:
                self.wait_until(lambda: panes[0].query(self.scope).state == 'ready')
            finally:
                for host, gate in activations:
                    host.close()
                    gate.close()
        self.assertEqual(conflicts, [0])

    def test_missing_explicit_capture_denies_without_activity_or_child(self):
        self.feed.snapshot.local_status["sessions"][0]["capture_content"] = False
        activations, panes = activate_capabilities((self.config_path,), self.api, self.feed)
        self.addCleanup(lambda: [(host.close(), gate.close()) for host, gate in activations])
        self.assertEqual(panes[0].query(self.scope).state, "unavailable")
        time.sleep(0.15)
        self.assertFalse(any(path.startswith("/api/activity") for path in self.api.calls))
        self.assertIsNone(activations[0][0].process)

    def test_ui_policy_check_never_waits_for_feed_commit(self):
        gate = LiveCaptureGate(self.api, self.feed)
        self.addCleanup(gate.close)
        entered = threading.Event()
        release = threading.Event()

        def hold_feed():
            with self.feed._capture_lock:
                entered.set()
                release.wait(2)

        thread = threading.Thread(target=hold_feed)
        thread.start()
        try:
            self.assertTrue(entered.wait(1))
            start = time.monotonic()
            self.assertFalse(gate(self.scope))
            self.assertLess(time.monotonic() - start, 0.01)
        finally:
            release.set()
            thread.join(timeout=2)

    def test_backend_revocation_clears_warm_model_before_feed_refresh(self):
        activations, panes = activate_capabilities((self.config_path,), self.api, self.feed)
        self.addCleanup(lambda: [(host.close(), gate.close()) for host, gate in activations])
        pane = panes[0]
        self.wait_until(lambda: pane.query(self.scope).state == "ready")
        host, gate = activations[0]
        self.assertIsNotNone(host.model)
        old_feed_stamp = self.feed.snapshot.updated
        self.api.allowed = False
        self.wait_until(lambda: not gate(self.scope))
        self.assertEqual(pane.query(self.scope).state, "unavailable")
        self.assertIsNone(host.model)
        self.assertEqual(self.feed.snapshot.updated, old_feed_stamp)
        self.assertEqual(self.feed.snapshot.local_status["sessions"][0]["capture_content"], True)

    def test_receiver_disconnect_expires_approval_and_withholds_cached_model(self):
        activations, panes = activate_capabilities((self.config_path,), self.api, self.feed)
        self.addCleanup(lambda: [(host.close(), gate.close()) for host, gate in activations])
        pane = panes[0]
        self.wait_until(lambda: pane.query(self.scope).state == "ready")
        host, gate = activations[0]
        self.api.disconnected = True
        self.wait_until(lambda: not gate(self.scope))
        self.assertEqual(pane.query(self.scope).state, "unavailable")
        self.assertIsNone(host.model)

    def test_config_is_explicit_and_cockpit_closes_runtime(self):
        tty = SimpleNamespace(isatty=lambda: True)
        with patch("observatory.terminal._run_terminal_app") as run, \
             patch.object(sys, "stdin", tty), patch.object(sys, "stdout", tty):
            run.return_value = 0
            code = run_cockpit(db_path=str(self.root / "cockpit.db"),
                               capability_config=(self.config_path,))
        self.assertEqual(code, 0)
        app = run.call_args.args[0]
        self.assertEqual([pane.id for pane in app.pane_registry.all()], ["prompt-signals"])
        self.assertFalse(any(thread.name == "agenturm-capability-policy" and thread.is_alive()
                             for thread in threading.enumerate()))
        with self.assertRaisesRegex(Exception, "omit --once"):
            run_cockpit(db_path=str(self.root / "cockpit.db"), once=True,
                        capability_config=(self.config_path,))

    def test_cli_forwards_only_named_capability_config(self):
        from observatory import __main__
        with patch.object(sys, "argv", ["agenturm", "cockpit", "--capability-config",
                                        str(self.config_path)]), \
             patch("observatory.terminal.run_cockpit", return_value=0) as run:
            __main__.main()
        self.assertEqual(run.call_args.kwargs["capability_config"], [self.config_path])

    def test_new_window_forwards_absolute_descriptor(self):
        with patch("observatory.terminal.open_terminal") as launch:
            run_cockpit(db_path=str(self.root / "cockpit.db"), new_window=True,
                        capability_config=(self.config_path,))
        command = launch.call_args.args[0]
        self.assertEqual(command[command.index("--capability-config") + 1],
                         str(self.config_path.resolve()))

    def test_duplicate_config_refused_before_starting_policy_threads(self):
        with self.assertRaisesRegex(ValueError, "duplicate cockpit capability"):
            activate_capabilities((self.config_path, self.config_path), self.api, self.feed)
        self.assertFalse(any(thread.name == "agenturm-capability-policy" and thread.is_alive()
                             for thread in threading.enumerate()))

    def test_reader_display_session_is_bound_to_its_evidence_source(self):
        store = Store(self.root / "reader.db")
        self.addCleanup(store.close)
        collector = LocalAgents(store, source_specs=(), reader_specs=())
        reader = SimpleNamespace(source_id="physical-source",
            status_sessions=lambda: [{"source_id": "display-session", "service": "svc",
                "session_id": self.scope.session_id, "agent_id": self.scope.agent_id,
                "capture_content": True}], diagnostics=lambda: {})
        collector.reader_specs = (SimpleNamespace(id="external-reader", coverage=None,
                                                 label="External reader"),)
        collector.reader_plugins = {"external-reader": reader}
        local = collector.status()
        self.assertEqual(local["sessions"][0]["evidence_source_id"], "physical-source")
        self.assertEqual(capture_sources(local, service="svc",
            session_id=self.scope.session_id, agent_id=self.scope.agent_id),
            ("physical-source",))

    def test_existing_nine_column_connection_writer_advances_capture_epoch(self):
        store = Store(self.root / "legacy-connection.db")
        self.addCleanup(store.close)
        source = "a" * 32
        with store._connect() as db:
            db.execute("INSERT INTO agent_connections VALUES(?,?,?,?,?,?,?,?,?)",
                (source, "svc", "legacy", "[]", 1, None, 1, 1, "running"))
        first = store.capability_source_token((source,))
        with store._connect() as db:
            db.execute("UPDATE agent_connections SET capture_content=0 WHERE source_id=?", (source,))
        self.assertIsNone(store.capability_source_token((source,)))
        with store._connect() as db:
            db.execute("UPDATE agent_connections SET capture_content=1 WHERE source_id=?", (source,))
        self.assertNotEqual(store.capability_source_token((source,)), first)

    def test_revision_watch_sees_commits_without_pinning_snapshot(self):
        store = Store(self.root / "revision-watch.db")
        self.addCleanup(store.close)
        watch, first = _capability_revision_watch(store.path)
        try:
            self.assertFalse(watch.in_transaction)
            source = "a" * 32
            store.register_connection({"source_id": source, "service": "svc",
                "runtime": "test", "integrations": ["test"],
                "capture_content": True, "state": "running"})
            self.assertNotEqual(watch.execute('PRAGMA data_version').fetchone()[0], first)
            self.assertFalse(watch.in_transaction)
            with store._connect() as db:
                result = db.execute('PRAGMA wal_checkpoint(PASSIVE)').fetchone()
            self.assertEqual(result[0], 0)
        finally:
            watch.close()
        with self.assertRaises(sqlite3.ProgrammingError):
            watch.execute('PRAGMA data_version')

    def test_rejected_stale_native_replay_cannot_restore_provenance(self):
        store = Store(self.root / "stale-native.db")
        self.addCleanup(store.close)
        facade = NativeEvidenceFacade(store)
        source = "a" * 32
        def span(offset):
            return normalize_native({"trace_id": "b" * 32, "span_id": "c" * 16,
                "name": "agent.run", "start_unix_nano": 1_000_000_000,
                "end_unix_nano": 1_000_000_100, "status": "OK",
                "scope_name": "observatory.local_agents",
                "attributes": {"agent.id": self.scope.agent_id,
                    "session.id": self.scope.session_id,
                    "observatory.native_file_id": "1:1",
                    "observatory.native_file_offset": offset,
                    "observatory.native_mtime_ns": 100},
                "resource_attributes": {"observatory.source_id": source},
                "events": []}, "svc")
        current = span(20)
        self.assertEqual(facade.ingest([current]), 1)
        with store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM native_capture_owner_v1").fetchone()[0], 1)
            db.execute("UPDATE spans SET status='ERROR' WHERE trace_id=? AND span_id=?",
                       ("b" * 32, "c" * 16))
        self.assertEqual(facade.ingest([span(10)]), 0)
        with store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM native_capture_owner_v1").fetchone()[0], 0)
        self.assertEqual(facade.ingest([current]), 1)
        with store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM native_capture_owner_v1").fetchone()[0], 1)

    def test_real_receiver_health_shape_and_http_exact_scope_over_200_siblings(self):
        store = Store(self.root / "receiver.db")
        collector = LocalAgents(store, source_specs=(), reader_specs=())
        path = self.root / "session-one.jsonl"
        path.write_text('{"type":"session","id":"session-one"}\n')
        native = Session(store, path, "Pi")
        collector.sessions[native.source_id] = native
        collector.runtime_updated = time.monotonic()
        store.local_agent_monitor = collector
        connection = {"source_id": native.source_id, "service": "pi",
                      "runtime": "Pi local sessions", "integrations": ["local-session-log"],
                      "capture_content": True, "state": "running"}
        store.register_connection(connection)
        epoch = store.capability_source_token((native.source_id,))
        store.register_connection(connection)
        self.assertEqual(store.capability_source_token((native.source_id,)), epoch)
        stamp = time.time_ns() - 1_000_000_000

        def span(letter, actor, events):
            attrs = {"agent.id": actor, "agent.runtime": "Pi",
                     "agent.namespace": native.agent_namespace,
                     "session.id": "session-one"}
            return normalize_native(dict(trace_id=letter * 32, span_id=letter * 16,
                name="agent.run", scope_name="observatory.local_agents",
                start_unix_nano=stamp,
                end_unix_nano=stamp + 100_000, status="OK", attributes=attrs,
                resource_attributes={"observatory.source_id": native.source_id},
                events=[dict(name="agent.activity", time_unix_nano=stamp + index * 100,
                    attributes={"agent.activity.domain": "message",
                                "agent.activity.action": "user",
                                "agent.activity.subject": "private text"})
                    for index in events]), "pi")

        native_ingest = NativeEvidenceFacade(store)
        native_ingest.ingest([span("a", "session-one", [0]),
                              span("b", "sibling", range(1, 202))])
        server = BoundedHTTPServer(("127.0.0.1", 0), handler_for(store, "secret", quiet=True))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            api = Api(f"http://127.0.0.1:{server.server_port}", "secret")
            health = api.get("/api/health")
            scope = PaneScope(native.agent_id, "pi", "session-one")
            self.assertTrue(_exact_capture(health["local_agents"], scope))
            feed = FakeFeed(scope)
            feed.snapshot.receiver_id = health["receiver_id"]
            feed.snapshot.local_status = health["local_agents"]
            gate = LiveCaptureGate(api, feed)
            try:
                self.assertFalse(gate(scope))
                self.wait_until(lambda: gate(scope))
                page = ReceiverActivityReader(api, gate).activity_page(limit=200,
                    service=scope.service, session_id=scope.session_id,
                    agent_id=scope.agent_id, subtree=False, include_reviews=False)
                self.assertEqual(len(page["activity"]), 1)
                self.assertEqual(page["activity"][0]["agent_id"], native.agent_id)
                self.assertRegex(page["capture_revision"], r"^[0-9a-f]{64}$")
                replacement = LocalAgents(store, source_specs=(), reader_specs=())
                replacement.sessions[native.source_id] = native
                replacement.runtime_updated = collector.runtime_updated
                store.local_agent_monitor = replacement
                try:
                    changed_monitor = ReceiverActivityReader(api, gate).activity_page(limit=200,
                        service=scope.service, session_id=scope.session_id,
                        agent_id=scope.agent_id, subtree=False, include_reviews=False)
                    self.assertNotEqual(changed_monitor["capture_revision"],
                                        page["capture_revision"])
                finally:
                    store.local_agent_monitor = collector
                scoped_path = ("/api/activity?capture_scope=1&limit=200&service=pi&"
                    f"session_id=session-one&agent_id={native.agent_id}&subtree=0&include_reviews=0")
                watches = []
                original_watch = server_module._capability_revision_watch
                def traced_watch(path):
                    watch, revision = original_watch(path)
                    watches.append(watch)
                    return watch, revision
                with patch.object(server_module, '_capability_revision_watch',
                                  side_effect=traced_watch):
                    self.assertEqual(len(api.get(scoped_path)["activity"]), 1)
                successful_watch = watches.pop()
                def watch_closed(watch):
                    try:
                        watch.execute('PRAGMA data_version')
                    except sqlite3.ProgrammingError:
                        return True
                    return False
                self.wait_until(lambda: watch_closed(successful_watch))
                class FailedWatch:
                    in_transaction = False
                    closed = False
                    def execute(self, _sql):
                        raise sqlite3.OperationalError('synthetic revision failure')
                    def close(self):
                        self.closed = True
                failed_watch = FailedWatch()
                with patch.object(server_module, '_capability_revision_watch',
                                  return_value=(failed_watch, 0)):
                    with self.assertRaises(urllib.error.HTTPError) as unavailable:
                        api.get(scoped_path)
                self.assertEqual(unavailable.exception.code, 409)
                self.wait_until(lambda: failed_watch.closed)
                with self.assertRaises(urllib.error.HTTPError) as widened:
                    api.get(scoped_path.replace('subtree=0', 'subtree=1'))
                self.assertEqual(widened.exception.code, 400)
                store.register_connection({**connection, "capture_content": False})
                with self.assertRaises(urllib.error.HTTPError) as denied:
                    api.get(scoped_path)
                self.assertEqual(denied.exception.code, 403)
                self.assertNotIn('"activity"', denied.exception.read().decode())
                store.register_connection(connection)
                regranted = api.get(scoped_path)
                self.assertEqual(len(regranted["activity"]), 1)
                self.assertNotEqual(regranted["capture_revision"], page["capture_revision"])
                native.capture_details = False
                with self.assertRaises(urllib.error.HTTPError) as denied:
                    api.get(scoped_path)
                self.assertEqual(denied.exception.code, 403)
                native.capture_details = True
                def held_request(change):
                    entered = threading.Event()
                    release = threading.Event()
                    original_read = store.activity_page
                    outcome = {}
                    def held_read(*args, **kwargs):
                        page = original_read(*args, **kwargs)
                        entered.set()
                        release.wait(2)
                        return page
                    def request_while_held():
                        try:
                            outcome["page"] = api.get(scoped_path)
                        except urllib.error.HTTPError as exc:
                            outcome["code"] = exc.code
                            outcome["body"] = exc.read().decode()
                    with patch.object(store, "activity_page", side_effect=held_read):
                        request = threading.Thread(target=request_while_held)
                        request.start()
                        try:
                            self.assertTrue(entered.wait(2))
                            change()
                        finally:
                            release.set()
                            request.join(timeout=3)
                    self.assertFalse(request.is_alive())
                    return outcome

                def revoke_and_regrant():
                    store.register_connection({**connection, "capture_content": False})
                    store.register_connection(connection)
                outcome = held_request(revoke_and_regrant)
                self.assertEqual(outcome.get("code"), 409)
                self.assertNotIn('"activity"', outcome.get("body", ""))
                unscoped = api.get("/api/activity?limit=200&service=pi&session_id=session-one")
                self.assertEqual(len(unscoped["activity"]), 200)
                self.assertNotIn("capture_revision", unscoped)
                self.assertFalse(any(row["agent_id"] == native.agent_id
                                     for row in unscoped["activity"]))

                # An authenticated OTLP/API writer can copy both the source
                # ID and scope name. The receiver must require its host mark.
                spoofed = span("c", "session-one", [203])
                def external_ingest(record):
                    request = urllib.request.Request(
                        f"http://127.0.0.1:{server.server_port}/api/spans",
                        data=json.dumps({"service": "pi", "spans": [record]}).encode(),
                        headers={"Content-Type": "application/json",
                                 "Authorization": "Bearer secret"}, method="POST")
                    with urllib.request.urlopen(request, timeout=2) as response:
                        self.assertEqual(response.status, 200)
                with patch.object(server_module, '_capability_revision_watch',
                                  side_effect=traced_watch):
                    outcome = held_request(lambda: external_ingest(spoofed))
                self.assertEqual(outcome.get("code"), 409)
                self.assertNotIn('"activity"', outcome.get("body", ""))
                conflicted_watch = watches.pop()
                self.wait_until(lambda: watch_closed(conflicted_watch))
                with self.assertRaises(urllib.error.HTTPError) as denied:
                    api.get(scoped_path)
                self.assertEqual(denied.exception.code, 400)
                self.assertNotIn('"activity"', denied.exception.read().decode())
                native_ingest.ingest([spoofed])
                self.assertEqual(len(api.get(scoped_path)["activity"]), 2)
                outcome = held_request(lambda: external_ingest(span("c", "session-one", [204])))
                self.assertEqual(outcome.get("code"), 409)
                self.assertNotIn('"activity"', outcome.get("body", ""))
                with self.assertRaises(urllib.error.HTTPError) as replaced:
                    api.get(scoped_path)
                self.assertEqual(replaced.exception.code, 400)
                native_ingest.ingest([spoofed])
                self.assertEqual(len(api.get(scoped_path)["activity"]), 2)
                log = {"trace_id": "d" * 32, "span_id": "d" * 16,
                    "time_unix_nano": stamp + 20500, "service": "pi",
                    "severity_number": 9, "severity_text": "INFO",
                    "event_name": "agent.activity", "body": "private log",
                    "attributes": {"agent.id": native.agent_id, "session.id": "session-one",
                        "agent.activity.domain": "message", "agent.activity.action": "user"},
                    "resource_attributes": {"observatory.source_id": native.source_id},
                    "scope_name": "observatory.local_agents"}
                native_ingest.ingest_logs([log])
                self.assertEqual(len(api.get(scoped_path)["activity"]), 3)
                store.ingest_logs([log])
                with self.assertRaises(urllib.error.HTTPError) as replaced_log:
                    api.get(scoped_path)
                self.assertEqual(replaced_log.exception.code, 400)
                native_ingest.ingest_logs([log])
                self.assertEqual(len(api.get(scoped_path)["activity"]), 3)
            finally:
                gate.close()
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)
            store.close()


if __name__ == "__main__":
    unittest.main()
