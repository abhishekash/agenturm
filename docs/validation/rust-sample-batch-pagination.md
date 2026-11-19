# Rust sample adapter batch pagination

Recorded 20 November 2026. The separately installed Rust JSONL sample adapter now stops a batch before a repeated event ID or the receiver's JSON structure limit. Python remains the authority for validation, Store commits, and acknowledgments. This change makes no performance claim.

## Failure and fix

When an original event and its same-ID correction were already in the source file before the first poll, the adapter put both in one batch. `plugin_wire.validate_batch` rejects duplicate `event_id` values in a batch, so the worker could not commit or acknowledge any part of that file.

The scan now tracks event IDs in the current batch. On a repeat it returns the records already collected and leaves the repeated line at the next cursor offset. A later poll, following the Store commit and acknowledgment, emits the correction. Repeated revisions page the same way. The scan also checks the serialized batch against the receiver's 8,192 structural-token ceiling as well as its byte and record ceilings. A record that cannot fit by itself still fails without cursor advancement.

## Validation

| Check | Result |
| --- | --- |
| Rust 1.100.0 `cargo fmt --all --check`, `cargo clippy -p agenturm-sample-harness --all-targets --locked -- -D warnings`, `cargo test -p agenturm-sample-harness --locked` | Passed. Unit coverage includes three revisions already present before first poll, a one-record limit, an exact byte limit, rejected undersized byte limit, and a 256-row full-usage backlog. |
| Rebuilt `agenturm-sample-harness` binary; `AGENTURM_RUST_SAMPLE_BIN=... python3 -m unittest tests.test_rust_sample_harness_plugin -v` | Seven passed through the installer, process protocol, durable Store, correction replay, projection and receiver. The new tests commit revisions across a worker restart and drain 256 valid usage rows over multiple acknowledgments. |

The sample adapter remains an opt-in JSONL example, not a native Pi or Eclipse reader. A malformed source line still stops polling at that line; source repair is required before the cursor can advance. This checkpoint does not change Python admission or storage authority.
