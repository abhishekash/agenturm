# Is Agenturm useful to developers running 20–50 agents?

**Verdict: yes, for reducing the effort of reviewing concurrent agent work—not for orchestrating it.** The strongest practical reason to use it is to answer: **“This agent says it is done. What recorded evidence supports that, for this particular execution?”**

Reviewed 14 November 2026 at commit `a601ad2236f22ef75600129e920ff763883bee7c`, which matched GitHub main when checked. This assessment uses source inspection, documentation, and 38 focused tests run locally. It is not a firsthand 50-agent production trial or an adoption study.

## One practical use: review a completed fix before integrating its branch

Imagine 30 local agents working in separate worktrees. One is fixing refresh-token retries. Several finish while you are reviewing another change. You no longer remember which terminal contains which test result.

An illustrative captured sequence for the retry worker is:

| Time | Recorded event |
| --- | --- |
| 10:04 | `pytest tests/test_retry.py -q` finishes with exit code 0 |
| 10:06 | The worker edits `retry.py` again |
| 10:07 | The worker reports “Done; tests pass.” |

The problem is not that the test result is false. It is that it predates a later change. A completion message compresses away that distinction.

**Your workflow:**

1. Run `uvx agenturm` in an extra terminal. Supported recent native records are discovered without restarting those coding agents.
2. In **Agents → Today**, select the retry task under **Ready for review**. Open its inspector and choose **Review** from Overview.
3. Inspect the captured checks and modifying operations. Agenturm can show the passing command followed by **later recorded edits**, scoped to the selected worker/execution, and suggest inspecting those edits and rerunning affected checks.
4. Return to that worker’s original terminal/worktree. Review the diff and rerun the necessary checks there. Integrate only after your normal review. Mark the run reviewed in Agenturm to clear it from Today, not to certify correctness.

**Why this earns a place beside your terminals:** you get a common review surface instead of manually reconstructing the sequence from several transcripts. Exact ownership matters: a sibling worker’s passing command must not become this worker’s validation. A native terminal can show the same evidence; Agenturm’s potential advantage is less reconstruction across concurrent runs and runtimes, not exclusive access to it.

This is a hypothetical scenario backed by implemented behavior and synthetic tests, not a reported customer outcome. Review exists in 0.2.3. The noninteractive `agenturm review` receipt command is main/Unreleased and should not be promised by the published 0.2.3 install.

## What I verified

- `workflow_evidence.py` separates recorded checks, assistant claims, modifying operations, errors, and coverage. It compares edit timestamps with check completion timestamps.
- `ExecutionScope.matches` constrains evidence by producer, worker, session, and run/turn. It does not simply combine everything from the same repository.
- `session_status.py` distinguishes quiet work, explicit waits, collector backlog, and possible stalls. Silence alone is not presented as proof of failure.
- **38 tests passed in 3.115 seconds:** session status, work receipts, and pure workflow-evidence cases. This included a real authenticated loopback receiver test that excluded parent/sibling evidence, preserved a later-edit observation, and exported a frozen receipt. It was synthetic and did not exercise 30 live agents.

Reproduction:

```sh
uv run --frozen python -m unittest tests.test_session_status tests.test_work_receipts tests.test_workflow_evidence.EvidenceTests -v
```

## Why I would not yet call it a proven 50-agent cockpit

1. **Coverage is bounded.** JSONL discovery selects up to 50 recent sources across Codex, Claude Code, and Pi; OpenCode separately selects up to 50 recent sessions. Historical and delegated sessions can consume those windows. These are not guarantees that every one of 50 active agents will be represented. Check collection diagnostics before trusting the roster.
2. **Performance remains an explicit open issue.** The repository’s retained-history report measured 112.96 seconds cold readiness and 15.90% of one CPU core for an idle complete monitor on its synthetic 35-day fixture. Its separate 20-minute soak reported rising RSS, with memory stability not established. These are reported results on older named revisions, not measurements I repeated on the reviewed head or a prediction for your laptop.
3. **It observes; it does not enforce.** It does not launch agents, answer approvals, isolate worktrees, prevent overlapping edits, independently attest commands, or prove that today’s files match the captured test run. An absence of later recorded edits does not establish freshness. CI and actual code review remain necessary.
4. **Value is plausible, not measured.** The repository explicitly says reduced reconstruction time and mistaken verification conclusions remain pilot hypotheses. I found implementation evidence, not evidence of repeat adoption or quantified time savings. Local capture also creates a sensitive extra copy of agent content; redaction is best effort.

## Recommendation

**Position it as a review inbox for parallel coding work: “See what supports each agent’s ‘done’ before you integrate it.”** That is a concrete engineering decision, not another activity dashboard. For a single agent, the extra surface may be unnecessary; for many concurrent sessions, I would trial this workflow before adopting it as an always-on monitor.

A practical trial: use comparable batches of completed runs, with and without Agenturm. Measure time to find the correct execution, identify whether a recorded check preceded a later edit, and choose the next action. Track wrong-worker conclusions and missing capture as well as speed. If the inbox does not improve those decisions, the dashboard alone has not earned its overhead.

## Source references

All links pin the reviewed revision:

- [Product scope and released/unreleased distinction](https://github.com/abhishekash/agenturm/blob/a601ad2236f22ef75600129e920ff763883bee7c/README.md)
- [Review projection, exact scope, and guidance](https://github.com/abhishekash/agenturm/blob/a601ad2236f22ef75600129e920ff763883bee7c/observatory/workflow_evidence.py)
- [Receipt tests, including authenticated worker isolation](https://github.com/abhishekash/agenturm/blob/a601ad2236f22ef75600129e920ff763883bee7c/tests/test_work_receipts.py)
- [JSONL discovery](https://github.com/abhishekash/agenturm/blob/a601ad2236f22ef75600129e920ff763883bee7c/observatory/local_agents.py) · [OpenCode limits](https://github.com/abhishekash/agenturm/blob/a601ad2236f22ef75600129e920ff763883bee7c/observatory/opencode.py)
- [Retained-history performance report](https://github.com/abhishekash/agenturm/blob/a601ad2236f22ef75600129e920ff763883bee7c/docs/validation/history-read-economics.md) · [Soak qualifications](https://github.com/abhishekash/agenturm/blob/a601ad2236f22ef75600129e920ff763883bee7c/docs/rigorous-validation.md)
- [Evidence contracts and unmeasured value hypotheses](https://github.com/abhishekash/agenturm/blob/a601ad2236f22ef75600129e920ff763883bee7c/docs/work-receipts.md) · [Privacy boundaries](https://github.com/abhishekash/agenturm/blob/a601ad2236f22ef75600129e920ff763883bee7c/docs/privacy.md)
