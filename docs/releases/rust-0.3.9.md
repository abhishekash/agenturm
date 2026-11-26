# Agenturm Rust 0.3.9 prerelease

Rust 0.3.9 bundles the Agenturm app and its required companion processes. It
installs beside the existing command as `agenturm-rust`; it does not replace
the Python `agenturm` launcher.

## Supported bundle targets

- Linux x86-64 and ARM64 on the qualified glibc baseline.
- Apple Silicon macOS 13 or newer as a preview. Intel Macs and Rosetta are not
  supported. Independent clean-host acceptance remains open.

## Install

The bootstrap detects the supported operating system and architecture, fetches
the exact versioned installer, and verifies the installer SHA-256:

~~~sh
curl --proto '=https' --tlsv1.2 -fsSL \
  https://raw.githubusercontent.com/abhishekash/agenturm/b6cd2a2e72185fc46eb89fc667f966bf10a05863/scripts/install-rust-bootstrap.sh \
  | bash
agenturm-rust
~~~

The installer downloads the matching release archive and checksum sidecar over
HTTPS, verifies the checksum, and installs the separate launcher. macOS
archives are unsigned and not notarized. If macOS blocks first launch, approve
only Agenturm in Privacy & Security; keep Gatekeeper enabled.

## Release contents

The release attaches these exact platform archives and `.sha256` sidecars:

- `agenturm-0.3.9-linux-x86_64.tar.gz`
- `agenturm-0.3.9-linux-aarch64.tar.gz`
- `agenturm-0.3.9-macos-aarch64.tar.gz`

Each release workflow checks that the artifact was built from the tagged commit
and refuses to replace an existing asset with different bytes.
