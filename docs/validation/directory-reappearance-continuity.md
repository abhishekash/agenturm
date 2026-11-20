# Directory member reappearance continuity

Recorded 20 November 2026 at `bff19a5`. This is a focused regression checkpoint for the opt-in Codex and Pi Rust directory readers. It does not change the wire protocol, Store schema, default source selection or measured performance.

## What an acknowledged removal guarantees

When a previously admitted file disappears, the shared directory reader emits a deterministic `possible_source_gap` record with an empty member cursor. Python Store commits the global cursor, removes that member checkpoint and records a member-scoped source-gap tombstone in one transaction before acknowledging the batch. Its projection keeps the affected native session **Unknown** with reason `source_gap`. A healthy sibling remains at its last recorded outcome.

Recreating the file at the same relative path creates the same member ID and starts its local read from the beginning. If its header contains the old native session ID, neither identical re-read facts nor a worker restart clear the tombstone or restore Completed. A file with a genuinely new native session ID can establish that new session from its complete header; the old session remains Unknown. Store retains the original session-to-member binding and tombstone. Recovery of the old session's outcome requires a deliberate new source ID and a complete source rebaseline, not an automatic claim from the reincarnated path.

## Boundary that remains

Deletion and recreation entirely between two polls can be unobservable when the acknowledged path and prefix are restored. The reader verifies acknowledged prefixes and detects visible identity or byte changes, but a current filesystem snapshot cannot prove that no unseen intermediate file or event existed. A filesystem journal or another trusted source of durable creation history would be needed to prove that. The reader does not infer completion from an observed gap.

## Verification

Installed Codex and Pi tests each admit two completed sessions, acknowledge removal of one member, recreate its path with the same native identity, restart the worker, then recreate it with a new identity. They assert the old session remains `Unknown/source_gap`, the sibling stays Completed, the new session can be admitted, and the source-gap tombstone persists. These tests exercise the existing Store and source binaries without a Rust rebuild. The broader installed directory suites remain part of the gate.

```sh
AGENTURM_RUST_CODEX_BIN=/tmp/agenturm-rust-opencode-target/debug/agenturm-codex-harness \
  AGENTURM_RUST_PI_BIN=/tmp/agenturm-rust-opencode-target/debug/agenturm-pi-harness \
  AGENTURM_RUST_PLUGIN_HOST_BIN=/tmp/agenturm-rust-opencode-target/debug/agenturm-plugin-host \
  uv run python -m unittest tests.test_rust_codex_directory_plugin \
    tests.test_rust_pi_directory_plugin -q
```
