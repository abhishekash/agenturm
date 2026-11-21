# Rust process-adapter catalog validation

The v2 collection configuration selects process adapters from an explicit
user-managed catalog. Catalog entries declare protocol 1, a feature set, and a
lower-case SHA-256 artifact pin. Catalog JSON and artifacts are bounded; plugin
IDs and feature names are validated; duplicate IDs, unsupported protocols,
malformed pins, unsupported features, unknown selections, and changed artifacts
are rejected. Relative catalog artifact paths resolve against the catalog file.

Before opening the SQLite store, collection loads the catalog and verifies the
artifact files. The runtime repeats the artifact hash check before each host
launch and requires the adapter handshake feature set to equal the catalog.
Tests cover successful relative-path resolution, unknown selected IDs,
duplicate IDs, modified artifacts, failure before database creation, valid and
invalid SHA-256 pins, and duplicate or extra handshake features. A real Pi
process also completes collection through a v2 pinned catalog and its durable
history is reopened; existing version 1 configuration and Pi/Codex process
collection remain covered.

The catalog is a selection and integrity mechanism. It does not automatically
discover plugins, dynamically load decoders or capabilities, sandbox adapters,
or reduce their OS privileges. Process children run as the Agenturm user.

Validation commands run for this slice:

```sh
cargo test -p agenturm-runtime -p agenturm-app --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --workspace --locked
cargo fmt --all -- --check
git diff --check
```

