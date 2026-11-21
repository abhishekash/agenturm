# Separately packaged Rust extension example

This small crate implements one `Capability` and one data-only
`UiContribution` using the public `agenturm-kernel` traits. It is deliberately
not a process plugin and does not implement dynamic discovery: the host loads
the crate through normal Rust composition and registers its values by stable
extension IDs.

From the repository root, build this package by itself:

```sh
cargo build --locked --manifest-path examples/rust_lifecycle_extension/Cargo.toml
```

An app can compose it without changing the renderer or adding ID-specific
dispatch:

```rust,ignore
app.register_capability(id("capability.example.lifecycle"), LifecycleCounts)?;
app.register_ui_contribution(id("pane.example.lifecycle"), LifecyclePane)?;
```

The capability reports aggregate typed lifecycle counts; the pane groups those
counts by source. Neither inspects message bodies, commands, paths, or tool output. The
integration test in `agenturm-app` registers the package through the same
composition methods used by the built-in bundle and checks that a failing
neighbor capability does not suppress its pane.
