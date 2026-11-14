# Shared native attention projection

14 November 2026. Stage F integration, dependent on the receiver inventory introduced by `818968f` and hardened by `f20fd94`. This closes the state-projection boundary between Agents, Review, Workflow and the headless watcher. It does not add approval, remediation or agent execution.

## One request state

The pure source projection applies validated native request metadata after current captured health and before attention flags and sorting. Only the exact producer, actor, session and recorded run/turn match. Relevant sources reuse their existing recorded execution anchor and proved turn allocation; no nearby event or latest native turn is selected. Review refresh applies the same overlay to its exact candidate before its existing freshness and scope guards. Headless attention merges the same remote observations with its durable local hook observations.

A pending explicit request survives missing later observations. Only a matching positive request resolution suppresses that request. A resolution does not invent a new running or completed state. Strong captured terminal outcomes, delayed sources, foreign identities, conflicting sender/order evidence and multiple ambiguous pending requests retain the existing conservative precedence rules. Approval still occurs in the native agent terminal.

## Beyond the latest Activity page

When full health advertises `native_lifecycle_v1: true`, Feed and the headless reader fetch `/api/native-lifecycle`. The response must contain the exact receiver namespace, a strictly typed completeness flag and no more than 1,000 versioned metadata states. Full health independently confirms the receiver after the read; the lightweight liveness probe is not identity proof. Legacy receivers retain existing discovery without guessed native capabilities.

The final Feed health check bypasses per-refresh request deduplication, applies the current capture policy and rechecks navigation/cancellation before publishing a new snapshot.

The receiver-owned inventory preserves source-backed request and resolution states independently of the latest 200 Activity rows. A restarted remote watcher therefore needs neither a local hook binding nor a recent native Activity row to recover the pending request or its later exact resolution. The receiver storage/index implementation and its maintenance belong to the separate dependency; this change supplies the consumer gates.

Feed and the runner also retain a bounded local metadata view across polls. It contains request IDs, exact scope, source time/clock and sender ordering, with no prompt/body, provider key or receiver-receipt timestamp. Pending states and exact resolution tombstones are kept rather than evicted to make room. Conflicted senders remain quarantined for the receiver generation. Overflow, an incomplete inventory, malformed scope/version metadata or a failed inventory read makes coverage visibly incomplete and disables baseline finalization and desktop delivery. An inventory read failure preserves already observed pending requests. Recovery from a transient read failure clears its diagnostic; absence never implies resolution.

## UI and privacy

Workflow storage is read only by its serialized background worker. The worker publishes detached metadata, bound to its receiver. UI source preparation never opens the Workflow database.

The app reuses at most two shallow overlay snapshots for the same source snapshot and metadata. This preserves the existing source-preparation cache rather than creating a new snapshot on every consumer call. Applying the metadata before `display_snapshot()` lets Pause freeze the displayed request state while live Workflow observation continues. Capture-policy revocation still applies to the frozen display immediately, and clears the overlay cache. Internal usage preparation is removed from published source items, preventing stale usage task text from reappearing through a metadata-only source.

No new modal, action key or navigation stack is added. Native coverage diagnostics describe collection quality; they do not become attention incidents or desktop alerts.

## Verification and limits

The source/cache/attention/controller/native/Review compatibility gate passed **69 tests in 52.271 seconds**. The combined receiver inventory, actual authenticated HTTP watcher and new projection gate passed **26 tests in 9.270 seconds**. Coverage includes exact request/resolution parity, foreign namespace and actor rejection, sender/order conflicts, malformed version/turn metadata, a 1,000-state cap, local cache reuse, direct capture revocation, actual Pause behavior, and an assertion that Workflow native reads occur off the UI thread. A real remote-watch journey hides the request and resolution behind 210 newer unrelated records, restarts the watcher without a local hook binding, and verifies one notification attempt followed by exact resolution. The final two actual UI journeys passed in 7.703 seconds: Pause with immediate capture revocation, and Review refresh retaining a worker-published request before the next Feed poll. The latter uses only the shared metadata wrapper at both Review publication boundaries, with the existing harness fallback preserved.

All tests use temporary receiver/Workflow stores, fixture telemetry and app harnesses. Notifications are mocked. Personal telemetry, source review notes and preferences are untouched. The HTML companion embeds its CSS and has responsive light/dark rules; browser visual inspection remains unavailable because the browser access host could not be reached.

The 1,000-state inventory is an explicit limit, not an all-history completeness claim. Reaching it defers delivery and exposes coverage rather than silently dropping pending requests. Missing or unsupported runtime approval identities remain unknown; this integration does not establish live Claude or Pi hook support. Complete-monitor CPU, RSS and latency effects require a separate matched measurement; no performance improvement is claimed here.


## Production Review and traversal-budget correction

Full CI found a real minimal-snapshot regression in headless Review and a second traversal of the Activity list during native overlay preparation. Both are corrected: preparation gathers only metadata native rows during its existing first bounded pass; detached cache snapshots preserve receiver identity; minimal contexts need no Activity member. The CLI reads optional advertised native inventory through the shared validator and preserves final receiver/capture fences and explicit unavailable/partial notes. It never backfills Review content with native bookkeeping. After exact scope equality, Review promotes the already-proven execution anchor before applying the same overlay used by Agents. This covers log-only executions whose numeric usage is unavailable.

The original three full-CI failures pass. The agent's 68-case source/cache/native/Review/capture gate and 26-case inventory/turn/continuity delta passed. After rebasing onto main through PR83 and combining investigation/native dirty queues, the **57-case Review/native/investigation/watcher gate passed in 15.042 seconds**. The existing 400-event test still asserts exactly 400 visits. Actual authenticated HTTP and 80×24 UI tests verify matching pending state across consumers; zero content records can remain zero. [Correction report](native-review-projection-fix.html). New-head full CI and combined resource acceptance remain open.
