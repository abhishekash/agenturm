# Rust cockpit UI parity · UI-05 lane notes

23 November 2026 · worktree `work/ui05-screens` · branch tip `179da32` (plan baseline `fd398c1`) · Python oracle `v0.2.8` (`2802d79930728bb331db1d4d718d6dea1c9c374d`)

This lane covers the observability evidence screens: Activity/Operations, MCP, Traces, Metrics, Usage, Findings, Host, retained-evidence paging and history search, and the copy/export scope of those screens. It preserves the pluggable architecture: every new label or control reads typed facts already produced by kernel ports, supervised source adapters and harness adapters. No provider-specific branch was added to a UI module.

The HTML companion is [rust-ui-parity-ui05-lane-notes.html](rust-ui-parity-ui05-lane-notes.html).

## Status

| Surface | This lane | Verified | Remaining |
| --- | --- | --- | --- |
| Activity | Selected-evidence detail composition, empty/truncation copy, status styling for State/Status/Coverage | Ratatui buffer checks at 120×32, 80×24, 60×18 | Page-local filter and prompt-only view need shared `UiModel` state (see hooks 1). |
| Operations | `/` page-local filter over tool, server, result, latency and every recorded evidence field; filter preserved across older/newer pages; explicit Unreported result/server/latency; exact worker scope; linked MCP call identity | Three-width render, filter/clear, page inheritance, selection mapping | Related-evidence key for the generic Operations pane is still gated by the shared shell (hook 2). |
| MCP | One inventory row per exact server name joining configured declarations and observed calls; `Disabled config`, `Configured · idle`, `Observed & config`, `Observed, disabled`; deduplicated client/scope origin; page-local call/error counts | Inventory at three widths; selection and return preserved; server filter/paging inheritance still pass | Aggregate metric-only MCP calls (e.g. Cursor) need an FD2 typed fact; payload-unavailable reason stays in the shell. |
| Traces | Evidence-table detail and return semantics; parent, target source, status and duration remain in the selected tree detail | Existing trace/return journeys still pass | Tree/list keyboard navigation and parent/child focus live in `workspace.rs` and `session_evidence/table.rs`; not changed in this lane. |
| Metrics | Home/End for the catalog, points and scrollable details; existing retained search, descriptions, pinned snapshots, older/newer pages and exemplars retained | Home/End unit check plus existing three-width browser journey | Session-scoped initial metric query is not wired; `MetricBrowser::scoped` exists but the app still starts unscoped. |
| Usage | `v` chart/list toggle; list shows date, value, report count (wide) and per-day markers/legend (compact); unreported, today, retention cutoff, scan limit and incomplete telemetry disclosed; chart/list recorded in export scope | List and chart at 120×32, 80×24, 60×18; mode/cost/currency journeys still pass | Combined and separate cohorts were already present; no new coverage change. |
| Findings | Exact occurrence total in the page summary; group caption count; detail adds occurrence N of M, kind, severity, full scope/source and the granted source route with an explicit unavailable route | Three-width drilldown, filtering and exact occurrence retainment | Retry/policy-change copy is not a Finding UI action; diagnostics surfaces belong to the shell/app lanes. |
| Host | Recorded process and history titles labelled `not live`; recorded provenance (source, viewer-clock age, recorded Unix ns, hostname where it fits) always present for recorded drills; process history says whether the exact source/boot/PID/start is present in the newest loaded sample (`exited or unreadable`) | Recorded identity, source switch, reorder and current-vs-recorded tests at three widths | Live process refresh/reordering was already implemented; current/sample liveness is still never inferred from recorded state. |
| Retained history search | Bounded back stack for investigation pages (max 4) with PgUp; PgDn next; page number and page-local/end-of-scope disclosure; mode/scope/query edits clear the stack; private query preserved across pages | Paging and private-query persistence tests at 120/80/60 | Reconnect/restart preservation remains the shell/app lifecycle contract; late responses are already ignored by the app. |
| Copy/export | `LoadedView::selected_evidence` helper for a single selected row; plain text discloses loaded-page coverage and row count; sanitization and size limits unchanged | Unit export test including control-escape sanitization | The shared shell still exports pane metadata by default (hook 3). |

## What changed

### `rust/agenturm-ui/src/operation_table.rs`

