# Using Agenturm

Agenturm is a native Rust terminal monitor for local AI-agent sessions and
OpenTelemetry data.

## Start the monitor

Install Agenturm using the [installation guide](install.md), then run:

```sh
agenturm
```

The monitor discovers supported local Codex, Claude Code, OpenCode, Pi, and
Kiro sessions. It reads their existing records; it does not launch or control
those agents. Press **F1** in the monitor for keyboard help.

## Collect OpenTelemetry from a process

`agenturm run` starts a local receiver, configures standard OTLP/HTTP
environment variables for the selected process, and stores the received data
in a SQLite database:

```sh
agenturm run --db ./history.sqlite --source-id otlp.example -- ./your-agent
```

The selected program must already use an OpenTelemetry SDK and OTLP exporter.
The launcher does not inject an SDK or modify the program. It forwards the
program's standard input, output, exit status, and termination signals.

To send data to an already-running authenticated receiver instead:

```sh
agenturm run \
  --endpoint http://127.0.0.1:14318 \
  --token-file /path/to/receiver.token \
  -- ./your-agent
```

The receiver owns storage and source identity. See the [receiver guide](receiver.md)
for authentication, JSONL watching, migration, and retention details.

## Privacy

Native session records and telemetry can include prompts, commands, paths, and
tool results. Keep the local database private and review evidence before
sharing it. The [privacy guide](privacy.md) describes collection bounds,
redaction, exports, and retention limits.

## Releases

The [changelog](../CHANGELOG.md) summarizes changes. Versioned notes for past
releases are kept in [`docs/releases`](releases/).
