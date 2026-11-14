# Evidence reconstruction and continuation pilot kit

**Prepared protocol. Participants observed: 0. Sessions observed: 0. Repeat-use observations: 0.**

This kit contains generated scenarios, expected answers and blank observer sheets. It is not evidence of adoption, measured time savings, alert precision, or live Claude/Pi compatibility. Product tests and synthetic records cannot supply those outcomes. The [pilot protocol](rfcs/workflow-pilot.md) defines the proposed paired study; this kit makes that protocol runnable without personal transcripts or provider accounts.

## Consent and data handling

Read this before recruitment or timing: “We are evaluating whether recorded agent evidence helps you reconstruct and continue work. Participation is optional. You may stop, skip a task or withdraw your notes at any time. We will use synthetic tasks first. We will not install collection hooks, run an agent, approve a native request, record your screen or collect personal transcripts without separate explicit consent. We are measuring the interface, not your performance.”

Agree who may see notes, whether recording is permitted, the withdrawal contact, the deletion date and the form of any published aggregate before starting. Keep signed consent and identifying information outside the repository, separate from observation notes. Use anonymous codes on private sheets; never commit participant identifiers, recordings, actual transcripts, tokens or credentials. Publish counts only with consent, denominators and limitations. This repository contains no participant results.

## Materials and safe setup

The scenario below is fully specified in this document, so an observer can run a paper reconstruction exercise offline. For an interactive product exercise, an operator can explicitly create a new synthetic database:

```sh
python scripts/generate_pilot_fixture.py --output /tmp/agenturm-pilot-new.db
agenturm --db /tmp/agenturm-pilot-new.db serve --host 127.0.0.1 --port 19418 --no-local-agents
```

Use a dedicated new file. Existing destinations are refused. The generator reads no transcript directories, starts no receiver or agent, invokes no provider and overwrites no database. The receiver command disables local discovery to keep real sessions out of the exercise. Keep authentication enabled; use the receiver's existing private-token mechanism. Stop that dedicated receiver and remove only the explicitly created synthetic files after the exercise. Never use an actual telemetry database as a pilot fixture.

Connect the cockpit explicitly to `http://127.0.0.1:19418`. Typed CLI examples, once the corresponding receiver integration is installed:

```sh
agenturm investigate path src/retry.py --endpoint http://127.0.0.1:19418 --format json
agenturm investigate command "python migrate.py" --failed --endpoint http://127.0.0.1:19418
agenturm investigate instruction "public API stable" --endpoint http://127.0.0.1:19418
agenturm investigate path src/古い.py --endpoint http://127.0.0.1:19418
```

Use a returned `event_id` with `agenturm investigate source EVENT_ID` to inspect the exact bounded record. Query matching is not workspace proof: a recorded relative path in two sessions does not establish that they edited the same checkout. Source reads and search results are retained observations, with capture, redaction and pruning limits.

## Generated scenario and task cards

All names, producers, paths and commands here are synthetic. The database includes nine explicit Activity records and two generated OpenCode-shaped step anchors. It is not a live-runtime interoperability test.

| Order | Recorded owner | Observation |
| --- | --- | --- |
| 1 | Codex / synthetic-review | User: “Keep the public API stable.” |
| 2 | Same owner | `pytest tests/retry.py`, recorded exit 0. |
| 3 | Same owner | File edit of `src/retry.py`, recorded success, after the check. |
| 4 | Claude / synthetic-worker | `python migrate.py`, recorded exit 1; stderr “Synthetic migration rejected”. |
| 5 | OpenCode / synthetic-overlap | Rename `src/古い.py` to `src/新しい.py`, recorded success; step trace `cccc…`. |
| 6 | Same OpenCode owner | File edit attempt on `src/retry.py`, result unknown; step trace `dddd…`. Both steps explicitly belong to generated turn `bbbb…`. |
| 7 | Claude / synthetic-worker | Assistant: “All checks passed.” No check invocation accompanies this claim. |
| 8 | Codex / synthetic-review | Explicit recorded `agent.waiting_for=Approval` message. No native approval action is performed and no real process/terminal is linked. |
| 9 | SDK / owner and session not reported | User: “Preserve the retry budget.” |

**A — Find the request.** Identify the recorded approval evidence and explain what is unknown. Ask the participant to distinguish this explicit captured wait from an unrelated unknown/stale status. Attempt native handoff only when the interface proves a real target; this fixture has none. Do not supply a fabricated PID or press an approval control.

