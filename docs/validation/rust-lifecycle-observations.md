# Rust lifecycle observation validation

Date: 21 November 2026

## Scope

The shared source decoder now stores lifecycle evidence as a source-neutral observation containing state (`running`, `completed`, `interrupted`, or `paused`) and an optional bounded reason code. The code accepts only lower-case identifier characters; raw messages, output and error text are rejected. Source identity, turn identity and timestamp remain attached to the event.

Codex task aborts and Pi stop errors now remain visible after collection and store reopen. The `agenturm query` JSON returns the typed state and reason code. The generic Overview pane shows the newest lifecycle observation with harness and session identity and labels it as evidence, rather than deriving a live task status from it.

Store format v5 persists the added variant. Existing v2 stores upgrade through v3, v4 and v5; v3 stores upgrade through v4 and v5; v4 stores upgrade to v5. Each marker/version transition is transactional and each intermediate schema shape is validated. Lifecycle facts collected before v5 cannot be reconstructed because they were not previously stored and the source cursor has advanced.

## Validation

- `cargo fmt --all -- --check`
- `cargo test --offline --locked --workspace --quiet`
- `cargo build --offline --locked --workspace`
- Configured real-process Codex and Pi tests with the Rust plugin host: `cargo test --offline --locked -p agenturm-runtime --test codex_process --test pi_process -- --nocapture`
- Configured multi-harness and watch lifecycle tests with the Rust plugin host, Pi adapter and Codex adapter: `cargo test --offline --locked -p agenturm-app --test configured_collection`
- Strict workspace Clippy passed: `cargo clippy --offline --locked --workspace --all-targets -- -D warnings`.
- Independent adapter audit confirmed both harnesses already emit fixed reason codes and identified no compatibility blocker.

Tests cover interrupted reasons across the real process boundary and database reopen, state/reason validation, unknown payload fields, query serialization, overview rendering, and v2/v3/v4 migration to v5.

## Limits

Lifecycle observations are source evidence. They do not by themselves establish that a session is still active or that work was unfinished; session selection and harness history rules still govern interpretation. Relationships, check facts, coverage gaps, content-grant behavior, and complete domain parity remain open. No performance gain is claimed.
