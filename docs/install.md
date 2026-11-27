# Install Agenturm

Agenturm 0.3.11 is a native Rust application. The installer detects your system,
fetches the matching release, verifies its pinned SHA-256, and installs the
`agenturm` command. Python and a Rust toolchain are not required.

## Supported systems

- Apple Silicon Mac (`arm64`) running macOS 13 or newer.
- Linux (`x86_64` or `aarch64`) with glibc 2.39 or newer.

Intel Macs and Rosetta are not supported.

## Install with curl

```sh
curl -fsSL https://www.agenturm.dev/install.sh | bash
agenturm
```

To update, run the same `curl` command again. The installer preserves an
existing launcher as a timestamped backup before installing Agenturm. Its
default binary directory is `~/.local/bin`; add that directory to `PATH` if
your shell cannot find `agenturm`. Set `AGENTURM_INSTALL_DIR` and
`AGENTURM_BIN_DIR` to choose custom install directories.

## Install with Homebrew

```sh
brew install abhishekash/tap/agenturm
agenturm
```

Update with `brew upgrade agenturm`.

## Next steps

Keep your coding agents open and run `agenturm` in another terminal. It
discovers supported local Codex, Claude Code, OpenCode, Pi, and Kiro records.
See the [usage guide](usage.md), [privacy notes](privacy.md), or [interactive
walkthrough](https://www.agenturm.dev/#demo).
