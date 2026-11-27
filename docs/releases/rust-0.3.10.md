# Agenturm Rust 0.3.10 prerelease

Rust 0.3.10 bundles the Agenturm app and its required companion processes. It
installs the `agenturm` command. The Python source and SDK documentation remain
available for developers.

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

Install with the environment-detecting bootstrap, then run `agenturm`:

~~~sh
curl -fsSL https://www.agenturm.dev/install.sh | bash
agenturm
~~~

Homebrew is also available with `brew install abhishekash/tap/agenturm`; update
with `brew upgrade agenturm`. The bootstrap verifies the pinned release and
installs the `agenturm` launcher. See [installation details](../install.md). macOS
archives are unsigned and not notarized. If macOS blocks first launch, approve
only Agenturm in Privacy & Security; keep Gatekeeper enabled.

## Release contents

The release attaches these exact platform archives and `.sha256` sidecars:

- `agenturm-0.3.10-linux-x86_64.tar.gz`
- `agenturm-0.3.10-linux-aarch64.tar.gz`
- `agenturm-0.3.10-macos-aarch64.tar.gz`

Each release workflow checks that the artifact was built from the tagged commit
and refuses to replace an existing asset with different bytes.
