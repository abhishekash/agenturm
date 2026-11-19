# Fake inbound connector

This is a separately installed, read-only connector proof. Its single-file Python executable imports no Agenturm package module. It calls a **local fixture service** at an exact `http://127.0.0.1:<port>/events` address, maps bounded lifecycle and usage records to Agenturm's process source protocol, and lets the host commit them under its own source lease and capture policy. The executable and manifest are inert until a receiver is started with the generated plugin config. It is not a general network connector, credential manager, or outbound action plugin.

Install from the repository into an empty absolute directory while a local fixture service is available:

```sh
python3 examples/fake_inbound_connector/install.py \
  --destination /absolute/path/to/inbound-plugin \
  --endpoint http://127.0.0.1:8765/events
agenturm serve --plugin-config /absolute/path/to/inbound-plugin/plugins.toml
```

The installer copies the executable, pins its SHA-256 in a manifest, writes a local endpoint config and an explicit source configuration. It refuses linked destination paths and entries, including dangling links. It does not start code or change a database. The manifest declares `source-adapter/1`, `configured-root/1`, `connector.inbound/1` and the versioned `connector-adapter/1` contract, with `configured-source-read` and `loopback-http-read` grants. The receiver config requires `connector-adapter/1`, so a missing child handshake feature prevents polling. See the [connector protocol](../../docs/inbound-connector-protocol.md). The current host treats grants as admission declarations; the child runs with the user's OS permissions, so these names are **not an OS network sandbox**. The child itself rejects non-loopback endpoints, redirects, credentials in URLs and paths other than `/events`.

## Fixture service contract

`GET /events?after=<token>` returns at most 256 metadata records:

```json
{"records":[{"id":"start-1","session":"demo-1","at_ns":"100","kind":"lifecycle","state":"started"}],"next":"r1:1"}
```

The `next` token is opaque to the connector but must be a bounded ASCII token and must advance when records are returned. An empty page with the same token is idle. Distinct record IDs are distinct occurrences. A correction repeats an ID with a higher integer `revision`; usage under different IDs is additive. Input accepts only typed lifecycle or usage metadata. Content fields, unknown fields, oversized pages, malformed JSON and duplicate keys stop this worker without acknowledging a cursor.

`429` with a bounded numeric `Retry-After` causes an idle poll until the next eligible time, without blocking the receiver thread. A process restart may retry sooner; the fixture's rate limit remains authoritative. `410` for a prior cursor emits a durable `cursor_expired` warning and advances to a gap checkpoint. After that checkpoint is acknowledged, the next poll starts from the service's first currently available page. The warning's timestamp is the last source event time encoded in the old cursor, **not the time expiry was detected**. The host preserves earlier facts with partial coverage; it does not infer that missing records were recovered. If even a fresh cursor returns `410`, the worker fails visibly rather than claiming coverage.

The process holds one bounded HTTP request per poll, follows no redirects, reads at most 1 MiB, and never sends credentials or mutations. Agenturm's current source cap of 50,000 distinct evidence records still applies; rollover evicts old evidence with an explicit retention gap, so it cannot certify complete remote history. Production connector design still needs credential resolution, endpoint identity pinning, broader endpoint policy, source-specific retention guarantees and rate-limit budgets shared across workers. Outbound delivery has a distinct fixture contract; this inbound process never performs it.
