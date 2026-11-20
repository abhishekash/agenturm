# Kiro collector hook boundary

20 November 2026. The trusted native JSONL collector no longer supplies a Kiro-specific empty `kiro_folders` status field. The Kiro source declaration supplies that empty-state shape, while an active Kiro adapter still supplies the current bounded workspace permission summary. A collector configured only with another source does not publish a Kiro field. The host validates declared fields against its reserved status keys and a 64 KiB JSON limit before publication.

Kiro constants and format helpers for credits, worker-file layout, approval identity, and MCP attributes now live with `KiroAdapter`. The generic collector retains bounded file reads, offsets, admission, persistence, diagnostics, and status publication. The existing `local_agents.kiro_home` compatibility import remains available for callers. Worker linking and folder permission reading had already moved behind adapter hooks before this change; this slice removes the remaining hardcoded status fallback and helper ownership from the collector module.

This is an internal trusted-adapter refactor. It does not activate an external process plugin, expand Kiro evidence coverage, change content policy, claim performance gains, or provide Rust storage authority.

Validation on the candidate branch: 22 native adapter/port/Kiro permission tests passed; 61 cross-native tests ran with one import compatibility failure, which was fixed; the 19 Kiro session tests then passed. `compileall` and `git diff --check` passed. The final post-rebase focused run and independent review are recorded in the pull request.
