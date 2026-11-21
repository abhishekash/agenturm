# Rust prompt signals process extension

This independently built executable demonstrates Agenturm's one-shot process
extension protocol. It links `agenturm-protocol` for framing, not
`agenturm-kernel`, and returns bounded metrics plus a data-only pane. The
signals summarize recorded message roles and token counters; they are not a
prompt efficacy score and do not establish causality.

## Build and select it

```sh
cargo build --release --manifest-path examples/rust_prompt_signals_extension/Cargo.toml
sha256sum examples/rust_prompt_signals_extension/target/release/agenturm-prompt-signals-extension
```

Create a user-managed catalog with the executable's absolute path and the
lowercase SHA-256 printed above:

```json
{
  "version": 1,
  "extensions": [
    {
      "plugin_id": "example.prompt-signals",
      "executable": "/absolute/path/agenturm-prompt-signals-extension",
      "sha256": "<64 lowercase hex characters>",
      "protocol": 1,
      "features": ["capability/1", "ui-contribution/1"],
      "grants": ["usage", "message-role"]
    }
  ]
}
```

Select it explicitly in the read-only inspector:

```sh
agenturm inspect --db ./agenturm.sqlite \
  --extension-catalog ./agenturm-extensions.json \
  --extension example.prompt-signals
```

## Data boundary

The host sends source ID, a decimal nanosecond timestamp, observation kind,
snapshot ID, page completeness, and only typed fields allowed by the catalog
grants. This example requests `usage` and `message-role`. It receives no event,
revision, session, worker, turn, or operation IDs; no prompt text, command,
tool output, paths, source root, or arbitrary adapter payload is serialized.
The mandatory source/time/kind envelope is still visible for rows whose typed
payload is filtered out.

The process runs as the Agenturm user. Grants limit protocol data only; they do
not prevent the executable from independently reading files available to that
OS user. SHA-256 pins detect changed artifacts but do not sandbox the process.
See [the process extension RFC](../../docs/rfcs/rust-process-extension-plugins.md)
for framing, bounds, failure behavior, and the limits of process-group cleanup.
