# Opt-in Rust plugin process host

Recorded 20 November 2026. This checkpoint introduces `agenturm-plugin-host`, a reusable Rust process boundary for a separately installed source adapter. It relays bounded v1 control and data frames to one child, enforces hello → poll → matching ack sequencing, and owns child cleanup. It is selected explicitly; the existing direct Python transport remains the default. This is an architectural Rust step chosen by the project owner, without a performance claim.

## How to run it

Build the host and a separately installed adapter with the repository's Rust 1.100.0 toolchain. For the sample adapter, follow its [install guide](../../examples/rust_sample_harness_plugin/README.md) to obtain `plugins.toml`.

```sh
cargo build --release --locked -p agenturm-plugin-host
sha256sum /absolute/path/to/target/release/agenturm-plugin-host
agenturm serve \
  --plugin-config /absolute/path/to/plugins.toml \
  --rust-plugin-host /absolute/path/to/target/release/agenturm-plugin-host \
  --rust-plugin-host-sha256 <the-64-character-digest>
```

The configured host must be an absolute, regular, non-symlink file whose SHA-256 matches the explicit digest. Agenturm checks it before creating the Store, then rechecks and copies it before every worker start. It also verifies and copies the child artifact against its manifest. The host receives only a minimal environment, the approved source root when granted, and the copied child executable path. The host does not open the Agenturm database or receive its capture-policy authority.

Python still owns manifest/grant approval, source replacement, the lease and policy epoch, validated batch admission, the only SQLite writer, projection, and acknowledgment after the Store commit. A failed commit closes the whole process group; restart uses the durable Store cursor. The Rust process validates the control sequence and bounds all frames; Python still validates plugin handshake features and every evidence record. Each enabled source uses a Rust host process plus one child, within the existing four-source admission limit.

On POSIX, Python starts each host in its own process group and kills the group on timeout, forced close or host crash, including any surviving child. The Rust host itself kills and reaps the child when its parent control pipe closes, even while the child is blocked producing a response. The Rust host forwards child stderr into Python's bounded stderr drain; it does not print source content or paths.

## Verification

| Check | Result |
| --- | --- |
| Rust 1.100.0 `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked` | Passed; two new Rust host state tests, plus the existing protocol, fixture and adapter tests. |
| `cargo build --locked --bin agenturm-plugin-host` and `cargo build --release --locked -p agenturm-plugin-host -p agenturm-sample-harness` | Passed; Python copied and SHA-pinned both debug and release host executables for integration. |
| `AGENTURM_RUST_PLUGIN_HOST_BIN=/tmp/agenturm-rust-process-host/target/debug/agenturm-plugin-host AGENTURM_RUST_SAMPLE_BIN=/tmp/agenturm-rust-process-host/target/debug/agenturm-sample-harness python3 -m unittest tests.test_rust_plugin_host -v` | Eight passed: installed adapter correction/restart, commit-before-ack crash recovery, policy revocation, pre-Store host hash refusal, real receiver health, host crash cleanup, timeout group cleanup and parent-pipe closure during a blocked child read. |
| The same eight integration tests with both environment variables pointing to `target/release/` | Passed against optimized installed artifacts. |
| Existing plugin process/runtime/supervisor/config and sample-adapter Python tests | Passed locally (41 tests, four Rust sample tests skipped when its binary environment variable was absent). |

This is an optional transport for POSIX hosts. The new no-orphan tests use Linux `/proc`; macOS process cleanup and installed-wheel delivery remain unqualified. The Rust process does not replace the Python Store, plugin supervisor policy or all native harness readers. It adds a process per source and no controlled whole-monitor performance result is available.
