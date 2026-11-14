# Frozen Git and approved continuity drafts

14 November 2026. Stage D implementation record. Git and source-backed drafting use explicit actions in the existing Review and saved-task workflow. No agent launch, command execution from telemetry, or remediation is added.

## What changes

Review **g** explicitly maps the selected recorded execution to a local checkout, then captures bounded read-only Git state in a worker. A receiver cwd, including one supplied over loopback, does not prove that its path is a local checkout. The first capture asks for a local path; **g** refreshes that explicit mapping, and **m** changes it. The receipt freezes the source scope, explicit mapping, observation time, HEAD, branch, base, status, tracked diff, warnings and clipping.

Review **d** prepares an editable continuity draft from the initiating request, recorded edits/checks/errors, optional frozen Git, and current human accepted/rejected decisions. The existing **Save task** manual-notes path remains available. Drafting performs no task mutation. **Ctrl+S / Approve** is the explicit save boundary; a new task is created atomically with its approved draft. In an existing task, **g / Draft** appends a version with a revision guard. Authored title, workspace, notes, brief and immutable decisions remain unchanged.

Approved drafts are stored in an additive `task_continuity_v1` table. Their reviewed text, source scope and frozen Review are immutable versions. There are at most 16 versions per task; existing task byte limits continue to apply, and no version or decision is removed to make space. Exact step/whole-turn identity, recorded membership and recognized checks survive both UI and CLI approval.

## Interpretation and boundaries

Current Git is an observation, not agent attribution or historical branch proof. The first successful capture establishes its base; later explicit captures retain that base only within the same execution mapping. Pre-existing, concurrent, staged, unstaged and later committed changes may be included. Untracked paths are listed, while their contents are excluded. Earlier recorded checks do not certify the observed checkout.

Parallel-work observations compare only explicitly captured mappings for distinct recorded agents. Recorded rename pairs retain both paths; failed/blocked attempts do not prove a modification. Different worktrees, missing mappings, relative/absolute path equivalence and unreported rename pairs are not guessed. Known contradictory recorded branches are excluded. Matching current HEADs never establish historical branch membership. A strictly timed recorded check can be shown before another agent's later modification in either input order, with source references and uncertainty retained.

Comparison work is bounded: at most 20 execution inputs, 40 edits and 20 checks per execution, 200 execution pairs and 100 observations. The UI retains at most eight explicit captures and 512 KiB of comparison metadata, with a 128 KiB admission limit per entry. Git diff bodies are excluded from this comparison cache. Partial comparisons disclose their limits. No Git polling is added.

## Capture and safety

The existing `capture_workspace()` contract remains: eight-second total budget, per-command deadlines, 128 KiB tracked diff, 100 displayed paths, no optional locks or credential prompts, disabled fsmonitor/external diff/textconv/filters and blocked lazy-fetch protocols. Temporary Git tests exercise the existing defense cases. Credential redaction runs across the complete bounded diff before quoted-key line handling, so multiline key blocks cannot bypass redaction. All metadata included in the new correlation fingerprint is sanitized; redaction/clipping makes content equivalence unavailable.

Receiver identity and source capture policy are checked before Git, after the worker, before draft publication/approval and before receipt publication. Closed or replaced Review generations discard late results. Comparison permissions are checked for each cached source. New source-backed draft/Git handoff sections are withheld offline or when source policy is revoked; independently authored task notes, brief, decisions and separately requested task workspace captures retain their own ownership. Re-enabling source capture does not invent a new observation time or validation result.

## CLI workflow

A frozen Review JSON file supplies the exact identifiers; they do not need to be re-entered. An endpoint is required for current namespace/capture-policy confirmation. Existing local token discovery is reused.

```sh
agenturm task draft --review-file review.json --endpoint http://127.0.0.1:4318 \
  --workspace /explicit/local/checkout --output continuation.json
# Review/edit the draft's text field; keep its frozen scope intact.
agenturm task approve-draft continuation.json --endpoint http://127.0.0.1:4318
```

`draft` writes a new private JSON/Markdown/HTML artifact when requested; it does not create a task database unless an existing task is explicitly read. `approve-draft` is a separate durable action. Use `--task-id` when preparing a draft for an existing task; its revision and human decisions are included automatically. Existing authored fields are preserved on approval. Auth failures, receiver replacement, capture denial, stale revision and output collisions fail without publishing a new draft or changing a task.

Saved-task handoff remains useful offline for authored material. Supply `task handoff … --endpoint …` to revalidate new source-backed draft/Git sections. Markdown and HTML companions come from the same frozen preview, and existing files are never overwritten.

## Verification

The final focused delivery gate passed **46 tests in 43.177 seconds**, covering source/model/Git contracts, real authenticated CLI, Review keyboard Git mapping at 60×18, editable draft approval at 60×20, revocation/late Git results, saved-task Git defenses, existing task CLI and whole-turn continuity. The authenticated CLI gate also passed independently (2 tests). A final **14-test policy/CLI gate passed** after adding consistent offline filtering to generic task JSON and per-identity checks for peer-derived comparison text; a revoked peer cannot leak through an otherwise permitted selected source. Existing Review/manual-note behavior remains covered; the unsafe loopback expectation was replaced by refusal plus an explicit-mapping positive case. The implementation plan and specific overlap/redaction boundaries were independently reviewed by the coordinator. Both HTML sizes were checked in Chrome: 1280 px/light and 390 px/dark, with no horizontal page overflow or external assets; mobile output was visually inspected. Tests use temporary repositories, receiver stores, task databases and UI harnesses. Personal telemetry, preferences and source review notes are untouched.

Terminal behavior is tested at Review's 60×18 floor and the existing task editor's 60×20 floor. Git mapping and draft approval use normal Tab/Enter/Ctrl+S/Escape navigation. The additional actions remain inside the existing Review and task surfaces; there is no additional dashboard.
