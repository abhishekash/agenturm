# Rust cockpit journey comparison

23 November 2026. Compared the Python 0.2.8 renderer with the joined Linux Rust development build and tested the subsequent Rust search changes. The comparison covers MCP, Metrics, Usage, Findings, Host and Workflow at 120×32, 80×24 and 60×18.

## Evidence

- Python ran the actual Textual cockpit from the tracked 0.2.8 wheel. Its Python source matched the tagged tree at `2802d79930728bb331db1d4d718d6dea1c9c374d`. The wheel SHA-256 was `4fda5471d0a6993e61908949ffc86d7ff28322928071c2463deea774848fba2e`. An in-memory feed supplied synthetic events, a waiting request and a configured MCP entry. The pilot captured populated, empty, loading and receiver-error states at all three sizes. MCP trace navigation returned to the same call.
- Rust ran the actual Linux cockpit in a pseudo-terminal with `NO_COLOR=1`, a disposable native database and recorded fixture inputs. The development executable SHA-256 was `e6feb88973338498ba99c623b3315bbf9147e4c1c6bc409c1dc1a6e7bdd3ced2`. Captures covered populated, empty and loading screens and terminal restoration at all three sizes. The metric and usage qualification also admitted actual JSON and protobuf OTLP requests and opened the exact exemplar at all three sizes. The two fixture inventories differ, so the comparisons concern states and journeys rather than totals.
- The follow-up Rust UI branch passed 59 tests, including exact selection, search key ownership and return paths. Simulated read errors preserve the active evidence view. The PTY excerpts below precede the follow-up search changes. Release archive qualification, an end-to-end rerun of the joined search routes and performance benchmarks are separate gates.

The [Host/Metrics follow-up](rust-host-metric-metadata.md) records the current
joined source checks; captured excerpts below retain their original revision.

## Journeys and remaining differences

| View | Python 0.2.8 | Rust |
| --- | --- | --- |
| MCP | Configured inventory, server and call search across the authorized receiver results, call inspector and trace return. | Configured and observed evidence are separate. `i` opens inventory and `f` selects a server. `/` submits a text query in the existing source, session or turn scope at the selected read snapshot. Linked traces require an exact recorded call and target scope. Escape restores the call selection. |
| Metrics | Local text search across the receiver's loaded catalog of up to 500 instruments, including descriptions. | `/` searches the retained authorized catalog before pagination across instrument, service, kind, unit, scope, source and description. Query/source/snapshot are cursor-bound; `d` opens description details. Schema 21 retains separately validated description companions without rewriting points. The joined frozen development app passed beyond-first-32 search at all three sizes; final optimized acceptance remains. Point and exemplar drilldowns keep the selected series and snapshot. |
| Usage | Seven-day usage modal plus receiver and source context. | Seven UTC days with distinct recorded evidence cohorts and explicit unreported counters. Recorded native usage, reported costs and trace or log instrumentation carry different coverage. An empty series has unreported days. |
| Findings | Search on loaded findings and a source route for authorized saved details. | `/` searches loaded groups by kind, severity, signal and source or one exact occurrence's session, worker and turn. A matching group retains its recorded occurrences. Drilldown includes source, event, revision and snapshot. Exact failed Codex command and failed span sources now use the current-grant private viewer; actual terminal checks cover all three widths, revocation and return selection. Current failed evaluation and explicitly captured generic error-log source routes now reuse that protected viewer. Passing corrections deny stale evaluation detail. Default log capture is metadata-only, with a distinct grant for text. Source-only log and failed-evaluation terminal checks passed all three widths, including revocation/passing correction and same-selection return. The explicit Python receiver Findings integration also passed the actual tagged receiver, all three widths and current owner/member/policy/identity fault checks. Its private authority is polled with bounded display and output, rather than a server-held lock. |
| Host | Live process table searches name, PID, parent PID, project and state. It displays host name and platform when reported. | `H` loads a typed same-user sample on the local Linux writer. Search covers runtime, PID, recorded parent PID, state and permitted working-directory basename. Inspection and refresh use boot ID, PID and process start. Recorded Host history is separate and can be made available through an explicitly enabled reader. Reported local hostname and Linux platform are retained when valid; older missing fields stay unreported. Remote H reads the latest authorized recorded sample per source and labels source/time/age. Multiple streams use a distinct inventory chooser; refresh preserves source through reorder and process drilldown/return. Joined separate-UID KVM and all three terminal sizes passed; final optimized acceptance remains. |
| Workflow | Attention list includes review, seen, locate, resume, refresh and settings controls. | Pending requests retain exact recorded scope. Seen and Locate are available from the workbench. On local history, `c` verifies the selected request and copies a fixed Codex or Claude resume command for its main session. The command preview returns to the same row. Desktop delivery uses the explicit attention command. `S` opens persisted token/USD/minute thresholds, desktop opt-in and ten-minute snooze. Tab switches to typed incidents; `T` retries an exact Failed/Unavailable attempt. The Rust Workflow Incidents list is gaining Python's `h` behavior: hide completions older than ten days by default and toggle them into the list without hiding other incident kinds. Work is on `feature/rust-workflow-history-toggle`; source compiles and formats, while the delegated state/selection/detail checks remain pending. Quiet initial baselines, complete current native evidence, authenticated elapsed clocks and shared quit cancellation gate delivery. The final joined artifact and terminal rerun remain in qualification. |

At 60×18, compact view labels can consume the full tab line. The follow-up avoids a partially rendered trailing tab and keeps numbered navigation and help available. Scoped loading, errors and modal returns keep the selected evidence anchor. The Rust command center follows the separately requested roster layout. The Workflow resume follow-up passed 61 UI tests, including rejection of a different source or read snapshot, selection movement during the read, closing during the read, unsupported sessions and safe command rendering at all three sizes.

## Captured terminal excerpts

### MCP inventory at 60×18

```text
┌ MCP servers · i calls / inventory ───────────────────────┐
│  Server               Evidence         Page calls Errors │
│› idle-provider        Disabled config           —       —│
│  recorded-provider    Observed                  2       0│
```

### Pending Workflow in Python at 60×18

```text
╭──────────────────────────────────────────────────────────╮
│ Workflow · 1 items · ↑↓ · Enter Inspect · Esc Back       │
│  Task / project      Attention             Wait          │
│  Review recorded ap  Approval requested    0m observed   │
│                                                          │
│  Review    Seen     Locate   Resume   Refresh  Settings  │
│ ──────────────────────────────────────────────────────── │
╰─ g Terminal · c Copy resume · h History · Shift+R Revi… ─╯
```

### Retained Usage loading in Rust at 60×18

```text
 Agenturm / Usage
┌Usage─────────────────────────────────────────────────────┐
│Loading history…                                          │
└──────────────────────────────────────────────────────────┘
┌Metric inspector──────────────────────────────────────────┐
```

## Repeatable checks

- [MCP configuration, scope and trace navigation](../../scripts/validate_rust_mcp_inventory.py)
- [OTLP metric points, daily usage and exact exemplar navigation](../../scripts/validate_rust_native_analytics.py)
- [Recorded Host process identity and history](../../scripts/validate_rust_host_history.py)
- [Opt-in remote Host and restored history fencing](../../scripts/validate_rust_remote_host_history.py)
- [Persisted Workflow settings, snooze and incidents](../../scripts/validate_rust_workflow_settings.py)
- [Exact Workflow retry and stale-request denial](../../scripts/validate_rust_workflow_retry.py)
- UI unit suite with `cargo test --offline --locked -p agenturm-ui --lib`
