# Agenturm Rust 0.3.2

The standalone bundle adds more precise turn and check evidence and extends the cockpit's grant-backed history views.

## Recorded activity

- Codex records structured turn failures with a bounded reason code. Recognized checks with a consistent native exit and terminal status retain their operation identity. Review shows edit counts tied to the exact turn, and `agenturm review` reads the same scoped evidence from a terminal command.
- Pi records recognized checks from explicit user shell exits at session scope. OpenCode associates recognized checks with the tool operation and uses a retained idle boundary to scope later activity.
- The cockpit can reveal eligible older OpenCode prompts after later sweeps. OpenCode history collected before receipt proofs were recorded may need recollection. A separate current grant can reveal a selected Codex MCP payload when the source row and operation identity still match.
- The connector driver has an opt-in Linux x86-64 isolation profile for pinned static executables that communicate over the local process protocol. This profile denies host filesystem, credential, and network access to the child.

## Broker and private edit details

- `agenturm broker collect`, `query`, `review` and `inspect` use the authenticated native storage service. The service owns history, validates the client identity and limits queries to its source allowlist. The inspector keeps broker reads in the background.
- Codex edit paths and recorded diffs can be opened with a separate `codex.edit_detail` approval. The preview requires a pinned single JSONL source configuration and exact retained turn and operation evidence. Configure it through `AGENTURM_CONTENT_CONFIG` or `sources.json` next to history.
- Broker grant administration, private disclosure, retention and workspace actions remain open. Its collector principal has both admission and history-read authority for its configured source IDs. Untrusted child adapters need an OS boundary that denies the broker endpoint.

[Broker commands](../rfcs/rust-broker-commands.md)

## Install or upgrade

Download the Linux x86-64 archive and its adjacent checksum. Run these commands from the download directory.

```sh
archive=agenturm-0.3.2-linux-x86_64.tar.gz
sha256sum -c "$archive.sha256"
mkdir -p "$HOME/.local/opt"
tar -xzf "$archive" -C "$HOME/.local/opt"
agenturm="$HOME/.local/opt/${archive%.tar.gz}/bin/agenturm"
"$agenturm" --version
"$agenturm"
```

Keep the extracted catalog and binaries together. Existing Rust installations can reopen their Rust history directory. For managed installs, use the service setup and upgrade steps in the Linux guide. The supported archive targets Ubuntu 24.04 x86-64 and runs without Python.
[Linux bundle guide](../rfcs/rust-linux-bundle.md) · [Harness qualification](../validation/rust-harness-evidence-coverage.md)
