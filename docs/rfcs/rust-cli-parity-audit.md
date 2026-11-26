# AP1 — SDK/CLI parity audit: Python 0.2.8 against the Rust rewrite

Updated 23 November 2026. This is the **AP1** audit required by the
[remaining-work plan](rust-migration-remaining-plan.md) and the
[execution plan](rust-only-execution-plan.md): reconcile every retained
Python 0.2.8 CLI entry point, cockpit keyboard journey, and selected SDK
integration with its actual Rust code path. Every **implemented** row was
verified by reading the Rust parse/dispatch code, not inferred from
documentation. The audit records dispositions; confirmed missing journeys
remain on the active Python-parity plan unless an explicit product decision
retires them.

## Method and boundaries

- Python surface: `observatory/__main__.py`, `observatory/attention_cli.py`,
  `review_cli.py`, `investigation_cli.py`, `task_cli.py`,
  `native_lifecycle.py`, and the journeys in
  [keyboard navigation](https://github.com/abhishekash/agenturm/blob/main/docs/keyboard-navigation.md).
- Rust surface: `rust/agenturm-app/src/lib.rs` Command enum (:237-302),
  `main.rs` dispatch (:27-300), `parse_command` (:849-947), and the
  `agenturm-ui` keymap/workbench/workspace handlers.
- Base comparison: the [cutover workflows table](rust-cutover-workflows.md).
- Python receiver-attach forms (`--receiver-protocol python-v0.2.8`) are
  Linux-only in the Rust app (`#[cfg(target_os = "linux")]`, lib.rs:863-905).
- Classifications are exactly: **implemented**, **test-only acceptance
  pending**, **confirmed missing feature**, **scope decision needed**. No CLI
  or SDK row required the *native evidence unavailable* class in this audit;
  that class stays reserved for harness-side items such as the actual Kiro
  transcript (A4/Q8), which this audit does not cover.

## Parity table

| Surface | Python 0.2.8 | Rust counterpart | Classification | Anchor |
| --- | --- | --- | --- | --- |
| bare `agenturm` / `cockpit` / `--db` | Opens cockpit, starts/attaches receiver | Bare TTY launch defaults to `cockpit`; `cockpit [--db]` | implemented | main.rs:16; lib.rs:880 |
| `cockpit --endpoint` | Attach to Python receiver | `--endpoint --token-file --receiver-protocol` → RemoteCockpit (Linux) | implemented | lib.rs:886-892 |
| `cockpit --once` | One snapshot, exit | `cockpit --once [--json] [--limit]` → Summary | implemented | summary_cli.rs:17-42 |
| `cockpit --new-window` | New terminal window | `--new-window` → Window (Linux launchers) | implemented (Linux) | new_window.rs |
| `cockpit --port` | Receiver port choice | none as a flag; receiver via `serve-traces --listen` | scope decision needed | see dispositions |
| `cockpit --no-local-agents` / `--metadata-only` | Disable discovery / omit native text | none as flags; inverted explicit-grant capture model | scope decision needed | see dispositions |
| `cockpit --retention-days` | Opt DB into age retention | `retention set --max-age-days N` | implemented (different command) | retention_cli.rs:66-98 |
| `cockpit --capability-config` | Capability pane activation (repeatable TOML) | Process-extension catalog `--extension-catalog`/`--extension` | implemented (different mechanism) | lib.rs:1243-1283 |
| `serve` | OTLP/HTTP receiver: traces/logs/metrics, auth, retention | `serve-traces --listen --db --source-id [--capture-log-text] [--payload-token-file]` | implemented for retained scope; Python `/api/*` wire, remote TLS, arbitrary legacy JSONL tail stay open (cutover:18) | trace_receiver.rs:165-215 |
| `serve --watch-jsonl` / `--watch-service` | Tail harness JSONL as spans | `watch-jsonl --file --db --source-id [--interval-ms]` | implemented (subset) | span_jsonl_follow.rs:21-46 |
| `serve --host/--port/--token/--no-auth/--allowed-host` | Remote binding, auth modes | `--listen` loopback-only + `--token-file` | confirmed missing for remote deployment (recorded open) | trace_receiver.rs:209-214 |
| `serve --retention-days` | Age deletion | `retention set/run/status` via collector/broker ticks | implemented | retention_cli.rs:88-95 |
| `serve --plugin-config` / `--rust-plugin-host[-sha256]` / `--use-bundled-rust-plugin-host` | Python plugin opt-ins | Rust-native plugin host + `collect --config` catalog with SHA-256 pins | implemented (different mechanism) | lib.rs:367-381 |
| `migrate-db` | Copy Python telemetry DB | none — retired by the Rust-only product decision; `backup`/`restore` cover Rust state | confirmed missing (retired by explicit decision, cutover:22) | cutover:22 |
| `run` | Launch agent with automatic SDK capture | `run [--db|--endpoint --token-file] --source-id [--instrument python] -- <program>` | implemented | trace_run.rs:59-135 |
| `run --transport otlp` identity flags | Structural spans with exact linkage | Same flags honored through the Rust launcher | implemented | docs/python.md:77-148 |
| `import-jsonl` | Import span JSONL | `import-jsonl` bounded ≤4 MiB/≤1024 spans, opt-in content | implemented | span_jsonl_import.rs:51-91 |
| `import-evals` | Join eval outcomes to traces | `import-evals` + `evals` views + private `eval-detail` grant/preview/revoke | implemented (superset) | eval_import.rs:50-95 |
| `prune --older-than-days` | Delete older telemetry | `prune --before-unix-ns` + persisted retention | implemented (unit changed: ns) | main.rs:221-227 |
| `attention watch` | Poll receiver; durable store; `--desktop --once --interval` | `attention watch --db [--interval-ms] [--max-polls] [--desktop] [--once]`; remote attach; `--workflow` body-free status | implemented; legacy workflow-state migration stays open (cutover:24) | attention.rs:58-166 |
| `attention status` | Offline durable records | `attention status --db`; `--workflow` | implemented | attention.rs:78-101 |
| `attention native bind/send/hook/retry` | Explicit owner-only native bindings | Same verbs, Linux-only, `--token-file` required, adapters `generic-v1`/`codex-notify-v1` | implemented; disposition now recorded here (previously unnamed in the RFCs) | attention_native.rs:162-248 |
| `review` | Exact-run evidence from receiver | Native `review --db --harness-id --session-id …`; attached Rust receiver; Python-receiver mode keeps exact Python flags (Linux) | implemented | lib.rs:1523-1586; remote_review.rs:33-110 |
| `investigate path\|command\|instruction\|source` | Bounded retained search | Native kinds incl. `failures`; Python-receiver mode keeps Python flags (Linux) | implemented (adds `failures`) | investigate_cli.rs:149-219 |
| `task create/list/show/update/archive` | Offline saved tasks | `task add/create/list/show/update/archive`; cursor paging via `--limit/--before` (no `--offset`) | implemented (paging model differs) | task_cli.rs:240-283 |
| `task create --snapshot` | Opt-in Git capture after creation | `--snapshot` captures the supplied local workspace; without it, create stays metadata-only. A capture failure returns the saved task plus `snapshot_error`. | Implemented on `feature/task-create-snapshot`; acceptance pending | task_cli.rs |
| `task snapshot` | Bounded read-only Git capture | `task snapshot ID [--include-diff]` (patch ≤64 KiB) | implemented | task_cli.rs:276-283 |
| `task link/unlink` | Link by trace ID or evidence file | Exact native identity only (`--source-id --session-id [--turn-id --worker-id]`) | implemented; trace-ID/evidence-file link forms confirmed missing (recorded, cutover:28) | task_cli.rs:379-391 |
| `task handoff` | Markdown/HTML/JSON brief; `--endpoint` revalidation | `task handoff [--format] [--output] [--include-decision-history] [--include-diff]`; offline only | implemented; receiver-revalidation flag confirmed missing | task_cli.rs:356-367 |
| `task backup` | Consistent SQLite backup | `task backup` + workspace `backup --bundle`/`restore --bundle` | implemented | task_cli.rs:226 |
| `task brief/decide/decisions` | Structured briefs; immutable supersession | `task brief/decide/decisions` + `continuity` reveal + `complete/reopen/export/handoffs` | implemented (superset) | task_cli.rs:297-392 |
| `task draft/approve-draft` | Source-backed draft from Review JSON | Native anchor draft + `--review-file`; Python-receiver variant (Linux) | implemented; `--workspace`/`--format` flags confirmed missing | task_cli.rs:335-347, 406-437 |
| cockpit views 1-7, `/`, `[ ] l`, `t`, `s`, back | Monitor navigation | All present (plus `9` Operations) | implemented | keymap.rs:17-67 |
| agent family nav, `f`, `w`, `v`, `X`, pause, `R`, `U`/`u`, `z`, Ctrl+R review, Ctrl+F investigate, F1/?, Ctrl+C, `q`, Ctrl+S | Core keyboard journeys | All present | implemented | workspace.rs:1939-2060; workbench.rs:1185-1240 |
| workflow route `g`, resume copy `c` | Verified route + resume | `l` Locate (local + cross-host route file) and `c` | implemented (key `g`→`l`) | workbench.rs:1198-1203 |
| Ctrl+P command palette (actions) | Theme + action index (choose session, save task, open workflow, investigate) | Ctrl+P/F2 is the theme selector only; actions reachable by direct keys | confirmed missing (palette action surface) | theme.rs:246-252 |
| `o` sort menu | Working/Updated/Status ordering | Updated ↓ always | scope decision needed (recorded intentional, cutover:77) | cutover:77 |
| saved-task `p` Brief / `d` Decisions screens | Structured editors in cockpit | CLI-only (`task brief/decide/decisions`); no cockpit editors | confirmed missing (cockpit journey) | workbench.rs:521-539 |
| Ctrl+R reload saved task after concurrent edit | Revision-conflict reload in cockpit | none in workbench key handling | confirmed missing (cockpit journey) | workbench.rs:1036-1230 |
| Workflow `h` include-older-completions | History scope toggle | not present (`R/r/s/X/l/c` only) | confirmed missing (cockpit journey) | workbench.rs:1190-1205 |
| compact/wide prompt-index dual mode | Dropdown confirmation / wide index | flat prompt list with Enter reveal | scope decision needed | workspace.rs:2023 |
| OpenAI + Anthropic SDK auto-capture | `auto.py` OpenInference bootstrap | `run --instrument python` against the Rust receiver; CI `automatic-capture-interoperability` covers OpenAI sync/stream/error + Anthropic flush | implemented (bootstrap stays Python — recorded decision, cutover:439) | ci.yml:387-415 |
| LangChain/LangGraph | auto + explicit callbacks | CI `langchain-interoperability` through the Rust launcher | implemented | ci.yml:359-385 |
| OpenAI Agents SDK | `create_processor` exporter | CI `openai-agents-interoperability` | implemented | ci.yml:333-357 |
| Google ADK | framework-native OTel | CI `google-adk-interoperability` | implemented | ci.yml:307-331 |
| Pydantic AI | framework-native instrumentation | CI `pydantic-ai-interoperability` | implemented | ci.yml:281-305 |
| Python manual client / Node.js client | manual spans to receiver | both exercised in CI `rust-protocol` against the Rust launcher | implemented | ci.yml:101-109 |
| Selected SDK jobs on the current head | — | five vendor jobs pass at recorded heads; current-head results are Q7's to record | test-only acceptance pending | remaining plan Q7 |

## Dispositions for previously unrecorded items

1. **`attention native bind/send/hook/retry`** — implemented in Rust
   (Linux-only, token-file required); this audit is its first recorded
   disposition. The cutover table's attention row now covers it by reference.
2. **`task draft --workspace` / `task handoff --endpoint` revalidation** —
   confirmed missing flags. The Rust model is offline-first with explicit
   local-history anchors; receiver-revalidated handoff sections need a
   product decision before becoming development work.
3. **`task create --snapshot`** — implemented as an explicit opt-in to match
   the Python convenience while preserving the safer default: task creation
   alone does not inspect a workspace. The existing `task snapshot` command
   remains available for later recaptures. A failed capture preserves the
   task and reports its ID plus a `snapshot_error`.
4. **`task list --offset`** — disposition: cursor paging (`--limit/--before`)
   is the retained model; offset paging is not planned.
5. **`cockpit --no-local-agents` / `--metadata-only` / `--capability-config`**
   — disposition: replaced by the inverted explicit-grant capture model and
   the process-extension catalog (see
   [process extension RFC](rust-process-extension-plugins.md)). Flag-level
   parity is not planned; the capabilities are reached by different,
   explicit means.
6. **`serve --no-auth` / `--allowed-host` / plugin flags** — remote
   deployment flags remain the recorded open wire/TLS gap (cutover:18);
   plugin flags are superseded by the Rust plugin host (disposition 5).

## Findings that gate the B1/D3 cutover decision

Confirmed missing cockpit journeys and flags remain active parity work under
the user's completion direction. The separate implementation plan assigns
their development; this audit records the source-backed gaps:

1. Ctrl+P action palette (choose session, save task, open workflow,
   investigate) beyond the theme selector.
2. Saved-task `p` Brief and `d` Decisions cockpit editors with the decisions
   version-chooser journey.
3. Ctrl+R reload of a concurrently edited saved task.
4. Workflow `h` include-older-completions toggle.
5. `task draft --workspace` binding and `task handoff` receiver
   revalidation.
6. Remote receiver deployment (`--allowed-host`, TLS) — already recorded open
   in the cutover table.

Scope decisions with no development implied: `o` sort menu (intentional
Updated ↓ ordering), compact/wide prompt-index dual mode, `task list
--offset`, `cockpit --port`.

## SDK coverage verdict

All five selected SDK families (OpenAI, Anthropic, LangChain/LangGraph,
Google ADK, Pydantic AI) have CI jobs exercising a real Rust path via the
launcher, plus the manual Python and Node clients. Caveats recorded:
Anthropic shares the automatic-capture job rather than a dedicated one; the
auto-capture bootstrap remains the Python OpenInference layer (a recorded
product decision); and passing jobs at older heads are not current-head
acceptance — Q7 owns that record. Nothing documented-but-untested remains in
the SDK scope.

No workflow is retired by this audit. Installed and paired-state acceptance
remains with the separate test owner (CF1–CF7 and Q1–Q10).
