# Qualify native storage progress and exact replay

This follow-up keeps the existing quality targets and replaces the ambiguous storage-error metric before a new candidate is frozen. The earlier candidate `8e7193b7f89b1cc5ad12d9f56814d08a0ded6e3d` remains separate: its isolated retained workflow passed 22 checks and 9/9 exact receipts, while its subsequent hour attempt failed a genuine Metrics HTTP 503 with 42/42 reconstructed turns; its 506.549-second elapsed result includes cleanup. Its 100-cycle modal attempt stopped on an Investigate navigation deadline. Neither is accepted or reclassified.

## Measured changes

A native file reader publishes its buffered batch before its synchronous writes finish. Its old total therefore includes healthy writes as well as failed retries. Keep that total and every existing source/capture/bootstrap/attention authority fence. Publish one coherent versioned diagnostic snapshot under a small metadata lock, with separate writing/retry/unknown states, an explicit retry boolean for zero-record policy failures, batch identity, age and successful progress. Never hold that lock during storage or policy I/O.

A generated 15-cycle replay found OpenCode's first traced fallback at cycle 4: 14 submitted spans across five traces contained 12 exact stored duplicates and two new rows. At cycle 14, 42 of 44 rows were exact duplicates. This establishes unnecessary updates, not the unique historical cause of HTTP 503. Skip exact unchanged native rows inside the existing source transaction, after policy scrubbing and conjunctive generation guards, with an explicit producer opt-in. Compare every stored field, including source positions and membership metadata. Preserve SDK acceptance counts, missing/dirty projection repair and transactional rollback. No persistent content cache or larger publication budget is introduced.

Modal navigation must observe a fresh foreground cockpit after Escape and a fresh command input after opening the palette. A heartbeat from before the triggering key is insufficient. Retain the same commands, 10 warmup and 100 measured cycles, deadlines and retention limits. An interrupted validation cannot report success.

## Revised functional prerequisite

Register a new clean application/harness SHA, seed and protocol `classified-native-storage-v1` before launch. Use the same 31,000-span / 41,000-log / 5,751-trace seed, 4 GiB reserve and 60-second / two-second functional workflow. Preserve every HTTP failure, namespace/capture/schema/source proof, exact receipt and post-both-restarts cycle check.

Retain all total buffered counts. A healthy writing sample requires coherent metadata and must complete or advance within five seconds, bounded by both reported monotonic ages and an independent observation of unchanged batch/progress identity. Any retry (including a zero-record retry), missing/unknown/incoherent state or stall fails. Final health must be idle with zero buffered records, no retry and no attempt in flight; derived queues must drain. This corrects the error classification; it does not authorize retrospective acceptance of old results.

## Remaining execution

After the revised prerequisite passes, run a genuine 3,600-second active workflow on that exact candidate. Recheck process lifetimes, 22 original checks plus classified-storage proofs, receipts, final readiness and graceful exits. Run modal/native-race and separate first-frame observation gates; overlapping active fixtures establish no idle CPU or HTTP latency result.

Then run sequential balanced BC/CB cold and untouched idle comparisons against `24ea0a9bab53c4f31fc450d04ed0cc2b00655626`, followed by the unchanged authenticated HTTP protocol against its pinned historical baseline `5e0c39786066fb88c0e270418573e361aeb6cf9a`. Targets remain full cold readiness <30 seconds, receiver+cockpit idle CPU <5% of one core over 300 untouched seconds after 60 seconds warmup, and Sessions/MCP HTTP p95 <100 ms with all 420 scheduled two-span writes accepted per arm. Failed schedules have no accepted loaded p95.

Apply the [registered RSS/fd/thread/cache and two-checkpoint-quantum storage rules](final-quality-gates.html). Include retained WAL in post-timing read-only integrity checks; [SQLite permits WAL/SHM retention after a read-only last connection](https://www.sqlite.org/walformat.html#file_lifecycles). Post-close WAL must remain within two checkpoint quanta with no owned open descriptors. Read one snapshot including the WAL; verify integrity, empty queues and unchanged DB/WAL hashes across that inspection. Never force GC, checkpoint or VACUUM inside timing. Archive stopped generated databases only after byte-exact round-trip verification; preserve seed and raw evidence.

Finish with exact-head CI, installed-wheel/CLI/privacy checks, current acceptance documents and the 0.2.4 release. Missing performance targets, live qualification and real human outcomes remain explicit; no source-derived fixture or synthetic participant substitutes for them.
