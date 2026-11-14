# Integrated derivation economics candidate

The previous frozen e795 full-history HTTP run failed both arms' paced writer deadlines; most later reads ran after writer progress stopped. [The complete failed checkpoint](full-http-e795-invalid.html) and generated-only raw result remain part of the evidence. No loaded p95 was accepted.

## Combined implementation

- Recheck existing dirty state under the writer transaction to coalesce duplicate repairs, retaining missing-cache repair and explicit backfills. Publish accepted span identities in the same transaction as their raw records.
- Reuse connection-local temporary aggregation state and deterministic SQL functions without changing persistent schema, invalidation or 16-trace batching/fairness.
- Keep Unicode/credential semantics while filtering ASCII controls efficiently and recognizing a finite proved-benign set of schema words after budget checks.
- Build bounded sanitized operation facts once per trace for Findings and error-reference consumers. The context holds at most 512 object IDs / 2 MiB sanitized JSON, preserves compact-reference semantics, and clears on every exit.
- Skip impossible MCP execution-association SQL when an exact same-snapshot probe of the existing partial index proves no execution candidates exist. A positive probe retains the full conservative association query; [equivalence and diagnostics](mcp-execution-absence.html) are recorded separately.
- Compute typed terminal labels through one shared parser. Shell exit code 1 retains full command qualification; the generic supersession result comparison uses only the same terminal evidence. No non-generic eligibility shortcut was retained.

Each component has frozen exact-output checks and independent peer review. The short component diagnostics use distinct source/fixture scopes and are not additive or whole-monitor speed claims. Original null results, failed deadlines and exploratory overlaps are retained in their reports.

## Root integration

The combined gate passed **97 tests in 41.067 seconds**, covering ingestion atomicity/rollback, external updates, missing-cache repair, cold and reused aggregation, bounded fact reuse, exhaustive Unicode/ASCII and redaction, frozen terminal/supersession outputs, ownership, incoming catch-up, snapshot readiness, recovery, and history economics. An earlier run passed 94 cases and failed three old test wrappers that did not forward the new internal keyword; those wrappers now forward it, and fixture threads have guaranteed cleanup on assertion failure. No product exception or assertion was removed. The failed gate is retained in `/tmp/agenturm-derived-combined-gate.log`.

The expanded derived/MCP integration gate ran **92 tests in 57.031 seconds**: 91 passed and one older query-plan fixture expected execution joins despite containing no execution candidates. The fixture now supplies an actual linked execution without changing its legacy numeric-source count. Its existing ownership/hydration/trace-index assertions remain intact. The complete affected history/MCP gate then passed **38 tests in 18.611 seconds** with ResourceWarnings treated as errors. The original failed gate is retained in `/tmp/agenturm-derived-mcp-combined-gate.log`; this is not a claim that the full CI matrix is green.

## Small-corpus strict-cadence failures

The unchanged full HTTP cadence ran against a tiny generated corpus in a separate receiver process. Both arms failed: frozen e795 accepted **5 of 420** writes before missing its next slot; combined `029719b` accepted **114 of 420** before missing the same schedule. Candidate POSTs 112/113 took 182/193 ms; the next slot was 175 ms late. Both retained real 503 reads. Of 80 read attempts in each arm, **78 / 56** occurred after the respective writer had already stopped. These unequal-input attempts are invalid: their nominal read percentiles are not loaded latency evidence. The protocol refused burst catch-up and retained every attempted read/write, authentication check, source count, drain and clean child exit.

[Before raw result](full-cadence-small-e795-invalid.json.gz) and [combined raw result](full-cadence-small-029719b-invalid.json.gz) contain only generated data and were checked for credential fields/Bearer values before packaging. The MCP absence optimization was added after those frozen attempts; it is not measured by them.

- `full-cadence-small-e795-invalid.json.gz`: uncompressed SHA-256 `db12a4170eab5c20527aedd1adedd2624121c967df229caadc3601b79a77dac4`.
- `full-cadence-small-029719b-invalid.json.gz`: uncompressed SHA-256 `d29528be7d3d57ee2d1cf8760a3a2d2d0cb8a81d29df2e07a3d964e56cd5a598`.

The next contention change will defer reconciliation of locally owned pending/in-flight jobs until their normal scheduling window, with an atomic commit-to-queue fence and explicit fairness for externally written work. A separate metadata-only initial Agents projection is planned while historical indexing continues. Neither is included in these measurements. Peer review requires no workflow/ledger authority, captured content or fabricated historical zeros from that partial display.

The combined implementation has responsive HTML reports checked at 1280 px/light and 390 px/dark with no document overflow, external asset requests or page errors. Current main96 CI is **not green**: asynchronous readiness and ephemeral status-text fixture failures are being corrected separately with actual HTTP diagnostics. Controlled full-history HTTP, logical-cold/idle resources, storage, terminal lifecycle and one-hour stability remain open. This candidate is unreleased.
