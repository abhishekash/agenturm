# Rust message role observation validation

Date: 21 November 2026

## Scope

The bundled Codex and Pi adapters already emit message facts. The shared decoder now retains only the bounded role (`user` or `assistant`) with the existing source event, session/worker/turn identity, and timestamp. Strict decoding rejects unsupported roles and any message body/content field. This path does not store prompt or response text.

Typed message observations pass through decoder validation, kernel admission, SQLite commit, read-only reopen, query JSON, and the Overview role-event count. Real-process fixtures exercise both roles from both Codex and Pi through durable reopen and assert private text is absent.

The store format is v7. Existing v2 through v6 stores upgrade through transactional marker/version transitions with intermediate shape validation. Message roles collected before v7 cannot be reconstructed because the role facts were not stored and the source cursor has advanced.

## Validation

- `cargo fmt --all -- --check`
- `cargo test --offline --locked --workspace --quiet`
- `cargo clippy --offline --locked --workspace --all-targets -- -D warnings`
- `cargo build --offline --locked --workspace`
- Real Codex and Pi process tests with configured Rust plugin host and adapters: `cargo test --offline --locked -p agenturm-runtime --test codex_process --test pi_process -- --nocapture`
- Configured multi-harness and watch tests with real host/adapters: `cargo test --offline --locked -p agenturm-app --test configured_collection`
- Independent Rust review found no blockers.

Tests cover both accepted roles, invalid-role and unknown-body rejection, kernel/store boundary validation, v6→v7 and older migrations, query serialization, Overview count, private-content absence, and real process restart/reopen behavior.

## Limits

The projection records role, not message content, ordering semantics beyond source timestamps, or a semantic classification of prompts and responses. This enables reliable message-role counts only; it does not establish prompt quality or efficacy. No performance gain is claimed.
