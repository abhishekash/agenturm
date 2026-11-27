# Agenturm Rust 0.3.10 prerelease

Rust 0.3.10 bundles the Agenturm app and its required companion processes. It
installs beside the existing command as `agenturm-rust`; it does not replace
the Python `agenturm` launcher.

## Changes since 0.3.9

- Refine the task roster with readable labels, supporting context, responsive task details, and clearer prompt empty states.
- Enable exact worker-scoped prompt history and isolate Codex prompt evidence across workers. Private text still requires a current source grant; private native prompt preview remains Linux-only.
- Scope MCP and Activity views to the selected agent.
- Resolve private prompt sources for retained transcripts outside active polling slots, and recover Codex native titles with explicit source configuration.
- Discover harness sources through pinned catalog declarations, including the Apple Silicon bundle.
- Preserve bounded, opt-in startup task labels in the retained Python UI. Python test suites remain excluded from CI.

## Supported bundle targets

- Linux x86-64 and ARM64 on the qualified glibc baseline.
- Apple Silicon macOS 13 or newer as a preview. Intel Macs and Rosetta are not
  supported. Independent clean-host acceptance remains open.

## Install

The bootstrap detects the supported operating system and architecture, fetches
the exact versioned installer, and verifies the installer SHA-256:

~~~sh
curl -fsSL https://www.agenturm.dev/install.sh | bash
agenturm-rust
~~~

The installer downloads the matching release archive and checksum sidecar over
HTTPS, verifies the checksum, and installs the separate launcher. macOS
archives are unsigned and not notarized. If macOS blocks first launch, approve
only Agenturm in Privacy & Security; keep Gatekeeper enabled.

## Release contents

The release attaches these exact platform archives and `.sha256` sidecars:

- `agenturm-0.3.10-linux-x86_64.tar.gz`
- `agenturm-0.3.10-linux-aarch64.tar.gz`
- `agenturm-0.3.10-macos-aarch64.tar.gz`

Each release workflow checks that the artifact was built from the tagged commit
and refuses to replace an existing asset with different bytes.
