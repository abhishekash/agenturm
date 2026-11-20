"""Synthetic end-to-end trusted private reference process fixture.

The fixture is deliberately test-only: the installed child sends a digest and
never receives the synthetic private source bytes or a production capture mode.
"""
import hashlib
import json
import re
import tempfile
import textwrap
import threading
import unittest
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

from observatory.core import Store
from observatory.plugin_activity import _opaque
from observatory.plugin_admission import activate_source, set_source_capture
from observatory.plugin_batch_handlers import DEFAULT_BATCH_HANDLERS
from observatory.plugin_capture_modes import CaptureMode, DEFAULT_CAPTURE_MODES
from observatory.plugin_manifest import PluginManifest
from observatory.plugin_projection import project_source
from observatory.plugin_reference_admission import (
    DEFAULT_REFERENCE_ADMISSIONS, PreparedReference, ReferenceAdmissionProvider,
    ReferenceAdmissionRegistry,
)
from observatory.plugin_reference_read import (
    ReferenceReadProvider, ReferenceReadRegistry, ReferenceText,
)
from observatory.plugin_runtime import PluginSession
from observatory.plugin_supervisor import PluginSource
from observatory.plugin_wire import ProtocolError
from observatory.receiver import BoundedHTTPServer
from observatory.server import handler_for

FEATURE = "fixture-private-ref/1"
PROVIDER = "synthetic-private"
EVENT = "fixed-private-event"
SOURCE_ID = "synthetic-private-source"
RAW = (b"password=synthetic-secret-value\n" + b"bounded synthetic payload. " * 300)


class SyntheticGrant:
    """Trusted source grant constructed only by the host capture mode."""

    def __init__(self, root: Path, fingerprint: str):
        self.path = root / "private-fixture.txt"
        self.fingerprint = fingerprint
        self.raw = self.path.read_bytes()

    @staticmethod
    def fingerprint_for_source(source):
        raw = (Path(source.source_root) / "private-fixture.txt").read_bytes()
        if not 1 <= len(raw) <= 16_384:
            raise ProtocolError("synthetic source exceeds trusted fixture bound")
        return hashlib.sha256(raw).hexdigest()

    @classmethod
    def from_source(cls, source, lease):
        fingerprint = cls.fingerprint_for_source(source)
        if source.source_id != lease.source_id or fingerprint != lease.content_fingerprint:
            raise ProtocolError("synthetic source fingerprint changed")
        return cls(Path(source.source_root), fingerprint)


