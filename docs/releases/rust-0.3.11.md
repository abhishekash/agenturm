# Agenturm Rust 0.3.11

**Stable release** for Apple Silicon macOS 13+ and Linux x86-64/ARM64 (glibc 2.39+).
Intel Macs and Rosetta are not supported.

Agenturm ships as a complete native bundle with its plugin host and five harness
adapters. Python and a Rust toolchain are not needed to run it.

## Install

```sh
curl -fsSL https://www.agenturm.dev/install.sh | bash
agenturm
```

Or use Homebrew:

```sh
brew install abhishekash/tap/agenturm
agenturm
```

Upgrade by repeating the curl command or running `brew upgrade agenturm`.
See the [installation guide](../install.md).

## What ships

This release includes the readable task roster, exact worker prompt isolation,
scoped Activity and MCP views, retained-source lookup, catalog-driven harness
discovery, and the direct `agenturm` installer from the preceding Rust releases.

The native curl installation checks pass on Apple Silicon, Linux x86-64, and
Linux ARM64. The Homebrew tap has a native Apple Silicon installation and
Codex/Pi collection smoke test. Each release archive is built and verified on
its native runner before being attached to the tag.

The macOS app supports foreground collection, local history and the terminal
cockpit. Linux-specific process inspection, managed services, workspace restore,
and private native content previews are not available on macOS. These limits
are listed in the bundle metadata. The macOS binaries are not Developer ID
signed or notarized.

## Release files

- `agenturm-0.3.11-linux-x86_64.tar.gz`
- `agenturm-0.3.11-linux-aarch64.tar.gz`
- `agenturm-0.3.11-macos-aarch64.tar.gz`
- A `.sha256` sidecar for each archive.
- The immutable `rust-v0.3.11` source tag for source builds.

Existing runtime behavior is unchanged from 0.3.10; this release establishes
the stable native distribution and includes the tested installation path.
