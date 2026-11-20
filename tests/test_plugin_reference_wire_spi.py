"""A trusted nonprivate reference family crosses the supervised process wire."""
import hashlib
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory.plugin_admission import set_source_capture
from observatory.plugin_batch_handlers import DEFAULT_BATCH_HANDLERS
from observatory.plugin_capture_modes import CaptureMode, DEFAULT_CAPTURE_MODES
from observatory.plugin_manifest import PluginManifest
from observatory.plugin_reference_admission import DEFAULT_REFERENCE_ADMISSIONS
from observatory.plugin_runtime import PluginSession
from observatory.plugin_supervisor import PluginSource
from observatory.plugin_wire import ProtocolError, ReferencePollBatch
from tests.test_plugin_reference_admission_spi import AuditAdmission


FEATURE = "fixture-audit-ref/1"
SCRIPT = '''#!/usr/bin/env python3
import json, sys

def read():
    size = int.from_bytes(sys.stdin.buffer.read(4), "big")
    return json.loads(sys.stdin.buffer.read(size))

def send(value):
    raw = json.dumps(value, separators=(",", ":")).encode()
    sys.stdout.buffer.write(len(raw).to_bytes(4, "big") + raw)
    sys.stdout.buffer.flush()

hello = read()
send({"type": "hello_ack", "protocol": 1, "plugin_id": hello["plugin_id"],
      "features": ["source-adapter/1", "fixture-audit-ref/1"]})
while True:
    request = read()
    if request["type"] == "stop":
        break
    if request["type"] != "poll":
        continue
    send({"type": "batch", "protocol": 1, "source_id": request["source_id"],
          "cursor": "next", "records": [{"event_id": "audit-event", "revision": 0,
          "kind": "lifecycle", "time_ns": "9007199254740993",
          "payload": {"state": "completed"}}],
          "reference_refs": {"schema_version": 1, "refs": [{"token": "audit-a"}]}})
'''


class AuditGrant:
    @staticmethod
    def fingerprint_for_source(source):
        return hashlib.sha256(source.source_id.encode()).hexdigest()

    @staticmethod
    def from_source(source, lease):
        if source.source_id != lease.source_id:
            raise ProtocolError("audit source grant mismatch")
        return "host-owned-audit"


def validate_audit_refs(refs, batch):
    if refs != ({"token": "audit-a"},) or batch.source_id != "audit-source":
        raise ProtocolError("invalid nonprivate audit reference")
    return ("audit-a",)


