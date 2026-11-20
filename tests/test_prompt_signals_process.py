"""Installed process capability: explicit admission, scoped facts, responsive UI."""
from __future__ import annotations

import hashlib
import json
import os
import signal
import threading
import time
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from examples.prompt_signals_process.install import install
from observatory.agent_hierarchy import canonical_agent_id
from observatory.capability_process import (CapabilityProcess, PromptCapabilityHost,
                                            read_capability_config)
from observatory.capability_facts import PromptSignalFacts
from observatory.core import Store, normalize_native
from observatory.plugin_wire import ProtocolError
from observatory.ui_contributions import PaneScope


class ProcessCapabilityTests(unittest.TestCase):
    def setUp(self):
        temp = TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.config_path = install(self.root / "installed")
        self.config = read_capability_config(self.config_path)
        self.store = Store(self.root / "signals.db")
        self.addCleanup(self.store.close)
        self.owner = canonical_agent_id("SDK", "svc", "owner")
        self.sibling = canonical_agent_id("SDK", "svc", "sibling")
        self.scope = PaneScope(self.owner, "svc", "session-1")
        self.allowed = True

    def seed(self):
        def span(letter, actor, session, events):
            attrs = {"agent.id": actor, "agent.runtime": "SDK",
                     "agent.namespace": "svc", "session.id": session}
            return normalize_native(dict(trace_id=letter * 32, span_id=letter * 16,
                name="agent.run", start_unix_nano=1_000_000_000,
                end_unix_nano=4_000_000_000, status="OK", attributes=attrs,
                events=[dict(name="agent.activity", time_unix_nano=stamp,
                    attributes={"agent.activity.domain": domain,
                                "agent.activity.action": action,
                                "agent.activity.subject": subject,
                                "agent.activity.detail": "PRIVATE_DETAIL"})
                    for stamp, domain, action, subject in events]), "svc")
        self.store.ingest([
            span("a", "owner", "session-1", [
                (1_000_000_000, "message", "user", "PRIVATE_PROMPT"),
                (1_250_000_000, "shell", "execute", "PRIVATE_COMMAND")]),
            span("b", "sibling", "session-1", [
                (1_100_000_000, "message", "user", "SIBLING_PROMPT")]),
            span("c", "owner", "session-2", [
                (1_200_000_000, "shell", "execute", "OTHER_SESSION_COMMAND")])])

    def host(self, timeout=0.2):
        host = PromptCapabilityHost(self.store, self.config,
                                    lambda _scope: self.allowed, timeout=timeout).start()
        self.addCleanup(host.close)
        return host

    def await_model(self, host, expected="ready"):
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            model = host.query(self.scope)
            if model.state == expected and "Loading" not in model.summary:
                return model
            time.sleep(0.02)
        self.fail(f"capability did not reach {expected}: {model}")

    def alter_child(self, before, after):
        artifact = self.root / "installed" / "prompt-signals"
        raw = artifact.read_text().replace(before, after)
        self.assertNotEqual(raw, artifact.read_text())
        artifact.write_text(raw)
        artifact.chmod(0o700)
        manifest = self.root / "installed" / "prompt-signals.toml"
        original = self.config.manifest.artifact_sha256
        manifest.write_text(manifest.read_text().replace(original,
                            hashlib.sha256(raw.encode()).hexdigest()))
        self.config = read_capability_config(self.config_path)

    def test_installed_artifact_exact_scope_and_content_free_pipe(self):
        self.seed()
        sent = []
        original = CapabilityProcess._send

        def record(process, value, *, timeout):
            sent.append(value)
            return original(process, value, timeout=timeout)

        with patch.object(CapabilityProcess, "_send", record):
            host = self.host()
            model = self.await_model(host)
        self.assertEqual([row.cells[1] for row in model.rows], ["1", "1", "250 ms"])
        self.assertIn("2 scoped events", model.summary)
        wire = json.dumps(sent)
        for private in ("PRIVATE", "SIBLING", "OTHER_SESSION", "session-1", "svc", self.owner):
            self.assertNotIn(private, wire)
        self.assertEqual(len([item for item in sent if item["type"] == "facts"]), 1)

    def test_policy_off_and_inexact_scope_do_not_read_or_start_child(self):
        self.seed()
        self.allowed = False
        host = self.host()
        with patch.object(self.store, "activity_page", side_effect=AssertionError("read")), \
             patch.object(CapabilityProcess, "start", side_effect=AssertionError("launch")):
            self.assertEqual(host.query(self.scope).state, "unavailable")
            self.allowed = True
            self.assertEqual(host.query(PaneScope("", "svc", "session-1")).state, "unavailable")
            time.sleep(0.05)

    def test_revocation_clears_cached_output(self):
        self.seed()
        host = self.host()
        self.await_model(host)
        self.allowed = False
        self.assertEqual(host.query(self.scope).state, "unavailable")
        self.assertIsNone(host.model)
        self.allowed = True
        self.assertEqual(host.query(PaneScope(self.owner, "svc", "session-2")).state,
                         "degraded")
        self.assertEqual(host.query(PaneScope("", "svc", "session-1")).state,
                         "unavailable")

    def test_pinned_artifact_and_explicit_grant(self):
        self.seed()
        artifact = self.root / "installed" / "prompt-signals"
        artifact.write_bytes(artifact.read_bytes() + b"\n# changed\n")
        host = self.host()
        self.assertEqual(self.await_model(host, "unavailable").summary,
                         "Capability process unavailable")
        descriptor = json.loads(self.config_path.read_text())
        descriptor["grants"] = []
        self.config_path.write_text(json.dumps(descriptor))
        with self.assertRaises(ValueError):
            read_capability_config(self.config_path)

    def test_revocation_between_activity_read_and_child_send(self):
        self.seed()
        from observatory import capability_process
        entered = threading.Event()
        release = threading.Event()
        sent = []
        actual = capability_process.prompt_signal_facts

        def held(*args, **kwargs):
            result = actual(*args, **kwargs)
            entered.set()
            release.wait(2)
            return result

        def record(process, value, *, timeout):
            sent.append(value)
            return original(process, value, timeout=timeout)

        original = CapabilityProcess._send
        with patch.object(capability_process, "prompt_signal_facts", held), \
             patch.object(CapabilityProcess, "_send", record):
            host = self.host()
            host.query(self.scope)
            self.assertTrue(entered.wait(2))
            self.allowed = False
            self.assertEqual(host.query(self.scope).state, "unavailable")
            release.set()
            time.sleep(0.05)
        self.assertEqual(sent, [])

    def test_slow_child_does_not_block_query_and_is_quarantined(self):
        self.seed()
        self.alter_child("import json\n", "import json\nimport time\n")
        self.alter_child('        send({"type": "model",', '        time.sleep(1.0)\n        send({"type": "model",')
        host = self.host(timeout=0.1)
        start = time.monotonic()
        self.assertEqual(host.query(self.scope).state, "degraded")
        self.assertLess(time.monotonic() - start, 0.05)
        self.assertEqual(self.await_model(host, "unavailable").summary,
                         "Capability process unavailable")

    def test_crash_and_malformed_model_are_unavailable(self):
        self.seed()
        self.alter_child('        send({"type": "model",', '        raise RuntimeError("fixture crash")\n        send({"type": "model",')
        host = self.host()
        self.assertEqual(self.await_model(host, "unavailable").summary,
                         "Capability process unavailable")
        host.close()
        self.alter_child('        raise RuntimeError("fixture crash")\n', '')
        self.alter_child('"Recorded user messages"', '"fabricated result"')
        other = self.host()
        self.assertEqual(self.await_model(other, "unavailable").summary,
                         "Capability process unavailable")

    def test_installer_rejects_symlink_destination(self):
        target = self.root / "outside"
        target.mkdir()
        link = self.root / "link"
        link.symlink_to(target, target_is_directory=True)
        with self.assertRaises(OSError):
            install(link)
        self.assertEqual(list(target.iterdir()), [])

    @unittest.skipUnless(hasattr(os, "killpg"), "POSIX process groups")
    def test_already_reaped_leader_does_not_signal_reused_group_id(self):
        process = CapabilityProcess(self.config).start()
        process.process.kill()
        process.process.wait(timeout=2)
        with patch("observatory.capability_process.os.killpg",
                   side_effect=AssertionError("must not signal released PGID")):
            process.close()

    @unittest.skipUnless(Path("/proc").is_dir() and hasattr(os, "killpg"), "Linux process groups")
    def test_process_group_cleans_helper_on_close_and_timeout(self):
        marker = self.root / "helper.pid"
        injected = ("import json\nimport subprocess\nfrom pathlib import Path\n"
                    f"Path({str(marker)!r}).write_text(str(subprocess.Popen(['sleep', '30']).pid))\n")
        self.alter_child("import json\n", injected)

        def helper_pid():
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline and not marker.exists():
                time.sleep(0.01)
            return int(marker.read_text())

        def assert_gone(pid):
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                path = Path(f"/proc/{pid}/stat")
                if not path.exists() or path.read_text().split()[2] == "Z":
                    return
                time.sleep(0.01)
            self.fail(f"capability helper {pid} survived process close")

        final_signals = []
        actual_killpg = os.killpg

        def checked_killpg(group, sent_signal):
            if sent_signal == signal.SIGKILL:
                # The leader is still waitable when its group is killed.
                self.assertTrue(Path(f"/proc/{group}/stat").exists())
                final_signals.append(group)
            return actual_killpg(group, sent_signal)

        process = CapabilityProcess(self.config).start()
        first = helper_pid()
        self.assertEqual(os.getpgid(first), process.process.pid)
        with patch("observatory.capability_process.os.killpg", checked_killpg):
            process.close()
        assert_gone(first)

        marker.unlink()
        self.alter_child("import json\n", "import json\nimport time\n")
        self.alter_child('        send({"type": "model",',
                         '        time.sleep(1.0)\n        send({"type": "model",')
        process = CapabilityProcess(self.config, timeout=0.1).start()
        second = helper_pid()
        self.assertEqual(os.getpgid(second), process.process.pid)
        with patch("observatory.capability_process.os.killpg", checked_killpg):
            with self.assertRaises(ProtocolError):
                process.query(PromptSignalFacts("ready", 0, 0, 0, None, True))
        process.close()
        assert_gone(second)
        self.assertEqual(len(final_signals), 2)


