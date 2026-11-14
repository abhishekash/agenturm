# Frozen 9bee functional checkpoint: failed

The single registered retained-corpus run **FAILED** during phase 3. `GET /api/activity?limit=50` returned HTTP 503 after **9.539 seconds of active work**. Strict post-both-restarts coverage is **INCOMPLETE**. This checkpoint closes no performance, cold-start target or one-hour acceptance gate.

## What was observed

| Check | Recorded outcome |
| --- | --- |
| Frozen application and harness | `9bee1f99746c49c5d04fbd12cc1aaaff1e2b4dfa` |
| Workload |60 seconds configured,2 second phases,4096 MiB reserve |
| Cold readiness |237.388 seconds; prior fixture preparation 19.315 seconds (descriptive only) |
| Earlier samples | Cycle 0 phases 0–2 were ready/coherent, with complete data reads |
| First failure | Phase3 Activity HTTP 503; incoming indexing ready=False, error=False, processed52,002 |
| Completed / verified turns |0 /0 |
| Receiver/cockpit restarts | None |
| Receiver shutdown | Exit 0; pending 1→0 |
| Cockpit shutdown | No graceful exit record; process subsequently absent |
| Final health / final index proof | **NULL / NULL: unavailable** |

The response metadata was parsed from the bounded readiness allowlist. It identifies the reported incoming-indexing state, without proving which queue caused the rejection. No lock traceback was found by the independent audit. Earlier readiness is not a substitute for final source completeness.

## All original errors

1. HTTPError: HTTP Error 503: Service Unavailable
2. No keyboard review acknowledgements were accepted
3. No complete three-worker cycle reconstructed
4. Required receiver/cockpit restart did not occur
5. Insufficient heartbeat coverage
6. Insufficient receiver process samples
7. Insufficient cockpit process samples
8. Insufficient synthetic_daemon process samples
9. Insufficient driver process samples
10. Derived work was not confirmed drained at both receiver shutdowns
11. Two receiver exit statuses were not confirmed
12. Two successful warning-free cockpit shutdowns were not confirmed

## Frozen inputs and registration

The immutable original registration SHA is `d689a9039fa456865c31fc53f1fbe06a4a318106d1d8f76da8b2fe74fe36fec0`. Its source certificate omitted the redundant `application_dirty` field which the harness adds. Before any cockpit import, UI event or active sample, the coordinator approved an immutable supplement (SHA `27499bb21577530c0d38d1715d149958463ed782aea575810eea97ce696f61ea`) binding the original registration and exact existing values, adding only strictly boolean `application_dirty=False`. The original registration was not rewritten. Actual source_start/source_end equality and all other fields remain exact requirements; the original unsupplemented dictionary mismatch remains recorded.

All 86 runtime module hashes match frozen 512, all 10 harness hashes and dependencies remained unchanged, and the reviewed phase 5-tail sampler SHA is `dda5a3019da41b34e560602519d9fed5c80416a4fb6559cb2a3cabf10cb62c4e`. The common raw seed physical SHA remains `a88727a946d8ba0e1dd3b45ed100061d9365496e1dba9d15e9e4f17881df0357`; its logical raw SHA remains `a7834f8369c95c9160d16fe1d4d1c23b401e76a58c68507ce8b4bafa43948588` with 31,000 spans, 41,000 logs and 5,751 trace-activity rows. Source and seed checks passed before and after the attempt.

Admission retained the unchanged 5,802,033,152-byte gate; observed free space was 6,358,208,512 bytes. No reserve, cadence, duration, failure classification or coverage requirement was relaxed.

## Independent reviews and process cleanup

The source/readiness reviewer independently verified 86 modules, 10 harness helpers, 3 imports, original seed equality and four absent registered process lifetimes. The cycle analyzer independently classified the original run as failed and strict coverage as incomplete: no replacement lifetimes, no post-both-restarts cycle, no reconstructed turns and no final coherent readiness proof exist. Its analyzer SHA is `a5146a98874d08a64290a2790210755fcda3a5d616fc83b6b2338498a78570fa`. Both original audit files and their hashes are recorded in the companion JSON.

The driver exited 1. Receiver, cockpit, synthetic daemon and driver PID/start-tick lifetimes were all independently confirmed absent. A graceful cockpit exit is not inferred from process absence. All original results, logs, configuration, telemetry, workflow state and native inputs remain preserved. No rerun, archive, cleanup deletion or hour launch occurred. The active failure happened between metadata polls, before a timely ACTIVE announcement; no timing comparison is claimed.

## Evidence and publication boundary

Original results SHA: `7277176be5876c7b36a7244f1ae24c97497175ddddb01ef51716e5aa0971a671`. The original remains private and unchanged. The accompanying screened gzip is an **explicit allowlisted transform**, not a compressed original; it preserves all 12 errors and safe failure metadata while omitting raw health, configuration, response bodies, captured session/task data and host process discovery. Its uncompressed SHA is `acebd71a96b21e925f58a55885b3b3a9dbcd074cef1cd6917ac0fbca2d69a0ea` and gzip SHA is `cbfb9175c243dd8635fae029cdf4b7deecbf6e22be3481ef6607ffa034c4fa1e`. Final health and final index proof remain null.

[Machine-readable report](frozen-9bee-functional.json) · [Screened results](frozen-9bee-functional-results-screened.json.gz)

The next bounded Activity dependency investigation must independently prove its source/readiness contract. This report makes no claim that a fix is implemented or that another run is authorized.
