# Agent inspector repair validation

Validated on Linux on 2026-11-27.

## Repaired behavior

- Default cockpit prompt, MCP payload, edit and private investigation reads use the active collector’s validated source authority. They do not read a stale sibling sources.json. File-configured history readers still reload their explicit configuration.
- The agent inspector exposes Overview, Prompts, Details, Activity, MCP and Tools. Activity uses the evidence list and row inspector; MCP and Tools use selectable operation tables. Switching sections preserves the selected session and worker.
- MCP calls are projected before the history page limit, so newer unrelated events cannot hide every older call.
- Failed prompt loads show the actual reason in the body and do not invent a metadata-only capture policy. When readable normalization is unavailable, an authorized read opens the captured original. The mode toggle stays synchronized.
- The selected tab remains visible at the 38-column minimum. All six tabs fit at 60 columns. The prompt footer no longer duplicates its status or claims to be loading while a granted reader is open.

## Verification

- 198 application unit tests and 176 UI unit tests passed.
- 8 private-content integration tests passed, with pinned Codex and Pi processes enabled.
- Clippy passed for application and UI, including all targets, with warnings denied.
- Isolated default-cockpit PTY journeys passed at 120×32, 80×24 and 60×18: deliberately stale sibling configuration, exact prompt delivery, Activity inspection, older MCP discovery, exact MCP payload delivery, Tools inspection, return to the roster and terminal-mode restoration.
- The PTY fixture contains 600 later command events after its MCP call. It uses only synthetic data and pre-existing explicit fixture grants.

Reproduce the terminal check:

```sh
python3 scripts/validate_rust_agent_inspector.py --app /path/to/agenturm --bundle /path/to/complete/bundle
```

## Limits

This is a Linux repair qualification, not a macOS runtime qualification. Existing source-specific approvals remain required; no grants or source configuration in the user’s live database were changed by validation. No running user session was restarted.
