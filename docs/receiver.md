# Receiver and OTLP

Start the local OTLP/HTTP receiver for the terminal cockpit:

```bash
agenturm serve --port 14318
```

The receiver defaults to `$XDG_STATE_HOME/agenturm/observatory.db`, or `~/.local/state/agenturm/observatory.db` when XDG state is unset (Windows: `%LOCALAPPDATA%/agenturm`). `--db` keeps an explicit path. The state directory and new database are owner-only. Its root address is a plain-text status message; inspect sessions in `agenturm`. It accepts OTLP traces, logs, and metrics at `/v1/traces`, `/v1/logs`, and `/v1/metrics` in protobuf or JSON.

Point an OpenTelemetry SDK at the receiver:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:14318
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
# Configure your exporter with Authorization: Bearer VALUE.
# VALUE is in the private agenturm/receiver-14318.token state file,
# or is the OBSERVATORY_TOKEN you explicitly configured.
```

The terminal cockpit starts a receiver if needed, and can also attach to an existing one:

```bash
agenturm cockpit --endpoint http://127.0.0.1:14318
```

CLI receivers authenticate by default. A generated owner-only `receiver-PORT.token` lives beside the managed database. The cockpit, Python launcher/SDK and Node client discover it for local endpoints. `OBSERVATORY_TOKEN` or an explicit SDK token overrides discovery. Generic OTLP exporters must send `Authorization: Bearer VALUE`; they do not automatically read Agenturm's token file. `serve --no-auth` explicitly disables bearer authentication while retaining Host/Origin validation.

Requests require a loopback Host or an explicitly configured `--allowed-host`; browser Origins must match that authority. Receiver clients refuse redirects. For remote access, configure an explicit token, `--allowed-host exact.hostname`, TLS and a trusted reverse proxy. The token does not isolate other programs running as the same OS user, and the receiver does not provide multiuser authorization.

The built-in receiver and cockpit receiver admit at most 32 concurrent request
workers. Excess connections receive HTTP 503 with `Retry-After: 1`; exporters
should retry with backoff. Requests have absolute header and body read deadlines
of 10 and 15 seconds, respectively, plus 5-second socket/write timeouts. Stalled
or slow-drip reads receive HTTP 408 when a response can still be sent. Shutdown
interrupts active connections before closing storage. These limits do not make
the local collector an internet-facing service.

### JSONL file watching

Use `--watch-jsonl path/to/spans.jsonl` to follow native span records. Each poll
reads at most 8 MiB; a large backlog is processed over successive polls. Records
larger than 2 MiB are dropped through their terminating newline, including when
the record spans multiple polls. Keep one complete JSON object per line.

### Storage migration and retention

Existing `./observatory.db` is preserved. Agenturm prints a notice when it detects that legacy path; it never silently relocates or prunes it. Continue with `agenturm --db ./observatory.db`, or copy it explicitly:

```bash
agenturm migrate-db ./observatory.db
# Optional: --destination /private/path/history.db
```

Migration uses SQLite backup, includes committed WAL data, checks integrity, refuses existing targets/sidecars, and leaves the source in place. A copy gets a new receiver namespace plus source provenance. To keep the old namespace and acknowledgement identity, stop the old receiver first and use `--preserve-identity --stopped-receiver`; that flag attests the old receiver will stay stopped. Do not run both copies with the same identity.

Only a genuinely new database created at the managed default location automatically receives **14-day retention**. An existing or explicitly selected database is not newly enrolled, including after relocation. Explicit retention persists in database metadata and runs in both cockpit-owned and standalone receivers. SQLite history migration happens in resumable background batches; authenticated health reports indexing stage/progress, and data endpoints return 503 until the initial projection is ready. Stop older receiver versions before upgrading a shared database. Source-change triggers queue compatible older-writer invalidations rather than returning stale indexed text.


```bash
agenturm serve --retention-days 30
agenturm prune --older-than-days 30
```

For complete OTLP field mappings, APIs, collector configuration, and examples, see the [full guide](usage.md).
For framework-specific connection examples, see [Connect agent frameworks](frameworks.md).

Retention deletes whole inactive traces according to their latest recorded activity and older unlinked telemetry. It is an age policy, not a hard byte cap or secure erasure. SQLite can retain allocated free pages after deletion; it does not automatically compact an existing database. Saved tasks and workflow receipts have separate storage/retention.

`cockpit --once` waits up to 15 seconds for initial discovery/index readiness. Exit 0 means a ready snapshot, including a known empty store; 2 means an input/read failure; 3 means the readiness deadline expired; 130 means interruption. Non-ready output does not print false zero totals. A large first upgrade can take longer than this deadline; the receiver reports progress and resumes interrupted work.
