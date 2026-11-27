# Prompt history: real-data and first-use validation

Linux qualification for Agenturm Rust 0.3.12, 2026-11-27.

## Repaired failures

- A source without an existing user-text grant now presents an explicit “a Enable this source” action in Prompts. Enabling authorizes user-text reads for the exact configured source and its sessions. It does not enable MCP payload or edit permissions.
- Native Codex metadata often includes base instructions larger than 8 KiB. Header verification now has its own 256 KiB bound, preserving the checkpoint digest, header digest, generation, inode and source-identity checks. Oversized metadata and guardian sources remain rejected.
- Verified prefixes up to 64 MiB are supported; an over-limit source reports its size limit instead of claiming its content changed. Revealed text and individual proof-row bounds remain unchanged.
- The default cockpit uses its active validated source authority, and the selected-agent Activity, MCP and Tools repairs are included in the versioned release.

## Verification

- 198 application, 177 UI and 8 content-proof unit tests passed.
- 8 private-content integration tests passed with pinned native Codex and Pi adapters enabled.
- All-target Clippy passed with warnings denied.
- Fresh unapproved-source PTY journeys passed at 120×32, 80×24 and 60×18. Each starts without prompt approval, enables the source from the UI, reads and redacts the captured prompt, inspects MCP and Tools, and restores terminal modes.
- The synthetic fixture includes a 20 KiB native metadata header, more than 600 retained events and a transcript larger than 8 MiB. A deliberately stale sibling sources.json is also present.
- A database copy made from the real local history reproduced the permission dead end and the valid 18,745-byte native header rejection. With the fix, the UI enable action loaded three prompt references; an exact captured prompt was read successfully. No private body was printed in validation logs.

The live user database was not granted access or modified by these checks. Test approvals were confined to isolated databases. The old running app was not killed or restarted.

## Reproduce

```sh
python3 scripts/validate_rust_agent_inspector.py --app /path/to/agenturm --bundle /path/to/complete/bundle
```

Native private prompt previews remain Linux-only. Sources above 64 MiB remain outside the supported verified-preview bound and show that limit explicitly.
