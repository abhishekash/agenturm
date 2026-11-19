"""A separate executable obeys bounded control/data and durable-ack ordering."""
import hashlib
import tempfile
import textwrap
import unittest
from pathlib import Path

from observatory.plugin_manifest import PluginManifest
from observatory.plugin_process import PluginProcess
from observatory.plugin_wire import ProtocolError, decode_message, validate_batch


PLUGIN_SCRIPT = r'''#!/usr/bin/env python3
import json, os, struct, sys, time
MODE = __MODE__
MARKER = __MARKER__

def read_exact(count):
    raw = b''
    while len(raw) < count:
        chunk = sys.stdin.buffer.read(count - len(raw))
        if not chunk:
            raise EOFError
        raw += chunk
    return raw

def receive():
    size = int.from_bytes(read_exact(4), 'big')
    return json.loads(read_exact(size))

def send(value):
    raw = json.dumps(value, separators=(',', ':')).encode()
    sys.stdout.buffer.write(len(raw).to_bytes(4, 'big') + raw)
    sys.stdout.buffer.flush()

try:
    hello = receive()
    send({'type':'hello_ack','protocol':1,'plugin_id':hello['plugin_id'],
          'features':['source-adapter/1']})
    while True:
        request = receive()
        if request['type'] == 'stop':
            break
        if request['type'] == 'ack':
            with open(MARKER, 'a', encoding='utf-8') as out:
                out.write(request['cursor'] + '\n')
            continue
        if request['type'] != 'poll':
            break
        if MODE == 'timeout':
            time.sleep(10)
            continue
        if MODE == 'crash':
            os._exit(3)
        if MODE == 'crash_once' and not os.path.exists(MARKER):
            open(MARKER, 'w', encoding='utf-8').close()
            os._exit(3)
        if MODE == 'bad_json':
            raw = b'{"type":"batch","type":"batch"}'
            sys.stdout.buffer.write(len(raw).to_bytes(4, 'big') + raw)
            sys.stdout.buffer.flush()
            continue
        if MODE == 'oversize':
            sys.stdout.buffer.write((4194305).to_bytes(4, 'big'))
            sys.stdout.buffer.flush()
            time.sleep(10)
            continue
        if MODE == 'stderr':
            sys.stderr.buffer.write(b'x' * 65536)
            sys.stderr.buffer.flush()
        if MODE == 'idle':
            send({'type':'batch','protocol':1,'source_id':request['source_id'],
                  'cursor':request['cursor'],'records':[]})
            continue
        record = {'event_id':'fixture-1','revision':0,'kind':'lifecycle',
                  'time_ns':'9007199254740993','payload':{'state':'completed'}}
        send({'type':'batch','protocol':1,
              'source_id':'wrong' if MODE == 'wrong_source' else request['source_id'],
              'cursor':'next','records':[record]})
except EOFError:
    pass
'''


class ProcessHostTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.marker = self.root / "ack.txt"

    def manifest(self, mode="ok"):
        script = self.root / "plugin.py"
        script.write_text(PLUGIN_SCRIPT.replace("__MODE__", repr(mode)).replace(
            "__MARKER__", repr(str(self.marker))))
        script.chmod(0o700)
        digest = hashlib.sha256(script.read_bytes()).hexdigest()
        manifest = self.root / "plugin.toml"
        manifest.write_text(textwrap.dedent(f'''\
            id = "fixture.plugin"
            version = "0.1.0"
            protocol = "1"
            entrypoint = ["{script}"]
            artifact_sha256 = "{digest}"
            provides = ["source-adapter/1"]
            requires = ["evidence-ingest/1"]
            depends_on = []
            grants = ["configured-source-read"]
            '''))
        return PluginManifest.read(manifest)

    def host(self, mode="ok", timeout=1.0):
        return PluginProcess(self.manifest(mode), source_id="fixture-source",
                             allowed_grants=("configured-source-read",), timeout=timeout)

    def test_commit_precedes_ack_and_pending_batch_blocks_second_poll(self):
        with self.host() as host:
            batch = host.poll("")
            self.assertEqual(batch.records[0]["time_ns"], "9007199254740993")
            self.assertIsNone(host.last_ack)
            self.assertFalse(self.marker.exists())
            with self.assertRaisesRegex(ProtocolError, "requires durable acknowledgment"):
                host.poll("")
            host.ack(batch.cursor)
            self.assertEqual(host.last_ack, "next")
            host.poll_commit("next", lambda accepted: accepted.cursor)
        self.assertEqual(self.marker.read_text().splitlines(), ["next", "next"])

    def test_failed_commit_never_sends_ack(self):
        host = self.host().start()
        def fail(_batch):
            raise RuntimeError("commit failed")
        with self.assertRaisesRegex(RuntimeError, "commit failed"):
            host.poll_commit("", fail)
        self.assertIsNone(host.process)
        self.assertFalse(self.marker.exists())

    def test_identity_oversize_and_timeout_close_the_worker(self):
        for mode in ("wrong_source", "oversize", "timeout", "crash", "bad_json"):
            with self.subTest(mode=mode):
                host = self.host(mode, timeout=0.3).start()
                with self.assertRaises(ProtocolError):
                    host.poll("")
                self.assertIsNone(host.process)

    def test_artifact_change_is_rejected_before_execution(self):
        manifest = self.manifest()
        Path(manifest.entrypoint[0]).write_text("#!/bin/sh\nexit 0\n")
        host = PluginProcess(manifest, source_id="fixture-source",
                             allowed_grants=("configured-source-read",))
        with self.assertRaisesRegex(ProtocolError, "hash mismatch"):
            host.start()
        self.assertIsNone(host.process)

    def test_stderr_is_drained_without_exposing_child_output(self):
        with self.host("stderr") as host:
            host.poll_commit("", lambda batch: batch.cursor)
            self.assertEqual(host.stderr_bytes, 16384)


class WireTests(unittest.TestCase):
    def test_duplicate_keys_nonfinite_numbers_and_depth_are_rejected(self):
        for raw in ('{"x":1,"x":2}', '{"x":NaN}', '{"x":1e9999}',
                    '[' * 21 + '0' + ']' * 21):
            with self.subTest(raw=raw[:20]), self.assertRaises(ProtocolError):
                decode_message(raw)

    def test_batch_rejects_wrong_timestamp_and_duplicate_event_ids(self):
        row = {"event_id": "one", "revision": 0, "kind": "usage",
               "time_ns": "1", "payload": {}}
        batch = {"type": "batch", "protocol": 1, "source_id": "source",
                 "cursor": "next", "records": [row, dict(row)]}
        with self.assertRaisesRegex(ProtocolError, "duplicate event"):
            validate_batch(batch, source_id="source")
        batch["records"] = [{**row, "time_ns": "9007199254740993"}]
        self.assertEqual(validate_batch(batch, source_id="source").records[0]["time_ns"],
                         "9007199254740993")
        batch["records"] = [{**row, "time_ns": "9223372036854775808"}]
        with self.assertRaisesRegex(ProtocolError, "timestamp"):
            validate_batch(batch, source_id="source")

    def test_record_byte_limit_and_boolean_revision_are_rejected(self):
        row = {"event_id": "one", "revision": 0, "kind": "message",
               "time_ns": "1", "payload": {"text": "α" * 9000}}
        batch = {"type": "batch", "protocol": 1, "source_id": "source",
                 "cursor": "next", "records": [row]}
        with self.assertRaisesRegex(ProtocolError, "byte limit"):
            validate_batch(batch, source_id="source")
        row["payload"] = {}
        row["revision"] = True
        with self.assertRaisesRegex(ProtocolError, "revision"):
            validate_batch(batch, source_id="source")


if __name__ == "__main__":
    unittest.main()
