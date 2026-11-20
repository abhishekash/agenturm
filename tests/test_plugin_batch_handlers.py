"""Host-owned response dispatch; a child cannot select an unapproved envelope."""
import hashlib
import json
import tempfile
import textwrap
import unittest
from pathlib import Path

from observatory.plugin_batch_handlers import (BatchHandlerRegistry,
                                               DEFAULT_BATCH_HANDLERS)
from observatory.plugin_manifest import PluginManifest
from observatory.plugin_process import PluginProcess
from observatory.plugin_wire import PollBatch, ProtocolError, validate_batch


ORDINARY = {"type": "batch", "protocol": 1, "source_id": "fixture",
            "cursor": "", "records": []}

SCRIPT = '''#!/usr/bin/env python3
import json, sys

def read():
    size = int.from_bytes(sys.stdin.buffer.read(4), "big")
    return json.loads(sys.stdin.buffer.read(size))

def send(value):
    raw = json.dumps(value).encode()
    sys.stdout.buffer.write(len(raw).to_bytes(4, "big") + raw)
    sys.stdout.buffer.flush()

hello = read()
send({"type": "hello_ack", "protocol": 1, "plugin_id": hello["plugin_id"],
      "features": ["source-adapter/1", "fixture-envelope/1", "pi-content-ref/1"]})
while True:
    request = read()
    if request["type"] == "stop":
        break
    if request["type"] == "poll":
        send({"type": "batch", "protocol": 1, "source_id": request["source_id"],
              "cursor": "", "records": [], "fixture_extra": 1})
'''


class BatchHandlerTests(unittest.TestCase):
    def test_ordinary_batch_remains_closed_to_content_envelope(self):
        validate = DEFAULT_BATCH_HANDLERS.select({"source-adapter/1", "harness.pi/1"})
        self.assertEqual(validate(ORDINARY, "fixture").records, ())
        with self.assertRaisesRegex(ProtocolError, "invalid batch fields"):
            validate({**ORDINARY, "content_refs": {"schema_version": 1, "refs": []}},
                     "fixture")

    def test_pi_feature_requires_envelope_and_valid_refs(self):
        validate = DEFAULT_BATCH_HANDLERS.select({"pi-content-ref/1"})
        with self.assertRaisesRegex(ProtocolError, "envelope is required"):
            validate(ORDINARY, "fixture")
        with self.assertRaisesRegex(ProtocolError, "directory member batch"):
            validate({**ORDINARY, "content_refs": {"schema_version": 1,
                                                   "refs": [{}]}}, "fixture")
        self.assertEqual(validate({**ORDINARY, "content_refs": {
            "schema_version": 1, "refs": []}}, "fixture").content_refs, ())

    def test_unknown_content_feature_and_conflicting_handlers_fail_closed(self):
        with self.assertRaisesRegex(ProtocolError, "unregistered reference batch"):
            DEFAULT_BATCH_HANDLERS.select({"next-content-ref/2"})
        with self.assertRaisesRegex(ProtocolError, "unregistered reference batch"):
            DEFAULT_BATCH_HANDLERS.select({"next-command-ref/2"})
        with self.assertRaisesRegex(ProtocolError, "conflicting batch envelope"):
            DEFAULT_BATCH_HANDLERS.select({"pi-content-ref/1", "pi-command-ref/1"})
        with self.assertRaisesRegex(ValueError, "duplicate host batch handler"):
            DEFAULT_BATCH_HANDLERS.with_handler("pi-content-ref/1", validate_batch)
        synthetic = DEFAULT_BATCH_HANDLERS.with_handler("fixture-envelope/1", validate_batch)
        with self.assertRaisesRegex(ProtocolError, "conflicting batch envelope"):
            synthetic.select({"pi-content-ref/1", "fixture-envelope/1"})

    def test_second_trusted_handler_works_without_process_dispatch_change(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            script = root / "plugin.py"
            script.write_text(SCRIPT)
            script.chmod(0o700)
            manifest_file = root / "plugin.toml"
            manifest_file.write_text(textwrap.dedent(f'''\
                id = "fixture.batch-handler"
                version = "1.0.0"
                protocol = "1"
                entrypoint = ["{script}"]
                artifact_sha256 = "{hashlib.sha256(script.read_bytes()).hexdigest()}"
                provides = ["source-adapter/1", "fixture-envelope/1", "pi-content-ref/1"]
                requires = ["evidence-ingest/1"]
                depends_on = []
                grants = []
                '''))
            manifest = PluginManifest.read(manifest_file)
            unknown_file = root / "unknown.toml"
            unknown_file.write_text(manifest_file.read_text().replace(
                '"fixture-envelope/1", "pi-content-ref/1"',
                '"next-content-ref/2"'))
            unknown_manifest = PluginManifest.read(unknown_file)
            with self.assertRaisesRegex(ProtocolError, "unregistered reference batch"):
                PluginProcess(unknown_manifest, source_id="fixture", timeout=2,
                              required_features=("next-content-ref/2",))

            def validate_fixture(message, source_id):
                if message.get("fixture_extra") != 1:
                    raise ProtocolError("invalid fixture envelope")
                return validate_batch({key: value for key, value in message.items()
                                       if key != "fixture_extra"}, source_id=source_id)

            handlers = DEFAULT_BATCH_HANDLERS.with_handler(
                "fixture-envelope/1", validate_fixture)
            with PluginProcess(manifest, source_id="fixture", timeout=2,
                               required_features=("fixture-envelope/1",),
                               batch_handlers=handlers) as host:
                self.assertEqual(host.poll("").records, ())
            def failed_validator(_message, _source_id):
                raise ValueError("trusted handler failed")

            failing = DEFAULT_BATCH_HANDLERS.with_handler(
                "fixture-envelope/1", failed_validator)
            with PluginProcess(manifest, source_id="fixture", timeout=2,
                               required_features=("fixture-envelope/1",),
                               batch_handlers=failing) as host:
                with self.assertRaisesRegex(ValueError, "trusted handler failed"):
                    host.poll("")
                self.assertIsNone(host.process)
                self.assertIsNone(host.pending)
            wrong_source = DEFAULT_BATCH_HANDLERS.with_handler(
                "fixture-envelope/1",
                lambda _message, _source_id: PollBatch("other", "", ()))
            with PluginProcess(manifest, source_id="fixture", timeout=2,
                               required_features=("fixture-envelope/1",),
                               batch_handlers=wrong_source) as host:
                with self.assertRaisesRegex(ProtocolError, "another source"):
                    host.poll("")
                self.assertIsNone(host.process)
                self.assertIsNone(host.pending)
            malformed = DEFAULT_BATCH_HANDLERS.with_handler(
                "fixture-envelope/1",
                lambda _message, _source_id: PollBatch("fixture", "", None))
            with PluginProcess(manifest, source_id="fixture", timeout=2,
                               required_features=("fixture-envelope/1",),
                               batch_handlers=malformed) as host:
                with self.assertRaisesRegex(ProtocolError, "invalid records"):
                    host.poll("")
                self.assertIsNone(host.process)
                self.assertIsNone(host.pending)
            # Advertising an unrequired feature does not select its handler.
            with PluginProcess(manifest, source_id="fixture", timeout=2) as host:
                with self.assertRaisesRegex(ProtocolError, "invalid batch fields"):
                    host.poll("")


if __name__ == "__main__":
    unittest.main()
