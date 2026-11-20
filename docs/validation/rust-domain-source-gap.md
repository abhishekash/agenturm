# Rust source-gap domain parity

Recorded 20 November 2026. This checkpoint extends the pure `agenturm-domain-reducer` fixture to version 3 and lets the read-only shadow snapshot represent durable source-gap invalidation. Python `Store` remains the sole live database writer and projection authority. This work makes no performance claim.

## Contract

Version 3 requires `source_gaps`, an array of `{native_session_id, last_record_ns}` records. Its source is `plugin_source_gap_invalidated_v1` in the same read transaction as admitted evidence, retention invalidation, and the published projection. Native IDs are validated against the plugin identity grammar. A legacy `"*"` sentinel means the whole source is quarantined; it must have timestamp `"0"` and cannot mix with concrete source-gap IDs. Duplicate IDs, malformed timestamps, unexpected fields, and oversized arrays fail closed. Version 1 and 2 inputs and outputs remain unchanged; they reject the version 3 field.

The reducer applies the same precedence as Python `project_source`: retention gaps take the `retention_gap` reason when both invalidations cover a session, otherwise a source gap yields `source_gap`. Both set the state to Unknown, erase usage and parent relationship, and suppress that session's Activity. A concrete source gap creates an Unknown row even if its evidence has been removed. For a missing session in both invalidation tables, the source-gap timestamp supplies `last_record_ns`, while retention still supplies the reason. The legacy sentinel covers all current evidence sessions without inventing rows for unknown historical identities. New sessions admitted after a concrete gap remain visible.

The shadow snapshot remains content-free and read-only. It compares the Rust result only against a policy-current Python projection. `current()` rejects results if receiver identity, source lease, capture policy, dirty sequence, or published projection revision changed while Rust ran. A pending projection or disabled capture produces no snapshot. The Rust CLI has no source-reader, database, policy, lease, or network authority.

## Verification

The [version 3 golden corpus](../../tests/fixtures/rust-domain-reducer-v3.json) contains four valid and twelve invalid cases. Valid outputs are checked against disposable Python `Store` projections and the installed Rust binary. They cover concrete gaps, erased usage and relationship, Activity suppression, new sessions, retention/source-gap overlap and timestamp precedence, the legacy sentinel, and an empty gap list. The malformed cases cover schema version fencing, identity, timestamps, sentinel shape, duplicate membership, unknown fields, and dirty sequence. Generated Rust and installed binary tests check the 2,001-entry capacity rejection. The live shadow tests capture an admitted diagnostic, compare the installed reducer, check revision fencing, and exercise migration to the legacy source-wide sentinel. Existing version 1 and 2 goldens remain in the same Rust and CI gate.

```sh
cd rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --locked -p agenturm-domain-reducer
cd ..
AGENTURM_RUST_REDUCER_BIN=rust/target/debug/agenturm-domain-reducer \
  python -m unittest tests.test_rust_domain_reducer \
  tests.test_rust_domain_reducer_v2 tests.test_rust_domain_reducer_v3 \
  tests.test_rust_shadow_snapshot tests.test_plugin_source_gap -v
```

The fixture and shadow gates do not authorize a live Rust projection cutover. Durability, query compatibility, and rollout gates remain separate work.
