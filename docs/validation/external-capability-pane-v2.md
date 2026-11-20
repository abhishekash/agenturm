# Installable observational capability pane

Recorded 20 November 2026. A second, independently installed capability process now contributes an **Action observations** pane beside the existing Prompt signals pane. The TUI uses its existing pane registry, navigation and table renderer. No capability-specific branch was added to `terminal_ui.py`, and neither pane starts by default.

## Install and activate

```sh
python examples/action_observations_process/install.py --destination /absolute/private/actions
python examples/prompt_signals_process/install.py --destination /absolute/private/prompts
agenturm cockpit \
  --capability-config /absolute/private/prompts/capability.json \
  --capability-config /absolute/private/actions/capability.json
```

The Action fixture writes a SHA-256-pinned executable, manifest and descriptor into a new private directory. The manifest must provide `capability-adapter/1`, require the host's `pane-model/1` feature and request the explicit `scoped-aggregate-read` grant. Admission parses both descriptors before launching a process; a malformed descriptor fails that explicit startup. Once admitted, a crashed or malformed child makes only its own pane unavailable. A healthy sibling pane remains usable.

## Closed schema-v2 model

The existing schema-v1 Prompt signals descriptor, process frames and displayed model are unchanged. Schema v2 adds a bounded `model.rows` declaration with one to six unique keys. A row may select only `observed_events`, `user_messages`, `shell_actions` or `first_shell_after_user_ms`, or calculate an integer ratio between the three count fields. The host supplies every row label and computes every value and coverage string from its own typed facts. It compares the child response, including state, summary, row order, cells and detail, to that exact result. A child cannot add a score, free text or an unsupported field to a displayed model.

```json
{"schema":2,"pane":{"id":"action-observations","title":"Action observations",
  "columns":[["Signal",34,2],["Observed",12,0]]},
 "model":{"rows":[
   {"key":"users","value":{"fact":"user_messages"}},
   {"key":"actions-per-user","value":{"ratio":["shell_actions","user_messages"]}}]}}
```

The complete descriptor also names the absolute manifest and `scoped-aggregate-read` grant. Schema-v2 columns have the fixed semantic headings `Signal`, `Observed`, and optionally `Coverage`; widths and priorities are bounded by the pane registry, and declared widths may total at most 54 cells.

The ratio is rounded half-up to two decimal places with integer arithmetic. A zero denominator renders `unavailable`. The sample's `shell_actions / user_messages` is a count relationship over a bounded recent Activity window. It is **not** prompt efficacy, quality, causality, productivity or a complete session measure. When the window is partial, the pane says so. The v2 compact layout has two columns whose declared widths total at most 54 cells; the summary carries coverage once instead of repeating it in every row.

```text
Action observations · 60 columns
Signal                         Observed
Recorded user messages          2
Recorded shell actions          1
shell actions / user messages   0.50
3 scoped events · complete recent window · observational signals only
```

## Policy and limits

Only an exact service, session and canonical agent with affirmative content capture can query the capability. A live receiver gate checks the same receiver and exact scope before and after the bounded Activity request; the reader rejects a response over its requested 200-row cap or containing non-object events. Revocation clears the host's warm model on the next pane query. Only aggregate counts and a coarse elapsed time cross the child pipe. Prompt text, commands, output, paths and source identities do not. The child never gains native capture authority. The policy watcher has the existing 250 ms cached-approval limit, so this does not claim atomic revocation of already displayed text.

This is a narrow external pane contract over the current host-owned facts. New fact domains, arbitrary calculations, production third-party SDK guarantees and OS-level process sandboxing remain open. The fixture is POSIX-only and runs as the installing user. The terminal shell remains a full-screen tabbed dashboard; at 60×18 it shows one selected capability pane, with no extra nested border or redundant per-row coverage field.

## Verification

`tests.test_action_observations_process` installs both process artifacts and checks exact v1/v2 coexistence, ratio and zero-denominator behavior, forged summary/detail/row rejection, invalid formulas and width bounds, exact-scope denial, oversized or malformed receiver rows, receiver revocation, one-child failure isolation, and both panes in Textual at 60×18. Five new tests pass. The legacy Prompt signals process/provider suite passes 15 tests. The broader cockpit activation suite has one existing unrelated `SimpleNamespace.coverage` fixture error, previously reproduced on main; this slice does not claim that suite green.

```sh
/home/abhishekash/abhi-codes/agentic/agenturm/.venv/bin/python -m unittest \
  tests.test_action_observations_process tests.test_prompt_signals_process \
  tests.test_prompt_signals_capability -q
```