- MCP inventory is now one row per exact recorded server name, matching the Python inventory rather than emitting a configured row and a separate observed row. Declarations are deduplicated by `(client, scope)`; the loaded page contributes exact call and error counts.
- States are explicit: `Disabled config` (disabled, no loaded calls), `Configured · idle` (enabled/unknown, no loaded calls), `Observed & config`, `Observed, disabled`, and `Observed` for names with calls but no declaration.
- Non-MCP operations panes (`Activity`/`Operations`) own a bounded `/` filter. Every whitespace-separated query word must match some recorded field in the row (tool, server, result, latency or evidence value). `Esc` clears the applied filter; `Enter` applies it. `Browser::inherit_filters` carries the committed query onto older/newer pages and preserves the nearest visible selection.
- The operation detail panel always renders `Server`, `Result`, `Latency` and `Worker`; absent values read `Unreported`, and a missing worker reads `Unreported (main session or not recorded)`. The exact linked retained MCP call (`source / event rN`) is shown when the row carries `mcp_event`; otherwise the panel says there is no linked MCP call.

### `rust/agenturm-ui/src/findings.rs`

- The page summary now reports groups, total exact occurrences and examined observations separately, so a group count is never mistaken for the occurrence count.
- The selected-group caption names the source, signal, exact occurrence count and the route into exact evidence.
- Occurrence detail adds `Occurrence N of M on this loaded page`, `Kind`, `Severity`, `Scope`, `Event`, `Revision`, `Snapshot`, session/turn/worker/operation identities and a `Source route` line derived from the typed `FindingSource` variant. An absent source is reported as unavailable, not as a silent omission.

### `rust/agenturm-ui/src/metric_history.rs`

- Home/End select the first/last catalog series or point in the current page; the same keys scroll series details, point details and exemplar traces to the start/end. Hints name the behavior.

### `rust/agenturm-ui/src/usage_chart.rs` and `workspace/analytics.rs`

- `v` toggles a seven-day list beside the existing chart. The list shows the selected measure, each UTC date and value, the report count where width permits, and per-day coverage. Compact rows use `—`, `†`, `*` and `‡` markers with a legend; wide rows spell out unreported, today, retention/scan limits and incomplete telemetry.
- The export document scope records `chart` or `list`, so a copied/exported document cannot masquerade as the other representation. Contextual help names `v`, Home/End and the retained-page keys.

### `rust/agenturm-ui/src/workbench/host.rs`

- Recorded process pages are titled `Recorded host processes · not live` (also `partial · not live`); recorded process history is `Recorded process history · loaded samples · not live`.
- Recorded process drills always show their source, viewer-clock age and recorded Unix timestamp. Hostname appears where the exact sample carries it and the compact inspector has room.
- Process history adds `Latest loaded sample`: present for the exact source/boot/PID/start, absent with `exited or unreadable`, or unavailable when the identity is not in the loaded page. Current-sample and recorded-sample presentations remain separate.

### `rust/agenturm-ui/src/workbench/investigation.rs`

- Investigation pages now form a bounded back stack (four pages). `PgDn` moves to the next bounded page; `PgUp` returns to the stored previous page without re-reading. The footer shows `Page N`, `PgUp previous`, `PgDn more`, `page-local` or `end of scope`.
- Changing mode with Tab/Shift+Tab, changing the failures scope with `s`, editing or clearing a private query, or pressing Enter for a fresh search resets the page stack. The exact private query and mode are preserved when paging.
- History identity and the pinned snapshot are still carried by every read request; page-local limits remain explicit.

### `rust/agenturm-ui/src/evidence_view.rs`

- Selected detail keeps page context before the selected row's recorded fields. An empty detail now says `No recorded detail for this selected row` and tells the user to select another row or press Enter. Truncation hints distinguish an expandable inspector (`Enter to read the full detail`) from one that only needs more width.
- State/Status/Coverage columns join Result/Severity/Root result in status styling.

### `rust/agenturm-ui/src/loaded_view.rs`

- `LoadedView::selected_evidence(title, scope, row, position, total)` builds an explicit single-row export document that names the selected position and says `loaded page only`.
- `plain_text` now always discloses row count with coverage. Escaping, clipboard encoding and export limits are unchanged.

## Verification

Commands run in `/tmp/agenturm-ui05-screens/rust` with the pinned 1.100.0 toolchain and offline locked dependencies:

```text
cargo build --offline --locked
  Finished `dev` profile … in 40s (agenturm-ui and agenturm-app compiled)

cargo test --offline --locked -p agenturm-ui --lib
  test result: ok. 86 passed; 0 failed; 0 ignored
  (baseline before this lane: 76 passed)

cargo test --offline --locked -p agenturm-app --lib findings
  test result: ok. 12 passed; 0 failed

cargo test --offline --locked -p agenturm-app --lib mcp_inventory
  test result: ok. 4 passed; 0 failed

cargo test --offline --locked -p agenturm-app --lib host
  test result: ok. 12 passed; 0 failed

cargo test --offline --locked -p agenturm-app --lib investigate
  test result: ok. 1 passed; 0 failed

cargo test --offline --locked -p agenturm-app --lib usage_history
  test result: ok. 0 passed; 0 failed (no unit tests in that module)
```

