# Rust typed tool observation validation

Date: 21 November 2026

## Scope

The shared Rust observation path now retains a deliberately small, privacy-safe tool fact: category (`shell`, `file`, `mcp`, or `other`), result (`started`, `succeeded`, `failed`, or `denied`), and native operation identity. Harness adapters still own parsing and provider-specific identity generation. The generic decoder does not retain tool names, arguments, commands, or outputs.

The fact flows through the bundled decoder, kernel validation/admission, SQLite store, store reopen, bounded query JSON, and the generic overview count. The store format is v4. Existing v2 stores upgrade through v3 and v4; existing v3 stores upgrade to v4. The v3-to-v4 migration changes the marker/version transactionally and retains the v3 durable history table and receipt sequence.

## Validation

All commands ran in `/tmp/agenturm-rust-tool-observations` on the Rust toolchain configured in `/tmp/agenturm-cargo`.

- `cargo fmt --all -- --check`
- `cargo test --offline --locked --workspace --quiet`
- `cargo build --offline --locked --workspace`
- Real-process integration tests with the Rust plugin host, Codex adapter, and Pi adapter configured:
  `cargo test --offline --locked -p agenturm-runtime --test codex_process --test pi_process -- --nocapture`
- Independent review of the branch diff: no actionable blockers reported.

The process tests cover failed initial commit and retry, ACK ordering, persisted usage plus tool facts, and reopened-store recovery. They assert opaque operation identity survives and that fixture command/output strings do not appear in the serialized stored observations. The app query test checks typed tool fields in JSON; store tests cover read-only reopen and both v2→v4 and v3→v4 migration results.

## Limits

This implements the tool fact subset only. Lifecycle, relationships, checks, coverage-gap reporting, content-grant behavior, full supported-harness parity, and the wider persistence/recovery gate remain open. No performance gain is claimed. The real-process checks described here were run locally; this report does not claim they are already in CI.
