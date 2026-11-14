# Evidence-qualified tool and usage explanations

14 November 2026 · Stage E implementation record. This change supplies bounded explanations inside the existing investigation, workflow-pattern and usage surfaces. The coordinator owns final cockpit/MCP wiring and combined integration verification.

## A useful denominator before a recommendation

`mcp_window()` describes the explicit supplied loaded page or time window. Tool groups retain receiver, producer, server and tool identity. Missing identities stay separate. Duplicate exact source occurrences do not increase counts. Unknown timestamps are disclosed; they are excluded when a requested time interval cannot place them. Non-tool protocol methods are counted separately.

Recorded protocol/tool outcome and response correctness have different meanings. An explicit MCP `isError` is interpreted through the shared operation facts. Known passed/failed outcomes supply the failure-fraction denominator; unknown outcomes remain visible. Duration coverage has its own denominator. Median and p95 are nearest-rank empirical quantiles over known finite nonnegative durations. Zero is a duration; missing, negative, Boolean and nonfinite values are unknown.

| Synthetic loaded window | Outcome coverage | Duration coverage | Interpretation |
| --- | --- | --- | --- |
| Three captured calls | 1 passed, 1 failed, 1 unknown | 2 known, 1 unknown | Failure fraction is 1/2 known outcomes; duration quantiles cover two calls. |
| Configured server without a matching call | No observed denominator | No observed denominator | Inspect capture/filter coverage. Absence does not justify removing configuration. |

Groups with fewer than 20 captured calls are explicitly small samples. Results retain exact source-call references and recommend inspecting recorded failures or unknown outcomes. They make no all-history, provider-health or response-quality claim. Work is capped at 500 supplied records, 100 returned tool groups, 100 configuration entries and 20 references per group; clipping and partial coverage are disclosed.

## Recorded repetition with progress context

Workflow observations reuse C1 operation facts and the caller's existing canonical Activity when provided. Grouping requires a known receiver, producer, canonical actor, session and recorded execution. An explicit foreign receiver or conflicting source trace cannot be relabelled to the caller's namespace. Matching raw source IDs must prove one trace before that trace can be attached to a canonical row that omitted it.

Complete sanitized operation inputs and explicit output roles qualify operation equivalence. The compatibility fingerprint helper cannot bypass sanitization with `sanitize=False`. Missing, clipped, redacted or ambiguous arguments do not become an equivalence claim. Repeated failures additionally need a complete safe error fingerprint. Generic labels and completion claims do not prove recovery.

Routine polling is excluded. Strictly later successful recorded edits and recognized checks break a repeated-call sequence only for the same exact owner/execution. Equal or missing timestamps do not establish progress order; another worker's edit cannot clear the selected worker's observation. A strictly later identical successful operation can be reported, with no claim that consequences or the task were repaired. Compaction remains a semantic observation, not proof of context loss.

These observations never create explicit attention incidents or desktop notifications. Intentional retries and repetition remain plausible; no wasted-work or stalled-agent verdict is emitted. Analysis handles at most 500 selected source records and returns at most 32 observations. Unknown identity, timestamps and clipped input coverage are explicit.

## Usage interpretation and optional configured estimates

The default `task_usage_summary()` return value remains the existing two-tuple, including the first-line sidebar protocol. No configured estimate appears by default. Opt-in explanation details distinguish cached-input token fraction from request hit rate, and input/cache/reasoning breakdowns from totals. Cumulative token usage is not context occupancy. Occupancy requires explicit authoritative runtime fields, a source reference and matching receiver/producer/actor/session. Aggregate usage cannot allocate tokens or cost to an individual retry or prove its waste; missing values remain unavailable.

Optional `cost_estimate_rates` in the existing UI JSON permits at most 16 exact model names. Every entry supplies explicit USD-per-million-token units and input accounting. For example:

```json
{
  "cost_estimate_rates": {
    "exact-recorded-model-name": {
      "unit": "USD_per_million_tokens",
      "input_accounting": "flat_input",
      "input": 2,
      "output": 8
    }
  }
}
```

These numbers are synthetic user inputs, not fetched provider prices. `flat_input` explicitly prices all reported input at one rate. `split_cache` additionally requires `cache_read` and `cache_write` rates, complete compatible token breakdowns, and subtracts cached tokens before pricing uncached input. Values must be finite, nonnegative and bounded. Aliases and inferred current-model names are not resolved.

An estimate requires complete unambiguous usage, explicit allocation proof and a complete single exact-model attribution. Missing cache breakdowns, partial model coverage, shared traces or ambiguous allocations make it unavailable. Reported cost is unchanged and remains separate. Reasoning output is not priced twice. No provider defaults, rate fetch, editor or subscription-bill claim is added.

## Verification and remaining integration boundary