The UI suite now includes targeted checks for:

- merged MCP inventory states and three-width inventory rendering;
- Operations local filtering, clearing, selection and cross-page filter inheritance;
- explicit Unreported operation attribution and linked-call detail;
- Findings occurrence totals, exact-scope detail and source-route disclosure;
- metric catalog/detail Home/End behavior;
- usage list markers, coverage legend and chart/list toggle at three widths;
- recorded Host provenance, three-width process identity, and an exact process missing from the newest loaded sample;
- investigation paging, page boundaries and private query persistence;
- selected-evidence export scope and control-escape sanitization;
- evidence-table empty detail and truncation copy.

Not run here: full workspace tests, PTY/tmux acceptance, paired Python frames, installed-candidate acceptance and the final optimized release gate. Those remain UI-Q.

## Remaining gaps and ownership

1. **Activity page-local filter and prompt-only view.** The Activity body is an `EvidenceTable` stored in the shared `UiModel`; there is no per-table filter field and `lib.rs` owns `evidence_key`. Needs hook 1.
2. **Trace tree/list navigation, parent/child focus and linked-evidence traversal.** Trace rows are built and drawn by `workspace.rs` and `rust/agenturm-app/src/session_evidence/table.rs`, outside this lane. The current detail already carries parent span, target source, status and duration; missing is tree navigation and a stable return identity across Activity ⇄ call ⇄ exemplar ⇄ trace.
3. **Generic Operations related-evidence key.** The shell only accepts `t` when the operation table `is_mcp()`, although an Operations row can carry the same typed `mcp_event`. The detail panel now renders the exact call; the key remains shell-gated (hook 2).
4. **Selection-scoped copy/export.** `workspace/view_io.rs` still exports pane metadata. The new `LoadedView::selected_evidence` helper is ready for the shell to prefer a selected evidence row and disclose the scope (hook 3).
5. **MCP aggregate metric calls.** Python shows aggregate metric-only MCP calls (e.g. Cursor). The Rust UI has no typed fact for that cohort, so the inventory deliberately shows loaded-call counts only and says so. Provider-specific payload viewing remains behind FD2 grants.
6. **Session-scoped Metrics.** `MetricBrowser::scoped` and source-filtered queries exist, but the app starts the catalog unscoped and there is no session Metrics view. Needs app/shared routing; not changed here.
7. **Diagnostics, health, reconnect and receiver-retry copy.** These surfaces are not in this lane's files. Findings retry/policy text is likewise not a Finding action.
8. **Activity worker attribution.** The Activity row details are built by `session_evidence/table.rs`; an absent worker is omitted there rather than rendered as Unreported. Operations now do this explicitly (hook 4 if Activity should match exactly).

## Shared-shell hooks for UI-02

These are the exact integration points. None of them requires a provider branch.

1. **Evidence-table local filter (Activity, Traces, Usage, Findings tables).**
   - Add per-pane filter state to the shared model, e.g. `evidence_filter: BTreeMap<ExtensionId, pane_filter::Filter>` alongside `evidence` in `rust/agenturm-ui/src/lib.rs`.
   - In `metadata_filter_key` (`workspace.rs`), when the selected pane has an `EvidenceTable`, route `/` and subsequent editing keys exactly like `OperationList::search_key`'s local branch: apply `Filter::matches` to `cells` plus every `MetricRow` label/value, remap selection with `pane_filter::selection`, show `Filter page: …`, clear on `Esc`, and carry the committed filter in `HistoryView::inherit_filters` so history pages preserve it.
   - The visibility count and `Page` wording should distinguish page-local rows from scope counts.
2. **Operation related evidence.** In the `workspace.rs` operation block, replace the `code == 't' && table.is_mcp()` guard with a check for a selected row's typed `mcp_event` (`table.selected_row().and_then(|row| row.mcp_event.as_ref())`). Keep the existing `OperationDetailKind::Mcp` matcher; it already compares source, event and revision. Then the detail line `t opens the linked retained trace` is true for Operations as well.
3. **Selection-scoped copy/export.** In `workspace/view_io.rs::loaded_view`, before the pane fallback:
   - if an `EvidenceTable` is selected and `open_inspection(selected)` is possible, return `LoadedView::selected_evidence(pane.title, scope, row, selected, table.rows.len())`;
   - if an `OperationList` row is selected, export that row's `evidence` with a scope naming the operation, source and page;
   - keep the loaded-page pane document as the fallback and show the returned `scope` in the export form so format/destination still name what is written.
