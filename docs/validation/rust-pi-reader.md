# Rust Pi process reader vertical slice

Recorded 20 November 2026. This checkpoint adds `rust/agenturm-pi-harness`, a separately installed Rust reader for one explicitly configured actual Pi session JSONL file. It uses the existing framed process protocol, host admission, one Python Store writer, policy-current projection and opt-in manifest. It does not replace the built-in Pi reader.

The installed-artifact conformance fixture uses Pi `session` and `message` rows with a user turn, an assistant message containing reported usage and two tool calls, two tool results, and an explicit error stop. A native Pi session and the Rust process path both report an interrupted session and four tool Activity observations. The process path reports the supported usage counters, correlates call/result phases through opaque operation IDs, and exposes exact turn Activity and a partial Review subset. It sends no prompt, argument, tool name, path or output into the host Store. A restart keeps the last user turn through the durable cursor; a subsequent explicit stop belongs to that turn. Replacement emits a coarse gap diagnostic and starts a new session and turn.

The parser emits all facts from a complete Pi line as one cursor unit. At a record, byte or structural budget boundary, it leaves the whole line unread and acknowledges only prior complete lines. A single line that cannot fit fails visibly without advancing the durable cursor. The cursor checks file identity, offset, preceding 256-byte digest and full header digest. The header digest prevents a distant session-header rewrite from inheriting a prior turn. A durable source generation changes at each detected gap, separating event IDs even when same-inode truncation reuses a native Pi row ID. An installed test exercises that case through host admission. Unsupported row kinds and roles emit coarse diagnostics. A malformed explicit result flag prevents an invented success. Capture revocation erases the projected data and fences the old worker.

## Remaining parity gates

| Area | Current slice | Required before a production Pi replacement |
| --- | --- | --- |
| Discovery | One explicitly configured file | Bounded directory discovery, ownership conflicts and finite source retention coverage. |
| Identity | Session header, optional worker and last user turn | Broader Pi version corpus, branches and parent/worker relationship conformance. |
| Activity | Content-free role and coarse tool phases | Model Activity, names where policy permits, unsupported Pi event types and exact native action mapping. |
| Review/export | Policy-current partial Activity and opaque operation keys | Native spans, trace membership, captured evidence policy and source-backed review proof. |
| Source changes | Replacement/truncation/header/near-tail rewrite detection with a distinct generation | Explicit treatment of distant in-place edits and rotated unread data. |
| Platform | POSIX executable, Linux x86_64 installed test | Cross-platform packaging, release qualification and retained-history behavior. |

No performance gain or full Pi parity is asserted. The Python Store remains the sole database writer.

## Verification

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `cargo test --workspace --locked`.
- `cargo build --locked -p agenturm-pi-harness` followed by `AGENTURM_RUST_PI_BIN=... uv run python -m unittest tests.test_rust_pi_harness_plugin -v` against an installed SHA-pinned binary.
- Existing plugin and native Pi regression tests.
