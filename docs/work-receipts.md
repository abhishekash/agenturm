# Execution evidence engineers can inspect

Agenturm should earn a place in an engineer's workflow by reducing the work of reconstructing **which agent did what, for which run, and what evidence supports its reported result**. The implementation must make those answers inspectable. A more attractive dashboard does not establish that value.

## Three concrete reasons to use it

1. **Attribute work across concurrent agents.** Supported native session records and configured OpenTelemetry signals can be inspected in one local workspace. Recorded delegation gives parent/worker trees. Selecting a worker keeps its producer, canonical agent, session and execution separate, including inside a shared trace. A worker's passing check must never become its parent's validation result.
2. **Review the evidence behind completion.** Agent prose, a recorded command result and a check on the current worktree are different evidence. Review shows the captured request for the selected run, agent-reported updates, recognized check commands, modifying file operations and evidence gaps. If a recorded edit follows a passing check, the engineer has a concrete reason to inspect the edit and rerun the affected checks.
3. **Carry an inspectable record into the next decision.** The same evidence projection powers the terminal Review and a noninteractive CLI. Frozen JSON retains identities, result basis, timestamps, coverage and source references. Markdown/HTML receipts can accompany review; existing saved tasks retain the engineer's brief, decisions, rationale and linked runs. Another engineer can inspect what the decision relied on instead of reconstructing the conversation from memory.

These are engineering jobs, not claims of exclusive functionality. If one agent's native terminal already provides enough context, Agenturm may add little. The stronger case is concurrent runs, delegation, interruptions and reviews that cross sessions or runtimes. Reduced reconstruction time and fewer mistaken verification conclusions remain hypotheses for the participant pilot; they have not been measured.

## A falsifiable example

An agent reports completion. Its recorded `pytest tests/test_retry.py -q` result is successful. A later captured `edit` changes `retry.py`. Review must show that ordering, the exact owner and the source records. It must suggest inspecting the later edit and rerunning affected checks. A later file **read** must not trigger that warning. A `pytest --version` invocation and an assistant saying “all tests pass” must not add passing checks.

When capture is partial, the answer is qualified. When no validation is captured, the answer is “no recognized checks in this loaded evidence,” not “untested” or “safe.” No observed later edit means freshness remains unknown: other tools, humans, external worktrees and uncaptured operations can change the files.

## Inspect without opening the cockpit

Available on **main / Unreleased**; the published 0.2.3 package does not yet include this command. Install the checkout with `uv tool install --force .` to try it. The receiver must also run the updated code.

Use identities from the session inspector's Details or Review's **j Copy JSON**. Connect to an existing receiver:

```sh
agenturm review --service codex \
  --agent-id ag1_REPLACE_WITH_RECORDED_ID \
  --session-id REPLACE_WITH_SESSION_ID \
  --trace-id REPLACE_WITH_RECORDED_TRACE_ID \
  --pages 2 --format json
```

`--trace-id` and `--service` are required. Supply `--agent-id` or, for records without canonical agent identity, `--session-id`. A session-only query excludes canonically owned worker rows rather than widening to their family. No trace identity means this command cannot isolate a run; inspect the disclosed native lifecycle interval in the terminal instead.

The default is readable text. `--format json`, `markdown` and `html` are explicit alternatives. `--output review.md` writes the frozen Markdown and HTML pair; `--output review.json` also writes JSON. Existing files and symlinks are refused. Output can include captured prompts, paths, identifiers or secrets; it is not redacted for sharing. Files are created with owner-only permissions on POSIX. Each complete file is published atomically; the pair is **not crash-atomic**. Ordinary failures roll back only this operation's new files.

The command never starts a receiver, launches an agent, executes a check or calls a model. It uses `OBSERVATORY_ENDPOINT` / `--endpoint` and `OBSERVATORY_TOKEN` for the existing receiver. Results go to stdout; diagnostics go to stderr. Exit 0 means inspection succeeded, including partial evidence or failed recorded checks. It is not a validation gate. Exit 2 means a read, identity, capability, argument or export failure; interrupt returns 130.

## Technical contracts

- **Exact ownership:** receiver namespace, producer, canonical agent, session and trace constrain every activity read. Descendants and sibling runs are excluded. An older receiver without exact-trace filtering is refused. Namespace changes during reads emit no receipt.
- **Bounded evidence:** one explicitly requested page by default, up to five pages of 200 records. Loaded source records are capped at 1,000 events / 2 MiB, with a 2 MiB transport limit per activity response. Earlier cursors and capacity limits are disclosed. Later-edit reference links are capped at eight per check / 128 KiB total; the full observed count and clipping flag remain available. All loaded modifying edits remain separately listed.
- **Shared interpretation:** Workflow, Review and the CLI consume the same pure edit/check projection. Check recognition is a conservative allowlist, not shell interpretation. Recognized help, version, inventory and collection-only modes are excluded; arbitrary scripts and wrappers may remain unrecognized. Recorded exit codes and producer result labels are distinguished. Neither is independent attestation.
- **Stable refresh:** successful refresh commits outcome, allocated usage and evidence for the same execution. A newer run cannot replace the selected run. Missing or stale metadata leaves previous facts explicitly qualified. Stable session task labels remain separate from the latest captured request for the run; an absent request stays unavailable.
- **Conservative accounting:** whole-trace usage/outcomes are allocated only when complete recorded ownership supports one producer, agent and session. Shared, incomplete, legacy or mixed-owner traces remain unavailable at the individual-agent level. Older cached summaries without ownership proof also remain unavailable until rebuilt by new ingestion. Native lifecycle records can still supply an exactly matched run outcome.
- **Capture policy:** metadata-only capture suppresses cached content. Policy follows the selected identity independently of run changes; a confirmed disable revokes open Review content. Reads, asynchronous formatting and export confirmation check policy. Copies/exports do not fetch extra history or add receiver credentials. Previously copied or exported content cannot be recalled.
- **Frozen artifacts:** copy/export use the previewed projection. JSON retains source identities; human receipts map observations to their receiver and event references. References do not guarantee future telemetry retention. Captured text is rendered literally, including Markdown fences and HTML, with no external receipt assets.

## Implementation and acceptance plan

1. Correct the evidence model: informational command classification, modifying edits shared between Workflow and Review, and same-execution metadata refresh.
2. Provide the headless exact-run consumer and readable frozen receipts over that same projection. Preserve explicit JSON access and existing navigation; no new primary tabs.
3. Verify source attribution against a real authenticated HTTP receiver with a shared parent/worker trace; test namespace changes, old receiver refusal, metadata-only policy changes, explicit paging, output streams and bounds.
4. Exercise the terminal with real keyboard/button routes, asynchronous policy changes, copy/export equality, exact-run refresh and a 60×18 layout. Inspect standalone HTML at mobile and desktop sizes in light and dark modes.
5. Peer review the plan and implementation, run the appropriate regression suite, and make the PR concrete before merging. Keep new functionality marked Unreleased until a release is requested.

The implementation was revised after peer review identified policy changes crossing run boundaries, asynchronous copy/export races, Textual button dispatch bypassing an override, ambiguous trace-wide accounting, and quadratic later-edit links. Regression coverage must retain these cases.

## Do the existing work better next

Finish the separately identified incident-acknowledgement isolation and graceful-shutdown seen-receipt durability fixes. Improve adapter coverage against real producer formats and preserve an explicit support matrix for recognized checks. Run the existing participant pilot with reconstruction tasks and measure mistaken attribution or verification conclusions, time to inspect evidence and voluntary repeat use. Make source lookup and large-run reading easier without hiding partial capture or adding speculative summaries.
