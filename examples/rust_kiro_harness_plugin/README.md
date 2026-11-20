# Opt-in Rust Kiro process reader

This separately installed example reads one actual Kiro JSONL transcript. Build the Rust executable locally, then pin it with the installer:

```sh
cargo build --release --locked -p agenturm-kiro-harness
python examples/rust_kiro_harness_plugin/install.py \
  --binary "$(pwd)/target/release/agenturm-kiro-harness" \
  --destination /absolute/private/kiro-plugin \
  --source /absolute/kiro/sessions/workspace/sess_123/messages.jsonl
agenturm serve --plugin-config /absolute/private/kiro-plugin/plugins.toml
```

The source can instead name one `sess_*/sub-executions/<worker>.jsonl`. The installer creates a private copied artifact, SHA-pinned manifest and explicit source config; it refuses existing files and symlinked destination components. It does not launch a plugin. No built-in Kiro reader is replaced by this example; run it on an isolated receiver or use a separately reviewed ownership switch before pointing two readers at the same source.

The process sends content-free lifecycle, message and tool facts. It represents one pending permission interaction as waiting until a matching resolution; decision details, Kiro credit amounts, rules, parent outcomes and full Review/export evidence are not represented yet. See the [validation report](../../docs/validation/rust-kiro-reader.md) for limits and tests.
