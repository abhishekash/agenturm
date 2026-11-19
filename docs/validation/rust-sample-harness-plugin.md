# Rust process adapter validation

Recorded 20 November 2026. This checkpoint adds a separately installed Rust JSONL source adapter in `rust/agenturm-sample-harness`, the installer and [usage guide](../../examples/rust_sample_harness_plugin/README.md), and a CI job that builds and tests the artifact through the real Python receiver. It adopts Rust as the chosen implementation language for this adapter; no performance gain is asserted.

The adapter uses the existing framed `source-adapter/1` and `configured-root/1` contracts. The child supplies lifecycle, usage, relationship and coarse gap records. The host owns source activation, artifact pinning, capture policy, durable cursor admission, correction compaction, session projection, worker supervision and HTTP health. No Python kernel dispatch edit was needed to install this new adapter. The record reader limits each poll to 256 records and approximately 1 MiB of scanning; malformed or oversized complete lines fail the worker without a cursor acknowledgment.

## Verification

| Check | Result |
| --- | --- |
| Rust 1.100.0 `cargo fmt --all -- --check` | Passed. |
| Rust 1.100.0 `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed. |
| Rust 1.100.0 `cargo test --workspace --locked` | Passed: protocol, existing fixture and four new adapter unit tests. |
| `cargo build --locked --bin agenturm-sample-harness` and `cargo build --release --locked -p agenturm-sample-harness` | Passed; each resulting executable was installed by `install.py` into an isolated directory with SHA-pinned manifest. |
| `AGENTURM_RUST_SAMPLE_BIN=/tmp/agenturm-rust-sample-adapter/target/release/agenturm-sample-harness python3 -m unittest tests.test_rust_sample_harness_plugin -v` | Four passed against the installed release executable: process replay/correction/relationship, replacement gap and revocation, malformed content refusal, and receiver `/api/health` projection. The same four tests passed against the debug executable. |

The integration test asserts that a corrected usage event replaces its prior revision without duplicate evidence; a restart resumes the durable cursor; and a parent relationship is confirmed after the parent is observed in the same source. It also verifies that source replacement emits a gap warning before subsequent records and that capture revocation erases projected sessions. The receiver test launches the real plugin config without a kernel registration change and waits for the completed session and running worker in `/api/health`.

This is a sample harness source, not parity with Pi, Codex or another production harness. The later [content-free Activity checkpoint](rust-sample-activity.md) adds message/tool/check metadata; prompt text, tool output and exact review proof are still absent. Linux wheel packaging of the opt-in Rust host arrived separately in #174; this sample adapter still requires its own installation and has no Windows build qualification or retained-history performance result. The Python Store remains the single database writer.
