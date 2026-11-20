# Shared Rust directory source extraction

Recorded 20 November 2026. The opt-in Codex directory worker now uses `agenturm-directory-source`, a format-independent Rust crate for bounded JSONL directory discovery and durable per-member checkpoints. Codex supplies its existing `SourceFormat` translator and the `codex-discovery-gap` diagnostic namespace. A separate synthetic format proves the shared engine is not tied to Codex rows or IDs. This change does not enable Pi directory mode, change defaults, or claim a performance gain.

## Authority and extension boundary

`DirectoryPoller<F>` implements the existing source-process `Poller` contract when `F` implements `DirectoryFormat` and `SourceFormat`. `DirectoryFormat` only adds a stable discovery-gap event-ID prefix. The shared engine scans the configured directory, reads each JSONL member with `poll_no_follow::<F>`, and returns `source-discovery/1` batches. `source-discovery-root-gap/1` remains an explicit capability in the process handshake. The host and Python Store still own plugin admission, capture policy, member-cursor persistence, source-wide or member-scoped invalidation, and acknowledgment. The source emits no committed state on its own.

The directory limit is 48 JSONL files, 256 directories, 4,096 entries, and six nested levels. Each poll checks at most four files, persists the scan position when more remain, and removes a missing durable member before admitting a new one at the capacity limit. Member IDs remain SHA-256 of the UTF-8 path relative to the configured root. Symlinks and non-UTF-8 member paths are refused. A file opened after enumeration is checked by the shared JSONL reader without following a final-component symlink.

## Durable cursor and replay contract

| Cursor | Meaning | Next poll |
| --- | --- | --- |
| `d1:sequence:last-member-id` | Normal checkpoint and round-robin position | Resume after the saved member; an idle poll at four or fewer files keeps the same cursor. |
| `d2:sequence:last-member-id` | Root enumeration failed and one source-wide gap was emitted | Repeated failure returns an error without another gap; successful enumeration resumes as `d1`, including when idle. |
| `d3:sequence:member-id` | A durable member failed to open or read after enumeration and one member gap was emitted | Retry that member first with its saved local cursor; repeated failure returns an error, recovery returns to `d1`, and disappearance clears the member checkpoint. |

The existing cursor syntax, ASCII validation, sequence increments, 64-character lowercase hexadecimal member IDs, deterministic event IDs, logical `time_ns`, batch fields, and local frame reserve of 1,024 bytes are unchanged. Codex discovery diagnostics retain `codex-discovery-gap:<sha256>` IDs. The first root outage sends a memberless `root_gap: true` batch; a missing member sends an empty `member_cursor`; a read outage retains the member cursor for prefix verification. Repeating a poll from the same acknowledged cursor and member map regenerates the same gap batch. Cursor and member changes become durable only after receiver acknowledgment. Format or budget errors do not become continuity gaps.

## Verification

The pre-extraction baseline passed 14 Codex Rust unit tests and nine installed Codex directory tests. After extraction, seven Codex format tests and eight shared-directory tests pass. The shared tests include an independent synthetic `SourceFormat` and reader, explicit `d1`/`d2`/`d3` transitions, deterministic replay, feature constants, and a distinct diagnostic namespace. The rebuilt installed Codex binary passes all nine directory tests against Store and the Rust host; those tests now assert exact Codex root, removal, and read-gap diagnostic IDs. `cargo fmt --all -- --check` and `git diff --check` pass.

```sh
RUSTUP_HOME=/tmp/agenturm-rustup-home CARGO_HOME=/tmp/agenturm-cargo-home \
  CARGO_TARGET_DIR=/tmp/agenturm-rust-opencode-target \
  CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_DEV_DEBUG=0 \
  /tmp/agenturm-cargo-home/bin/cargo test -p agenturm-directory-source -p agenturm-codex-harness
RUSTUP_HOME=/tmp/agenturm-rustup-home CARGO_HOME=/tmp/agenturm-cargo-home \
  CARGO_TARGET_DIR=/tmp/agenturm-rust-opencode-target \
  CARGO_PROFILE_DEV_DEBUG=0 \
  /tmp/agenturm-cargo-home/bin/cargo build -p agenturm-codex-harness -p agenturm-plugin-host
AGENTURM_RUST_CODEX_BIN=/tmp/agenturm-rust-opencode-target/debug/agenturm-codex-harness \
  AGENTURM_RUST_PLUGIN_HOST_BIN=/tmp/agenturm-rust-opencode-target/debug/agenturm-plugin-host \
  uv run python -m unittest tests.test_rust_codex_directory_plugin -q
```

This extraction establishes a reusable source component. A Pi directory adapter still needs its own format binding, installer configuration, installed Store/host tests, and explicit rollout before it can be enabled.
