"""Installed, explicit Pi directory reference transport with no text on wire."""
import hashlib
import json
import tempfile
import textwrap
import time
import unittest
from dataclasses import replace
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import set_source_capture
from observatory.plugin_config import read_plugin_config
from observatory.plugin_process import PluginProcess
from observatory.plugin_runtime import PluginSession
from observatory.plugin_supervisor import PluginSupervisor
from observatory.plugin_wire import (ContentPollBatch, ProtocolError,
                                     validate_batch, validate_content_batch)


SCRIPT = r'''#!/usr/bin/env python3
import hashlib, json, os, sys
MODE = __MODE__
MARKER = __MARKER__

def sha(raw):
    return hashlib.sha256(raw).hexdigest()

def receive():
    prefix = sys.stdin.buffer.read(4)
    if len(prefix) != 4:
        raise EOFError
    size = int.from_bytes(prefix, 'big')
    raw = b''
    while len(raw) < size:
        part = sys.stdin.buffer.read(size - len(raw))
        if not part:
            raise EOFError
        raw += part
    return json.loads(raw)

def send(value):
    raw = json.dumps(value, separators=(',', ':')).encode()
    sys.stdout.buffer.write(len(raw).to_bytes(4, 'big') + raw)
    sys.stdout.buffer.flush()

try:
    hello = receive()
    features = ['source-adapter/1', 'harness.pi/1', 'source-discovery/1',
                'source-discovery-root-gap/1',
                'configured-root/1', 'pi-content-ref/1']
    if MODE == 'missing_hello':
        features.remove('pi-content-ref/1')
    if MODE == 'missing_root_gap':
        features.remove('source-discovery-root-gap/1')
    send({'type':'hello_ack', 'protocol':1, 'plugin_id':hello['plugin_id'],
          'features':features})
    while True:
        request = receive()
        if request['type'] == 'stop':
            break
        if request['type'] == 'ack':
            with open(MARKER, 'a', encoding='utf-8') as stream:
                stream.write(request['cursor'] + '\n')
            continue
        if request['type'] != 'poll':
            raise RuntimeError('unexpected control')
        if request['cursor'] and MODE != 'repeat':
            response = {'type':'batch','protocol':1,'source_id':request['source_id'],
                        'cursor':request['cursor'],'records':[]}
            if MODE != 'missing_envelope':
                response['content_refs'] = {'schema_version':1,'refs':[]}
            send(response)
            continue
        root = os.environ['AGENTURM_PLUGIN_SOURCE_ROOT']
        relative = 'project/one.jsonl'
        path = os.path.join(root, relative)
        with open(path, 'rb') as stream:
            raw = stream.read()
        header, row = raw.splitlines(keepends=True)
        info = os.stat(path)
        header_hash = sha(header)
        origin = sha(f'{info.st_dev}:{info.st_ino}:{header_hash}'.encode())
        member_id = sha(relative.encode())
        event_id = f'pi:{origin}:user-1:message'
        checkpoint = (f'v2:{info.st_dev}:{info.st_ino}:{len(raw)}:{sha(raw)}:'
                      f'{header_hash}:{origin}:user-1')
        record = {'event_id':event_id,'revision':0,'kind':'message',
                  'time_ns':'1792843200000000000',
                  'payload':{'role':'user'},
                  'identity':{'session_id':'pi-session','turn_id':'user-1'}}
        ref = {'schema':1,'source_id':request['source_id'],'member_id':member_id,
               'relative_member_path':relative,'row_offset':len(header),
               'row_length':len(row),'row_sha256':sha(row),'event_id':event_id,
               'revision':0,'native_session_id':'pi-session',
               'native_turn_id':'user-1','kind':'pi.user_text'}
        if MODE == 'forged_hash':
            ref['row_sha256'] = '0' * 64
        if MODE == 'raw_text_field':
            ref['text'] = 'private request'
        response = {'type':'batch','protocol':1,'source_id':request['source_id'],
                    'cursor':'d1:1:' + member_id,'records':[record],
                    'member_id':member_id,'member_cursor':checkpoint}
        if MODE != 'missing_envelope':
            response['content_refs'] = {'schema_version': True if MODE == 'bad_schema'
                                        else 1, 'refs':[ref]}
        send(response)
except EOFError:
    pass
'''


def line(value):
    return (json.dumps(value, separators=(",", ":")) + "\n").encode()


class PiContentTransportTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.source_root = self.root / "sessions"
        (self.source_root / "project").mkdir(parents=True)
        self.source_file = self.source_root / "project" / "one.jsonl"
        self.source_file.write_bytes(line({"type": "session", "id": "pi-session"}) +
                                     line({"type": "message", "id": "user-1",
                                           "timestamp": "2026-10-24T12:00:00Z",
                                           "message": {"role": "user", "content": [
                                               {"type": "text", "text":
                                                "password=private-value"}]}}))
        self.marker = self.root / "ack.txt"
        self.store = Store(self.root / "store.db")
        self.addCleanup(self.store.close)

    def source(self, mode="ok", *, capture="pi-review-text-v1", features=None,
               replaces="pi", grants='["configured-source-read"]', root=True,
               force_content_feature=False):
        script = self.root / "plugin.py"
        script.write_text(SCRIPT.replace("__MODE__", repr(mode)).replace(
            "__MARKER__", repr(str(self.marker))))
        script.chmod(0o700)
        digest = hashlib.sha256(script.read_bytes()).hexdigest()
        provided = features or ["source-adapter/1", "harness.pi/1", "source-discovery/1",
                                "source-discovery-root-gap/1", "configured-root/1",
                                "pi-content-ref/1"]
        manifest = self.root / "plugin.toml"
        manifest.write_text(textwrap.dedent(f'''\
            id = "fixture.pi"
            version = "1.0.0"
            protocol = "1"
            entrypoint = ["{script}"]
            artifact_sha256 = "{digest}"
            provides = {json.dumps(provided)}
            requires = ["evidence-ingest/1"]
            depends_on = []
            grants = ["configured-source-read"]
            '''))
        config = self.root / "plugins.toml"
        config.write_text('schema = 1\n[[sources]]\n'
                          f'manifest = "{manifest}"\n'
                          'source_id = "pi-directory"\n'
                          f'grants = {grants}\n'
                          f'capture = "{capture}"\n'
                          'poll_interval = 0.05\n'
                          'timeout = 1.0\n'
                          f'replaces = ["{replaces}"]\n'
                          + (f'root = "{self.source_root}"\n' if root else '')
                          + 'requires_features = ["source-discovery/1",'
                          '"source-discovery-root-gap/1",'
                          '"configured-root/1"'
                          + (',"pi-content-ref/1"' if capture == "pi-review-text-v1"
                             or force_content_feature else '') + ']\n')
        return read_plugin_config(config)[0]

    def session(self, source):
        return PluginSession(self.store, source.manifest, source_id=source.source_id,
                             allowed_grants=source.allowed_grants, timeout=source.timeout,
                             source_root=source.source_root,
                             required_features=source.runtime_features,
                             content_source=(source if source.capture_mode == "pi-review-text-v1"
                                             else None))

    def snapshot(self):
        with self.store._connect() as db:
            cursor = db.execute("SELECT cursor,content_fingerprint FROM plugin_source_cursor_v1 "
                                "WHERE source_id='pi-directory'").fetchone()
            content = [row[0] for row in db.execute(
                "SELECT redacted_text FROM plugin_content_v1 WHERE source_id='pi-directory'")]
        return (tuple(cursor) if cursor else None), content

    def test_installed_reference_only_poll_ack_idle_and_replay(self):
        source = self.source()
        with self.session(source) as session:
            batch = session.poll_once()
            self.assertIsInstance(batch, ContentPollBatch)
            self.assertEqual(len(batch.content_refs), 1)
            self.assertEqual(session.poll_once().content_refs, ())
            self.assertEqual(session.cursor, batch.cursor)
        snapshot, content = self.snapshot()
        self.assertEqual(snapshot[0], batch.cursor)
        self.assertEqual(len(content), 1)
        self.assertIn("password=[REDACTED]", content[0])
        self.assertNotIn("private-value", str((snapshot, content)))
        self.assertEqual(self.marker.read_text().splitlines(), [batch.cursor, batch.cursor])
        with self.session(source) as resumed:
            self.assertEqual(resumed.cursor, batch.cursor)
            self.assertEqual(resumed.poll_once().content_refs, ())
        self.assertEqual(self.snapshot(), (snapshot, content))

    def test_identical_content_batch_replay_preserves_one_row(self):
        source = self.source("repeat")
        with self.session(source) as session:
            first = session.poll_once()
            original = self.snapshot()
            self.assertEqual(session.poll_once(), first)
            self.assertEqual(self.snapshot(), original)
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 1)
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_content_v1").fetchone()[0], 1)
        self.assertEqual(self.marker.read_text().splitlines(), [first.cursor, first.cursor])

    def test_supervisor_config_uses_same_opt_in_grant(self):
        source = self.source()
        supervisor = PluginSupervisor(self.store, (source,), backoff_seconds=0.05).start()
        try:
            deadline = time.monotonic() + 3
            while not self.snapshot()[1] and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertEqual(len(self.snapshot()[1]), 1)
            self.assertEqual(supervisor.snapshot()[0]["state"], "running")
        finally:
            supervisor.close()

    def test_missing_feature_or_malformed_reference_never_acknowledges(self):
        for mode in ("missing_hello", "missing_root_gap", "missing_envelope", "bad_schema",
                     "raw_text_field", "forged_hash"):
            with self.subTest(mode=mode):
                self.store = Store(self.root / f"store-{mode}.db")
                self.addCleanup(self.store.close)
                self.marker.unlink(missing_ok=True)
                source = self.source(mode)
                session = self.session(source)
                try:
                    if mode in ("missing_hello", "missing_root_gap"):
                        with self.assertRaisesRegex(ProtocolError, "handshake features"):
                            session.start()
                    else:
                        session.start()
                        with self.assertRaises(ProtocolError):
                            session.poll_once()
                    self.assertFalse(self.marker.exists())
                    self.assertEqual(self.snapshot()[0][0], "")
                    self.assertEqual(self.snapshot()[1], [])
                finally:
                    session.close()

    def test_metadata_mode_rejects_child_content_and_purges_on_downgrade(self):
        source = self.source()
        with self.session(source) as session:
            session.poll_once()
        self.assertEqual(len(self.snapshot()[1]), 1)
        metadata = self.source(capture="metadata-only")
        with self.session(metadata) as session:
            self.assertEqual(session.lease.content_fingerprint, "")
            self.assertEqual(self.snapshot()[1], [])
            with self.assertRaisesRegex(ProtocolError, "invalid batch fields"):
                session.poll_once()
        self.assertEqual(self.snapshot()[1], [])

    def test_live_opt_in_worker_is_fenced_by_metadata_downgrade(self):
        source = self.source()
        session = self.session(source).start()
        try:
            first = session.poll_once()
            self.assertEqual(len(self.snapshot()[1]), 1)
            metadata = self.source(capture="metadata-only")
            with self.session(metadata) as downgraded:
                self.assertEqual(downgraded.lease.content_fingerprint, "")
                with self.store._connect() as db:
                    self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_content_v1")
                                     .fetchone()[0], 0)
                    self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_content_batch_v1")
                                     .fetchone()[0], 0)
                with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
                    session.poll_once()
                self.assertEqual(self.marker.read_text().splitlines(), [first.cursor])
        finally:
            session.close()

    def test_config_rejects_unauthorized_content_modes(self):
        for kwargs in ({"replaces": "codex"}, {"root": False},
                       {"grants": "[]"},
                       {"features": ["source-adapter/1", "harness.pi/1",
                                     "source-discovery/1", "configured-root/1",
                                     "pi-content-ref/1"]},
                       {"capture": "metadata-only", "force_content_feature": True}):
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                self.source(**kwargs)

    def test_direct_session_cannot_mint_content_without_pi_replacement(self):
        source = self.source()
        unowned = replace(source, replaces=(),
                          required_features=(*source.required_features, "harness.pi/1"))
        session = self.session(unowned)
        with self.assertRaisesRegex(ProtocolError, "trusted directory configuration"):
            session.start()
        self.assertIsNone(session.lease)
        self.assertIsNone(self.snapshot()[0])

    def test_envelope_is_separately_versioned_and_bounded(self):
        ordinary = {"type": "batch", "protocol": 1, "source_id": "pi-directory",
                    "cursor": "", "records": []}
        message = {**ordinary, "content_refs": {"schema_version": 1, "refs": []}}
        with self.assertRaisesRegex(ProtocolError, "invalid batch fields"):
            validate_batch(message, source_id="pi-directory")
        self.assertEqual(validate_content_batch(message, source_id="pi-directory")
                         .content_refs, ())
        for envelope in ({"schema_version": 1, "refs": [{}] * 49},
                         {"schema_version": 1, "refs": [{"x": "x" * 65536}]},
                         {"schema_version": True, "refs": []},
                         {"schema_version": 1, "refs": [], "text": "forbidden"}):
            with self.subTest(envelope=str(envelope)[:50]), self.assertRaises(ProtocolError):
                validate_content_batch({**ordinary, "content_refs": envelope},
                                       source_id="pi-directory")

    def test_revocation_fences_reference_poll_and_deletes_content(self):
        source = self.source()
        with self.session(source) as session:
            session.poll_once()
            set_source_capture(self.store, source_id=source.source_id, enabled=False)
            self.assertEqual(self.snapshot()[1], [])
            with self.assertRaises(ProtocolError):
                session.poll_once()


if __name__ == "__main__":
    unittest.main()
