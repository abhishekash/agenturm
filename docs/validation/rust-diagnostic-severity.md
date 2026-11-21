# Rust diagnostic severity validation

Date: 21 November 2026

## Scope

Kiro emits an identity-bearing `kiro_display_error` diagnostic with severity `error`. The shared decoder previously accepted only `info` and `warning`, causing a valid Kiro batch containing this fact to be rejected. The decoder now accepts exactly `info`, `warning`, and `error`, while preserving strict payload field rejection.

The diagnostic code/severity is still validated and counted as an unprojected fact; this change does not persist the diagnostic or its source event identity. The actual Kiro adapter test verifies it emits the fixed code and severity without display-error text. The runtime test verifies the batch is accepted and rejects unknown severities and extra payload fields.

## Validation

- `cargo fmt --all -- --check`
- `cargo test --offline --locked --workspace --quiet`
- `cargo clippy --offline --locked --workspace --all-targets -- -D warnings`
- `cargo build --offline --locked --workspace`

## Limits

Kiro diagnostics are not yet durable query observations. Identity-less diagnostics from several adapters also need a source-scoped model before they can be persisted without inventing a session identity. No performance gain is claimed.
