# JSONL source polling: verified incremental prefixes

## Finding

The JSONL source adapter verified a committed cursor by hashing the complete
prefix before reading and hashing that prefix again before returning. That
preserved detection of in-place edits, but made each 1 MiB catch-up batch cost
grow with the transcript's already-committed size. A synthetic 16 MiB source
showed both repeated work while catching up and repeated work on idle polls.

The local native 0.3.16 process was separately observed using about 80% of one
logical core at roughly 82 MiB RSS. Direct `perf` and `strace` attachment were
blocked by the host's `perf_event_paranoid=4` and ptrace policy, so those
samples do not identify a whole-process call stack. This change addresses the
measured JSONL polling cost; it is not a claim that whole-app CPU has been
measured after the change.

## Change

- Retain a bounded, process-local SHA-256 state for each exact adapter, path,
  and cursor after its prefix has been verified.
- Reuse that state to hash newly scanned rows rather than rereading the entire
  committed prefix on every batch.
- Check file identity, size, mtime, and ctime before reuse and again before
  returning. A cache miss or metadata change keeps the full-prefix verification
  fallback; a mutation during a cached poll also falls back to a full check.
- Keep the persisted cursor at v2, so existing cursor-proof consumers and the
  independent content readers are unchanged. Cache entries disappear on
  process exit and are bounded to 128 entries.

Regression coverage checks unchanged idle polls, continued scanning when
complete rows remain, stable incomplete tails, append invalidation, same-size
prefix rewrites with the original mtime restored, and mutation during a
header-upgrade poll.

## Reproduce and measure

```sh
cargo test --release -p agenturm-jsonl-source --locked
cargo test --release -p agenturm-jsonl-source --locked \
  benchmark_idle_poll_large_committed_prefix -- --ignored --nocapture
```

The ignored benchmark uses a 16 MiB synthetic JSONL file with 4 KiB rows and
12 warm samples per case. It reports the legacy full-prefix path (cache
disabled by a test-only switch) and the new cache path in the same test
process. The idle case starts at EOF. The catch-up case begins with a 1 MiB
cursor and scans one more 1 MiB per sample. `prefix_hash_bytes` counts bytes
reread by the full-prefix verifier; it does not count the newly scanned rows,
which both paths still parse and hash.

Local release-build sample:

| Workload | Full-prefix path, median (min–max) | Cached path, median (min–max) | Prefix bytes reread, full → cached |
| --- | ---: | ---: | ---: |
| Idle at EOF | 23.324 ms (22.945–23.475) | 8 µs (8–9) | 402,653,184 → 0 |
| Catch-up, 1 MiB batches | 11.305 ms (3.564–18.894) | 1.320 ms (1.304–1.355) | 176,160,768 → 0 |

There were 12 samples per case. The catch-up full-prefix timing had substantial
spread, so the deterministic read-work reduction is stronger evidence than a
speedup ratio. This is a warm synthetic adapter benchmark, not a live
collector, cold-cache, or whole-application measurement. The cache is
process-local: source restarts and metadata changes still incur full
verification.

## Other audited work

The live process replaced 14 of 15 source children in a sample aligned with
the 5-second follow-discovery interval. This is consistent with `FollowSchedule`
rotating queued sources, but attach restrictions prevented proving the cause of
every replacement; adapter failures are not ruled out. No scheduling change
was made because rotation provides queued-session coverage. The inspected
local roots contained 112 Codex JSONL files (about 991 MB total), 31 Pi files
(about 79 MB), and 9 Claude files (about 17 MB). A separate read-only directory
walk completed in about 1.6 ms, 0.2 ms, and 0.1 ms respectively; this was a
filesystem sample, not a timing of Rust discovery or `/proc` handle scanning.

A read-only warm query against the active schema-28 history store returned
16,385 rows / about 1.24 MB of kind JSON in 50.90 ms median (five samples,
49.75–51.97 ms) using the default recent-page filters. This measures SQLite
fetching through Python, not Rust deserialization or the full collector
refresh. The earlier navigation-index issue is already addressed in schema
28; this query remains a distinct source of per-refresh work to profile if
whole-app CPU remains high after deploying the JSONL change.
