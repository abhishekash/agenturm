# Saved tasks and handoffs

Keep one task across agent sessions. Save its intent, decisions, next step, linked runs, and workspace evidence locally; then prepare a brief to continue with the agent you choose.

## The daily loop

1. Select the session you want to keep in the monitor. Press **b**, then **n**. Or use **Ctrl+P → Save selected session as a task**.
2. Review the suggested title and local workspace. Write the intent, decisions, and next step in the brief. **Ctrl+S** saves the task, links the selected run or session, and captures Git changes if a workspace was supplied. A blank task works without a receiver.
3. Open the task to **Edit** its brief, inspect **Runs**, **Link** the monitoring selection, or **Capture** the workspace again. To link another agent/run, return to the monitor, select it, and reopen the saved task. Existing exact runs are refreshed rather than counted twice.
4. Choose **Handoff**. Review the saved notes, diff, run usage, and recorded checks. **Copy brief** copies what you previewed; paste it into the agent you want to continue with. **Write MD + HTML** saves the same evidence as two new files.
5. **Archive** finished tasks to clear the list. **Archived** includes them again; **Restore** returns a task to the active list. Archival retains its notes and evidence.

**Esc** moves back one screen. In an edited form, the first Esc warns about unsaved changes; the second discards them. **Tab** moves between fields and actions, including out of the notes editor. **Ctrl+R** reloads a saved task after a concurrent edit. Revision conflicts keep your text visible and refuse to overwrite the newer record: copy your edits, return to the task, reload, and reconcile them.

Saving **Review → Save task** keeps the frozen execution boundary. An OpenCode recorded turn retains all its recorded step IDs, checks and turn usage; selecting a later member does not turn the receipt into one step. Linking both a turn and a contained step makes aggregate usage unavailable rather than counting the same tokens twice. A frozen snapshot whose scope contradicts its wrapper is rejected.

Frozen usage must agree with the saved association. Decision excerpts keep the chosen event's step identity, including a later step within a turn. Capture revocation is rechecked before saving source evidence; independently edited titles and notes remain yours.

Older saved links with inconsistent scope remain readable, including your notes and original evidence. **Runs** labels these **Repair scope**, and handoffs explain why their usage is unavailable. Explicitly relink the matching Review, or unlink the inconsistent record. Merely opening or editing your task does not rewrite the original evidence.

## Keep the monitor calm

Saved tasks use a separate full-screen path. The monitor gains a shortcut and palette entries; its session rows and panels do not grow. At 120 columns the task list keeps room for titles:

```text
Saved tasks · 2
[ Find a saved task…                                                                                                  ]
Task                                                                                Links  Saved
Fix refresh token expiry                                                              2    Nov 04 12:10
Finish release notes                                                                  1    Nov 04 11:35

Saved locally. Open a task to capture changes or prepare a handoff.
[ New · n                              ][ Open                             ][ Archived · a                           ]
↑/↓ Move  Enter Open  n New  Esc Monitor
```

The same single-pane layout fits a 60-column split; long table titles truncate and open in the detail view:

```text
Saved tasks · 2
[ Find a saved task…                                     ]
Task                           Links  Saved
Fix refresh token expiry       2      Nov 04 12:10
Finish release notes           1      Nov 04 11:35

Saved locally. Open a task to capture changes or
prepare a handoff.
[ New · n         ][ Open          ][ Archived · a       ]
↑/↓ Move  Enter Open  n New  Esc Monitor
```

At **80×24** and **60×20**, the list, details, run links, and handoff are separate screens. The details scroll; actions stay below them. At the narrow size, the editor uses the whole screen so several lines of the brief remain visible. Below 60×20, a size notice replaces the content while retaining edits. Enlarge the terminal to continue.

The layout has no outer panel border around the task list or detail body. Only the editor adds one enclosing border, with standard input outlines inside. Every row contains three fields: task title, link count, saved time; no repeated runtime/status paragraph, permanent marker, or cost/cache boilerplate. List controls use 10 of 20 rows at the minimum size, leaving 10 for the table including its header. Details reserve 7 rows for heading, feedback, actions, and hints, leaving 13 for content. Handoff keeps its destination field visible and leaves 10 rows for the preview at the floor size.

Loading and saving display a status immediately and disable duplicate actions. Empty lists explain how to create a task. Capture failures retain the task and show the issue. Disconnected monitoring does not prevent editing existing saved work. SQLite, Git capture, and file export run outside the UI input thread. There is no background Git polling.

## Commands without a running monitor

```bash
agenturm task create --title "Fix refresh expiry" --workspace . \
  --notes "Intent: reject expired tokens. Next: add a replay test." --snapshot
# Prints the saved task UUID. Substitute it for TASK_ID below.

agenturm task list
agenturm task show TASK_ID
agenturm task update TASK_ID --notes-file ./brief.txt
agenturm task snapshot TASK_ID
agenturm task handoff TASK_ID
agenturm task handoff TASK_ID --output ./refresh-handoff.md
agenturm task archive TASK_ID
```

`handoff --output` creates both `.md` and `.html`, with embedded styles and light/dark support. A `.json` destination also creates a JSON copy. Existing destinations are never overwritten. Copy and export are explicit local operations; Agenturm does not send the brief or launch another agent. A linked session may offer **Copy resume** when its runtime has a supported native resume command; check whether the session is already active before running it.

