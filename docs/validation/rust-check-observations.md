# Rust check observation validation

Date: 21 November 2026

## Scope

The shared decoder now retains Codex check observations as a source-neutral typed fact: category (`test`, `lint`, or `build`) and result (`passed`, `failed`, or `unknown`). The decoder rejects unknown fields and invalid category/result values. Command strings, output, and raw error messages are not stored.

Check facts pass through runtime decoding, kernel validation, transactional SQLite persistence, read-only reopen, query JSON, and the Overview check count. The Codex process test uses a completed `npm run check` command with private output and verifies the typed lint/passed fact survives while the command and output do not.

The store format is v6. Existing v2, v3, v4, and v5 stores upgrade through transactional marker/version changes and validate each intermediate shape. Check facts from before v6 cannot be reconstructed because they were not previously stored and the source cursor has advanced.

## Validation

- `cargo fmt --all -- --check`
- `cargo test --offline --locked --workspace --quiet`
- `cargo build --offline --locked --workspace`
- Configured real-process Codex and Pi tests with the Rust plugin host: `cargo test --offline --locked -p agenturm-runtime --test codex_process --test pi_process -- --nocapture`
- Configured multi-harness and watch lifecycle tests with the Rust plugin host, Pi adapter and Codex adapter: `cargo test --offline --locked -p agenturm-app --test configured_collection`
- Strict workspace Clippy: `cargo clippy --offline --locked --workspace --all-targets -- -D warnings`
- Independent Rust review: no blockers; reviewer recommended explicit invalid-category cases at decoder, kernel, and store boundaries. Those cases are now covered.

Tests cover typed check decoding, rejection of unsupported values and unexpected fields, admission/store validation, store close/reopen, v5-to-v6 and older migrations, query serialization, Overview count, and privacy assertions on real-process data.

## Limits

Only the Codex adapter currently emits these check facts. The projection records one coarse category/result pair; it does not preserve command details, individual test cases, coverage gaps, or richer failure explanations. Complete harness/domain parity remains open. No performance gain is claimed.
