# Navigable attention

The Agents header counted attention items without offering a path to them. The
list only included agents in the current scope/search/expansion, while the
summary was computed from all available agents, so the number could disagree
with what the list showed.

## Behavior

- Clicking the attention count or pressing `!` opens Agents, clears the Agents
  search and individual session scope, reveals matching children inside
  collapsed branches, and selects the first matching agent. The new
  **Attention N** filter is also available in the Show selector and the command
  palette. `a` returns to Today.
- The filter counts the same agents as the header summary: current unfinished
  work and today's unreviewed failures/status checks, excluding reviewed and
  dismissed items and internal reviews. Ancestor context rows shown for tree
  navigation keep their own status and are not counted as extra items.
- Rows name the reason instead of repeating **Needs attention**: **Needs
  approval**, **Needs reply**, **Run failed**, **Interrupted**, **Incomplete**,
  **Check progress**, **Check status**, or **Status unknown**.
- The inspector preview starts with **Why attention** and **Next step**. A
  recorded approval/reply names the agent terminal and points to Workflow; a
  failure points to Activity or a linked trace; a possible stall points to the
  terminal and Diagnostics. An unconfirmed wait explicitly says no approval or
  reply request was recorded.
- `!` remains literal text in search inputs.

## Limits

The count reflects only sources the receiver can currently observe. Unavailable
older history, omitted sources and unconfirmed parent/child relationships remain
unknown; the interface does not convert that gap into an attention item or a
confirmed zero. Rows with `Status unknown` are listed but are not counted as
actionable attention.

## Tests

```sh
.venv/bin/python -m unittest tests.test_attention_navigation -v
```

Focused attention plus hierarchy, readability, session, observation-deck,
Today, error-reminder and workflow modules passed 98 tests on Python 3.13.5 /
Textual 8.2.8. The new tests use generated agent records and cover:

- header click and `!` from another view at 120×36, 80×24 and 60×18;
- clearing a search, an individual session scope and a collapsed branch;
- selecting a recorded request before a failed or uncertain row;
- row labels for approval, reply, failure, possible stall and unknown status;
- the reason and next step in both compact and expanded inspectors, including
  the explicit "no request was recorded" wording;
- `!` typing in search, an empty attention result, and reset behavior.

The full Python 3.13 suite passed 1,377 tests (14 skipped). A footer-width
regression was corrected before that successful full run.

These are headless behavioral tests, not real-terminal rendering measurements.
