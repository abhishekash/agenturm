# Native peer adapter hook checkpoint

19 November 2026. The generic collector now groups active native sessions by registered adapter type and calls each adapter's optional `link_sessions` hook once before and once after a poll. Kiro's exact spawn-to-worker relationship and parent-outcome reducer moved into `KiroAdapter`. The host still owns bounded reads, persistence, source ownership and scheduling. An injected adapter can link its own peers without adding a runtime-name branch to the collector.

Kiro continues to require exact `sub_agent_start.subSessionId` evidence in a spawning transcript. A worker's directory alone is not treated as proof of its direct parent. The adapter keeps the existing protection against overwriting a worker's terminal state with a parent-reported outcome.

The provenance manifest records the new reader hashes against commit `204b471` and keeps the preceding hash set as historical evidence. No fixture bytes or observed producer-version claim changed.

Validation: 78 focused provenance, Kiro session/permissions, native port, session, lifecycle and source projection tests passed. `compileall` and `git diff --check` passed. This is still an in-process trusted adapter hook. Kiro folder-status wiring, remaining source-specific reducers, production process adapters, Rust persistence authority and the retained-history performance gate remain open.
