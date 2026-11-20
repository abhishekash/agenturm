# Rust relationship and Activity domain parity

Recorded 20 November 2026. This checkpoint extends the pure `agenturm-domain-reducer` fixture to version 2. It derives plugin parent relationships and content-free message, tool, and check Activity from the same highest-revision admitted snapshot as lifecycle and usage. Python `Store` remains the only live database writer and projection authority. This work makes no speed claim.

## Contract

Version 1 fixture input and output remain unchanged. Version 2 requires a declared plugin ID and permits the additional admitted kinds `relationship`, `message`, `tool`, `check`, and `diagnostic`. Rows may carry native worker, turn, and operation IDs. The reducer validates the admitted metadata shape and identity rules, retains the highest revision of each event, and rejects identity or kind changes across revisions. It emits relationship fields on every projected session and a bounded array of Activity fact objects. Diagnostic facts can update a session's last recorded time but create no Activity row.

The relationship decision follows the production Python reducer: a unique newest parent claim is confirmed only when that parent exists in the same current source snapshot; missing parents remain unresolved, equal-time conflicting claims are conflicted, and cycles or self-links are invalid. Retention invalidation clears parent and usage projection and suppresses that session's Activity. Activity carries only typed role/category/result values and opaque host-derived session, event, agent, turn, and operation IDs. No prompts, commands, paths, tool output, or free text are accepted in these fact payloads.

The CLI remains a read-only stdin/stdout reducer. It has no policy, lease, database, source-reader, or network authority. A live Rust storage cutover needs a separate snapshot fence and the complete durability and query compatibility gates.

## Verification

The [version 2 golden corpus](../../tests/fixtures/rust-domain-reducer-v2.json) contains three valid and ten invalid cases. The installed CLI test first validates each valid row against Python's wire and metadata admission, then writes it into a disposable `Store`, runs production `project_source`, and compares every persisted session and Activity payload with Rust and the checked-in golden output. Cases cover confirmed/unresolved/conflicted/invalid relationships, same-event correction, all three Activity kinds, worker/turn/operation correlation, retention suppression, diagnostic behavior, malformed metadata, missing identity, and changed identity or kind across revisions. The existing version 1 corpus and tests remain part of the gate.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --locked -p agenturm-domain-reducer
AGENTURM_RUST_REDUCER_BIN=target/debug/agenturm-domain-reducer \
  python -m unittest tests.test_rust_domain_reducer tests.test_rust_domain_reducer_v2 -v
```

These tests compare an isolated fixture; they do not certify a live Rust projection or any performance improvement.
