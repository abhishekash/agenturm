"""Installed capability activation with live receiver policy and CLI routing."""
from __future__ import annotations

import sys
import threading
import time
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from unittest.mock import patch

from examples.prompt_signals_process.install import install
from observatory.agent_hierarchy import canonical_agent_id
from observatory.capability_activation import (LiveCaptureGate,
    ReceiverActivityReader, _exact_capture, activate_capabilities)
from observatory.core import Store, normalize_native
from observatory.local_agents import LocalAgents, Session
from observatory.server import BoundedHTTPServer, handler_for
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
        self.assertFalse(gate(PaneScope(self.scope.agent_id, "svc", "other-session")))

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

    def test_real_receiver_health_shape_and_http_exact_scope_over_200_siblings(self):
        store = Store(self.root / "receiver.db")
        collector = LocalAgents(store, source_specs=(), reader_specs=())
        path = self.root / "session-one.jsonl"
        path.write_text('{"type":"session","id":"session-one"}\n')
        native = Session(store, path, "Pi")
        collector.sessions[native.source_id] = native
        collector.runtime_updated = time.monotonic()
        store.local_agent_monitor = collector
        stamp = time.time_ns() - 1_000_000_000

        def span(letter, actor, events):
            attrs = {"agent.id": actor, "agent.runtime": "Pi",
                     "agent.namespace": native.agent_namespace,
                     "session.id": "session-one"}
            return normalize_native(dict(trace_id=letter * 32, span_id=letter * 16,
                name="agent.run", start_unix_nano=stamp,
                end_unix_nano=stamp + 100_000, status="OK", attributes=attrs,
                events=[dict(name="agent.activity", time_unix_nano=stamp + index * 100,
                    attributes={"agent.activity.domain": "message",
                                "agent.activity.action": "user",
                                "agent.activity.subject": "private text"})
                    for index in events]), "pi")

        store.ingest([span("a", "session-one", [0]),
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
                unscoped = api.get("/api/activity?limit=200&service=pi&session_id=session-one")
                self.assertEqual(len(unscoped["activity"]), 200)
                self.assertFalse(any(row["agent_id"] == native.agent_id
                                     for row in unscoped["activity"]))
            finally:
                gate.close()
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)
            store.close()


if __name__ == "__main__":
    unittest.main()