The focused gate passed **28 tests in 15.004 seconds**, covering loaded-window outcome/duration denominators, mixed/unknown identities, MCP error flags, source clipping, exact rates and cache accounting, context ownership, redacted argument hashing, routine polling, strict edit/check progress, canonical Activity reuse, foreign receivers and existing usage/allocation behavior. A final 17-test pure gate passed in 2.480 seconds after robustness checks; the same gate was repeated after adding exact turn/trace qualification for context sources. Both report layouts were checked in Chrome at 1280 px/light and 390 px/dark, with no page overflow or external assets, and the mobile layout was visually inspected. The actual Textual usage journey verifies the selected task, inspector, modal and trace use the same usage. Synthetic fixtures and temporary stores are used; personal telemetry, preferences and review notes remain untouched.

Root integration must pass `receiver` and existing canonical `activity` to `analyze_patterns`, supply the explicit MCP loaded-window namespace/label, preserve the new optional preference key through app saves, and pass only proven model/allocation/context evidence into optional usage explanation arguments. Default callers retain their existing protocol. UI presentation and final combined gates belong to that integration; this report does not claim an end-to-end MCP inventory or performance benchmark.

## Existing-view integration and final peer review

The MCP inspector now shows the selected producer/server/tool’s outcomes and duration denominators from the currently loaded page, including unknowns, small samples and explicit coverage limits. Inventory/aggregate metric counters remain separate. Canonical workflow-pattern analysis receives the transaction’s receiver and already-computed Activity.

The existing `u` usage details explain cache/token accounting, unavailable retry allocation and the absence of authoritative context occupancy. No context percentage is guessed from lifetime usage. Optional exact-model rates are loaded and preserved through existing UI preferences (`$XDG_CONFIG_HOME/agenturm/ui.json`, normally `~/.config/agenturm/ui.json`); they produce a separate configured estimate only under complete source/model/allocation proof. No configured-estimate line appears by default.

A detached bounded formatting key includes every formatter option, exact usage, owner, namespace, selection, capture policy, width and theme. Live selection/policy checks always run; only unchanged pure formatting is skipped. Twenty-five unchanged refreshes invoke the formatter once. Unsupported, cyclic, oversized or metadata-only inputs bypass retained keys. This is a formatting-work result, not a measured complete-monitor CPU result.

Independent integration review caught a real modal privacy gap: `self.data` may stay frozen while a modal is open even after the Feed publishes revoked content or a replaced receiver. Usage now validates the live Feed against its original frozen owner and receiver. Denial clears details and remains sticky after restoration, requiring a new opening. The actual-app tests exercise Feed snapshot replacement rather than only in-place mutation. The default empty-rate noise was also removed. Final read-only review reproduced both denial and receiver-replacement paths successfully and found no remaining blocker in the reviewed scope.

The integrated 39-test usage/explanation/formatting/pattern/source-policy gate passed in 61.255 seconds. It includes actual MCP denominator rendering, optional estimate invalidation, unknown context, source-policy changes, three-size usage rendering and live modal revocation. Earlier module gates remain documented above. Combined PR CI, resource budgets and external user outcomes remain separate and open.


## R6–R8 follow-up and corrected full-suite fixtures

Tool details now name configured servers without calls in the loaded window, explicitly qualify absence by capture/filter coverage, and offer a keyboard-reachable “Inspect flagged call” action cycling exact failed/unknown source calls. Configurations are read from the actual client/server/scope records. Switching identity or revoking capture blocks that action. MCP inventory and aggregate metric counters remain separate.

Usage labels preserve a recorded whole turn and its member-step count instead of calling its total one trace. Exact-owner captured session-lifetime numbers appear as a separately labelled context; calendar-day usage remains unavailable when it cannot be safely attributed from the existing source. Neither becomes current context occupancy. Optional estimates still require the exact selected producer/actor/session and known receiver, as well as complete model and usage proof.

Opt-in pattern Findings can now retain a qualified later matching success with both source references and informational severity. Repetition/compaction observations may attach reported leaf-model contributions strictly inside their observed interval, with exact source references and complete source/owner/timing proof. These are reported contributions, not cumulative-counter deltas, retry cost, waste or context occupancy. The real Store passes source completeness into the shared analysis. A versioned resumable repair replaces older opt-in heuristic caches before readiness; default-off stores do not acquire a historical pattern scan. See [source-accounting detail](reported-usage-progress.html).

The old full-suite failures were positive fixtures missing the receiver/single-owner proof or complete typed command arguments now required by the stricter source contract. Fixtures now supply those proofs; negative unknown/shared/partial/redacted tests remain. No production matcher or privacy check was relaxed. After rebasing onto main through PR83, the **55-test integrated gate passed in 75.184 seconds**, covering MCP UI, actual Store cache repair, usage/progress/patterns, the original usage journeys and local monitoring. The corrected six-test UI/cache gate also passed in 21.647 seconds. New-head full CI and combined-tree resource/acceptance checks remain pending.
