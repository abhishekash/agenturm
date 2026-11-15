# Finish the existing quality gates

Baseline: merged main `24ea0a9bab53c4f31fc450d04ed0cc2b00655626` (PR108). Its full CI matrix passes. Its retained synthetic workflow passed all 22 strict checks, 9/9 turns and exact review receipts, both restarts, final readiness and clean exits. This is the starting point; new performance, hour and human outcomes are not yet accepted.

## Execution order

1. Profile cold preparation on the unchanged 31,000-span/41,000-log/5,751-trace generated corpus. Attribute source indexing, canonical summaries, outcomes, endpoints and native startup writer admission separately. Diagnostic profiling is not a performance pass.
2. Fix measured work only. Candidate ideas under independent review are bounded initial native ingestion, duplicate outcome migration and bounded canonical preparation reuse. Preserve raw/capture/ownership semantics, durable invalidations, interrupted migration, exact pagination and truthful readiness. Do not meet the 30 s target by declaring incomplete history ready.
3. Close harness gaps before freeze: deterministic SQLite closure, strict post-both-restarts coverage, first useful native terminal-frame observation and post-timing integrity checks. Keep the existing workload, deadlines, failure accounting and 4 GiB reserve.
4. Freeze exact application/harness/source/seed/dependency identities. Pass the three-worker functional prerequisite, all review receipts and restarts. Then run coordinated cold/untouched-idle, actual authenticated HTTP cadence, native concurrency and modal/storage gates. Use balanced order and report every failure; no post-abort loaded p95.
5. Run a genuine 3,600-second active workload on the final source. Require normal exits/drains, final readiness, exact identities, no surviving owned process and a complete cycle after both replacement processes. Short fixtures and configured duration alone are insufficient.
6. Qualify available live producers without publishing captured content; record actual producer versions and limits. Human reconstruction/resumption/repeat-use outcomes require real participants. An exercise has been requested; observations stay at zero until it happens.
7. Reconcile the implementation plan and 38-item acceptance ledger with actual results and immutable historical references. Run exact-head CI and installed-wheel/CLI/privacy checks. Prepare the patch release, review its artifacts and publish only the claims supported by completed gates.

## Existing acceptance targets

- Full retained-history readiness below 30 seconds; native first-frame timing reported separately.
- Complete-monitor idle CPU below 5% of one core in an untouched 300-second window after 60-second warmup.
- Authenticated Sessions and MCP HTTP p95 below 100 ms during the unchanged 420-write schedule; all writes accepted and every deadline retained.
- Bounded modal cache/stack plus measured RSS, descriptors, threads, DB/WAL/storage and backlog behavior. Timing windows contain no forced GC, checkpoint or VACUUM.
- One-hour active coverage, exact receipts, namespace/source/capture consistency, graceful shutdown, integrity and empty dirty queues.

Targets remain open when missed. No different corpus, larger timeout, filtered 503, relaxed cadence, simulated hour, synthetic participant or inferred native approval can substitute for evidence.

## Frozen resource decision rules

Stage version 0.2.4 before freezing source. Any application or harness change invalidates that candidate's subsequent evidence. Use baseline 24ea0a9 and the same generated seed; retain every attempt. Cold and idle comparisons run sequentially in BC then CB order. CPU is receiver plus cockpit as a percentage of one core; coordinator and synthetic producer are reported separately. HTTP denominators remain five warmup pairs, 20 idle pairs, 40 loaded pairs, and 420 accepted two-span writes per arm, with the existing absolute cadence and failure rules.

Modal acceptance uses ten warmup and 100 measured cycles. For each unchanged PID/start tick, compare the first and last 25 post-close samples: late median RSS may grow by at most max(15 MiB, 10% of the early median); descriptor and thread medians must not grow. Preserve hard bounds of four Review cache entries / 8 MiB, two native-cache entries and modal stack depth one. Unexplained monotone retained growth requires investigation even within the RSS tolerance.

For the hour, assess each process lifetime lasting at least ten minutes: compare its 60–300-second warm median with its last five minutes, using the same RSS/fd/thread rules. Shorter segments are insufficient for plateau acceptance. Active coverage must be at least 3,600 seconds, independently of startup and shutdown. Require final Activity/Findings/native/search dirty queues empty, no surviving owned process, and SQLite integrity_check=ok after timing. Keep every WAL sample, require no continuing growth during an untouched idle window, and record the SQLite autocheckpoint N and page size P; define Q=32+N×(P+24). Require WAL high-water growth no greater than the readiness/warm baseline plus 2Q (about 7.86 MiB at N=1000/P=4096), then absent/zero WAL and SHM after ordinary clean closure. For the fixed-input modal run, require unchanged canonical/index row counts and at most 2Q extra DB+WAL high-water after warmup. Exceeding these bounds requires investigation; these are pre-registered review gates, not universal SQLite limits. Report durable storage growth against exact accepted source counts; generated new writes are expected to increase storage and are not a leak by themselves. No forced checkpoint, GC or VACUUM inside any timing window.

## Review and isolation

The independent source review identified initial native 10,000-owner drains, repeated outcome projection, canonical replay and fixed migration sleeps as hypotheses, not accepted root-cause measurements. The validation review identified an unclosed harness receipt connection, missing early-frame measurement and the need for explicit integrity/plateau and post-restart coverage checks. Those findings guide execution; profiling will select the implementation.

Use owned generated fixture directories, authenticated loopback servers and isolated homes/configuration. Keep personal telemetry/preferences and the user-owned review notes untouched. Preserve original failed results and private raw evidence; verified byte-exact archives may replace stopped working copies to keep disk headroom. No provider call or agent launch occurs as an incidental benchmark side effect.
