# Independently packaged Rust UI extension

`examples/rust_lifecycle_extension` is a separate Cargo package and workspace.
It depends on the public `agenturm-kernel` crate and implements one `Capability`,
one data-only `UiContribution`, and one `ExtensionPlugin` package that installs
both contributions atomically. The app integration test installs it alongside
the built-in composition, verifies duplicate package IDs are rejected, and
checks nested package IDs, package identity/registration panics, registration
errors, and contribution-ID conflicts without partial package contributions. A
deliberately failing neighbor capability still does not suppress the example
pane; its failure is reported in the generic Attention view.

The catalog-backed Pi process integration test collects a fixture with a user
message and interrupted assistant turn, reopens the database through a
read-only store, installs the independent package, and verifies the capability
metrics and pane model beside the built-in Overview. The fixture's prompt text
is checked absent from persisted observations before the extension runs.

The UI crate's TestBackend test invokes the external pane implementation and
renders its returned `PaneModel` through generic dispatch at 80×24 and 60×18.
The renderer has no sample extension ID check. The example aggregates typed
lifecycle facts only; it reads no message bodies or tool output.

Validation commands:

```sh
cargo build --offline --locked --manifest-path examples/rust_lifecycle_extension/Cargo.toml
cargo test --offline --locked -p agenturm-app --test sample_extension_composition
cargo test --offline --locked -p agenturm-app --test configured_collection version_two_catalog_runs_a_pinned_real_pi_process
cargo test --offline --locked -p agenturm-ui --lib independently_packaged_pane_renders_generically_at_standard_and_narrow_sizes
```

This proves crate-level Rust composition through the package-level installation
API with statically linked trusted code. Registration is staged and committed
atomically. It does not establish runtime discovery, binary loading,
sandboxing, deadlines, or isolation from panics and resource exhaustion.