**B — Review the completion.** Reconstruct check → later edit → reported result. Ask whether the earlier passing check verifies the later file contents, and whether the assistant's claim supplies another passing check. Inspect the failure's command, exit and actual source. Record any unsupported verification conclusion.

**C — Investigate retained history.** Find who received the public-API instruction, which sessions recorded a modification attempt on `src/retry.py`, which command failed, and the source of the old Unicode rename path. Inspect the unknown-owner instruction without assigning it to the shared trace's root. Repeat a query with a small page size and continue its cursor.

**D — Preserve a turn and continue.** Review the generated OpenCode turn containing step `cccc…`; inspect both step references. Save a task and a handoff only after the participant deliberately edits and approves it. Use these supplied scenario decisions: “Keep public API signatures” (accepted constraint), “Do not raise retry counts without measured evidence” (rejected approach), “Inspect the failed migration before changing behavior” (next action). They are task-card inputs, not claims about a real human decision. Reopen the saved task in a fresh session/view and count corrections or missing context. Never silently replace authored text.

**E — Challenge provenance.** Two sessions mention `src/retry.py`, but the fixture has no proved shared workspace/branch binding. Ask what can and cannot be concluded about overlap. The OpenCode edit has an unknown result. Do not claim a confirmed collision or successful persistent change from these observations alone.

**F — Reconnect and recover.** Stop and restart only the dedicated synthetic receiver while a cockpit is open. Record whether the UI indicates loss/staleness and recovers against the same namespace. Marking a request seen is not native approval or proof that the request was resolved. Repeat a saved-decision handoff after reconnection. For a real native-terminal exercise, use separately consented local evidence; mark this measure unavailable for the synthetic fixture.

## Observer answer key

- The public-API instruction belongs to Codex / synthetic-review. The retry-budget instruction has no reported owner; shared trace membership supplies no owner.
- The passing check precedes the Codex edit, so it cannot certify the later state. “All checks passed” is an assistant statement, not terminal evidence of a test.
- The failed command is Claude's `python migrate.py`, exit 1. Search/comparison exit 1 is a separate informational case; it must not be generalized to this command.
- Both Unicode rename paths are explicit. The OpenCode steps are distinct traces in one generated recorded turn; saving a whole-turn receipt must preserve both, without merging another producer's shared trace.
- Codex records a successful file edit; OpenCode records an unknown-result attempt. Their relative path agreement is insufficient for a collision claim or shared Git attribution.
- The fixture records an approval wait but proves no native terminal target. A correct answer states this limitation. No native action should occur.
- Retention, disabled capture, unsupported shapes or clipped text can remove search evidence. Zero matches cannot establish that no matching work occurred.

## Blank observation sheet

Print or copy this sheet privately for each consented exercise. Do not fill it with repository fixtures or invented results.

| Measure | Baseline | Agenturm | Notes / corrections |
| --- | --- | --- | --- |
| Task card / anonymous code | ______ | ______ | Consent and retention recorded separately |
| Reconstruction start / finish | ______ | ______ | Same scenario difficulty; counterbalance task order |
| Incorrect owner/session selections | ______ | ______ | Count unknown correctly distinguished |
| Unsupported verification conclusions | ______ | ______ | Check-after-edit and assistant-claim probes |
| Failure source reached / wrong source | ______ | ______ | Preserve exact event identity |
| Native target verified / unavailable | ______ | ______ | Never count a synthetic target as success |
| Saved-task corrections on reopening | ______ | ______ | Include lost accepted/rejected decisions |
| Repeated rejected approach | ______ | ______ | Record observable behavior, not assumed intent |
| Missed explicit requests / known requests | ______ / ______ | ______ / ______ | Supply denominators |
| Actionable alerts / rated alerts | ______ / ______ | ______ / ______ | Keep heuristic observations separate |
| Reconnect recovery and stale-state mistakes | ______ | ______ | Record version and source coverage |
| One-week voluntary reuse | Not observed | ______ | Contact only with separate consent |

## Reporting limits and completion boundary

Recruitment, consent, actual measurements, live Claude/Pi coverage, willingness to pay and voluntary repeat use remain open external work. Prepared scripts, rendered HTML and automated assertions do not change the observed participant count from zero. Report paired individual measurements, missing observations and sample selection before aggregate percentages. Do not describe proposed 90% actionable-alert precision as achieved. Archive synthetic technical validation separately from consented human-study results.
