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

Every match carries the actual Activity event ID, source ID/type, producer, actor, session, trace and recorded turn reference. CLI and UI integrations use that shared reference for an exact bounded source read. No “nearest event” or broader-session fallback is allowed. Source reads validate receiver/event identity and original plus per-source capture policies before publication, including a final health check. Missing, clipped, unsupported, redacted or pruned evidence remains disclosed as unavailable or partial. Zero matches never establishes that no matching work occurred.

## Validation and measurement

Fourteen initial tests passed for extraction, exact Unicode/case/rename paths, literal phrase behavior, role exclusions, credential redaction, canonical scalar ownership, unknown global workers, stable equal-time paging, update/revocation/deletion, CLI stdout/stderr/exit behavior, receiver replacement and additive source policy. The generated nine-record pilot database also verifies two separately recorded OpenCode-shaped step traces in one explicit synthetic turn.

`scripts/benchmark_investigation.py` measures the added index in isolation: alternating baseline/indexed SQLite builds, identical compact Activity payloads, WAL checkpoints before database-size measurement, and repeated exact/phrase reads. It includes preparation/FTS write cost and the additional database size. It excludes receiver, UI, native discovery and source hydration; it makes no whole-monitor CPU or readiness claim. The final raw samples and host-concurrency boundary will be recorded after an agreed quiet measurement interval.

The [self-contained participant kit](../pilot-kit.html) supplies consent/data-handling instructions, generated scenarios, an answer key and blank observer sheets. Recruitment, consent and observed user outcomes remain external dependencies, with zero observations recorded here.
