# Installed Rust sample Activity facts

Recorded 20 November 2026, based on `9170ce2` and rebased onto later architecture checkpoints. The separately installed `agenturm-sample-harness` Rust executable now parses three additional JSONL fact kinds: content-free `message` role, coarse `tool` category/result, and typed `check` category/result. Its `SourceRow` rejects unknown fields; each variant rejects fields from the other variants, missing results, nulls and unapproved content. The Python host still authenticates the copied executable, validates the wire record, owns capture policy and Store writes, and projects Activity only when the source's policy epoch and dirty sequence match. This change does not move storage authority to Rust.

The installed-artifact test built the Rust release executable with the repository's pinned Rust 1.100.0 toolchain, ran `examples/rust_sample_harness_plugin/install.py`, and started that copied SHA-pinned binary through `PluginSession`. It admitted lifecycle, usage, message, tool and failed-check facts from one JSONL source. Activity and `/api/activity` showed three content-free rows with the same opaque session identity and empty details; the failed check left the session running. Append-only corrections changed the check to explicitly `passed` and replaced usage totals. Between commit and projection, old Activity disappeared, including from a warm HTTP response cache. Capture revocation erased Activity and projected sessions and fenced the worker. No Python sample executable participated.

| Validation | Result |
| --- | --- |
| `cargo fmt --all --check` | Passed after formatting. |
| `cargo test --locked -p agenturm-sample-harness` | Four unit tests passed, including strict new kind and malformed-content cases. |
| `cargo clippy --locked -p agenturm-sample-harness --all-targets -- -D warnings` | Passed. |
| `cargo build --locked --release -p agenturm-sample-harness` | Passed, reusing the existing `/tmp/agenturm-rust-sample-adapter/target` directory. |
| `AGENTURM_RUST_SAMPLE_BIN=/tmp/agenturm-rust-sample-adapter/target/release/agenturm-sample-harness uv run python -m unittest tests.test_rust_sample_harness_plugin` | Five installed-binary tests passed, including the new Activity/correction/revocation path. |

This is a synthetic metadata-only adapter. It does not prove parity with any native harness's prompt text, command output, trace membership, review evidence or complete capture policy. The producer's file still has the documented append-only and finite-retention constraints. No performance comparison or speedup is asserted; Rust is the chosen implementation language for this adapter.