class ReferenceWireSPITests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        script = root / "plugin.py"
        script.write_text(SCRIPT)
        script.chmod(0o700)
        manifest_path = root / "plugin.toml"
        manifest_path.write_text(textwrap.dedent(f'''\
            id = "fixture.audit-wire"
            version = "1.0.0"
            protocol = "1"
            entrypoint = ["{script}"]
            artifact_sha256 = "{hashlib.sha256(script.read_bytes()).hexdigest()}"
            provides = ["source-adapter/1", "{FEATURE}"]
            requires = ["evidence-ingest/1"]
            depends_on = []
            grants = []
            '''))
        manifest = PluginManifest.read(manifest_path)
        self.source = PluginSource(manifest, "audit-source", required_features=(FEATURE,),
                                   capture_mode="audit-reference-v1", timeout=2)
        self.config = root / "plugins.toml"
        self.config.write_text(textwrap.dedent(f'''\
            schema = 1
            [[sources]]
            manifest = "{manifest_path}"
            source_id = "audit-source"
            grants = []
            capture = "audit-reference-v1"
            poll_interval = 0.1
            timeout = 2
            requires_features = ["{FEATURE}"]
            '''))
        self.store = Store(root / "receiver.db")
        self.addCleanup(self.store.close)
        self.provider = AuditAdmission()
        self.admissions = DEFAULT_REFERENCE_ADMISSIONS.with_provider(self.provider)
        self.modes = DEFAULT_CAPTURE_MODES.with_mode(CaptureMode(
            "audit-reference-v1", frozenset({FEATURE}),
            reference_feature=FEATURE, reference_family="reference",
            grant_type=AuditGrant, reference_provider_id=self.provider.id))
        self.handlers = DEFAULT_BATCH_HANDLERS.with_reference_handler(
            FEATURE, validate_audit_refs)

    def session(self, **overrides):
        arguments = dict(source_id=self.source.source_id,
                         required_features=self.source.runtime_features,
                         capture_source=self.source, capture_modes=self.modes,
                         reference_admissions=self.admissions,
                         batch_handlers=self.handlers, timeout=2)
        arguments.update(overrides)
        return PluginSession(self.store, self.source.manifest, **arguments)

    def test_process_reference_commits_replays_and_revokes_atomically(self):
        with self.session() as session:
            first = session.poll_once()
            self.assertIsInstance(first, ReferencePollBatch)
            self.assertEqual(first.reference_feature, FEATURE)
            with self.store._connect() as db:
                row = db.execute("SELECT cursor,digest FROM audit_fixture").fetchone()
                dirty = db.execute("SELECT cursor,dirty_seq FROM plugin_source_cursor_v1").fetchone()
            self.assertEqual(row[0], "next")
            self.assertEqual(tuple(dirty), ("next", 1))
            self.assertEqual(session.poll_once(), first)
            with self.store._connect() as db:
                self.assertEqual(tuple(db.execute(
                    "SELECT cursor,dirty_seq FROM plugin_source_cursor_v1").fetchone()),
                    ("next", 1))
            set_source_capture(self.store, source_id="audit-source", enabled=False,
                               reference_admissions=self.admissions)
            with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
                session.poll_once()
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM audit_fixture").fetchone()[0], 0)

    def test_unknown_or_unbound_reference_family_fails_before_start(self):
        with self.assertRaisesRegex(ProtocolError, "unregistered reference batch"):
            self.session(batch_handlers=DEFAULT_BATCH_HANDLERS)
        with self.assertRaisesRegex(ValueError, "unsupported plugin capture mode"):
            self.session(capture_modes=DEFAULT_CAPTURE_MODES)
        with self.assertRaisesRegex(ProtocolError, "provider is unavailable"):
            self.session(reference_admissions=DEFAULT_REFERENCE_ADMISSIONS)
        metadata = DEFAULT_CAPTURE_MODES.get("metadata-only")
        with self.assertRaisesRegex(ProtocolError, "reference envelope differs"):
            self.handlers.require_capture_feature((FEATURE,), metadata.reference_feature)
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_source_cursor_v1").fetchone()[0], 0)

    def test_common_envelope_rejects_unapproved_shape_and_limits(self):
        validate = self.handlers.select((FEATURE,))
        ordinary = {"type": "batch", "protocol": 1, "source_id": "audit-source",
                    "cursor": "next", "records": []}
        with self.assertRaisesRegex(ProtocolError, "envelope is required"):
            validate(ordinary, "audit-source")
        with self.assertRaisesRegex(ProtocolError, "invalid reference envelope"):
            validate({**ordinary, "reference_refs": {
                "schema_version": 1, "refs": [{"token": "audit-a"}] * 49}}, "audit-source")
        with self.assertRaisesRegex(ProtocolError, "invalid reference envelope"):
            validate({**ordinary, "reference_refs": {
                "schema_version": True, "refs": []}}, "audit-source")
        with self.assertRaisesRegex(ProtocolError, "invalid batch fields"):
            validate({**ordinary, "provider_id": "audit-fixture",
                      "reference_refs": {"schema_version": 1, "refs": []}}, "audit-source")
        with self.assertRaisesRegex(ProtocolError, "invalid nonprivate audit reference"):
            validate({**ordinary, "reference_refs": {
                "schema_version": 1, "refs": [{"token": "other"}]}}, "audit-source")

    def test_receiver_preflight_requires_host_handler_before_store_creation(self):
        from observatory.server import serve
        with patch("observatory.server.Store") as store:
            with self.assertRaisesRegex(ProtocolError, "unregistered reference batch"):
                serve(str(self.config.parent / "unused.db"), no_auth=True,
                      local_agents=False, plugin_config=self.config,
                      capture_modes=self.modes, reference_admissions=self.admissions)
            store.assert_not_called()
        with (patch("observatory.server.ThreadingHTTPServer") as server,
              patch("observatory.plugin_supervisor.PluginSupervisor") as supervisor):
            supervisor.return_value.start.return_value = supervisor.return_value
            serve(str(self.config.parent / "receiver-served.db"), no_auth=True,
                  local_agents=False, plugin_config=self.config,
                  capture_modes=self.modes, reference_admissions=self.admissions,
                  batch_handlers=self.handlers)
        self.assertIs(supervisor.call_args.kwargs["batch_handlers"], self.handlers)
        self.assertTrue(server.return_value.serve_forever.called)

    def test_faulty_trusted_handler_cannot_commit_a_wrong_feature_even_when_empty(self):
        faulty = DEFAULT_BATCH_HANDLERS.with_handler(
            FEATURE, lambda _message, source_id: ReferencePollBatch(
                source_id, "next", (), reference_feature="forged-ref/1"))
        with self.session(batch_handlers=faulty) as session:
            with self.assertRaisesRegex(ProtocolError, "trusted capture mode"):
                session.poll_once()
        with self.store._connect() as db:
            row = db.execute("SELECT cursor,dirty_seq FROM plugin_source_cursor_v1").fetchone()
        self.assertEqual(tuple(row), ("", 0))


if __name__ == "__main__":
    unittest.main()
