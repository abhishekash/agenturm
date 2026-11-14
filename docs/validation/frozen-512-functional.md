# Frozen 512 functional run: harness success, registered coverage incomplete

The original harness exited 0 with no reported errors. The stricter preregistered post-restart proof is **incomplete**: cycle 2 starts on both replacement processes, but no cycle 2 phase 5 sample binds its completion to those lifetimes. This gap is not retrospectively waived, and the complete registered functional gate has not passed.

## Scope and verified facts

Application and harness both used frozen premerge candidate `512adcf98e2bfb70ffddd0c015d87e8457cb2383`. The unchanged common synthetic seed contains 31,000 spans,41,000 logs and 5,751 traces. All 86 runtime modules and 10 harness hashes remained fixed, and seed physical/logical hashes were rechecked after the run. The unchanged 4 GiB reserve passed launch admission. The configured workload was 60 seconds with 2-second phases; actual active elapsed time was 73.711 seconds after an observed 227.278 seconds to full readiness. These are descriptive observations, not timing targets or a one-hour result.

The run reconstructed all nine completed synthetic parent/child/OpenCode turns. Expected, accepted and persisted review counts were all nine, with exact receipt identities checked independently. Three approval inboxes rendered. Receiver recovery was observed, both receiver exits were 0 with queues drained 0→0, and both wide and compact cockpit exits were successful and warning-free. All registered child PIDs are absent. No artifact, database clone or frozen source was removed or modified by this reporting step.

## Exact evidence gap

The independent analyzer verified the two replacement process lifetimes, three fresh coherent replacement-cockpit heartbeats, all nine exact scoped receipts, final coherent readiness and clean exits/drains. It could not establish `phase0_to5_after_both_replacements` or the associated `post_restart_cycle_receipts_after_new_ui` predicate because the final phase 5 sample was absent. The second failed predicate expresses that missing cycle binding, not a missing receipt among the globally verified nine.

The final cycle's phase 0–4 samples use the replacement receiver/cockpit. The frozen loop can finish phase 5 work after its last ordinary timed sample and then stop. Aggregate verification counts and pre-restart timestamps do not replace the required phase 5 process-bound evidence. A separately reviewed tail-sampling harness and fresh registered run are required; this run is not rerun or relabeled.

Independent analyzer SHA256: `15fcc6940a9f0f9a9f27363afea8bbdaaa93fa2e1ebd20ab74a4b3cd5c411604`. Its allowlisted checks, input hashes, process lifetimes and exact receipt identities are retained in the JSON companion. Original results SHA256: `fd2363e3e0e6124bdebb9d3478a95bfdd73d67a5ea811f0f9054ba9e97754f11`.

## Screened evidence and remaining gates

The original private `results.json` is unchanged. Its raw final health includes captured session fields and real host metadata, so wholesale compression/publication would be inappropriate. `frozen-512-functional-results-screened.json.gz` is explicitly a screened projection: raw `index_availability.final_health` is omitted and replaced with readiness/identity/count metadata. The original hash and separate screened/compressed hashes distinguish it from a byte-for-byte original. No token/configuration/event body or raw health is published.

The JSON report preserves both outcomes: original harness success and strict coverage incompleteness. Full workflow/restart/drain acceptance, first useful cold-frame and full readiness targets, loaded API/CPU/RSS/WAL/storage bounds, one-hour stability, live-runtime and human-use evidence remain open. No performance target closes from this generated functional run.
