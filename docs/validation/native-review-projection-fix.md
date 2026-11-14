# Native Review and source-pass correction

Parent source: PR82 head `bd90dcb7809a1575ef8cccfef3452cef32a3724c`. Scope: source preparation, exact Review collection and the smallest shared ReviewFacts execution-anchor hook. No database/schema, producer matching, receiver startup or UI layout changes.

## Problem and resulting behavior

Full PR82 CI exposed two regressions across Python 3.11–3.14: exact headless Review crashed on its deliberately lightweight snapshot, and roster preparation traversed the full Activity collection twice. Both production boundaries are corrected. Missing Activity is an empty optional input to native metadata preparation; the existing source-grouping pass also retains only a bounded native metadata subset for the roster overlay. The exact 400-entry visit budget remains 400.

Headless Review now reads the dedicated native inventory only when the receiver advertises it. It uses the existing shared validator, accepts at most 1,000 metadata states, matches receiver/producer/actor/session/trace/turn and retains final health namespace/capture-policy checks. Optional inventory failure, malformed/partial response or capability changes disclose uncertainty in the evidence window. Absence does not resolve a request. A foreign receiver or late capture-policy change prevents publication.

The narrower Review content page intentionally excludes native lifecycle bookkeeping. A real authenticated HTTP/CLI fixture can therefore report the captured request state while its Review Activity count remains zero. The collector does not backfill general Activity, synthesize source records or import the terminal UI.

## Exact scope and cache invariants

The source-preparation return value gains a third private tuple element, containing only whitelisted metadata and independently recorded source identity. Native rows are collected within the same first 1,000 Activity positions previously admitted by the native merger. No prompt, detail, body or unknown envelope key enters this metadata subset. Existing source caches still bind two detached snapshot generations, receiver/scope/identity/text mutations and live capture denial; nested changes invalidate the preparation and consumers receive detached output.

A separate actual HTTP parity case found a native row with a proved `latest_trace_id` but no `trace_id` and unavailable exact usage. The roster already promoted that execution anchor before the shared native overlay; ReviewFacts did not. The approved three-line shared hook promotes it only after exact scope equality and a positive run ID, without borrowing usage or broadening matching. Review CLI applies the same overlay after authoritative whole-turn membership is resolved, preserving exact turn matching.

## Verification

The original three failed CI cases pass in a 26-case quick gate (2.330 seconds). The source/cache/native/Review/CLI/capture gate passes 68 cases (36.139 seconds), including a real 80×24 Review refresh and paused policy revocation. The final whole-turn, optional-inventory and continuity delta passes 26 cases (20.905 seconds). Gates overlap and are not summed as unique coverage.

```sh
python -m unittest tests.test_native_review_projection_fix tests.test_terminal_review.SessionWorkBudgetTest tests.test_work_receipts tests.test_native_review_contract tests.test_native_projection tests.test_source_preparation -q
python -m unittest tests.test_native_review_projection_fix.ReviewInventoryFixTests tests.test_opencode_turn_review tests.test_turn_task_continuity -q
```

New regression contracts include minimal namespaces without Activity; exact 400 visits with a native record; metadata-only retention; first-1,000 compatibility and legacy envelope defaults; nested request/receiver cache invalidation; roster/Review/headless parity; unadvertised inventory; failed/partial/malformed/oversized inventory; exact foreign owner/execution/receiver rejection; capture denial after inventory or Activity; changed capability; and an actual authenticated Store→HTTP→Review subprocess. The existing producer-shaped CLI test still verifies checks, edits, request text, usage and saved-task composition.

## Limits

These focused results correct PR82's reproduced failures. Full new-head CI and combined-tree acceptance remain required. Native hooks still require authoritative identities and order; no approval is executed. Unknown or incomplete inventory does not establish resolution, complete capture or accurate desktop delivery. No new performance percentage, whole-monitor CPU/cold-readiness or memory/WAL claim follows from restoring the existing one-pass budget.
