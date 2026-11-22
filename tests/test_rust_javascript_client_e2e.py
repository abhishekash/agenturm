"""Verify the dependency-free Node producer against the standalone Rust receiver."""
from __future__ import annotations

import json
import os
import select
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


RUST_APP = os.environ.get("AGENTURM_RUST_APP_BIN")
NODE = shutil.which("node")
CLIENT = (Path(__file__).resolve().parents[1] / "clients/javascript/index.mjs").as_uri()


@unittest.skipUnless(RUST_APP and NODE, "set the standalone Rust binary and install Node.js")
class RustJavaScriptTraceTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.database = self.root / "history.sqlite"
        self.producer = self.root / "producer.mjs"

    def write_producer(self, code):
        self.producer.write_text(f"import {{ Client }} from {json.dumps(CLIENT)};\n{code}")

    def observations(self, source):
        query = subprocess.run(
            [RUST_APP, "query", "--db", self.database, "--harness-id", source, "--limit", "4096"],
            capture_output=True, text=True, check=True, timeout=10,
        )
        self.assertNotIn("synthetic private task", query.stdout)
        self.assertNotIn("synthetic private span", query.stdout)
        return json.loads(query.stdout)["observations"]

    def test_rust_run_keeps_nested_trace_scope_and_node_exit_status(self):
        self.write_producer("""
const client = new Client({ transport: 'otlp', strict: true });
const identities = {};
await client.trace({
  task: 'synthetic private task', sessionId: 'node-session', harnessId: 'harness.node',
  attributes: { 'session.id': 'conflicting-unverified-attribute' },
}, async run => {
  identities.trace = run.trace_id;
  await run.span('synthetic private span', async parent => {
    identities.parent = parent.span_id;
    await run.span('child', child => {
      identities.child = child.span_id;
      child.setAttribute('agenturm.source.id', 'harness.unverified');
      child.setStatus('OK');
    });
  });
});
console.log(JSON.stringify(identities));
process.exitCode = 23;
""")
        environment = os.environ | {
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": "http://invalid.example/v1/traces",
            "OTEL_EXPORTER_OTLP_HEADERS": "authorization=must-not-inherit",
        }
        result = subprocess.run(
            [RUST_APP, "run", "--db", self.database, "--source-id", "otlp.node",
             "--", NODE, self.producer],
            env=environment, capture_output=True, text=True, timeout=15,
        )
        self.assertEqual(result.returncode, 23, result.stderr)
        identities = json.loads(result.stdout)
        rows = self.observations("otlp.node")
        self.assertEqual(len(rows), 3)
        for row in rows:
            self.assertEqual(row["identity"]["session_id"], "node-session")
            self.assertEqual(row["observation"]["target_source_id"], "harness.node")
            self.assertEqual(row["observation"]["trace_id"], identities["trace"])
        spans = {row["observation"]["span_id"]: row["observation"] for row in rows}
        parent = spans[identities["parent"]]
        child = spans[identities["child"]]
        self.assertEqual(child["parent_span_id"], identities["parent"])
        self.assertEqual(child["status"], "ok")
        self.assertIsNone(spans[parent["parent_span_id"]]["parent_span_id"])

    def test_standalone_receiver_rejects_unscoped_spans_and_accepts_bounded_batches(self):
        self.write_producer("""
const client = new Client({ transport: 'otlp' });
await client.trace(async run => {
  if (await run.progress('ignored')) throw new Error('progress unexpectedly exported');
  if (!client.lastError?.message.includes('does not export progress')) {
    throw new Error('progress was not refused by the client');
  }
});
if (!client.lastError?.message.includes('rejected one or more spans')) {
  throw new Error('unscoped spans were not reported as rejected');
}
await client.trace({ sessionId: 'large-session' }, async run => {
  for (let i = 0; i < 1024; i += 1) await run.span('bounded-child', () => {});
});
if (client.lastError) throw client.lastError;
""")
        receiver = subprocess.Popen(
            [RUST_APP, "serve-traces", "--listen", "127.0.0.1:0", "--db", self.database,
             "--source-id", "otlp.standalone"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        try:
            self.assertTrue(select.select([receiver.stdout], [], [], 8)[0])
            announcement = receiver.stdout.readline()
            self.assertTrue(announcement.startswith("Listening for OTLP traces at "))
            endpoint = announcement.rsplit(" ", 1)[-1].strip()
            result = subprocess.run(
                [NODE, self.producer],
                env=os.environ | {"OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": endpoint},
                capture_output=True, text=True, timeout=15,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
        finally:
            if receiver.poll() is None:
                receiver.terminate()
            try:
                receiver.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                receiver.kill()
                receiver.communicate()
        self.assertEqual(receiver.returncode, 0)
        rows = self.observations("otlp.standalone")
        self.assertEqual(len(rows), 1025)
        self.assertEqual({row["identity"]["session_id"] for row in rows}, {"large-session"})
        spans = [row["observation"] for row in rows]
        root = next(span for span in spans if span["parent_span_id"] is None)
        self.assertEqual(
            {span.get("parent_span_id") for span in spans}, {None, root["span_id"]}
        )


if __name__ == "__main__":
    unittest.main()
