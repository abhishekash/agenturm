# Why Agenturm is useful

**Agenturm addresses a real engineering problem: reconstructing what coding agents actually did well enough to decide what needs attention, what needs another check, and what can be continued.** Its value increases when work spans long conversations, multiple sessions, delegated workers or different agent runtimes.

This assessment checks main at `7c80dc3` on 14 November 2026. Some current-main capabilities remain unreleased; this is not a claim that installing published 0.2.3 supplies all of them. The earlier assessment in this worktree remains a dated review of `bbbfe45`.

## A decision it can improve

Consider this illustrative sequence:

1. An agent changes the authentication code.
2. It runs a test command and records exit 0.
3. It edits the code again.
4. It reports that the task is done and tests passed.

The earlier success does not verify the later file contents. Agenturm's Review keeps recorded checks and subsequent edits visible for the selected run. You can decide to inspect the edit and rerun affected tests before accepting the result. Chronology alone does not establish which tests an edit invalidates; that judgment remains yours.

I exercised the native-record integration test that reconstructs two passing checks and two later edits, then exports the resulting evidence. The two checks each retained a later-edit count of two. This is a functioning implementation of the review problem, using synthetic records.

Other checked cases matter to the same decision. `pytest || true` is not classified as a verified passing check. A failure in `cd missing && pytest -q` is not falsely assigned to pytest. A failed file-change operation remains an attempt. These distinctions help keep a reassuring-looking command history from becoming unjustified confidence. [Native review checks](https://github.com/abhishekash/agenturm/blob/7c80dc3/tests/test_native_review_contract.py).

## Three other reasons to open it

**Find work waiting for you.** When several agents are running, checking each terminal is repeated supervisory work. The roster and attention workflow provide a common place to find captured requests and completions. In the check I ran, a recorded approval remained pending after 210 newer unrelated records pushed it outside the latest Activity page. A restarted watcher did not notify again, and the exact resolution cleared the pending approval. This checks durable attention state with synthetic events and a notification test sink; it does not establish native notification delivery or every runtime's capture coverage. [Attention integration check](https://github.com/abhishekash/agenturm/blob/7c80dc3/tests/test_attention_cli.py).

**Recover a specific fact from old work.** Current main supports retained-history searches for a path, command or instruction. This can help answer which recorded session attempted to edit a file, where a command failed, or which session received a constraint. Results lead to a specific source record. This is useful even for one engineer using one agent over many sessions. Git tells you about changes; agent records can additionally preserve requests and failed tool attempts. Recorded activity remains incomplete when capture or retention omits evidence. [Investigation workflow](https://github.com/abhishekash/agenturm/blob/7c80dc3/docs/usage.md#investigate-retained-paths-commands-and-instructions).

**Continue with rationale intact.** Saved tasks preserve the brief, accepted and rejected decisions, and linked evidence for a handoff. The practical benefit is avoiding another explanation of constraints and previously rejected approaches. It requires deliberate upkeep. I checked that drafting does not silently save a task or overwrite authored fields, and that actual CLI review JSON can be linked to a saved task. These are concrete persistence behaviors, not measured reductions in repeated work.

## What the engineering evidence establishes

The implementation has explicit handling for incomplete results, misleading shell commands, failed operations, persisted requests and source ownership. Those behaviors indicate deliberate work on correctness beyond presenting activity attractively.

Nine focused checks passed on the reviewed main revision:

- Seven native review contract tests, including native fixture ingestion, later edits, command ambiguity, failed file operations, clipping and a real authenticated local HTTP/CLI review-to-saved-task path.
- One attention integration test covering a request outside recent Activity, watcher restart, duplicate notification suppression and exact resolution.
- One continuity test covering deliberate save and preservation of authored fields.

The first run passed seven checks; two could not open sockets under the sandbox. Those two passed after rerunning with local loopback access. No new tests were written for this assessment. This focused result does not establish whole-application reliability, current performance or usability across all terminals and runtimes.

## The adoption judgment

I would use Agenturm when I repeatedly need to reconstruct agent work, supervise concurrent sessions or resume a task with its reasoning intact. A single short conversation with fresh checks already visible in the terminal creates much less need for it.

Its core value is **less effort finding the relevant evidence and fewer opportunities to accept the wrong conclusion from a completion message**. The mechanisms exist and the checked scenarios work. The amount of time saved and frequency of prevented mistakes remain unmeasured: the repository's [pilot kit](https://github.com/abhishekash/agenturm/blob/7c80dc3/docs/pilot-kit.md) reports zero observed participants, sessions and repeat-use observations.

Whether AI helped write the application does not settle its usefulness. The stronger evidence will be engineers repeatedly choosing it because finding the right request or reconstructing a completion is easier with it. Agenturm has a credible practical basis for that choice today; broad adoption remains a hypothesis to test.