For scripting, each command accepts `--json`. `handoff --format html` or `--format json` selects stdout format. Diagnostics go to stderr. Exit codes are 0 for success, 2 for invalid input/storage errors, and 3 for revision conflicts. Mutating an existing task can include `--expected-revision N`; inspect the revision with `show --json` first.

```bash
agenturm task link TASK_ID --session-id SESSION_ID --runtime Codex
agenturm task link TASK_ID --evidence-file ./run-evidence.json
agenturm task unlink TASK_ID LINK_KEY
agenturm task list --all --offset 1000 --json
agenturm task archive TASK_ID --restore
```

The terminal loads metadata for the newest 1,000 tasks and shows a coverage notice if more exist. Search covers loaded tasks. CLI `list --offset` pages through older tasks; JSON includes total and next offset. Full records load only when opened.

## Evidence and usage boundaries

An exact run link uses a 32-character hexadecimal trace ID. Its usage comes from that recorded run, not the lifetime session total. Different agents can be linked to one saved task deliberately. Session-only associations are kept for context and excluded from numeric totals. A task allows 64 links; refreshing an existing trace replaces its evidence without adding it again.

Input and output form the token total. Cached input, cache writes, and reasoning tokens are breakdowns, so do not add them again. Reported USD costs are captured evidence; Agenturm does not invent prices, estimate subscription charges, or claim cache savings. Missing or partial telemetry is labeled. The selected run's loaded activity can contribute up to eight recorded test/check commands; this is a bounded excerpt, not a complete validation history. Relink to update it.

The first successful workspace capture pins the current Git commit as the base. Later captures include committed, staged, and unstaged tracked changes relative to that base, plus untracked paths. Untracked file contents and ignored files are omitted. Git hooks, configured clean/smudge/process filters, external diff programs, and text conversion are disabled. Capture does not modify files, refs, or the index and does not run checks.

Changes may predate the task or come from another agent in the workspace. Active writers may change files during or after capture. A recorded passing command is not proof that the captured diff passes. Handoffs state these limits and include capture timestamps. Changing the task workspace resets the saved base. A temporarily unavailable workspace preserves the existing base for the next capture.

Git capture has an eight-second work budget, individual command timeouts, a 128 KiB diff limit, and at most 100 displayed changed paths. Partial output and unavailable repositories are labeled. Notes are limited to 32,000 characters and each saved record to 512 KiB. Nothing silently evicts saved tasks.

## Local storage and sharing

Saved tasks live in `$XDG_STATE_HOME/agenturm/tasks.sqlite3`, or `~/.local/state/agenturm/tasks.sqlite3` by default. Set `AGENTURM_TASKS_DB` to choose another file; CLI commands also accept `--tasks-db PATH`. This database is separate from telemetry, so telemetry retention/clearing does not delete the saved notes, links, or captured diff. Back up this file to preserve saved tasks.

The database and exported files are created owner-readable/writable only. Notes, workspace paths, commands, and captured diffs can contain sensitive project material. Capture reads the repository directly; saved handoffs are not automatically redacted. Review the preview before copying or sharing it. HTML exports escape captured content and contain no remote assets. When monitoring a remote receiver, its workspace path is retained as evidence but is not automatically selected as a local capture directory.

## Structured briefs and decision history

**Brief** edits intent, acceptance criteria, constraints, open questions, and the
next action. **Decisions** records accepted and rejected approaches with rationale,
scope and optional source references. Manual entry is the default. Explicit
excerpt capture opens an unsaved draft for review. Supersession creates an
immutable version in the same task; active decisions lead the handoff, while
history remains available. Missing retained telemetry does not delete a decision
or establish that its source still exists.

Brief and decision writes share the task's optimistic revision. A stale revision
refuses the write and retains your draft. Limits are 128 decision versions per
task, 4,000 characters per text/rationale field and the existing 512 KiB aggregate
task budget. Notes retain their 32,000-character bound and links their 64-link
bound. Over-budget edits fail visibly; user decisions are never silently evicted.

Before opening an older task database with a new version, make an explicit SQLite
backup using `agenturm task --tasks-db OLD backup NEW`. This command uses SQLite's
backup API. Stop all writers before restoring a backup. The original task table
columns are preserved; structured data lives in additive side tables. An older
notes-only writer cannot display decisions but preserves those tables; the newer
client observes the changed task revision. Mixed-version editing is discouraged.
No migration infers structured decisions from existing freeform notes.

```sh
agenturm task brief TASK_ID --intent "Reject expired tokens" --next-action "Add replay regression"
agenturm task decide TASK_ID --kind accepted --text "Preserve the public API" --rationale "Existing callers depend on it"
agenturm task decisions TASK_ID --json
agenturm task handoff TASK_ID --include-decision-history
```

For bounded file input, use `task brief TASK_ID --file brief.json` and
`task decide TASK_ID --kind accepted --text-file decision.txt`. Brief JSON accepts
only the documented fields. File and field limits are validated before saving.

Native runs with an explicit recorded lifecycle start can be linked with
`task link TASK_ID --receiver RECEIVER --producer SERVICE --session-id SESSION --run-start-ns START`.
The positive start timestamp must come from the runtime record. This identifies
a run without inventing a trace or numeric usage: native-only accounting remains
unknown unless exact recorded run totals are available. Exact trace-backed links
and older session-only links remain supported. A saved review carries its frozen
scope; saving it does not silently capture a different currently selected turn.
