# Source-backed progression and reported usage context

14 November 2026 · Stage E acceptance follow-up. Existing opt-in workflow observations gain exact matching-success evidence and bounded reported-usage context. This does not create a new always-on alert, infer waste, assign retry cost or estimate context occupancy.

## What a user can inspect

With `AGENTURM_WORKFLOW_PATTERNS=1` on the receiver, Findings can include **Later matching success recorded**. It is an informational observation, with both the actual failed source and a strictly later successful source. The operation inputs, producer, actor, session, receiver and execution must match under the existing sanitized operation facts. Equal timestamps, foreign owners, unproved inputs and informational nonzero searches do not become recovery evidence. A passing check resets repetition counters while retaining the bounded evidence needed to describe the matching later success. Success is not proof that consequences or the full task were repaired.

The existing repeated-failure, repeated-call and repeated-compaction observations may also show reported input/output contributions from completed source records strictly inside their captured interval. There is no separate usage-growth Finding. The text describes reported contributions and the absence of established successful captured edit/check progress; intentional repetition and unobserved effects remain possible.

## Accounting contract

The helper uses existing `INPUT_TOKEN_KEYS` / `OUTPUT_TOKEN_KEYS` and the same candidate/ancestor leaf rule as `summarize()`. Input and output leaves are selected independently. A wrapper can be the output leaf while a child supplies input; the result is a count of **source records**, not model calls. Cache and reasoning breakdowns are never added again. The helper does not subtract successive token reports or reinterpret them as cumulative counters.

A caller must supply `source_complete=True`, derived from the existing complete source window. The helper requires:

- the exact receiver, producer, canonical actor, session and execution;
- one explicit trace that the existing exact-source reader can hydrate;
- known source IDs and complete noncyclic ancestry, without cross-owner usage accounting;
- finite nonnegative integer token fields, with contradictory aliases rejected;
- terminal source records with known positive timestamps strictly inside the observation interval;
- at least two ordered positive usage source records and both token directions;
- no successful captured edit/check progress, uncertain modification effect, unknown check outcome or ambiguous progress timing inside that interval.

A known failed check is not successful progress. A failed or unknown-effect file modification remains uncertain. Sources crossing/equal to interval boundaries, unresolved parents, mixed traces in a turn, missing counts, malformed/foreign identities, capture denial or partial/clipped windows make the context unavailable with a reason. Absence is not converted to zero. At most 500 source/progress records and 20 context references are used. No global cache, new source protocol, provider assumption, model-rate default or shared accounting change is added.

## Integration and ownership

`analyze_patterns(..., source_complete=False)` keeps the conservative default. Root integration supplies the existing trace summary's evidence-window completeness in the same source transaction and owns invalidation of older cached pattern versions. The new context is attached only to existing opt-in repeated-work Findings. Actual source references are retained, the real recorded turn is carried, and mixed-trace evidence is not published into the existing single-trace source reader.

This delivery changes only the progression/usage tail of `evidence_explanations.py`, `workflow_patterns.py`, the old positive backend fixtures and new tests. Existing MCP configuration helpers, UI usage scope labels, storage, core accounting and native adapters are outside this patch. The backend fixtures now provide complete native-shaped shell command, cwd and session evidence; matcher strictness and negative fixtures are preserved.

## Verification

The **49-test gate passed in 10.318 seconds**, covering new progression/accounting cases plus existing workflow-pattern, backend, usage-explanation and occurrence-owned Findings gates. A final eight-case delta passed in 0.452 seconds after distinguishing known failed checks from successful/uncertain progress and retaining the success reference under the 20-reference cap. The report passed Chrome checks at 1280 px/light and 390 px/dark with no page overflow or external assets; mobile rendering was visually inspected.

Tests compare wrapper/child totals against the existing `summarize()` accounting, exercise unknown/foreign identities, booleans/nonfinite/conflicting counters, absent/cyclic/cross-owner ancestry, partial windows, zero/equal/crossing timestamps, same-turn different traces and edit/check uncertainty. They assert source immutability, actual usage source IDs, no new always-on kind and `attention=False`. An actual Store test verifies the informational matching-success Finding, both failed/successful source IDs and successful exact hydration of both references. Actual Store usage-context publication requires the root-owned completeness hook and its combined integration gate; this report does not claim that hook is already merged.

All records are synthetic. Personal telemetry, settings and the original review notes are untouched. Resource budgets, live runtime coverage and participant outcomes remain separate acceptance gates.
