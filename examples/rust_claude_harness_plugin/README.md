# Install the opt-in Rust Claude Code reader

Build the Rust executable, then install a SHA-pinned copy for one explicit main transcript file:

```sh
cargo build --locked --release -p agenturm-claude-harness
python examples/rust_claude_harness_plugin/install.py \
  --binary /absolute/target/release/agenturm-claude-harness \
  --destination /absolute/private/plugin-dir \
  --source /absolute/claude-session.jsonl
agenturm serve --no-local-agents \
  --plugin-config /absolute/private/plugin-dir/plugins.toml
```

The installer refuses existing plugin files and symlinked destination components, copies the executable and records its SHA-256. Installation and manifest reading do not launch the executable. The process starts only through the explicit receiver configuration. `--no-local-agents` prevents a second native reader in this demonstration, but also disables all other native local-agent discovery for that receiver. This example does not replace the native Claude Code reader by default or enable automatic project discovery. The [validation checkpoint](../../docs/validation/rust-claude-reader.md) records supported facts and incomplete coverage.
