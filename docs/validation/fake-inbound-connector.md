# Fake inbound connector checkpoint

Recorded 20 November 2026. This checkpoint adds a standalone local HTTP connector executable and installer under `examples/fake_inbound_connector/`. It uses the existing explicit process source adapter protocol, manifest verification, host-owned Store cursor, projection, supervision and receiver health. No core dispatch or UI source file was edited.

The fake service pages bounded lifecycle and usage metadata. The connector handles idle pages, a one-second `429` retry, process restart at an acknowledged cursor, corrected event revisions and `410` cursor expiry. Expiry produces a durable `cursor_expired` diagnostic before re-reading from the new revision. Raw content and malformed fields cause worker failure before the cursor advances. The installer and child permit only exact loopback `/events` URLs and the child rejects redirects.

Validation on this branch:

```sh
python3 -m unittest tests.test_fake_inbound_connector
```

Five tests passed, including an installed plugin observed through a real receiver `/api/health` response. The test suite uses only a local fake HTTP server; no account or external message is touched. The existing plugin test suite and repository checks should run in CI before release. This is a **contract proof**, not a general connector release. Grants are declarative rather than a network sandbox. Output delivery, credentials, distributed quotas and full message/tool/check capture remain open architecture gates.

## Rust loopback counterpart

The opt-in `examples/rust_loopback_inbound_connector/` executable implements the same connector and source adapter contracts through `agenturm-source-process`. It adds no receiver dispatch or UI branch. The generic host still owns source activation, capture lease, durable cursor, Store transaction, ACK, replay and projection. Its installer accepts only a previously built binary, pins the copied bytes, writes the configured endpoint and plugin config, and never invokes Cargo.

The Rust process accepts only `http://127.0.0.1:<port>/events`, opens direct loopback TCP without proxy support, and rejects redirects. It bounds HTTP bodies at 1 MiB, record count at 256, projected records at 16 KiB, process frames at the shared 4 MiB limit, and pages at 8,192 structural tokens. It rejects unknown/duplicate fields, private content, malformed cursors and unsupported states. A `429` produces an unchanged-cursor idle batch and bounded retry delay. A `410` on a nonfresh cursor produces the same durable `cursor_expired` gap as the Python fixture.

The Python fixture remains the default when the Rust binary environment variable is unset. CI separately runs the same receiver/Store assertions against both fixtures; the Rust run also covers bad/duplicate Content-Length, chunked responses, overlong headers/bodies and full-cursor rollback. Both runs use a local synthetic HTTP server. The connector grants are declarations and are not an OS network sandbox; the fixture is not credentialed or suitable for arbitrary external endpoints.

Rust fixture checks:

```sh
cargo test -p agenturm-loopback-inbound-connector --locked
AGENTURM_RUST_LOOPBACK_INBOUND_BIN="$PWD/target/debug/agenturm-loopback-inbound-connector" \
  python -m unittest tests.test_rust_loopback_inbound_installer tests.test_fake_inbound_connector -v
```

On this feature branch, workspace formatting and Clippy passed, and all 175 workspace Rust tests passed across 46 test binaries. The Rust connector crate passed 6/6 tests; the binary-backed installer suite passed 6/6 and receiver/Store suite passed 12/12. The Python-default receiver suite passed 11 tests with the single Rust-only framing test skipped. GitHub CI was queued for the PR; no CI result is claimed here.