4. **Contextual help and footer.** Add `Home/End` to the global metrics help, `v` to Usage (done locally in `HistoryAnalytics::help`), `PgUp/PgDn` to the `Ctrl+F` help line, and `/ filter · Esc clear` to the Activity/Operations help. Host help should keep the `/` and history-paging hints.
5. **Trace navigation.** When trace work is scheduled, add the tree state in `workspace.rs` and typed child/parent navigation over `TraceSpan.parent_span_id`; carry `HistorySelection` identity plus selected span/trace so return restores the same tree node. No new provider grant is needed.
6. **Activity attribution parity.** If Activity should state absent workers explicitly, add `("Worker", "Unreported")` in `session_evidence/table.rs` rather than inferring from the UI.

## Pluggable-seam notes

- **Inventory** is joined only by the typed `ConfiguredMcpServer.name` and `OperationRow.server`, with `enabled: Option<bool>` from the explicit configuration reader. Counts remain page-local; no MCP client is special-cased. Aggregate metric-only calls are deliberately not inferred.
- **Findings routes** are selected from the existing `FindingSource::{Command, Telemetry, Log, Evaluation}` enum. The UI names the grant route but never opens a payload itself; `s` still goes through the current-grant private viewer.
- **Usage** list arithmetic uses `UsageDay`, `UsageTotals`, `CombinedUsageTotals` and `UsageCurrencyTotal`; coverage markers come from `retention_partial`, `scan_partial`, `partial_traces` and `cost_currencies_truncated`. No provider-specific cost or token math was introduced.
- **Host** current-versus-recorded is derived from `CurrentHostSample.recorded_source` and exact `HostSample { boot_id, pid, start_ticks }` identity. Recorded evidence never claims liveness; the newest-sample presence is an explicit recorded fact.
- **Investigation** keeps the typed `history_identity`, `ReadSnapshot`, cursor and owner scope in every request. The page stack is UI state only; the app remains the grant and scope authority.
- **No kernel port, adapter, plugin or process-plugin host changes** were needed. Remaining typed-fact gaps are the MCP aggregate metric cohort (FD2) and any provider payload reason already owned by FD2.

## Acceptance journeys

All keys assume the current Rust workspace dispatch; Esc returns one layer at a time.

- **MCP.** `3` → `i`: idle/disabled/observed states, origin, page calls and errors. `Esc`, `f` cycles server filters, `/query` + Enter searches the retained scope, Enter inspects the call, `t` follows the linked trace, Esc restores the same server/query/call.
- **Operations.** Open Operations, `/worker` + Enter filters the loaded page; Enter inspects and shows Result/Worker/Trace (Unreported where absent); Esc clears the filter; `[`/`]` keeps the same filter.
- **Metrics.** `6` → `/description` + Enter, `Home`/`End`, `[`/`]`, Enter opens points, `e` cycles exemplars, Enter opens the exact span, three Escs return to the same series and snapshot.
- **Usage.** Open Usage → `v` for list, `s` for combined/native/span/trace/log, `t` for counters, `n` for totals, `C` for cost currency. Check `Unreported`, `†`, `*`, `‡` at 60×18 and with `NO_COLOR=1`.
- **Findings.** `7` → Enter occurrences → Enter detail; verify occurrence N of M, scope, source and source route; `s` opens the granted source; Esc returns to the same occurrence and group.
- **Host.** `5` or `H` → `/` searches the current sample; record history with `H`/reader and open a sample → process → history; verify `not live`, source/age, and `Latest loaded sample` present or exited. Esc returns through the same process.
- **History search.** `Ctrl+F` → mode → query → Enter; `PgDn` shows `Page 2` and `PgUp previous`; `PgUp` restores the exact prior page and query; Esc closes.
- **Copy/export.** `c` requests an OSC 52 write and reports terminal support; `x` opens the destination form and writes MD/HTML/JSON. Until hook 3 lands, the document is the loaded page; the helper now makes the selected row form available.

## Files changed

```text
rust/agenturm-ui/src/evidence_view.rs
rust/agenturm-ui/src/findings.rs
rust/agenturm-ui/src/loaded_view.rs
rust/agenturm-ui/src/metric_history.rs
rust/agenturm-ui/src/operation_table.rs
rust/agenturm-ui/src/usage_chart.rs
rust/agenturm-ui/src/workbench/host.rs
rust/agenturm-ui/src/workbench/investigation.rs
rust/agenturm-ui/src/workspace/analytics.rs
docs/validation/rust-ui-parity-ui05-lane-notes.md
docs/validation/rust-ui-parity-ui05-lane-notes.html
```

No shared shell file was edited: `keymap.rs`, `workspace.rs`, `workbench.rs`, `lib.rs`, `theme.rs` and UI-06 workflow/task modules remain owned by their lanes.
