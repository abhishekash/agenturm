# Install Agenturm

Agenturm 0.3.16 is a native Rust application. The installer detects your system,
fetches the matching release, verifies its pinned SHA-256, and installs the
`agenturm` command. No compiler or runtime is required.

## Supported systems

- Apple Silicon Mac (`arm64`) running macOS 13 or newer.
- Linux (`x86_64` or `aarch64`) with glibc 2.39 or newer.

Intel Macs and Rosetta are not supported.

## Install with curl

```sh
curl -fsSL https://raw.githubusercontent.com/abhishekash/agenturm/main/scripts/install-rust-bootstrap.sh | bash
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
See the [usage guide](usage.md) or [privacy notes](privacy.md).

## Prompt history on Linux

Open an agent and choose Prompts (or press P from the agent list). If the local source has not been enabled, press a at “Enable prompt history”. Choose a request and press Enter or Right to read it. This enables saved user prompts for that source and its sessions; tool payload permissions remain separate.

Close an older running Agenturm process before launching the updated command. Confirm the installed command with `agenturm --version`; this update reports `agenturm 0.3.16`.

Version 0.3.16 upgrades native history to schema 28 on the first writer open.
Index construction can take longer on a large history. Older Agenturm versions
cannot read the upgraded store; do not keep an older collector running against it.
