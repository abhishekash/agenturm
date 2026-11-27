# Agenturm Rust 0.3.12

This update repairs the Linux prompt-history entry flow and includes the selected-agent inspector fixes that were missing from 0.3.11.

## Read a session’s prompts

1. Open an agent and choose Prompts, or press P from the agent list.
2. If this local source is not enabled, press a at “Enable prompt history”.
3. Choose a request and press Enter or Right to read it. Esc closes the text, then returns to the agent.

The explicit enable action allows saved user prompts from that source and its sessions. MCP payloads and other tool details keep their independent permissions. Missing native files, changed source proofs and other read errors show the actual reason instead of claiming every failure is metadata-only.

## Included fixes

- Native metadata headers have a separate 256 KiB bound instead of the 8 KiB prompt-row limit. Verified transcript prefixes up to 64 MiB are supported with the same checkpoint and identity checks.
- Default cockpit reads use the collector’s validated configuration instead of a stale sibling sources.json.
- Captured original text opens when readable normalization is unavailable.
- Activity, MCP and Tools remain scoped to the selected agent, with interactive row inspection.
- MCP calls are filtered before the page limit so busy sessions can find earlier calls.

Private native prompt and tool-content previews remain Linux-only. macOS supports collection, history and the cockpit; this release does not add native private previews on macOS.

## Upgrade

After the 0.3.12 installer pin is published, repeat:

```sh
curl -fsSL https://www.agenturm.dev/install.sh | bash
agenturm
```

Close an already-running older Agenturm process before opening the updated command. Installing an update does not replace a running process.