class ProcessCapabilityShellTests(unittest.IsolatedAsyncioTestCase):
    async def test_generic_pane_at_60_by_18(self):
        from observatory.terminal_ui import ObservatoryApp
        from tests.test_terminal_search import ManualFeed
        from textual.widgets import TabbedContent
        temp = TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        config = read_capability_config(install(root / "installed"))
        store = Store(root / "signals.db")
        self.addCleanup(store.close)
        scope = PaneScope(canonical_agent_id("SDK", "svc", "owner"), "svc", "session-1")
        host = PromptCapabilityHost(store, config, lambda _scope: True).start()
        self.addCleanup(host.close)
        pane = host.contribution()
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            model = host.query(scope)
            if model.state == "ready":
                break
            await __import__("asyncio").sleep(0.02)
        self.assertEqual(model.state, "ready")
        feed = ManualFeed()
        feed.service = scope.service
        feed.session_id = scope.session_id
        feed.scope_agent_id = scope.agent_id
        app = ObservatoryApp(feed, persist_preferences=False, ui_panes=(pane,))
        async with app.run_test(size=(60, 18)) as pilot:
            app.action_page("contrib-prompt-signals")
            await pilot.pause()
            app.feed.service = scope.service
            app.feed.session_id = scope.session_id
            app.feed.scope_agent_id = scope.agent_id
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline and host.query(scope).state != "ready":
                await __import__("asyncio").sleep(0.02)
            self.assertEqual(host.query(scope).state, "ready")
            app.render_contributed_pane("contrib-prompt-signals")
            self.assertEqual(app.query_one("#pages", TabbedContent).active,
                             "contrib-prompt-signals")
            table = app.query_one("#contrib-prompt-signals-table")
            self.assertEqual(table.row_count, 3)
            self.assertEqual(str(table.get_cell_at((0, 0))), "Recorded user messages")


if __name__ == "__main__":
    unittest.main()
