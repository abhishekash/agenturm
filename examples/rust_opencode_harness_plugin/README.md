# Opt-in Rust OpenCode SQLite reader

`agenturm-opencode-harness` reads one explicitly configured OpenCode SQLite database through the shared framed source-process runner. It recognizes the current `session_v2`, `session_message`, `session_part` layout represented by the repository's OpenCode 2.2.5 synthetic schema fixture. The built-in OpenCode reader stays the default. Installing this adapter does not replace it.

Build and install a locally built, SHA-pinned copy:

```sh
cargo build --locked --release -p agenturm-opencode-harness
python examples/rust_opencode_harness_plugin/install.py \
  --binary /absolute/path/to/target/release/agenturm-opencode-harness \
  --destination /absolute/path/to/opencode-plugin \
  --source /absolute/path/to/opencode.db
agenturm serve --plugin-config /absolute/path/to/opencode-plugin/plugins.toml
```

The installer pins one executable and one absolute database path. The Python receiver checks the manifest, artifact hash, grant, capture policy and source lease; it alone writes admitted evidence and projections. The Rust child opens SQLite read-only with `query_only`, reads bounded message and part rows in a transaction, and sends only role, coarse tool category/result, usage counters, lifecycle state and reason, plus opaque session and tool-operation identities. It does not send prompt text, assistant prose, command strings, paths, tool arguments, outputs or model names.

Current-schema `idle` rows with a recognized outcome provide completion, failure or interruption. The latest user or assistant message keeps the session running. An assistant `finish: stop`, tool completion, PR text, source silence and end of a sweep do not complete a session. The adapter omits process turn IDs because a later idle backfill can change a previously admitted turn boundary. The v2 durable cursor pins finite rowid high watermarks for message, part and session sweeps, with ctime-aware file marks and a bounded prior-row identity digest. An old v1 cursor or a lost/replaced row identity conservatively marks prior sessions Unknown/source_gap. Each DB/WAL file is capped at 512 MiB and the combined table row count at 50,000; over-limit sources fail before acknowledgment. Changes start a new revision sweep after the current sweep finishes, so continuing appends do not starve old rows or parts. The source database is never updated by adapter SQL. WAL mode requires existing `-wal` and `-shm` sidecars; the child refuses a WAL database without them, avoiding creation. SQLite may coordinate through an existing SHM file. See [validation and limits](../../docs/validation/rust-opencode-reader.md).

This is partial, metadata-only coverage. It does not discover the OpenCode database, map older SQLite layouts, retain content, reproduce native spans and Review proof, or replace the built-in reader at parity. An in-place idle outcome change revises the stable session status fact. Reduced table counts emit a source-gap signal that makes prior source sessions Unknown and hides their old Activity, including after a deleted terminal idle. Same-count delete-and-insert changes still need a stronger retraction contract. Source change detection uses main and WAL file metadata, so equal-size/equal-time rewrites can escape detection. No performance gain is asserted.
