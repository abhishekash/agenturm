"""Durable outbound contract against a fixture sink; no real destination."""
import json
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

from observatory.core import Store
from observatory.outbound_delivery import (ACTION, OutboundProcess, OutboundQueue,
                                           read_destination_config)
from observatory.plugin_manifest import PluginManifest
from observatory.plugin_wire import ProtocolError


PACKAGE = Path(__file__).resolve().parents[1] / "examples" / "fake_outbound_connector"


class SinkState:
    def __init__(self):
        self.lock = threading.Lock()
        self.accepted = {}
        self.posts = 0
        self.lookups = 0
        self.mode = "accept"
        self.lookup_unavailable = False
        self.delay = 0.6


class FakeSink(BaseHTTPRequestHandler):
    state: SinkState

    def body(self, status, value):
        raw = json.dumps(value, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        try:
            self.wfile.write(raw)
        except BrokenPipeError:
            pass  # The host may time out after the sink accepted the request.

    def do_POST(self):
        if self.path != "/deliver":
            self.send_error(404)
            return
        size = int(self.headers.get("Content-Length", "0"))
        if not 0 < size < 4096:
            self.send_error(400)
            return
        payload = json.loads(self.rfile.read(size))
        key = self.headers.get("Idempotency-Key")
        with self.state.lock:
            self.state.posts += 1
            mode = self.state.mode
            delay = self.state.delay
            previous = self.state.accepted.get(key)
            if previous is not None:
                if previous[0] != payload:
                    self.body(409, {})
                    return
                receipt = previous[1]
            elif mode in ("retryable", "terminal", "drop_before"):
                receipt = None
            else:
                receipt = "receipt-" + str(len(self.state.accepted) + 1)
                self.state.accepted[key] = (payload, receipt)
        if mode == "retryable" and previous is None:
            self.body(503, {})
        elif mode == "terminal" and previous is None:
            self.body(422, {})
        elif mode == "drop_before" and previous is None:
            time.sleep(delay)
            self.body(503, {})
        else:
            if mode == "accept_then_wait" and previous is None:
                time.sleep(delay)
            self.body(201 if previous is None else 200, {"receipt": receipt})

    def do_GET(self):
        parsed = urlsplit(self.path)
        if parsed.path != "/status" or set(parse_qs(parsed.query)) != {"key"}:
            self.send_error(404)
            return
        key = parse_qs(parsed.query)["key"][0]
        with self.state.lock:
            self.state.lookups += 1
            accepted = self.state.accepted.get(key)
            unavailable = self.state.lookup_unavailable
        if unavailable:
            self.body(503, {})
            return
        self.body(200, {"status": "accepted", "receipt": accepted[1]}
                  if accepted else {"status": "missing"})

    def log_message(self, _format, *_args):
        pass


class FakeOutboundTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.sink = SinkState()
        handler = type("BoundFakeSink", (FakeSink,), {"state": self.sink})
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.server.daemon_threads = True
        thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(thread.join, 2)
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        self.endpoint = f"http://127.0.0.1:{self.server.server_port}/deliver"
        result = subprocess.run([sys.executable, str(PACKAGE / "install.py"),
                                 "--destination", str(self.root / "installed"),
                                 "--endpoint", self.endpoint],
                                capture_output=True, text=True, check=True)
        self.descriptor_path = Path(result.stdout.strip())
        self.descriptor = read_destination_config(self.descriptor_path)
        self.manifest = self.descriptor.manifest
        self.store = Store(str(self.root / "agenturm.db"))
        self.addCleanup(self.store.close)
        self.queue = OutboundQueue(self.store)
        self.queue.grant("fake-sink", ACTION, self.endpoint, self.manifest,
                         allowed_grants=("loopback-sink-write",))

    def process(self, timeout=2.0):
        return OutboundProcess(self.manifest, destination_id="fake-sink", endpoint=self.endpoint,
                               allowed_grants=("loopback-sink-write",), timeout=timeout)

    def test_explicit_grant_queue_and_durable_idempotent_delivery(self):
        queued = self.queue.enqueue("delivery-one", "fake-sink", ACTION, 7, now_ns=100)
        self.assertEqual((queued["state"], queued["attempts"]), ("queued", 0))
        self.assertEqual(self.sink.posts, 0)
        same = self.queue.enqueue("delivery-one", "fake-sink", ACTION, 7, now_ns=101)
        self.assertEqual(same["idempotency_key"], queued["idempotency_key"])
        with self.assertRaisesRegex(ProtocolError, "identity reused"):
            self.queue.enqueue("delivery-one", "fake-sink", ACTION, 8)
        with self.process() as child:
            sent = self.queue.dispatch_one("fake-sink", child, now_ns=100)
            self.assertEqual((sent["state"], sent["attempts"], sent["receipt"]),
                             ("delivered", 1, "receipt-1"))
            self.assertIsNone(self.queue.dispatch_one("fake-sink", child, now_ns=101))
        self.assertEqual(self.sink.posts, 1)
        self.assertEqual(OutboundQueue(self.store).status("delivery-one")["state"], "delivered")

    def test_revocation_blocks_queued_and_retryable_without_post(self):
        self.queue.enqueue("first", "fake-sink", ACTION, 1, now_ns=100)
        self.queue.revoke("fake-sink")
        with self.process() as child:
            self.assertIsNone(self.queue.dispatch_one("fake-sink", child, now_ns=100))
        self.assertEqual(self.queue.status("first")["state"], "blocked")
        self.assertEqual(self.sink.posts, 0)
        with self.assertRaisesRegex(ProtocolError, "not granted"):
            self.queue.enqueue("after", "fake-sink", ACTION, 2)
        self.queue.grant("fake-sink", ACTION, self.endpoint, self.manifest,
                         allowed_grants=("loopback-sink-write",))
        self.assertEqual(self.queue.status("first")["state"], "blocked")
        with self.sink.lock:
            self.sink.mode = "retryable"
        self.queue.enqueue("second", "fake-sink", ACTION, 3, now_ns=100)
        with self.process() as child:
            self.assertEqual(self.queue.dispatch_one("fake-sink", child, now_ns=100)["state"],
                             "retryable")
        self.queue.revoke("fake-sink")
        self.assertEqual(self.queue.status("second")["state"], "blocked")
        self.assertEqual(self.sink.posts, 1)

    def test_bounded_retry_ends_in_visible_dead_letter(self):
        with self.sink.lock:
            self.sink.mode = "retryable"
        self.queue.enqueue("retry", "fake-sink", ACTION, 4, now_ns=100)
        with self.process() as child:
            for attempt in range(1, 4):
                row = self.queue.dispatch_one("fake-sink", child, now_ns=100 + attempt * 100_000_000_000)
                self.assertEqual(row["attempts"], attempt)
                self.assertEqual(row["state"], "dead_letter" if attempt == 3 else "retryable")
            self.assertIsNone(self.queue.dispatch_one("fake-sink", child, now_ns=999_000_000_000))
        self.assertEqual(self.sink.posts, 3)

    def test_two_dispatchers_claim_one_delivery(self):
        self.queue.enqueue("only-once", "fake-sink", ACTION, 13, now_ns=100)
        with self.process() as first, self.process() as second:
            with ThreadPoolExecutor(max_workers=2) as pool:
                futures = [pool.submit(self.queue.dispatch_one, "fake-sink", process,
                                       now_ns=100) for process in (first, second)]
                outcomes = [future.result() for future in futures]
        self.assertEqual(sum(row is not None for row in outcomes), 1)
        self.assertEqual(self.queue.status("only-once")["state"], "delivered")
        self.assertEqual(self.sink.posts, 1)

    def test_recovery_refuses_to_race_an_active_dispatch(self):
        with self.sink.lock:
            self.sink.mode = "accept_then_wait"
            self.sink.delay = 1.2
        self.queue.enqueue("active", "fake-sink", ACTION, 14, now_ns=100)
        with self.process(timeout=3.0) as child:
            with ThreadPoolExecutor(max_workers=1) as pool:
                future = pool.submit(self.queue.dispatch_one, "fake-sink", child, now_ns=100)
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline:
                    with self.sink.lock:
                        if self.sink.posts:
                            break
                    time.sleep(0.01)
                else:
                    self.fail("dispatch did not reach the fixture sink")
                with self.assertRaisesRegex(ProtocolError, "dispatch or recovery is active"):
                    self.queue.recover_in_flight()
                self.assertEqual(future.result()["state"], "delivered")
        self.assertEqual(self.queue.recover_in_flight(), 0)

    def test_accept_then_timeout_requires_lookup_and_never_blindly_resends(self):
        with self.sink.lock:
            self.sink.mode = "accept_then_wait"
        self.queue.enqueue("slow", "fake-sink", ACTION, 5, now_ns=100)
        with self.process(timeout=0.15) as child:
            unknown = self.queue.dispatch_one("fake-sink", child, now_ns=100)
        self.assertEqual((unknown["state"], unknown["code"]),
                         ("uncertain", "response_unknown"))
        self.assertEqual(self.sink.posts, 1)
        with self.process() as child:
            self.assertIsNone(self.queue.dispatch_one("fake-sink", child, now_ns=999_000_000_000))
            found = self.queue.reconcile("slow", child, now_ns=200)
        self.assertEqual((found["state"], found["receipt"]), ("delivered", "receipt-1"))
        self.assertEqual(self.sink.posts, 1)
        self.assertEqual(self.sink.lookups, 1)

    def test_missing_lookup_allows_same_key_retry_after_uncertainty(self):
        with self.sink.lock:
            self.sink.mode = "drop_before"
        original = self.queue.enqueue("dropped", "fake-sink", ACTION, 6, now_ns=100)
        with self.process(timeout=0.15) as child:
            self.assertEqual(self.queue.dispatch_one("fake-sink", child, now_ns=100)["state"],
                             "uncertain")
        with self.sink.lock:
            self.sink.mode = "accept"
        with self.process() as child:
            self.assertIsNone(self.queue.dispatch_one("fake-sink", child, now_ns=200))
            self.assertEqual(self.queue.reconcile("dropped", child, now_ns=200)["state"],
                             "retryable")
            self.assertEqual(self.queue.dispatch_one("fake-sink", child, now_ns=200)["state"],
                             "delivered")
        self.assertEqual(self.queue.status("dropped")["idempotency_key"],
                         original["idempotency_key"])
        self.assertEqual(self.sink.posts, 2)

    def test_failed_status_lookup_keeps_uncertain_without_retry(self):
        with self.sink.lock:
            self.sink.mode = "accept_then_wait"
        self.queue.enqueue("lookup-failed", "fake-sink", ACTION, 11, now_ns=100)
        with self.process(timeout=0.15) as child:
            self.assertEqual(self.queue.dispatch_one("fake-sink", child, now_ns=100)["state"],
                             "uncertain")
        with self.sink.lock:
            self.sink.lookup_unavailable = True
        with self.process() as child:
            self.assertEqual(self.queue.reconcile("lookup-failed", child)["state"],
                             "uncertain")
            self.assertIsNone(self.queue.dispatch_one("fake-sink", child,
                                                      now_ns=999_000_000_000))
        self.assertEqual(self.sink.posts, 1)

    def test_revocation_does_not_claim_a_remote_acceptance_was_cancelled(self):
        with self.sink.lock:
            self.sink.mode = "accept_then_wait"
        self.queue.enqueue("accepted-before-revoke", "fake-sink", ACTION, 12, now_ns=100)
        with self.process(timeout=0.15) as child:
            self.assertEqual(self.queue.dispatch_one("fake-sink", child, now_ns=100)["state"],
                             "uncertain")
        self.queue.revoke("fake-sink")
        self.assertEqual(self.queue.status("accepted-before-revoke")["state"], "uncertain")
        with self.process() as child:
            resolved = self.queue.reconcile("accepted-before-revoke", child)
        self.assertEqual((resolved["state"], resolved["receipt"]),
                         ("delivered", "receipt-1"))
        self.assertEqual(self.sink.posts, 1)

    def test_ungranted_action_and_external_endpoint_fail_before_queueing(self):
        with self.assertRaisesRegex(ValueError, "unsupported fixture action"):
            self.queue.grant("other", "send_message", self.endpoint, self.manifest,
                             allowed_grants=("loopback-sink-write",))
        with self.assertRaisesRegex(ValueError, "loopback"):
            self.queue.grant("other", ACTION, "https://example.com/deliver", self.manifest,
                             allowed_grants=("loopback-sink-write",))
        with self.assertRaisesRegex(Exception, "ungranted"):
            self.queue.grant("other", ACTION, self.endpoint, self.manifest,
                             allowed_grants=())
        with self.assertRaisesRegex(ProtocolError, "not granted"):
            self.queue.enqueue("other", "other", ACTION, 1)

    def test_descriptor_rejects_destination_scope_changes(self):
        original = json.loads(self.descriptor_path.read_text())
        for change in ({"endpoint": "https://example.com/deliver"},
                       {"action": "send_message"}, {"grants": []},
                       {"schema": 2}, {"extra": "unexpected"}):
            with self.subTest(change=change):
                self.descriptor_path.write_text(json.dumps({**original, **change}))
                with self.assertRaises(ValueError):
                    read_destination_config(self.descriptor_path)
        self.descriptor_path.write_text(json.dumps(original))

    def test_installer_refuses_linked_destination_and_parent_directories(self):
        destination = self.root / "linked-destination"
        victim = self.root / "victim-directory"
        victim.mkdir()
        destination.symlink_to(victim, target_is_directory=True)
        for target in (destination, self.root / "linked-parent" / "nested"):
            if target != destination:
                (self.root / "linked-parent").symlink_to(victim, target_is_directory=True)
            result = subprocess.run([sys.executable, str(PACKAGE / "install.py"),
                                     "--destination", str(target), "--endpoint", self.endpoint],
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(list(victim.iterdir()), [])
        dangling = self.root / "dangling-destination"
        missing = self.root / "missing-target"
        dangling.symlink_to(missing, target_is_directory=True)
        result = subprocess.run([sys.executable, str(PACKAGE / "install.py"),
                                 "--destination", str(dangling), "--endpoint", self.endpoint],
                                capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(missing.exists())

    def test_installer_refuses_existing_and_dangling_linked_entries(self):
        names = ("fake-outbound", "fake-outbound.toml", "destination.json")
        for name in names:
            for dangling in (False, True):
                with self.subTest(name=name, dangling=dangling):
                    target = self.root / f"linked-entry-{name}-{dangling}"
                    target.mkdir()
                    victim = self.root / f"victim-{name}-{dangling}"
                    if not dangling:
                        victim.write_text("sentinel")
                    (target / name).symlink_to(victim)
                    result = subprocess.run([sys.executable, str(PACKAGE / "install.py"),
                                             "--destination", str(target),
                                             "--endpoint", self.endpoint],
                                            capture_output=True, text=True)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(sorted(p.name for p in target.iterdir()), [name])
                    if dangling:
                        self.assertFalse(victim.exists())
                    else:
                        self.assertEqual(victim.read_text(), "sentinel")

    def test_restart_recovery_and_revocation_preserve_uncertain_visibility(self):
        self.queue.enqueue("crashed", "fake-sink", ACTION, 9, now_ns=100)
        with self.store._source_write(), self.store._connect() as db:
            db.execute("UPDATE outbound_delivery_v1 SET state='in_flight',attempts=1 "
                       "WHERE delivery_id='crashed'")
        recovered = OutboundQueue(self.store)
        self.assertEqual(recovered.recover_in_flight(), 1)
        self.assertEqual(recovered.status("crashed")["state"], "uncertain")
        recovered.revoke("fake-sink")
        self.assertEqual(recovered.status("crashed")["state"], "uncertain")
        with self.process() as child:
            self.assertIsNone(recovered.dispatch_one("fake-sink", child, now_ns=200))
            missing = recovered.reconcile("crashed", child, now_ns=200)
        self.assertEqual((missing["state"], missing["code"]),
                         ("blocked", "grant_revoked"))
        self.assertEqual(self.sink.posts, 0)

    def test_destination_scope_and_artifact_integrity(self):
        self.queue.enqueue("scoped", "fake-sink", ACTION, 10, now_ns=100)
        with self.assertRaisesRegex(ProtocolError, "differs"):
            self.queue.dispatch_one("fake-sink", OutboundProcess(
                self.manifest, destination_id="another-sink", endpoint=self.endpoint,
                allowed_grants=("loopback-sink-write",)), now_ns=100)
        self.assertEqual(self.queue.status("scoped")["state"], "queued")
        Path(self.manifest.entrypoint[0]).write_text("#!/bin/sh\nexit 0\n")
        with self.assertRaisesRegex(ProtocolError, "hash mismatch"):
            self.process().start()
        self.assertEqual(self.sink.posts, 0)


if __name__ == "__main__":
    unittest.main()
