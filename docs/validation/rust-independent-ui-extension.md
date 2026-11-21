# Independently packaged Rust UI extension

`examples/rust_lifecycle_extension` is a separate Cargo package and workspace.
It depends on the public `agenturm-kernel` crate and implements one `Capability`
and one data-only `UiContribution`. The app integration test registers both
alongside the built-in composition; it also registers a deliberately failing
neighbor capability and verifies the external pane remains available and the
failure is reported in the generic Attention view.

The UI crate's TestBackend test invokes the external pane implementation and
renders its returned `PaneModel` through generic dispatch at 80×24 and 60×18.
The renderer has no sample extension ID check. The example aggregates typed
lifecycle facts only; it reads no message bodies or tool output.

Validation commands:

```sh
cargo build --offline --locked --manifest-path examples/rust_lifecycle_extension/Cargo.toml
cargo test --offline --locked -p agenturm-app --test sample_extension_composition
cargo test --offline --locked -p agenturm-ui --lib independently_packaged_pane_renders_generically_at_standard_and_narrow_sizes
```

This proves crate-level Rust composition with statically linked trusted code.
It does not establish runtime discovery, binary loading, sandboxing, deadlines,
or isolation from panics and resource exhaustion.
