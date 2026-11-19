# Rust plugin host bounded transport

Recorded 20 November 2026. This checkpoint hardens the opt-in Rust process host when either peer produces frames faster than the relay can consume them. It makes no performance claim.

## Failure and change

The host had a two-frame control queue, but its stdin reader used a blocking channel send. While an adapter was blocked responding to `poll`, three additional control frames could fill that queue and park the reader. The reader then could not notice that the parent pipe had closed; the relay kept waiting on the adapter, leaving its child running until Python intervened.

Both readers now fail closed if their bounded queue is full. The relay checks their health every 100 ms while it waits and exits on loss of either peer; its child owner kills and reaps the adapter. The child reader also accepts at most one response frame per `hello` or `poll`. Unsolicited extra frames terminate the host. These checks bound queued frames and preserve the existing Python authority over artifact hashes, grants, Store leases and commits, policy epochs, and acknowledgments.

## Verification

| Check | Result |
| --- | --- |
| Rust 1.100.0 `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked` | Passed. Unit tests cover a saturated control queue and an extra child response. |
| Rebuilt host and sample adapter; `AGENTURM_RUST_PLUGIN_HOST_BIN=... AGENTURM_RUST_SAMPLE_BIN=... python3 -m unittest tests.test_rust_plugin_host tests.test_rust_sample_harness_plugin -v` | Fourteen passed. The new installed-path test keeps Python's pipe open, fills the control queue while the separately installed adapter is blocked on a FIFO, and verifies the host exits and reaps the child. Existing cursor, policy, crash, timeout, Activity, and receiver tests still pass. |
| Adjacent Python process/runtime/supervisor/frame/fixture tests; ten repeats of the new saturation test | 22 ran with one existing skip; all ten repetitions passed. |

The reader rejects frames beyond the available response count, but protocol v1 has no request nonce. A malicious adapter could still race an early response into the window opened for a new request. Python continues to validate every response and evidence batch. The integration cleanup assertion uses Linux `/proc`; other platforms require their own process-lifecycle qualification. This host remains opt-in.
