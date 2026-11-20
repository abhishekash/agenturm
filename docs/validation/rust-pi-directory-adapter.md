# Opt-in Pi directory adapter

Recorded 20 November 2026. Pi's Rust process reader can now use the shared `agenturm-directory-source` component for an **explicitly configured** directory of Pi session JSONL files. The single-file path remains the default. This is an adapter extension and correctness gate, with no performance claim or default replacement of native Pi discovery.

## Extension and authority boundary

`PiFormat` retains the existing per-file parser and turn checkpoint. Its new `DirectoryFormat` binding adds only the stable `pi-discovery-gap` diagnostic namespace. The binary selects `DirectoryPoller<PiFormat>` only when the process host sets `AGENTURM_PLUGIN_DISCOVERY_MODE=1` after negotiating `source-discovery/1`. The installer writes that feature and `source-discovery-root-gap/1` only with `--directory`; it requires an existing, non-symlink directory. The host still owns process startup and feature negotiation. Python Store owns source policy, lease, per-member checkpoints, admission, invalidation, projection, and acknowledgment.

The shared engine accepts at most 48 JSONL members across 256 directories and 4,096 entries at depth six. It scans at most four files per poll. Member IDs are SHA-256 of their paths relative to the configured root. Each file must begin with a Pi session header. A session identity cannot switch files within one source. Unrecognized Pi rows become coarse diagnostics; malformed Pi headers or record and budget errors fail the poll without being reclassified as a continuity gap. Source text, prompts, tool arguments and outputs stay out of process facts.

## Continuity

The global `d1` cursor tracks a bounded round-robin scan. A failed root enumeration emits one source-wide diagnostic and advances to `d2`; repeated failure degrades without emitting another gap. A previously acknowledged member that disappears emits a member-scoped diagnostic and drops its member cursor. If an acknowledged member cannot be read after enumeration, `d3` retains its local cursor, invalidates only that member's sessions, and retries its acknowledged prefix first after recovery. The deterministic event ID is `pi-discovery-gap:<sha256>` over the same source/cursor evidence used by the generic engine. Store commits the diagnostic and cursor together before acknowledgment. A healthy sibling remains current through a member gap.

Deletion followed by reappearance under the same path and identity is still a source-history limit. Pi's process facts remain content-free and do not provide full native Pi Review or export parity. Directory mode is bounded and opt-in; broader native corpus coverage and rollout gates remain.

## Verification

Focused Rust tests pass 13/13 and Clippy with warnings denied passes. Installed Pi directory tests pass 6/6 through the Python Store and include the Rust process host path, exact root/removal/read-gap diagnostic IDs, durable restart, sibling isolation, recovery, malformed format behavior, and installer rejection of unsafe directory inputs. Existing installed single-file Pi tests pass 8/8. Formatting and diff checks pass.

```sh
RUSTUP_HOME=/tmp/agenturm-rustup-home CARGO_HOME=/tmp/agenturm-cargo-home \
  CARGO_TARGET_DIR=/tmp/agenturm-rust-opencode-target \
  CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_DEV_DEBUG=0 \
  /tmp/agenturm-cargo-home/bin/cargo test -p agenturm-pi-harness --locked
AGENTURM_RUST_PI_BIN=/tmp/agenturm-rust-opencode-target/debug/agenturm-pi-harness \
  AGENTURM_RUST_PLUGIN_HOST_BIN=/tmp/agenturm-rust-opencode-target/debug/agenturm-plugin-host \
  uv run python -m unittest tests.test_rust_pi_directory_plugin \
    tests.test_rust_pi_harness_plugin -q
```