class SyntheticAdmission(ReferenceAdmissionProvider):
    id = PROVIDER

    def initialize_schema(self, db):
        db.execute("CREATE TABLE IF NOT EXISTS synthetic_private_v1("
                   "source_id TEXT NOT NULL,event_key TEXT NOT NULL,source_digest TEXT NOT NULL,"
                   "redacted_text TEXT NOT NULL,PRIMARY KEY(source_id,event_key))")

    def erase_source(self, db, source_id):
        db.execute("DELETE FROM synthetic_private_v1 WHERE source_id=?", (source_id,))

    def prepare(self, refs, grant, batch, lease):
        if (type(refs) is not tuple or len(refs) != 1 or type(grant) is not SyntheticGrant
                or batch.source_id != lease.source_id or len(grant.raw) > 16_384):
            raise ProtocolError("invalid synthetic private reference")
        ref = refs[0]
        if (type(ref) is not dict or set(ref) != {"event_id", "source_sha256"}
                or ref["event_id"] != EVENT
                or ref["source_sha256"] != hashlib.sha256(grant.raw).hexdigest()
                or not any(item["event_id"] == EVENT for item in batch.records)):
            raise ProtocolError("synthetic reference does not prove source bytes")
        digest = hashlib.sha256(grant.raw).hexdigest()
        return PreparedReference(self.id, ((EVENT, bytes(grant.raw)),), digest, grant)

    def verify(self, prepared, batch, prior_cursor):
        grant = prepared.grant
        try:
            current = grant.path.read_bytes()
        except OSError as exc:
            raise ProtocolError("synthetic source bytes unavailable") from exc
        if (current != grant.raw or hashlib.sha256(current).hexdigest() != grant.fingerprint
                or grant.fingerprint != prepared.digest):
            raise ProtocolError("synthetic source fingerprint changed during admission")
        return prepared.checked, ""

    def replay(self, db, source_id, cursor, prepared):
        if prepared is None:
            row = db.execute("SELECT 1 FROM synthetic_private_v1 WHERE source_id=? LIMIT 1",
                             (source_id,)).fetchone()
            if row:
                raise ProtocolError("synthetic reference replay mismatch")
            return
        rows = db.execute("SELECT source_digest FROM synthetic_private_v1 WHERE source_id=?",
                          (source_id,)).fetchall()
        if not rows or any(row[0] != prepared.digest for row in rows):
            raise ProtocolError("synthetic reference replay mismatch")

    def commit(self, context):
        if context.prepared is None:
            context.db.execute("DELETE FROM synthetic_private_v1 WHERE source_id=?",
                               (context.lease.source_id,))
            return None
        raw = context.prepared.checked[0][1]
        # This is a deliberately narrow demo redactor for fixed fake fixture data.
        text = raw.decode("utf-8", "strict")
        redacted = re.sub(r"password=[^\r\n]*", "password=[REDACTED]", text)
        receiver, source, native_hash = (context.lease.receiver_id,
            context.lease.source_id,
            hashlib.sha256((context.lease.source_id + "\0" + EVENT).encode()).hexdigest())
        event_key = "plugin-event:" + _opaque(receiver, source, native_hash)
        context.db.execute("INSERT INTO synthetic_private_v1 VALUES(?,?,?,?) "
                           "ON CONFLICT(source_id,event_key) DO UPDATE SET "
                           "source_digest=excluded.source_digest,redacted_text=excluded.redacted_text",
                           (source, event_key, context.prepared.digest, redacted))
        return None


class SyntheticRead:
    def read(self, store, event_id):
        from observatory.plugin_reference_read import _binding
        binding = _binding(store, PROVIDER, event_id)
        if binding is None:
            return None
        with store._read() as db:
            row = db.execute("SELECT source_digest,redacted_text FROM synthetic_private_v1 "
                             "WHERE source_id=? AND event_key=?", (binding[1], event_id)).fetchone()
        if row is None:
            return None
        return ReferenceText(PROVIDER, binding[0], binding[1], event_id,
                             *binding[2:], row["redacted_text"])


def validate_refs(refs, batch):
    if (len(refs) != 1 or type(refs[0]) is not dict
            or set(refs[0]) != {"event_id", "source_sha256"}
            or refs[0]["event_id"] != EVENT
            or type(refs[0]["source_sha256"]) is not str
            or not re.fullmatch(r"[0-9a-f]{64}", refs[0]["source_sha256"])):
        raise ProtocolError("invalid synthetic ref envelope")
    return tuple(dict(ref) for ref in refs)


