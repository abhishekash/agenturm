# Native turn review contribution

Recorded 20 November 2026. The generic `Store.review_turn` query port now asks trusted bundled adapter contributions for exact turn evidence. OpenCode owns its bounded SQL membership and usage logic in `observatory/opencode_turn_review.py`, declared by its bundled adapter module. Adding another bundled turn reviewer does not require a runtime branch in `core.py`. The registry rejects duplicate source declarations and competing claims on the same trace.

This is a trusted native compatibility extension. Independently installed process adapters cannot execute SQL through this hook; their evidence still enters through the host-owned admission and projection contracts. Python Store remains the sole writer. The OpenCode turn output and ownership checks are unchanged.

## Exact turn URL repair

`ExecutionScope.path` now omits empty optional query parameters. A whole-turn review sends `turn_id` without an empty `trace_id`; a step review sends the exact trace. This repairs the HTTP whole-turn path after the Activity endpoint began validating blank query values. It does not broaden a selection to a session-wide query.

## Verification

Focused OpenCode turn, proof, current schema, and native publication tests passed: 47 tests. Registry and workflow evidence tests passed: 18 tests. The existing HTTP whole-turn regression previously failed with `400 invalid trace_id` on merged main and now passes. The test files that enforce the seam are `tests/test_native_turn_review_registry.py` and `tests/test_opencode_turn_review.py`.

This checkpoint does not make process harness Review/export parity complete. That requires the remaining per-harness source-to-evidence work.
