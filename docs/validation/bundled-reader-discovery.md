# Bundled reader factory discovery checkpoint

Recorded 20 November 2026. OpenCode now declares its `NativeReaderSpec` in `observatory.adapters.opencode_reader`. The trusted bundled package scanner used for JSONL harness factories also loads reader factories. The generic reader registry contains no OpenCode-specific construction branch. Adding another bundled reader factory requires a new module in Agenturm's installed adapter package, not a collector dispatch edit. Duplicate reader IDs and malformed factories fail before collection.

This remains an in-process compatibility reader, with the Python Store as sole writer. The scanner does not import project-local Python or independently installed plugins; external sources continue through explicit SHA-pinned process configuration. This does not yet make OpenCode a process plugin or establish full message/tool/check/review parity over the process wire.

Validation: `/home/abhishekash/abhi-codes/agentic/agenturm/.venv/bin/python -m unittest tests.test_native_adapter tests.test_native_reader_registry tests.test_collector_runtime tests.test_opencode tests.test_opencode_turn_proof` — 47 passed. The tests exercise OpenCode registration, injected bundled reader discovery without a central edit, duplicate refusal, collector composition and OpenCode turn proof. `git diff --check` passed.
