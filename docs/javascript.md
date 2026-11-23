# JavaScript client

The dependency-free client supports Node.js 18+.

From a repository checkout:

```bash
npm install ./clients/javascript
```

Example:

```js
import { Client } from '@agenturm/client';

const telemetry = new Client({ service: 'javascript-research-agent' });
await telemetry.trace(
  { task: 'Find current guidance', sessionId: 'thread-42' },
  async run => {
    await run.progress('planning', 'Choosing a search strategy', 10);
    return run.span('search', { attributes: { 'tool.name': 'web_search' } }, async () => {
      return searchWeb();
    });
  },
);
```

The client sends completed spans to the local receiver. By default, delivery is best effort so telemetry failures do not interrupt the agent; enable `strict: true` to raise delivery errors. See [all client options](usage.md).

## Send traces to standalone Rust

Choose the `otlp` transport when running against a standalone Rust receiver.
Set `sessionId` to the exact native session and `harnessId` to its collected
source ID to make the trace available from that session. The client attaches
those identities to every span in the trace. Add `turnId` for a collected
native turn with a Rust receiver that supports exact trace Review. It requires
both `sessionId` and `harnessId`. Add `workerId` when that turn belongs to
a worker. These options set `agenturm.turn.id` and `agenturm.worker.id`
on every span in the trace.

```js
import { Client } from '@agenturm/client';

const telemetry = new Client({ transport: 'otlp', strict: true });
await telemetry.trace(
  {
    sessionId: 'thread-42', harnessId: 'harness.codex',
    turnId: 'turn-7', workerId: 'worker-1',
  },
  async run => {
    await run.progress('planning', '', 0);
    await run.span('work', async span => {
      await doWork();
      span.setStatus('OK');
    });
  },
);
```

Launch the producer with the installed Rust app to configure its loopback
trace endpoint for this invocation.

```sh
agenturm run --db history.sqlite --source-id otlp.node -- node agent.mjs
```

Open the linked spans from an exact retained turn and worker in Rust Review.
For a main turn, omit `workerId` in the producer and `--worker-id` in Review.

```sh
agenturm review --db history.sqlite --harness-id harness.codex \
  --session-id thread-42 --turn-id turn-7 --worker-id worker-1 --linked-traces
```

To send from an existing process, start the receiver with
`agenturm serve-traces --listen 127.0.0.1:14318 --db history.sqlite --source-id otlp.node`.
Set `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://127.0.0.1:14318/v1/traces` for the
producer. Use a literal loopback IP. An explicit `endpoint` overrides the environment. With neither
`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` nor `OTEL_EXPORTER_OTLP_ENDPOINT` set,
the transport uses that same loopback address.

The broker launcher sets `OTEL_EXPORTER_OTLP_TRACES_HEADERS` and
`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` for its producer. The authorization
value travels directly to the matching numeric loopback trace endpoint.
For a separately started broker receiver, set both variables for the
authorized producer. Use `authorization=Bearer%20<token>` with the
receiver's 64-digit lowercase hexadecimal token.

OTLP/HTTP JSON exports trace IDs, parent relationships, timestamps, status
and optional session, harness, turn, and worker identity. Payloads use fixed span names
and omit custom attributes, events and task text. The OTLP receiver can
group unscoped traces under an `agenturm run` invocation. Supply a
`sessionId` to export from a standalone receiver.

`run.progress(stage, message, percent)` publishes the stage before the
trace's spans are flushed. For the Rust receiver, supply a public stage
identifier and omit `message` or pass an empty string to reach the
optional `percent` argument. Percent uses 0 to 100 and up to two decimal
places. An omitted percent stays unknown. The default `agenturm` transport
continues to send the legacy message to the Python receiver.

Node.js 22.23.4 and the published Rust 0.3.2 Linux x86-64 bundle verified
the launcher, durable parent and source/session identity, nonzero producer
exit status, and standalone partial rejection and 1,025-span export across
the receiver's bounded requests. The existing JavaScript-to-Python
integration test also passed.

The exact-turn integration test collects native Pi worker turns, launches a
Node producer, and reopens its trace in Rust Review. It checks that spans from
other workers, turns, sessions, and sources stay outside the selected scope.
