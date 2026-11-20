# Codex directory root gaps

Recorded 20 November 2026. The opt-in Rust Codex directory reader now treats failed bounded enumeration of its configured root as a source-wide continuity gap. The Python Store remains the sole writer, policy authority, and acknowledgment authority. This change does not claim a performance gain.

## Failure and recovery contract

The reader reports the first missing, unreadable, symlinked, or over-budget directory enumeration as one `possible_source_gap` diagnostic without a member ID. Its `d2` cursor records the outage in the same Store transaction as the diagnostic and source-wide invalidation, before acknowledgment. Existing projected sessions, including those without retained evidence, become Unknown with `source_gap`; their usage, relationships and Activity are suppressed. This deliberately quarantines healthy files too: while the root is inaccessible, the reader cannot prove which prior members remained continuous.

The worker retains the per-file checkpoints. While the root remains unavailable, a resumed `d2` poll errors, so supervisor health becomes degraded and it does not emit duplicate gap evidence. The supervisor quarantines after its configured failure limit; an operator must restart it if the root recovers after that limit. On the first successful full bounded enumeration, the cursor returns to `d1`, including when no file changed. The existing member checkpoints are then checked against current files and their acknowledged prefixes. Old invalidated session IDs remain Unknown; newly admitted session IDs can appear. A later outage can generate a fresh gap for sessions admitted since recovery.

The directory gap diagnostic's `time_ns` is a cursor-derived logical sequence, **not** the wall-clock time of failure. It makes regenerated root and member gap batches byte-for-byte replayable after a commit/ack crash. Store admission time and supervisor failure time carry the receipt clocks. Agenturm exposes the diagnostic value as raw metadata and does not format it as an occurrence time in the current UI.

The directory installer requires `source-discovery-root-gap/1` in addition to `source-discovery/1`. The process transport accepts a memberless gap only for that explicitly configured and negotiated feature. A plugin without it cannot commit the root-gap batch. Ordinary single-file process sources retain their existing source-wide gap behavior.

## Verification

The Rust unit tests cover byte-for-byte replay of root and member gaps, one-shot outage evidence, repeated failure, retained member checkpoints and idle recovery. Installed reader tests cover durable source-wide invalidation, degraded and recovered coverage, a new session after recovery, restart during outage, and rejection when the feature is omitted. Existing member-scoped tests ensure a missing file alone leaves its healthy siblings intact. A per-file open/read failure after successful enumeration still degrades health without durable invalidation; that continuity gate remains open. The result is a conservative root disposition; it does not prove root identity across mount replacement or establish a filesystem sandbox.

```sh
RUSTUP_HOME=/tmp/agenturm-rustup-home CARGO_HOME=/tmp/agenturm-cargo-home \
  PATH=/tmp/agenturm-cargo-home/bin:$PATH cargo test -p agenturm-codex-harness
AGENTURM_RUST_CODEX_BIN=/path/to/agenturm-codex-harness \
  uv run python -m unittest tests.test_rust_codex_directory_plugin -q
```

The default native reader cutover, full Review/export parity, Rust storage authority, and production performance acceptance remain open gates.
