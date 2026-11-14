# Task usage formatting guard

14 November 2026. Stage B increment based on C3 `39451b1`. This change removes repeated pure usage formatting from the cockpit tick. It does not establish a whole-monitor CPU improvement.

## Boundary

`ObservatoryApp.refresh_task_usage()` previously formatted its full summary and tooltip before checking whether their output had changed. It now compares one bounded, detached input key before calling `task_usage_summary()`. The existing output comparison still avoids unnecessary widget updates.

The retained key belongs to one app instance. It contains the current usage, title, scope, execution identity, receiver namespace, selected source owner, active view and scope, current capture policy, theme, width and optional cost-rate context. Nested in-place changes invalidate the key. The default formatter arguments and recorded-usage accounting are unchanged. Stage E must include any added formatter options in both the input key and formatter call.

Selection, execution allocation and capture policy are evaluated on every call. Health, attention, freshness, source ownership and feed reads are outside this guard. Capture denial clears the retained key and uses a metadata-only usage title, so a stale captured task title cannot return in the tooltip. Metadata-only observations use the normal formatting path on each call. In Traces, the title and capture policy belong to a complete single recorded `review_owner` and the frozen authenticated response namespace. The selected roster source cannot supply another trace’s ownership. Missing, shared or incomplete ownership, missing response namespace and a receiver replacement produce a generic metadata-only trace caption without retaining its raw task title in the key.

## Bounds and refusal

The pure helper admits only explicitly supported JSON-shaped values, with type tags distinguishing booleans, integers, floats and container types. Float keys retain signed zero. Dictionaries use deterministic key order. No original mutable container is retained.

Admission is bounded to 2,048 nodes, 32 KiB of UTF-8 text and depth 16. Cycles, unsupported objects, non-string dictionary keys, nonfinite floats and unusually large integers bypass the guard. An unsupported or oversized input clears the previous key and uses ordinary formatting; it cannot receive a stale cached result. There is no global cache, private framework patch, extra polling, disk access or subprocess work.

## Verification

The focused new and existing usage gate passed **13 tests in 26.294 seconds**. Actual app tests verify that 25 unchanged calls invoke the formatter once, while nested usage/model/owner mutation, source selection, theme, 120/80/60-column resize, namespace replacement and optional rate changes invalidate it. Unsupported data formats on every call. Direct in-place capture revocation removes the private title and retained key immediately. The actual Traces journey keeps roster A selected while opening trace B, then tests B-only capture revocation, receiver replacement with an old frozen response, a refreshed matching response, missing ownership and shared/incomplete allocation. Existing task usage tests preserve cost, token/cache accounting, unknown-versus-zero distinctions, session/worker ownership and inspector/modal/trace selection.

Tests use a generated in-memory feed and app harness. They do not read personal telemetry, write user preferences, contact a provider or emit desktop notifications. The HTML companion embeds its styling and loads no external assets. Its viewport, responsive CSS and light/dark rules passed static verification. Browser visual inspection was unavailable because the browser access host could not be reached.

## Limits

The guard skips pure formatting only. Execution usage selection and live policy checks still run. Large and metadata-only inputs deliberately receive no cache benefit. Legacy or fallback trace summaries without explicit response namespace proof also receive a generic caption. Other inspector formatting remains outside this change. A matched complete-monitor measurement is required before making CPU, memory or latency claims; the broader Stage B resource targets remain open until those measurements pass.
