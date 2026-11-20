# Opt-in Rust Pi content references

Recorded 20 November 2026. The bundled Rust Pi **directory** adapter can nominate source ranges for user text when an installed source explicitly chooses `capture = "pi-review-text-v1"`. It emits byte offsets, lengths, SHA-256 digests, member identity, and exact event/session/turn identity. It never sends raw prompt text in the process batch. The receiver opens the configured source itself, verifies each reference, redacts text, and owns retention. This is a correctness slice; it makes no performance claim and adds no public Review or export reader.

## Configuration and boundaries

`examples/rust_pi_harness_plugin/install.py --directory --content-refs` writes a pinned manifest with `harness.pi/1` and `pi-content-ref/1`, requires both in the runtime handshake, and sets `replaces = ["pi"]`. A trusted content configuration passes a generic enablement flag to the process host, which sets `AGENTURM_PLUGIN_CONTENT_REFS=1`. The Pi format declares its own features and nominator to the shared directory runtime; the shared Rust runtime contains no Pi-specific feature name. The single-file and ordinary directory modes remain metadata-only. The installer gives content mode its own default source ID, `rust-pi-content-source`, separate from metadata directory mode's `rust-pi-directory-source`. Reusing a source ID across different cursor modes is not a supported migration. The receiver's Pi reference parser remains a closed, format-specific validator; a general extension registry is future work.

The source root must be an existing absolute directory without symlink components at install time. The receiver binds it to a durable lease and independently walks relative member paths without following symlinks. The emitter only nominates from the one member already selected by bounded discovery; it does not run a second scan. Its file proof checks the selected path, current and prior member cursors, file identity and prefix digest, canonical origin, and an exact same-batch user message fact. A failed proof withholds references while the metadata batch and cursor advance.

Each batch can nominate at most 48 rows and 64 KiB of source bytes, within a 64 KiB reference envelope. The emitter checks the final serialized batch against the requested outer frame and structural-token budgets. The receiver performs its own independent checks. A source gap, changed file, incompatible path, header longer than the receiver's 8 KiB bound, header identity field the receiver cannot validate, out-of-range source number, timestamp outside the receiver's shape, over-budget prefix, or missing same-batch fact can cause an empty or partial reference set. **An admitted batch does not prove complete prompt coverage.** The retained content status records the batch reason and reference count; rows without a reference have no content row. The status value `none` means nominated references were accepted, not that every user message was captured.

## Verification

Focused Rust tests cover exact Unicode member paths, duplicate JSON keys, changed or incompatible paths, 48-reference capping, and final frame-budget rejection. Installed receiver tests cover explicit feature negotiation, source-derived offset/hash/event/session/turn binding, redaction, restart/replay, default metadata-only mode, direct and Rust-host process paths, gap invalidation, revocation, and 50 metadata events with only 48 references. The existing receiver test corpus separately rejects forged references, changed source bytes, traversal, symlinks, FIFO paths, and unanchored post-gap origin. Existing Pi directory and single-file regression suites also run.

```sh
RUSTUP_HOME=/tmp/agenturm-rustup-home CARGO_HOME=/tmp/agenturm-cargo-home \
  CARGO_TARGET_DIR=/tmp/agenturm-rust-opencode-target \
  /tmp/agenturm-cargo-home/bin/cargo test -p agenturm-pi-harness \
  -p agenturm-source-process --locked --offline
AGENTURM_RUST_PI_BIN=/tmp/agenturm-rust-opencode-target/debug/agenturm-pi-harness \
  AGENTURM_RUST_PLUGIN_HOST_BIN=/tmp/agenturm-rust-opencode-target/debug/agenturm-plugin-host \
  python3 -m unittest tests.test_rust_pi_content_emitter \
    tests.test_rust_pi_directory_plugin tests.test_rust_pi_harness_plugin \
    tests.test_plugin_pi_content
```
