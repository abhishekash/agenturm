# Rust configured harness collection

The Rust CLI can load an explicit version 1 JSON configuration and poll up to
16 process adapters in one kernel refresh:

```sh
agenturm collect --config ./agenturm-sources.json --db ./agenturm.sqlite
```

The configuration enumerates plugin identities and executable paths. Paths may
be absolute or relative to the configuration file's directory. Relative paths
are resolved before any child process starts.
Source IDs must be unique. Existing adapter and source-root paths are
canonicalized when checking ownership; missing paths use lexical normalization.
The same normalized adapter plus source-root pair cannot be listed twice under
different IDs.

```json
{
  "version": 1,
  "plugin_host": "/opt/agenturm/bin/agenturm-plugin-host",
  "sources": [
    {
      "plugin_id": "example.rust-pi-harness",
      "adapter": "/opt/agenturm/bin/agenturm-pi-harness",
      "source_root": "/home/alex/.pi/agent/sessions",
      "source_id": "harness.pi"
    },
    {
      "plugin_id": "example.rust-codex-harness",
      "adapter": "/opt/agenturm/bin/agenturm-codex-harness",
      "source_root": "/home/alex/.codex/sessions",
      "source_id": "harness.codex"
    }
  ]
}
```

Collection performs one bounded poll per configured source. The kernel validates
each source batch, writes it to the Rust SQLite store, and ACKs only after the
durable commit. A startup or poll failure for one source is reported in the
summary while healthy configured sources continue. Any source failure makes
the command exit nonzero; successful sources still retain their committed
observations. A durable history read failure is reported as a storage failure
and also makes the command exit nonzero, even if source commits and ACKs already
succeeded. The JSON file is a user-managed configuration input. The command
does not run continuously, scan plugin directories, or load in-process native
libraries.

For this first configured command, adapters must emit Agenturm's shared bounded
source protocol, and the app installs `UsageBatchDecoder`. It stores token
usage observations and counts other supported content-free facts as skipped.
The runtime API accepts injected `SourceBatchDecoder` implementations for other
typed fact projections; registering those decoders in a general plugin catalog
is a later step.
