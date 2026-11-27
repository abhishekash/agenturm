# Contributing

Agenturm is a Rust workspace. Install the pinned toolchain from
[`rust-toolchain.toml`](rust-toolchain.toml), then validate changes from the
repository root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

For changes to a bundled release, also run the relevant `scripts/verify-rust-*.sh`
check on its supported platform. Keep changes focused, add or update Rust tests
for behavior changes, and avoid committing local databases, session captures,
credentials, or real customer telemetry.
