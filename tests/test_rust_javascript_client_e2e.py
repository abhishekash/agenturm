"""Verify the dependency-free Node producer against the standalone Rust receiver."""
from __future__ import annotations

import hashlib
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


@unittest.skipUnless(NODE, "install Node.js")
class JavaScriptTraceCredentialsTests(unittest.TestCase):
    def test_inherited_bearer_stays_bound_to_its_numeric_trace_endpoint(self):
        script = f"import {{ Client }} from {json.dumps(CLIENT)};\n" + """
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { once } from 'node:events';

const token = 'a1'.repeat(32);
const valid = 'Authorization=Bearer%20' + token;
const requests = [];
let malformed = false;
let redirect;
async function receiver() {
  const server = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    requests.push({
      port: server.address().port, path: request.url, headers: request.headers,
      body: Buffer.concat(chunks).toString('utf8'),
    });
    const reply = malformed ? 'reflected bearer ' + token : '{}';
    const headers = { 'content-length': Buffer.byteLength(reply) };
    if (redirect) headers.location = redirect;
    response.writeHead(redirect ? 307 : 200, headers);
    response.end(reply);
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  return server;
}
const primary = await receiver();
const alternate = await receiver();
const authority = server => `http://127.0.0.1:${server.address().port}`;
const source = authority(primary) + '/v1/traces';
process.env.OTEL_EXPORTER_OTLP_TRACES_ENDPOINT = source;
process.env.OTEL_EXPORTER_OTLP_TRACES_HEADERS = 'irrelevant=ignored,' + valid;
process.env.OTEL_EXPORTER_OTLP_HEADERS = 'authorization=Bearer%20' + 'c1'.repeat(32);
process.env.OBSERVATORY_TOKEN = 'legacy-private-marker';

async function send(options = {}) {
  const client = new Client({ transport: 'otlp', ...options });
  await client.trace({ sessionId: 'native-session' }, () => {});
  assert.ok(!JSON.stringify(client).includes(token));
  return client;
}
const last = () => requests.at(-1);
try {
  await send({ strict: true });
  assert.equal(last().port, primary.address().port);
  assert.equal(last().path, '/v1/traces');
  assert.equal(last().headers.authorization, 'Bearer ' + token);
  assert.equal(last().headers.irrelevant, undefined);
  assert.ok(!last().body.includes(token));
  await send({ endpoint: authority(primary), strict: true });
  assert.equal(last().headers.authorization, 'Bearer ' + token);
  await send({ endpoint: authority(alternate), strict: true });
  assert.equal(last().port, alternate.address().port);
  assert.equal(last().headers.authorization, undefined);
  await send({ token: 'explicit-option', strict: true });
  assert.equal(last().headers.authorization, 'Bearer explicit-option');

  for (const wrongHeader of [
    valid + ',authorization=Bearer%20' + 'b1'.repeat(32), valid + ',Authorization',
    valid + '%0A', valid + '%ZZ', valid.replace('%20', '+'), valid.replaceAll('a1', 'A1'),
    valid + ',' + 'x'.repeat(4096),
  ]) {
    process.env.OTEL_EXPORTER_OTLP_TRACES_HEADERS = wrongHeader;
    await send({ strict: true });
    assert.equal(last().headers.authorization, undefined);
  }
  delete process.env.OTEL_EXPORTER_OTLP_TRACES_HEADERS;
  await send({ strict: true });
  assert.equal(last().headers.authorization, undefined);

  for (const endpoint of [
    'http://127.1:14318', 'http://127.0.0.1:14318/V1/TRACES',
    'http://example.invalid:14318/v1/traces', 'http://0x7f000001:14318',
  ]) {
    assert.throws(() => new Client({ endpoint, transport: 'otlp', strict: true }), /loopback/);
  }
  process.env.OTEL_EXPORTER_OTLP_TRACES_HEADERS = valid;
  const beforeRedirect = requests.length;
  redirect = authority(alternate) + '/v1/traces';
  const redirected = await send();
  assert.equal(redirected.lastError.message, 'OTLP receiver returned HTTP 307');
  assert.equal(requests.length, beforeRedirect + 1);
  redirect = undefined;
  malformed = true;
  const client = await send();
  assert.equal(client.lastError.message, 'OTLP receiver returned an invalid reply');
} finally {
  primary.close();
  alternate.close();
}
"""
        result = subprocess.run(
            [NODE, "--input-type=module", "--eval", script],
            env=os.environ | {
                "NODE_USE_ENV_PROXY": "1",
                "HTTP_PROXY": "http://127.0.0.1:1", "http_proxy": "http://127.0.0.1:1",
                "NO_PROXY": "", "no_proxy": "",
            },
            capture_output=True, text=True, timeout=15,
        )
        self.assertEqual(result.returncode, 0, result.stderr)


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
    parent.setAttribute('gen_ai.usage.cost', 'synthetic private invalid amount');
    parent.setAttribute('gen_ai.usage.cost.currency', 'EUR');
    await run.span('child', child => {
      identities.child = child.span_id;
      child.setAttribute('agenturm.source.id', 'harness.unverified');
      child.setAttribute('agenturm.turn.id', 'unverified-turn');
      child.setAttribute('agenturm.worker.id', 'unverified-worker');
      child.setAttribute('llm.cost_usd', '0.000000001');
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
        costs = [row["observation"] for row in rows if row["observation"]["kind"] == "reported_cost"]
        self.assertEqual(costs, [{
            "kind": "reported_cost", "currency": "USD", "nanounits": "1",
            "span": {"trace_id": identities["trace"], "span_id": identities["child"]},
        }])
        rows = [row for row in rows if row["observation"]["kind"] == "trace_span"]
        self.assertEqual(len(rows), 3)
        for row in rows:
            self.assertEqual(row["identity"]["session_id"], "node-session")
            self.assertIsNone(row["identity"].get("turn_id"))
            self.assertIsNone(row["identity"].get("worker_id"))
            self.assertEqual(row["observation"]["target_source_id"], "harness.node")
            self.assertEqual(row["observation"]["trace_id"], identities["trace"])
        spans = {row["observation"]["span_id"]: row["observation"] for row in rows}
        parent = spans[identities["parent"]]
        child = spans[identities["child"]]
        self.assertEqual(child["parent_span_id"], identities["parent"])
        self.assertEqual(child["status"], "ok")
        self.assertIsNone(spans[parent["parent_span_id"]]["parent_span_id"])

    def test_explicit_turn_and_worker_reopen_with_exact_native_scope(self):
        binaries = Path(RUST_APP).resolve().parent
        plugin_host = Path(os.getenv("AGENTURM_RUNTIME_PLUGIN_HOST", binaries / "agenturm-plugin-host"))
        pi_adapter = Path(os.getenv("AGENTURM_RUNTIME_PI_ADAPTER", binaries / "agenturm-pi-harness"))
        if not plugin_host.is_file() or not pi_adapter.is_file():
            self.skipTest("build the Rust plugin host and Pi adapter for native turn fixtures")
        adapter = self.root / "agenturm-pi-harness"
        shutil.copyfile(pi_adapter, adapter)
        adapter.chmod(0o755)
        catalog = self.root / "catalog.json"
        catalog.write_text(json.dumps({"version": 1, "plugins": [{
            "plugin_id": "example.rust-pi-harness",
            "adapter": str(adapter),
            "sha256": hashlib.sha256(adapter.read_bytes()).hexdigest(),
            "protocol": 1,
            "features": ["source-adapter/1", "configured-root/1"],
            "observation_format": "agenturm-observations/1",
        }]}))
        sources = []
        for source_id in ["harness.node", "harness.sibling"]:
            source = self.root / f"{source_id}.jsonl"
            rows = [
                {"type": "session", "id": source_id,
                 "sessionId": "node-worker-session", "agentId": "worker.one"},
                *[
                    {"type": "message", "id": turn, "timestamp": f"2026-11-20T12:00:0{index}Z",
                     "message": {"role": "user", "content": "Synthetic native turn"}}
                    for index, turn in enumerate(["turn.one", "turn.two"])
                ],
            ]
            source.write_text("".join(json.dumps(row) + "\n" for row in rows))
            sources.append({"source_id": source_id, "source_root": str(source),
                            "plugin_id": "example.rust-pi-harness"})
        config = self.root / "sources.json"
        config.write_text(json.dumps({"version": 2, "plugin_host": str(plugin_host.resolve()),
                                      "plugin_catalog": str(catalog), "sources": sources}))
        collected = subprocess.run(
            [RUST_APP, "collect", "--config", config, "--db", self.database],
            capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(collected.returncode, 0, collected.stderr)
        self.assertEqual(collected.stdout.count("ACK sent=true"), 2, collected.stdout)

        self.write_producer("""
const client = new Client({ transport: 'otlp', strict: true });
const scope = {
  sessionId: 'node-worker-session', harnessId: 'harness.node',
  turnId: 'turn.one', workerId: 'worker.one',
};
for (const invalid of [
  {sessionId: undefined}, {harnessId: undefined}, {turnId: undefined},
  {turnId: 'invalid\\n'}, {turnId: 42}, {workerId: 'x'.repeat(513)},
]) {
  try {
    await client.trace({...scope, ...invalid}, () => {
      throw new Error('invalid scope reached the callback');
    });
    throw new Error('invalid scope was accepted');
  } catch (error) {
    if (!(error instanceof TypeError)) throw error;
  }
}
const traces = {};
traces.exact = await client.trace(scope, async run => {
  await run.span('parent', async () => {
    await run.span('child', child => {
      child.setAttribute('agenturm.turn.id', 'unverified-turn');
      child.setAttribute('agenturm.worker.id', 'unverified-worker');
    });
  });
  return run.trace_id;
});
for (const [key, fields] of Object.entries({
  otherTurn: {turnId: 'turn.two'},
  otherWorker: {workerId: 'worker.two'},
  mainWorker: {workerId: undefined},
  otherSession: {sessionId: 'other-session'},
  otherSource: {harnessId: 'harness.sibling'},
  sessionOnly: {turnId: undefined, workerId: undefined},
})) {
  traces[key] = await client.trace({...scope, ...fields}, run => run.trace_id);
}
console.log(JSON.stringify(traces));
""")
        completed = subprocess.run(
            [RUST_APP, "run", "--db", self.database, "--source-id", "otlp.node",
             "--", NODE, self.producer],
            capture_output=True, text=True, timeout=15,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        traces = json.loads(completed.stdout)
        observations = self.observations("otlp.node")
        self.assertEqual(len(observations), 9)
        selected = [
            row for row in observations if row["observation"]["trace_id"] == traces["exact"]
        ]
        self.assertEqual(len(selected), 3)
        for row in selected:
            self.assertEqual(row["identity"]["session_id"], "node-worker-session")
            self.assertEqual(row["identity"]["turn_id"], "turn.one")
            self.assertEqual(row["identity"]["worker_id"], "worker.one")
            self.assertEqual(row["observation"]["target_source_id"], "harness.node")

        def review(*options, source="harness.node", session="node-worker-session"):
            return subprocess.run(
                [RUST_APP, "review", "--db", self.database, "--harness-id", source,
                 "--session-id", session, "--format", "json", *options],
                capture_output=True, text=True, timeout=10,
            )

        def read_review(result):
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)

        def linked(turn, worker=None):
            worker_args = ["--worker-id", worker] if worker else []
            receipt = read_review(review("--turn-id", turn, "--linked-traces", *worker_args))
            return [row["observation"]["trace_id"] for row in receipt["observations"]]

        self.assertEqual(linked("turn.one", "worker.one"), [traces["exact"]] * 3)
        self.assertEqual(linked("turn.two", "worker.one"), [traces["otherTurn"]])
        self.assertEqual(linked("turn.one", "worker.two"), [])
        self.assertEqual(linked("turn.one"), [])

        exact_trace = ["--turn-of-trace", traces["exact"], "--trace-source-id", "otlp.node"]
        resolved = read_review(review(*exact_trace))
        self.assertEqual(resolved["scope"]["turn_id"], "turn.one")
        self.assertEqual(resolved["scope"]["worker_id"], "worker.one")
        self.assertEqual(
            {row["observation"]["kind"] for row in resolved["observations"]},
            {"lifecycle", "message"},
        )
        for row in resolved["observations"]:
            self.assertEqual(row["harness_id"], "harness.node")
            self.assertEqual(row["identity"]["turn_id"], "turn.one")
            self.assertEqual(row["identity"]["worker_id"], "worker.one")
        for denied in [
            review(*exact_trace, source="harness.sibling"),
            review(*exact_trace, session="other-session"),
            review(*exact_trace, "--worker-id", "worker.two"),
            review("--turn-of-trace", traces["sessionOnly"], "--trace-source-id", "otlp.node"),
        ]:
            self.assertNotEqual(denied.returncode, 0, denied.stdout)
            self.assertEqual(denied.stdout, "")

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
