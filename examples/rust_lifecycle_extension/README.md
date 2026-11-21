# Separately packaged Rust extension example

This small crate implements one `Capability` and one data-only
`UiContribution` using the public `agenturm-kernel` traits. It also provides an
`ExtensionPlugin` package that registers both contributions together. It is a
build-time Rust plugin: the host links the crate and installs the package via
normal composition; it does not implement dynamic discovery or process
isolation.

From the repository root, build this package by itself:

```sh
cargo build --locked --manifest-path examples/rust_lifecycle_extension/Cargo.toml
```

An app can compose it without changing the renderer or adding ID-specific
dispatch:

```rust,ignore
app.install_plugin(LifecycleExtensionPlugin)?;
```

The capability reports aggregate typed lifecycle counts; the pane groups those
counts by source. Neither inspects message bodies, commands, paths, or tool output. The
integration test in `agenturm-app` installs the package alongside the built-in
Overview, rejects duplicate package IDs, and checks that a failed package
registration leaves no partial contributions behind. A failing neighbor
capability does not suppress the package's pane.
