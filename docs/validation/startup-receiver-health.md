# Startup health and native polling: bounded integration

Two generated-data integration tests pass against the startup writer-admission fix. They exercise a real Store, real native Session/LocalAgents polling and an authenticated loopback HTTP health route while initial history work is pending. The partial Feed withholds captured source text; complete history readiness is established only after the real migration finishes.

This is **functional evidence**, not a cold-start, writer-deadline, availability, resource or one-hour acceptance result. The original frozen-473 retained-history startup failure remains a failure. No provider, native agent process or personal transcript/database was used.

## Exact runtime and fixture

The tested runtime is `3fb8254` from the startup writer-admission branch, cherry-picked onto frozen `473d338` as `61a8c3c`. The tested `observatory/core.py` SHA-256 is `a91eb782970314004302e40177469f23875867f4f2388dceee8061e80b399d20`. Only the new test file and this report were authored in the independent integration track; the runtime fix itself is described in the [writer-admission report](startup-writer-admission.html).

Each private fixture starts with 501 generated spans and 401 generated logs written through the actual schema/triggers, then reopens the Store for initial work. This exceeds the real 200-source Activity batch and 500-group endpoint page sizes. It is not the retained-history corpus. The first case adds two generated Codex-format files with exact parent/child metadata and a captured request sentinel. Discovery is restricted to those explicit files; runtime process scanning, project MCP configuration discovery and the OpenCode database are excluded. Workflow-pattern inference is explicitly disabled for this fixture.

## Cases

1. **Native writer, health and history interleaving.** A deterministic barrier holds the first actual native source write inside its policy transaction. Initial migration and an independently authenticated HTTP health request are then started. The request cannot finish before writer release; initial native polling and complete history readiness remain false. After release, migration parks at its actual 25 ms inter-batch yield, outside writer admission. Health and native polling can progress during that gap. The returned metadata preserves the genuine canonical child-to-parent relation and persisted hierarchy revision; repeated unchanged health does not increment the revision. Feed publishes only the explicit partial native frame: `updated=0`, `indexing_ready=false`, two observed native rows, no historical sessions/Activity and no request sentinel. The authenticated sessions endpoint returns an explicit 503 with incomplete indexing, without a sessions payload. Releasing history allows actual multi-page endpoint work, final readiness and a nonempty ordinary sessions response. Native read errors, hierarchy write errors, derived errors and uncaught server exceptions are all absent. The private server, native monitor, migration and derived worker shut down cleanly.
2. **Stop at a committed fairness boundary, then resume.** Initial history pauses after exactly one real 200-job batch. Closing the Store sets its real stop event while the writer gate remains available. Shutdown preserves the remaining dirty queue and leaves the endpoint-completion marker absent. Reopening the same private database resumes the actual pipeline, drains the queue, preserves all 501 spans/401 logs and establishes the endpoint marker and ready state. Consuming 200 source jobs is deliberately distinct from emitting 200 Activity rows: generic source spans need not produce Activity entries.

Events control interleaving; they do not synthesize health bodies, successful readiness, parent facts, source parsing or migration output. The HTTP server records uncaught exceptions as failures rather than suppressing them. Authentication is checked with an unauthenticated request returning 401 before the authenticated journey. Test timeouts prevent hangs and are not service-latency assertions.

## Verification and limits

The final local command uses `ResourceWarning` as an error:

```text
python -W error::ResourceWarning -m unittest tests.test_startup_receiver_health -v
```

The final two cases passed in **2.616 seconds**. This duration measures only the small test suite. It cannot be compared to retained-history readiness or used as an API percentile.

There is no controlled benchmark, no actual provider turn, no measured full-corpus useful frame, no strict 420-write cadence pass and no new one-hour stability result here. The fix must still pass integration/CI and the unchanged frozen-source performance protocol. Existing failed artifacts remain retained rather than being overwritten or relabelled by these small positive cases.
