# Recorded work before completion claims

14 November 2026. Stage C3 implementation: source-backed Agents Overview, shared source preparation for terminal and headless consumers, and transaction-fenced native log ingestion. Stage B resource targets and integration of stages D–G remain open.

## What users see

The compact inspector and expanded Overview lead with recorded file-operation and path counts, validation outcomes and issues. The expanded view lists up to five paths and the latest three checks before the agent-reported narrative. “All tests pass” in an assistant message cannot create a passing check. Check results without typed terminal evidence remain unknown. The existing Review action provides scoped paging and exact source references.

The summary uses the same Review projection and typed operation facts as the rest of the application. It requires receiver, producer, actor, session and execution ownership; a recorded turn joins steps only under a complete matching owner proof. Duplicate source updates use the latest recorded timestamp. Capture revocation suppresses source content. Candidate inspection stops at 1,000 records, the summary retains at most 200 owned records, and clipped or unloaded history stays explicitly partial. The view does not certify current worktree correctness or whole-run capture.

At 120×36 the roster and compact inspector remain side by side. At 80×24 and 60×18, `e` opens the expanded inspector and Escape returns to the preserved agent selection. The summary reuses existing sections and introduces no new panel, status badge or border. Narrow frames keep the existing single-pane drilldown and scroll controls.

## Shared projection and receiver fence

The UI-free `source_projection` module owns bounded roster preparation, current health and attention classification. Terminal imports remain compatible; headless consumers can use the identical preparation without importing Textual. Static cache behavior and fresh capture policy remain unchanged.

Health supplies a typed receiver clock and explicit indexing readiness. Native JSON and protobuf log writes may pin `X-Agenturm-Expected-Receiver-ID`; the store checks the current namespace inside the same immediate write transaction, before policy changes or inserts. A replaced receiver rejects the complete stale batch. Existing OTLP clients without the optional header retain their prior behavior.

## Verification and limits

The 44-test source/cache/session-view gate passed in 82.284 seconds. The latest 11-test pure projection/source/clock gate passed in 2.276 seconds. The actual-app Overview journey passed at 120×36, 80×24 and 60×18 in 17.640 seconds; the 120-column and 60-column frames were visually inspected using generated telemetry. Three direct-store and authenticated JSON/protobuf receiver-fence cases passed in 1.335 seconds. A subprocess test confirms source preparation imports no Textual modules.

Fixtures cover false assistant claims, typed/unknown check results, foreign owners and executions, partial owner proof, duplicate updates, bounded infinite candidates, capture denial and preserved keyboard selection. Personal telemetry, preferences and source review notes remain untouched. These checks establish behavior, not complete-monitor CPU improvements, live runtime compatibility or participant adoption.
