# Native state context adapter hook

Recorded 20 November 2026. The generic `Session.observe_state` now asks an optional trusted adapter hook for the row that carries working directory and Git context. Codex supplies its nested `payload`; adapters without the hook continue to use the top-level row. The collector no longer branches on `runtime == "Codex"` for this context.

The hook changes no lifecycle result, storage authority, capture policy, process plugin contract, or external adapter permissions. The host still publishes the project name with detail capture disabled and withholds the full working directory and branch. A separately installed harness remains on the process plugin path; this hook belongs to the trusted native adapters.

Validation: `tests.test_native_adapter` covers Codex, Claude Code, Pi, Kiro and an injected adapter, including nested-versus-top-level context and detail capture disabled. The broader native session, lifecycle, Activity, Review, schema, Kiro and reader-registry suites were run from the installed environment. This extraction makes one collector branch pluggable; it does not claim full native parity, Rust storage authority, or a performance gain.