class PrivateReferenceE2ETests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        child_dir = root / "separately-installed-child"
        child_dir.mkdir()
        fixture_root = root / "trusted-private-root"
        fixture_root.mkdir()
        self.fixture_path = fixture_root / "private-fixture.txt"
        self.fixture_path.write_bytes(RAW)
        digest = hashlib.sha256(RAW).hexdigest()
        script = child_dir / "fixture_child.py"
        script.write_text(textwrap.dedent(f'''\
            #!/usr/bin/env python3
            import json, sys
            def read():
                size = int.from_bytes(sys.stdin.buffer.read(4), "big")
                return json.loads(sys.stdin.buffer.read(size))
            def send(value):
                raw = json.dumps(value, separators=(",", ":")).encode()
                sys.stdout.buffer.write(len(raw).to_bytes(4, "big") + raw)
                sys.stdout.buffer.flush()
            hello = read()
            send({{"type":"hello_ack","protocol":1,"plugin_id":hello["plugin_id"],
                  "features":["source-adapter/1","configured-root/1","{FEATURE}"]}})
            while True:
                request = read()
                if request["type"] == "stop": break
                send({{"type":"batch","protocol":1,"source_id":request["source_id"],
                      "cursor":"synthetic-cursor","records":[{{"event_id":"{EVENT}",
                      "revision":0,"kind":"message","time_ns":"9007199254740993",
                      "identity":{{"session_id":"synthetic-session"}},
                      "payload":{{"role":"user"}}}}],
                      "reference_refs":{{"schema_version":1,"refs":[{{
                      "event_id":"{EVENT}","source_sha256":"{digest}"}}]}}}})
            '''))
        script.chmod(0o700)
        manifest_path = child_dir / "plugin.toml"
        manifest_path.write_text(textwrap.dedent(f'''\
            id = "fixture.synthetic-private"
            version = "1.0.0"
            protocol = "1"
            entrypoint = ["{script}"]
            artifact_sha256 = "{hashlib.sha256(script.read_bytes()).hexdigest()}"
            provides = ["source-adapter/1", "configured-root/1", "{FEATURE}"]
            requires = ["evidence-ingest/1"]
            depends_on = []
            grants = ["configured-source-read"]
            '''))
        manifest = PluginManifest.read(manifest_path)
        self.source = PluginSource(manifest, SOURCE_ID,
                                   allowed_grants=("configured-source-read",),
                                   source_root=str(fixture_root),
                                   required_features=(FEATURE, "configured-root/1"),
                                   capture_mode="synthetic-private-v1", timeout=2)
        self.store_path = root / "receiver.db"
        self.store = Store(self.store_path)
        self.addCleanup(self.store.close)
        self.admission = SyntheticAdmission()
        self.admissions = DEFAULT_REFERENCE_ADMISSIONS.with_provider(self.admission)
        self.read_provider = SyntheticRead()
        self.readers = ReferenceReadRegistry((ReferenceReadProvider(PROVIDER,
                                                                      self.read_provider.read),))
        self.handlers = DEFAULT_BATCH_HANDLERS.with_reference_handler(FEATURE, validate_refs)
        self.modes = DEFAULT_CAPTURE_MODES.with_mode(CaptureMode(
            "synthetic-private-v1", frozenset({FEATURE, "configured-root/1"}),
            requires_root=True, required_grants=frozenset({"configured-source-read"}),
            reference_feature=FEATURE, reference_family="reference",
            grant_type=SyntheticGrant, reference_provider_id=PROVIDER))

    def http(self, event_id, store=None, readers=None):
        from observatory.server import handler_for
        store = store or self.store
        handler = handler_for(store, quiet=True, reference_readers=readers or self.readers)
        self.addCleanup(handler.response_cache.close)
        server = BoundedHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        path = "/api/plugin-reference-content?" + urlencode({
            "provider_id": PROVIDER, "event_id": event_id})
        try:
            with urlopen(Request(f"http://127.0.0.1:{server.server_port}{path}"), timeout=5) as r:
                return r.status, json.load(r)
        except HTTPError as exc:
            return exc.code, json.loads(exc.read())

    def run_process(self):
        with PluginSession(self.store, self.source.manifest, source_id=SOURCE_ID,
                           source_root=self.source.source_root,
                           allowed_grants=self.source.allowed_grants,
                           required_features=self.source.runtime_features,
                           capture_source=self.source, capture_modes=self.modes,
                           reference_admissions=self.admissions,
                           batch_handlers=self.handlers, timeout=2) as session:
            batch = session.poll_once()
            return batch

    def event_key(self):
        with self.store._read() as db:
            row = db.execute("SELECT event_key FROM activity_index_v1 "
                             "WHERE kind='plugin' AND owner_key=?", (SOURCE_ID,)).fetchone()
        self.assertIsNotNone(row)
        return row[0]

    def test_separate_child_proves_admits_projects_reads_redacts_and_fails_closed(self):
        batch = self.run_process()
        self.assertEqual(batch.cursor, "synthetic-cursor")
        with self.store._read() as db:
            evidence = db.execute("SELECT COUNT(*) FROM plugin_evidence_v1 "
                                  "WHERE source_id=?", (SOURCE_ID,)).fetchone()[0]
            owner = db.execute("SELECT provider_id FROM plugin_reference_owner_v1 "
                               "WHERE source_id=?", (SOURCE_ID,)).fetchall()
            saved = db.execute("SELECT source_digest,redacted_text FROM synthetic_private_v1 "
                               "WHERE source_id=?", (SOURCE_ID,)).fetchone()
        self.assertEqual(evidence, 1)
        self.assertIn((PROVIDER,), [tuple(row) for row in owner])
        self.assertEqual(saved[0], hashlib.sha256(RAW).hexdigest())
        self.assertNotIn("synthetic-secret-value", saved[1])
        self.assertIn("password=[REDACTED]", saved[1])
        project_source(self.store, SOURCE_ID)
        event_id = self.event_key()
        from observatory.plugin_reference_read import _binding
        self.assertIsNotNone(_binding(self.store, PROVIDER, event_id))
        self.assertIsNotNone(self.readers.read_exact(self.store, PROVIDER, event_id))
        status, body = self.http(event_id)
        self.assertEqual(status, 200)
        self.assertEqual(body["event_id"], event_id)
        other_event = event_id[:-1] + ("0" if event_id[-1] != "0" else "1")
        self.assertEqual(self.http(other_event)[0], 404)
        self.assertEqual(len(body["redacted_text"].encode()), 4096)
        self.assertTrue(body["clipped"])
        self.assertNotIn("synthetic-secret-value", json.dumps(body))
        self.assertNotIn(str(self.fixture_path), json.dumps(body))

        set_source_capture(self.store, source_id=SOURCE_ID, enabled=False,
                           reference_admissions=self.admissions)
        status, body = self.http(event_id)
        self.assertEqual(status, 404)
        self.assertNotIn("bounded synthetic payload", json.dumps(body))
        with self.store._read() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM synthetic_private_v1 "
                                        "WHERE source_id=?", (SOURCE_ID,)).fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_reference_owner_v1 "
                                        "WHERE source_id=?", (SOURCE_ID,)).fetchone()[0], 0)

        # Reopening the DB simulates a receiver restart; an unregistered reader
        # and the old event remain unavailable after revocation.
        self.store.close()
        restarted = Store(self.store_path)
        self.addCleanup(restarted.close)
        from observatory.plugin_reference_read import DEFAULT_REFERENCE_READERS
        self.assertIsNone(DEFAULT_REFERENCE_READERS.read_exact(restarted, PROVIDER, event_id))
        self.assertIsNone(self.readers.read_exact(restarted, PROVIDER, event_id))
        self.assertEqual(self.http(event_id, restarted, self.readers)[0], 404)
        with self.assertRaisesRegex(ProtocolError, "plugin source capture revoked"):
            activate_source(restarted, plugin_id=self.source.manifest.id, source_id=SOURCE_ID,
                            generation=self.source.manifest.artifact_sha256,
                            content_fingerprint=hashlib.sha256(RAW).hexdigest())

    def test_fingerprint_change_erases_saved_private_text_before_new_lease(self):
        self.run_process()
        project_source(self.store, SOURCE_ID)
        event_id = self.event_key()
        changed = RAW.replace(b"synthetic-secret-value", b"synthetic-changed-value")
        self.fixture_path.write_bytes(changed)
        lease = activate_source(self.store, plugin_id=self.source.manifest.id,
                                source_id=SOURCE_ID,
                                generation=self.source.manifest.artifact_sha256,
                                content_fingerprint=hashlib.sha256(changed).hexdigest(),
                                reference_admissions=self.admissions)
        self.assertGreater(lease.epoch, 0)
        with self.store._read() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM synthetic_private_v1 "
                                        "WHERE source_id=?", (SOURCE_ID,)).fetchone()[0], 0)
        # Projection is stale until rebuilt; no prior reference can be served.
        self.assertEqual(self.http(event_id)[0], 404)
        restarted = Store(self.store_path)
        self.addCleanup(restarted.close)
        from observatory.plugin_reference_read import DEFAULT_REFERENCE_READERS
        self.assertIsNone(DEFAULT_REFERENCE_READERS.read_exact(restarted, PROVIDER, event_id))
        self.assertIsNone(self.readers.read_exact(restarted, PROVIDER, event_id))
        self.assertEqual(self.http(event_id, restarted, self.readers)[0], 404)


if __name__ == "__main__":
    unittest.main()
