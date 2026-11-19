# Rust plugin framing checkpoint

19 November 2026. This checkpoint adds a dependency-free Rust workspace and the first shared wire contract for a future supervised plugin host. It does not start plugins or move any production ingestion or storage to Rust.

## Contract

Each data frame has a four-byte unsigned big-endian byte length followed by nonempty UTF-8 bytes. The host-wide ceiling is 4 MiB; a negotiated peer limit can only lower it. A declared oversize is rejected before allocating or reading its payload. Clean EOF is valid only before the first prefix byte. A truncated prefix or payload, invalid UTF-8, empty frame, or I/O failure closes the connection; the caller must resume from its last durable cursor.

The Rust crate and Python companion share the exact `{"id":"α"}` and `{}` wire vector. Partial reads and writes are tested. Neither side interprets JSON yet. Framing does not establish source identity, authorization, cancellation, policy, evidence admission, or durable acknowledgment.

## Validation

- `cargo fmt --all -- --check`: passed locally with Rust 1.100.0.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed locally.
- `cargo test --workspace --locked`: five Rust unit tests passed.
- `python -m unittest tests.test_plugin_frames tests.test_plugin_manifest tests.test_native_fixture_provenance`: eleven Python tests passed.
- `git diff --check`: passed.

CI now runs Rust formatting, Clippy, and tests beside the existing Python and interoperability matrix. This checkpoint is intentionally not wired to live traffic. The next host milestone needs protocol/schema negotiation, a distinct control channel, child supervision, bounded queues, and replay tests. The eventual Rust writer requires a separate storage-authority review and full parity gates from the [execution plan](../rfcs/rust-plugin-kernel-plan.md).
