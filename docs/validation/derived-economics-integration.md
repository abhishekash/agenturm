# Integrated derivation economics candidate

The previous frozen e795 full-history HTTP run failed both arms' paced writer deadlines; most later reads ran after writer progress stopped. [The complete failed checkpoint](full-http-e795-invalid.html) and generated-only raw result remain part of the evidence. No loaded p95 was accepted.

## Combined implementation

- Recheck existing dirty state under the writer transaction to coalesce duplicate repairs, retaining missing-cache repair and explicit backfills. Publish accepted span identities in the same transaction as their raw records.
- Reuse connection-local temporary aggregation state and deterministic SQL functions without changing persistent schema, invalidation or 16-trace batching/fairness.
- Keep Unicode/credential semantics while filtering ASCII controls efficiently and recognizing a finite proved-benign set of schema words after budget checks.
- Build bounded sanitized operation facts once per trace for Findings and error-reference consumers. The context holds at most 512 object IDs / 2 MiB sanitized JSON, preserves compact-reference semantics, and clears on every exit.
- Compute typed terminal labels through one shared parser. Shell exit code 1 retains full command qualification; the generic supersession result comparison uses only the same terminal evidence. No non-generic eligibility shortcut was retained.

Each component has frozen exact-output checks and independent peer review. The short component diagnostics use distinct source/fixture scopes and are not additive or whole-monitor speed claims. Original null results, failed deadlines and exploratory overlaps are retained in their reports.

## Root integration

The combined gate passed **97 tests in 41.067 seconds**, covering ingestion atomicity/rollback, external updates, missing-cache repair, cold and reused aggregation, bounded fact reuse, exhaustive Unicode/ASCII and redaction, frozen terminal/supersession outputs, ownership, incoming catch-up, snapshot readiness, recovery, and history economics. An earlier run passed 94 cases and failed three old test wrappers that did not forward the new internal keyword; those wrappers now forward it, and fixture threads have guaranteed cleanup on assertion failure. No product exception or assertion was removed. The failed gate is retained in `/tmp/agenturm-derived-combined-gate.log`.

The next check uses the unchanged full HTTP cadence and strict deadline/failure accounting on a small generated corpus, with a separate receiver process and exact frozen source hashes. It is a development check, **not** full-history acceptance. Controlled full-history HTTP, logical-cold/idle resources, storage, terminal lifecycle and one-hour stability remain open. This candidate is unreleased.
