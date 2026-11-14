# Findings that keep the right owner and source

14 November 2026. Stage C2 implementation and independent peer review complete; PR preparation. C3 Overview, Stage B resource targets and stages D–G remain open. This report makes no release, adoption or complete-monitor performance claim.

## What changed

Findings now keep each recorded occurrence's producer, actor, session, turn and exact source reference. A shared trace no longer lets a parent borrow a child's error, task description or counts. Records without sufficient ownership remain in the global view and cannot enter an exact-agent/subtree filter by inference. A trace outcome is allocated only by the existing complete whole-trace owner proof; progress or mixed owners revoke that proof.

The same typed operation facts used by Review classify errors here. Commands, exit codes and useful bounded output have provenance; conflicting terminal observations stay uncertain. Simple qualified search/comparison exit 1 may be informational; compound commands and `diff --check` are not excused. Incomplete, redacted or contradictory inputs remain separate observations. Counts describe retained finding occurrences, not guaranteed unique executions when source records cannot prove equivalence. A later success does not delete historical findings.

Credential findings split at exact span-event ownership. The shared sanitizer protects all excerpts, including standalone SDK error bodies and quoted credential-bearing text, before presentation or export. Standalone raw SDK errors survive even without a tool Activity mapping. A source log that safely supersedes its tool span does not return as a generic duplicate. Derived logs retain their actual span pointer.

## One snapshot and a resumable upgrade

`finding_cache` adds scalar producer/actor/session/group keys and owner/group indexes. Scope revision, receiver namespace, page rows and aggregate counts are read under one `BEGIN`; the cursor limits page rows and does not limit aggregate counts. Page/count queries do not hydrate historical spans or borrow trace-summary context. Unknown legacy writes remain readable globally and receive a conservative occurrence key.

Raw span/log/outcome/progress mutations atomically invalidate affected findings and queue both old/new trace identities. The existing bounded worker repairs at most sixteen queued traces per batch. Restart drains durable pending work even when the versioned historical backfill was already complete. Pruned/deleted traces clear their queue entries. Reads report retryable catch-up until current evidence is ready. `trace_derived_v15` performs the existing resumable historical upgrade and retains compatibility completion markers.

## Exact source navigation

In Findings, press `s` or Source to read exact recorded evidence; `t` opens its container trace. Source reads preserve global filters and selection. The source panel supports arrows, Page Up/Down, Home/End, Tab, `r` Refresh and Escape Back at 120×36, 80×24 and 60×18. Strings render as readable text while field paths preserve provenance. Repeated `s` does not stack modals.

The authenticated read-only `/api/findings/<id>/source` endpoint resolves current references in the same snapshot. It reads at most eight references, rejects malformed source identifiers, bounds raw records before JSON decoding, caps each sanitized encoded projection at 8,000 bytes and the combined result below 64 KiB. Omitted, clipped, revoked or pruned evidence has an explicit reason. Selecting a parent span does not expose another worker's events; an event reference resolves only that event.

Receiver/finding/owner identity and both original-finding and per-source capture policy fence asynchronous publication. Revocation clears loaded source content. Copy and export refuse stale/revoked Findings; an export preview that loses permission remains invalid even if policy is later restored, requiring a new review. Author-authored saved task material is outside this source-derived change.

## Verification and limits

The main 157-test storage/API/ownership/native/receipt/UI gate passed in 94.587 seconds. A 40-test latest delta passed in 25.197 seconds. Independent peer review included source-size and credential probes, interleaved writers, restart recovery, malformed legacy references, exact ownership and actual-app navigation/export checks. New source tests use isolated fixtures and private temporary stores; personal telemetry, settings and source review notes remain untouched.

Regression cases cover shared parent/child/unknown owners, malformed session aliases, typed/falsy MCP values, expected versus compound nonzero exits, fallback errors, credential fields, same-turn repeats, keyset paging, one-WAL-snapshot counts, external writers, progress invalidating outcome ownership, deleted traces, upgrades, restart, source limits, late responses, receiver replacement, denial/restoration and export refusal. The additional authenticated source-route gate passed 17 tests in 5.241 seconds. All three updated HTML companions were checked in Chrome at 1280 px/light and 390 px/dark, with no page overflow or external assets. The 60×18 exact-source frame was visually inspected.

Findings analysis still uses the disclosed bounded per-trace evidence window. Absence of a finding does not establish absence of a fault; incomplete sources can describe the same execution. The source resolver is read-only and never retries an agent operation or certifies a worktree.
