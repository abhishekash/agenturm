# Typed investigations and participant kit

Status: Stage G modules implemented in isolation; receiver/CLI/UI integration and final acceptance are owned by the integrating change. The participant kit is prepared, not conducted. Observed participant count: **0**. No live Claude/Pi, adoption, reconstruction-time or repeat-use outcome is claimed.

## Search and ownership contract

Typed search reuses the Activity event identity. A compact terms table and SQLite FTS5 index store bounded sanitized input text; canonical ownership and terminal result are joined from Activity scalar columns in the same read transaction. JSON payload ownership cannot override those columns. Unknown workers remain visible globally and are excluded by explicit known-worker filters. No trace-root task, agent or session is borrowed.

- **Path:** exact recorded modifying-file path, case and Unicode sensitive. No filesystem lookup, Unicode normalization, case folding, checkout resolution or guessed `old -> new` parsing. Explicit recognized rename fields expose both paths. A failed/unknown operation remains an attempt; search does not prove a persisted change or shared workspace.
- **Command:** qualified recorded command input. A generic display label and JSON printed by a tool are insufficient. The failed filter uses recorded terminal classifications, excluding informational search/comparison exits and contradictory unknown results.
- **Instruction:** readable user-message content only. Assistant claims, runtime control envelopes and context-only boilerplate are excluded. A subject-only or clipped message is labelled partial. No inference is made about whether an agent followed the instruction.

Command/instruction matching is a literal **word/token phrase**, with case-insensitive Unicode tokenization and accents preserved. It is not exact shell-string matching or arbitrary substring search; punctuation is tokenized. User input is quoted as a phrase and cannot become an FTS expression. Paths use an indexed exact comparison.

The new term index is captured content: sanitization occurs before insertion, known internal review/disabled-capture rows are excluded, and Activity deletion removes text/postings transactionally without relying on `PRAGMA foreign_keys`. The receiver integration must queue existing source identities once through the existing bounded durable drain and disclose readiness until complete. FTS5 absence is an explicit unavailable capability, with no raw-JSON scan fallback and no failure of unrelated cockpit functionality.

## Bounds and source navigation

At most eight bounded terms are prepared per event; text is limited to 4,096 UTF-8 bytes. Pages contain at most 100 events; the command reads one page by default and at most five, with a 2 MiB result budget. Cursors bind query, filters and receiver namespace, and preserve full timestamp/event-ID ties. Newer inserts do not shift an older keyset page; later updates/deletions can change retained evidence across separate requests. These are retained observations, not a frozen whole-database snapshot or an exhaustive audit.

Every match carries the actual Activity event ID, source ID/type, producer, actor, session, trace and recorded turn reference. Its source URL and copyable command include a bounded encoded reference pinning all those identity fields, including unknown values. CLI and UI integrations require that reference for an exact bounded source read; establishing a new receiver identity cannot authorize a same-ID replacement. No “nearest event” or broader-session fallback is allowed. Source reads compare every owner/execution field, actual source scope and original plus per-source capture policies before publication, including a final health check. The reference is encoded metadata, not authentication or encryption. Missing, clipped, unsupported, redacted or pruned evidence remains disclosed as unavailable or partial. Zero matches never establishes that no matching work occurred.

## Validation and measurement

The focused 17-test gate passes for extraction, exact Unicode/case/rename paths, literal phrase behavior, role exclusions, credential redaction, canonical scalar ownership, unknown global workers, stable equal-time paging, update/revocation/deletion, CLI stdout/stderr/exit behavior, receiver replacement and additive source policy. Adversarial source reads reject changes to producer, actor, session, trace, span and turn even when the source/event IDs are reused; a replacement receiver is refused before source hydration. The generated nine-record pilot database also verifies two separately recorded OpenCode-shaped step traces in one explicit synthetic turn. Actual authenticated receiver and full-cockpit integration remain separate required gates for the integrating change.

`scripts/benchmark_investigation.py` measures the added index in isolation: alternating baseline/indexed SQLite builds, identical compact Activity payloads and existing Activity indexes, WAL checkpoints before database-size measurement, and repeated exact/phrase reads. It includes preparation/FTS write cost and the additional database size. It excludes receiver, UI, native discovery and source hydration; it makes no whole-monitor CPU or readiness claim.

The [accepted raw samples](investigation-index.json) use frozen source `7d85608`, 6,000 synthetic events, three alternating build pairs and twenty reads per type/pair. Root and both other local agents held tests, browsers and heavy tools throughout the 18.515-second interval, 14 November 2026 at 09:00:22.987–09:00:41.502 UTC. Remote CI was outside this host. This is one host and an isolated SQLite workload, not a controlled full-receiver comparison.

| Measured index cost | Result |
| --- | --- |
| Median baseline / indexed build | 608.53 / 4,017.29 ms |
| Additional preparation/write time | 3,408.76 ms, about 0.568 ms per generated event |
| Baseline / indexed database size | 20,766,720 / 24,793,088 bytes |
| Additional storage | 4,026,368 bytes, about 19.4% |
| Exact-path per-run median / p95 | 2.38–3.94 / 2.81–5.72 ms |
| Common-command phrase median / p95 | 31.56–34.23 / 39.76–42.92 ms |
| Common-instruction phrase median / p95 | 34.45–39.84 / 41.49–52.86 ms |

The extra write and space cost is material. These figures establish the cost of this bounded secondary index at the stated frozen source; they do not prove that complete-monitor resource targets are met. Later move/conflict path guards and pinned-reference navigation encoding/fences were not measured in that run; final combined-source reads must cover their additional work. Receiver integration/backfill, sustained writes and final combined-source monitoring still need their own acceptance. A [rejected preliminary run](investigation-index-rejected-minimal-schema.json) is retained transparently: its fixture omitted the receiver's existing Activity owner lookup index, causing repeated synthetic table scans. Its timings are not evidence about receiver index cost.

The [self-contained participant kit](../pilot-kit.html) supplies consent/data-handling instructions, generated scenarios, an answer key and blank observer sheets. Recruitment, consent and observed user outcomes remain external dependencies, with zero observations recorded here.
