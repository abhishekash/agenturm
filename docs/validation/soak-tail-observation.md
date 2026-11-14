# Completed-cycle tail observations

The functional soak now records its normal API/process sample after every completed phase 5, even when the periodic sample deadline has not arrived. Sample elapsed time is captured after phase processing. This closes an observation gap; it does not change product behavior or establish a resource target.

## Why the frozen 512 run remains incomplete

The 60-second, two-second-phase run on `512adcf98e2bfb70ffddd0c015d87e8457cb2383` returned `errors=[]`, nine completed/verified turns, nine matching expected/accepted/persisted receipts, both process restarts, fresh replacement cockpit heartbeats, and clean exits/drains. Its sampled cycle 2 has phases 0–4 on both replacement lifetimes but no phase-5 observation. Phase 0 was sampled at 55.952 seconds and phase 4 at 66.962 seconds. The registered post-both-restarts coverage therefore remains **incomplete**. Counts and later receipts do not substitute for the missing registered sample.

Original results SHA-256: `fd2363e3e0e6124bdebb9d3478a95bfdd73d67a5ea811f0f9054ba9e97754f11`. Original samples SHA-256: `12eb580be37b1ead0ffe451bc9b3d7168bd743d3cf76f7ec3bc1bc7e4b02c6f3`. The private analyzer read these artifacts without changing them. No original health body or captured source content is included here.

## Exact schedule revision

A per-iteration flag is set only after the existing phase-5 workflow action and all three reconstruction checks finish. The normal sample then runs when that flag is set **or** the periodic deadline is due. It retains health, Sessions, Activity, Metrics, MCP, durable index proof, and process PID/start-tick observations. A failed reconstruction call or sample API call still propagates and cannot publish a complete sample; a reconstruction mismatch may still produce a diagnostic observation, but remains in errors[] and leaves the verified-turn count incomplete.

The sample clock is read after phase processing and supplies sample elapsed time and the next sample deadline. The separate pre-phase elapsed clock still drives the existing receiver/cockpit restart thresholds. Loop termination, six phases, two-second phase delays, API assertions, receipt comparisons, failure handling and restart rules are unchanged. This is a **revised functional observation schedule**, not a performance comparison with 512 or earlier runs. Each forced observation adds the actual five API reads and process/index checks; it is not free bookkeeping.

Candidate harness script SHA-256: `dda5a3019da41b34e560602519d9fed5c80416a4fb6559cb2a3cabf10cb62c4e`. Base: exact frozen 512. A new exact app/harness registration, root freeze and authorized fresh retained-corpus run are required before claiming post-restart functional completion. No retained clone, workload run or benchmark was started for this patch.

## Validation

- Four new deterministic tests execute the actual production while-loop AST with generated local collaborators. They cover a final phase 5 crossing duration while the sample deadline is still in the future; all five sample API routes; replacement PID/start ticks; completion-plus-two-second phase cadence; ordinary interval gating; projection/API failure propagation; and preserved reconstruction mismatch errors.
- Frozen-512 negative control: the same final-cycle test fails because the original loop issues none of the five sample requests. This is an expected regression demonstration, not a product failure or an accepted functional run.
- Focused combined gate: **28 tests passed in 2.310 seconds**, Python 3.13.5 with `ResourceWarning` treated as an error. Modules: `test_soak_tail_observation`, `test_soak_monitor`, `test_soak_readiness_errors`, `test_soak_http_diagnostics`, `test_soak_http_roundtrip`. The four-test subset also passed on Python 3.12.3 in 0.246 seconds; counts overlap and are not added.
- Independent peer review found no blocker; its narrow four-test gate passed in 0.283 seconds with strict ResourceWarnings. This overlaps the combined gate.
- The initial new-test attempt had three test-fixture errors because the deterministic clock lacked `time()`. Adding the missing clock method corrected the fixture; no harness behavior changed to accommodate it.

These tests exercise production loop control and generated reconstruction responses, not a real receiver, native runtime, provider, retained corpus or terminal. The original frozen run, failed historical runs and all performance targets remain unchanged.
