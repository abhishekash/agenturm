# Next retained-corpus functional run: registration draft

**Not frozen and not executed.** The next run needs the coordinator's exact merged startup-plus-MCP application and harness commits, clean source trees, candidate module hashes and explicit go-ahead. Main104 `1ca4faf` is a reference only; it is not the forthcoming candidate. No new database clone, workload, benchmark or broad suite was launched during this preparation.

The approved setting is **60 seconds with unchanged two-second phases**, the existing common generated corpus and the unchanged 4 GiB reserve. This is longer functional coverage, not a cadence comparison or resource acceptance result. The [JSON registration](next-retained-functional-registration.json) intentionally leaves target identities/hashes null and `root_go=false` until freeze.

## Source, seed and admission prerequisites

- Pin full application/harness SHA values and the candidate's 86 Python module hashes plus the 10 registered harness helper hashes. Record clean tracked/untracked code status, dependency/uv-lock hash, interpreter, Textual/Rich/Markdown, tmux and host versions; compare before/after and actual child import roots. The JSON includes main104 reference hashes for comparison, not certification of later changed modules.
- The exact common seed file is 188,383,232 bytes, file SHA-256 `a88727a946d8ba0e1dd3b45ed100061d9365496e1dba9d15e9e4f17881df0357`; canonical raw SHA-256 `a7834f8369c95c9160d16fe1d4d1c23b401e76a58c68507ce8b4bafa43948588`, 31,000 spans/41,000 logs/5,751 trace-activity rows. Preparation rechecked the file hash/size and empty WAL, without opening or changing the source database. Normal authorized clone preparation must recheck file/raw equality before and after, retain raw DDL/content and verify production named indexes against a fresh schema from the same candidate.
- Keep `--minimum-free-mib 4096`. The clone guard also reserves eight times the seed size: 1,507,065,856 required working bytes plus 4,294,967,296 reserve bytes means at least 5,802,033,152 free bytes at clone admission. Recheck the guard at launch and during the workload. A failing disk gate is not permission to lower the reserve or delete seeds/results.
- Use a new private owned output directory, preserve prior failed artifacts and register actual child PID/start identities. Use the harness's isolated homes/configuration/proxy exclusion; no personal discovery, provider or real native agent process. Candidate migration/source/owner/capture/receiver/cache gates must pass their own reviewed correctness checks before functional freeze.

## Why 12 and 36 seconds are insufficient promises

If no raised failure interrupts it, the loop finishes an entered six-phase cycle before stopping, but it checks wall duration while its last phase is 5. Blocking phase work and restarts count toward duration. The next phase is scheduled two seconds **after** phase work completes. Reconnect `at` is captured before blocking restart work, not when replacement startup finishes.

With 12 s / 2 s, phases 0–5 and both restart operations can execute, yet the loop can stop immediately after those restarts. That proves no post-restart reconstructed cycle or sampled new-process resources. This limitation does not relabel the earlier 12-second run, which actually failed earlier on phase 2 MCP 503.

Even 36 seconds is not reliably enough: with zero API/database latency, phase 1 adds at least 0.5 s of key sleeps and phase 5 adds 3.1 s. Phase 5 starts near 10.5 s and 26.1 s. The 36-second restart thresholds are 11.88 / 23.76 s, so both can occur at the second boundary. Phase 5 processing ends near 29.2 s; eight seconds of fixed receiver downtime alone already exceeds 36 before compact-cockpit startup. This is source-based schedule reasoning, not a measured run.

Sixty seconds provides a better opportunity for another cycle, but **no theoretical guarantee**. Actual API/reconstruction/restart/three-frame UI overhead can still consume the remaining wall time while phase 5 remains current. The next result must prove the coverage below; duration alone never supplies it.

## Mandatory actual coverage

All existing harness checks remain required. Additionally:

1. Identify the replacement receiver and cockpit from actual child imports and sampled PID/start ticks. Observe both genuine restarts, then at least three fresh healthy coherent heartbeats from the new cockpit in the same receiver namespace. Aggregate old-process samples do not certify a replacement.
2. Show one cycle K initiated and reconstructed after **both** replacements: phase 0 and phase 5 evidence on the new receiver/cockpit identities, exact parent/child/OpenCode request/check/edit/usage/source identities, and all three reconstructed turns. Require all completed turns verified and expected review IDs equal accepted and persisted IDs, with no missing IDs.
3. Preserve both graceful receiver exits, both successful warning-free cockpit exits, per-shutdown queue drains, final fresh same-receiver readiness and source/index/capture completeness. No forced cleanup is a substitute for graceful-exit evidence.

The existing result stores verified-turn **counts**, not individual verification timestamps/IDs. Counts alone and pre-restart `reconnects.at` values are insufficient. Corroborate with unambiguous phase/cycle/source and new-PID evidence from samples, heartbeats and the frozen reconstruction code. If that cannot establish a post-restart cycle, record **failed/incomplete coverage**, even if the old harness's `errors` array is empty. No pass predicate is weakened here.

## Failure and artifact accounting

Startup's explicit indexing error remains fatal. Every actual active HTTP failure remains failed and re-raises its original error; no retry/filtering or missing-value-as-zero success. The new helper retains at most eight metadata-only failure observations, reads at most 64 KiB plus an oversize sentinel and records unavailable/malformed/oversized diagnostics as unknown. It cannot reconstruct the unretained body of the earlier MCP 503.

Require a complete `results.json`, phase/source records, individual errors, source/seed manifests and owned cleanup evidence. Capture driver exit status; missing or interrupted result/worker/restart coverage remains failed/incomplete. Publish only screened/allowlisted generated evidence, not configuration/token/native-source/log directories. Existing failed473 and 12 s artifacts remain unchanged; later reversible working-clone cleanup stays a separate historical addendum.

## Real HTTP 503 boundary check

Two new focused tests pass in **1.037 s on Python 3.13.5**, with `ResourceWarning` treated as error. They send actual authenticated loopback HTTP 503 responses through the existing `Api` and `observed_request`, for ordinary incoming work and a fatal indexing error. They assert the exact original exception object, one request/no retry, closed response handle, correct safe metadata and absence of captured prompt/path/configuration/token/error-body sentinels. They use no Store, database clone, native agent or provider. Malformed/oversize pure cases remain in the existing unit suite and are not duplicated here.

```sh
python -W error::ResourceWarning -m unittest tests.test_soak_http_roundtrip -q
```

## Command after freeze and coordinator go-ahead

Set `FROZEN_APP_ROOT` and `NEW_OWNED_RUN_DIR` to the registered clean candidate and a nonexistent owned-run destination. Execute only after all null registration fields are filled and explicit coordinator go-ahead:

```sh
python "$FROZEN_APP_ROOT/scripts/soak_monitor.py" \
  --app-root "$FROZEN_APP_ROOT" \
  --seed /tmp/agenturm-final-common-seed/raw-seed.db \
  --output "$NEW_OWNED_RUN_DIR" \
  --duration 60 --phase-seconds 2 --minimum-free-mib 4096
```

No run is authorized or executed by this document. The 60-second workload is not a one-hour soak, strict 420-write HTTP comparison, cold <30 s, idle <5% of one core, API p95, RSS/WAL plateau, live-runtime or human-use result. All those acceptance gates remain separate.
