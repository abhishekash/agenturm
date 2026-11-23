# Agenturm Rust 0.3.8

The Rust cockpit now opens with an Agenturm command center: full-width task rows, explicit project groups, selectable status counts, and exact selection restoration after inspecting a session. Task titles and project headings come from retained evidence or explicit operator task links.

This release restores Python keyboard navigation and Agenturm styling, adds selectable evidence tables and inspectors, and implements protected Ctrl+F investigations. `X` marks an exact current request seen; `z` switches the same granted prompt between Readable and original captured layout. Copy and export include bounded loaded metadata and exclude private viewer bodies.

## Install or update

From a source checkout, install the exact version with the opt-in installer:

```sh
agenturm=$(bash scripts/install-rust.sh install --version 0.3.8)
"$agenturm"
```

It installs the complete bundle and an `agenturm-rust` launcher. Use the matching Linux x86-64 or aarch64 archive and verify its `.sha256` sidecar. Keep all bundled binaries and the pinned catalog together.

## Validation

- 634 workspace tests across 93 suites; zero failed or ignored. Strict all-target workspace Clippy passed.
- Native x86-64 and ARM64 archive qualification passed, including ARM64 prior-release upgrade and systemd user service checks.
- Six packaged terminal groups cover evidence navigation, exact project links, current-request acknowledgement, clipboard/export privacy, history replacement rejection, and prompt redaction/expiry.
- Seven distinct-UID KVM cases passed for private native evidence, captured spans, private eval text, recorded Host, authenticated remote history, opt-in remote Host and eval inventory. The actual tagged Python 0.2.8 attachment also passed.
- All five mandatory SDK jobs passed. Comparative performance benchmarking remains deferred.

Linux x86-64 archive SHA-256:

```text
528d4c7057d280258788d44e684505b7c552bc5f4dc8f730c9a24d568fc0d657
```

Linux ARM64 / aarch64 archive SHA-256:

```text
95e37c0254f9bcd1650b1a7d9a630a40c180f61dc80f43035e934c732dd556cc
```

Both architectures passed [tag-specific qualification](https://github.com/abhishekash/agenturm/actions/runs/45121070134). All four published archive/sidecar digests match the qualified assets.

## Remaining gates

This is an opt-in Rust release. The default Rust switch remains gated. Remaining UI parity includes configured idle MCP inventory and per-call related traces, generic metric history/exemplars and the seven-day chart, grouped Findings, per-process projects and native internal-review capture. Real Kiro and authorized production connector endpoint qualification also remain open; these requirements are not retired.

[Acceptance record](https://github.com/abhishekash/agenturm/blob/rust-v0.3.8/docs/validation/rust-038-ui-acceptance.md) · [Execution plan](https://github.com/abhishekash/agenturm/blob/rust-v0.3.8/docs/rfcs/rust-only-execution-plan.md)
