# Agenturm Rust bundle for Linux

The bundle contains the Agenturm CLI and inspector, the process host, the
Codex, Pi, Claude Code, Kiro, and OpenCode adapters and their
SHA-256-pinned catalog, the trusted connector driver, and the storage service. The runtime
requires a compatible Linux system and does not require Python or Rust.
The managed collector uses a running systemd user manager. Foreground
collection is available on Linux systems without a user manager.

| Platform | Build | Collector |
| --- | --- | --- |
| Ubuntu 24.04 x86-64 | Local bundle and CI artifact | Foreground and systemd user service verified |
| Ubuntu 24.04 ARM64 / aarch64 | Native tagged bundle | Foreground, prior-release upgrade and systemd user service verified |
| Other Linux releases | Host build script available | Distribution qualification open |
| macOS and Windows | Bundle qualification open | Service integration open |

## Build and install

Rust 0.3.8 is the latest published opt-in release. Rust 0.3.9 is a qualified
candidate for Linux x86-64 and ARM64, held for the next daily release batch.
Use the source installer from a checkout and verify the matching archive.

~~~sh
agenturm=$(bash scripts/install-rust.sh install --version 0.3.8)
"$agenturm" --version
~~~

[Published Rust 0.3.8 assets](https://github.com/abhishekash/agenturm/releases/tag/rust-v0.3.8)

It installs the complete versioned bundle and an agenturm-rust launcher,
preserving the existing agenturm command and leaving services and history
unchanged. Use an exact version; there is no automatic latest selection.
The hosted bootstrap is not published. See the
[installation assessment](rust-distribution-installation.md) for update,
offline verification, uninstall, supported runtime and integrity details.
Manual archive installation remains available below.

From a source checkout on Linux with the pinned Rust toolchain and a native C
build toolchain, run `scripts/build-rust-linux-bundle.sh ./dist`. The script
builds for the local Linux architecture and emits a tarball and checksum.

From the directory containing the published archive and its checksum, use
the matching asset for your architecture. This example uses x86-64.

```sh
archive="agenturm-0.3.8-linux-x86_64.tar.gz"
sha256sum -c "$archive.sha256"
mkdir -p "$HOME/.local/opt"
tar -xzf "$archive" -C "$HOME/.local/opt"
agenturm="$HOME/.local/opt/${archive%.tar.gz}/bin/agenturm"
"$agenturm" --version
```

Use `agenturm-0.3.8-linux-aarch64.tar.gz` and its sidecar on ARM64. Keep the
catalog and binaries in the extracted directory. Verify the matching release
sidecar for the version being installed.

The published **Rust 0.3.8** Linux archives have these SHA-256 digests:

| Platform | SHA-256 |
| --- | --- |
| Linux x86-64 | `528d4c7057d280258788d44e684505b7c552bc5f4dc8f730c9a24d568fc0d657` |
| Linux ARM64 / aarch64 | `95e37c0254f9bcd1650b1a7d9a630a40c180f61dc80f43035e934c732dd556cc` |

The 0.3.8 tag run passed archive qualification and the offline installer check on
both native Linux architectures. The published assets match the digests above.
[Native tag qualification](https://github.com/abhishekash/agenturm/actions/runs/45121070134)

### Rust 0.3.9 development candidate

The earlier 0.3.9 source passed local Linux x86-64 qualification and native
x86-64/ARM64 CI. The final migration changes are undergoing their own archive
qualification in PR #379; those earlier checks do not certify this new source.
This is not a published release. Build the source you intend to test, or use
its matching checksum-verified CI archive, then install locally:

```sh
bash scripts/build-rust-linux-bundle.sh ./dist
archive="./dist/agenturm-0.3.9-linux-$(uname -m).tar.gz"
agenturm=$(bash scripts/install-rust.sh install --version 0.3.9 \
  --archive "$archive" --checksum "$archive.sha256")
"$agenturm" --version
```

The release policy permits at most one publication in a rolling 24-hour window.
Rust 0.3.8 was published on 23 November 2026 at 06:03:24 UTC. The next batch is
eligible no earlier than 24 November at 06:03:24 UTC (07:03:24 Europe/Berlin),
subject to the completed batch's qualification and any later publication.
Implementation, PR merges and candidate qualification continue between releases.
See the [qualification record](../validation/rust-real-session-roster.md).

## Open the cockpit

From a terminal, run the installed binary.

```sh
"$agenturm"
```

The cockpit opens on the Agenturm command center (Agents). It uses
`${XDG_STATE_HOME:-$HOME/.local/state}/agenturm-rust/history.sqlite`.
It starts session-owned collection of supported local harnesses when the default database is
available for writing. When another writer owns the database, the cockpit
reads the same history. When the cockpit owns collection, it also receives
OTLP/HTTP traces at `http://127.0.0.1:14318/v1/traces`. Set `AGENTURM_OTLP_PORT`
to use another loopback port. Both stop on quit.

Follow collection prioritizes exact transcript handles held by supported native
Codex and Claude processes, skips confirmed guardian reviews, and rotates queued
sources within the existing source limit. The footer reports selection and
partial discovery coverage. A fixed `discover` configuration remains a bounded
recent-file selection. An explicit `--db` cockpit states that discovery is
disabled and needs a separate writer for fresh observations.

The roster uses explicit saved task names first, then safe native Codex and
Claude conversation names. Provider, agent name and role provide supporting context. Native recorded
workspaces supply project headings when an operator task link does not override
them. Missing names remain “Name unavailable”; tool activity and session IDs do
not become names. Source and session IDs stay in the inspector and copy/export.
Names are display metadata and never change stable selection or task links.
Metadata is rechecked against the selected history and exact retained source
receipt, including after restart. Native logs and databases remain read-only.

Counts cover the loaded retained page. Working reflects recorded lifecycle,
not proof of current process activity. Open handles prioritize collection and
do not change lifecycle state. Header-only files without any session facts do
not yet create retained roster rows. Discovery scans are bounded and can report
partial coverage. Claude mutable-name scans are bounded to 16 MiB per transcript
and 32 MiB per roster read. A verified first-line workspace can survive that
limit with a partial-title notice; a two-record snapshot prelude requires a
fully verified committed prefix before its workspace is shown.

Press `F2` or `Ctrl+P` to preview Observation, Current, Bright, Dark and Solarized.
Enter saves the selected theme; Escape restores the previous one. The shared
palette applies to the command center, evidence tables, inspectors and dialogs.

A newly created default cockpit history uses a 14-day age policy. Existing
histories retain their current policy; an explicitly selected database can
receive a policy through the commands below. Active writers check the policy
once a minute. The policy removes expired observation rows while preserving
source checkpoints and replay receipts. Native source files and existing
backups retain their original data.

```sh
"$agenturm" retention status --db history.sqlite
"$agenturm" retention set --db history.sqlite --max-age-days 30
"$agenturm" retention disable --db history.sqlite
"$agenturm" retention run --db history.sqlite
```

`set` configures the age; `run` applies it immediately using the writer's
clock. `disable` persists across restart. Increasing the age or disabling
retention preserves the established cutoff and does not recover expired
rows. Stop a local collector before changing its policy through a separate
CLI process. A broker maintenance identity can configure, disable or apply
the policy while the service is running:

```sh
"$agenturm" broker retention set --broker-config maintenance.json --max-age-days 30
"$agenturm" broker retention status --broker-config reader.json
```

Reader and collector identities can inspect the broker policy. Its
maintenance identity owns changes and explicit cleanup. Backups preserve the
policy; a broker owner can disable it before starting a restored history.

### Keyboard navigation

These controls apply to Rust 0.3.9. The previous 0.3.8 controls remain in its [tagged guide](https://github.com/abhishekash/agenturm/blob/rust-v0.3.8/docs/rfcs/rust-linux-bundle.md).

Use `↑` and `↓` to choose a session, `/` to search the loaded roster,
and Enter to open its Overview. The newest roster page refreshes while open.
Press `R` in Agents to refresh it immediately. Open selected Activity with `s` or
Review with `Ctrl+R` or `V`, and open the selected session’s Usage with `u`.
`Tab` selects the status filter, `G` changes grouping, and `Enter` opens the
inspector. `Esc` restores the same group, filter and exact session. Press
`y` in Review to acknowledge the displayed evidence. The acknowledgement
is stored next to the history database and applies to the reviewed version.

Print a bounded session report without entering the full-screen UI:

```sh
"$agenturm" cockpit --once --db history.sqlite
"$agenturm" cockpit --once --db history.sqlite --json --limit 64
"$agenturm" broker cockpit --once --broker-config reader.json --json
```

`agenturm --once` uses the default cockpit's discovery and history, then
stops its session-owned workers after printing. An explicit `--db` reads
retained history. The report preserves each source and session, reports an
unknown state when no lifecycle was recorded, and indicates when older
sessions are outside its page. A trusted remote reader supports the same
report with `--endpoint` and `--token-file` in place of `--db`.

From Agents, `g` opens the cross-source Overview. Use `2` for Activity,
`3` for MCP, `4` for Traces, `5` for recorded Host, `6` for Metrics,
`7` for Findings, `9` for Operations, and `U` for workspace Usage. `H` opens
live Host; `8` remains a trace alias. Press `T` in a history view to select a
recorded turn, `F1` for keyboard help, and `q` to quit. The available
evidence varies by source. For Codex MCP calls the view includes the server,
tool, method, duration and outcome when recorded.

Press `m` in the MCP pane to choose an exact recorded Codex call. Enter
opens a byte-verified payload where an MCP approval is active. Use
`content grant --kind codex.mcp_payload` with the history path, source
configuration and source ID to establish it. Use
`content revoke --kind codex.mcp_payload` to remove that approval.

In MCP, `/` searches retained calls before pagination at the selected history
snapshot. It matches server, tool, method and result, treating `%` and `_`
literally. `f` chooses a server on the loaded page, and `i` switches between
calls and configured inventory. Enter on an eligible call opens its exact
linked traces; Escape restores the same call. A configured server is not a
claim that it is currently connected.

In Metrics, `/` filters the loaded instrument page by name, service, kind,
unit, scope or source; the filter continues across pages. Choose a series to
browse points and exact retained exemplars. Usage displays seven UTC days,
keeps currencies separate, and distinguishes explicit zero from unreported
counters. Its coverage labels distinguish native reports, representative
traces and activity logs.

In Findings, `/` filters loaded groups and occurrences. Enter opens a group,
then an exact occurrence. Where an exact retained failed Codex command has a
source route, `s` requests its protected evidence under the current grant;
unsupported bodies stay unavailable. Source text never enters loaded-view
copy or export. In live Host (`H`), `/` filters same-user runtime, PID and
working-directory basename. Enter opens process details and `R` refreshes
while retaining boot/PID/start identity. `5` opens recorded Host history.
Escape first clears an active filter, then returns to the prior view.

From Agents or a session or turn detail, press `b` to open saved tasks.
Use `n` to create a task linked to the selection, `r` to see its linked
runs, and `h` to create a handoff using the current selection. Open the
handoff list with `o` and export the selected handoff with `x`. Tasks can
also record a Git snapshot from a directory you choose with `G`.

From Agents, `X` marks one exact current pending request seen, with a chooser
when more than one matches. `Ctrl+F` opens retained investigations. `c` requests
copy of the loaded metadata and `x` exports that metadata to a new private
Markdown/HTML/JSON directory. Private modal bodies stay outside these exports.

From Agents, press `w` to see recorded requests. Use `s` to mark an item
seen and `l` to focus the matching tmux pane when Agenturm verifies the
route. `c` verifies the selected exact pending main-session request and previews
and copies `codex resume …` or `claude --resume …` for supported sources.
It never launches the command; Escape restores the request selection. Respond
in the harness terminal.

Press `S` in Workflow to open notification settings. Enter toggles desktop
notifications; Tab selects the task token, USD and elapsed-minute thresholds;
Ctrl+S saves them. Zero disables a threshold. `z` snoozes delivery for ten
minutes or ends the snooze. Tab from the request list switches to typed
incidents. A Failed or Unavailable delivery can be retried with `T`; retries
require the same current request or threshold crossing and exact delivery
attempt. Claimed and uncertain attempts are not retried automatically.

Settings start with desktop notifications unset, leaving the explicit watch
configuration in control. Saving Off overrides that configuration. New or
already-above-threshold evidence establishes a quiet baseline. Token and cost
alerts require complete native turn evidence and a fresh below-to-above
crossing; missing values remain unknown. Elapsed alerts require an
authenticated current execution-start receipt. Completion is quiet. Cockpit
delivery is checked while fresh views load; paused or stale history does not
replay notifications. Messages contain only a generic count, without task
names, commands or private bodies. These controls are implemented in the
unpublished candidate and remain subject to final package qualification.

In Review, press `n` for saved decision notes. In the decision list, press
`e` to write a note against the selected evidence version and Ctrl+S to save it.
Review shows recorded Codex edit counts. Press `e` from Review or turn
activity to select a recorded edit, then Enter to view its paths and any
bounded diff or file contents. Escape clears the preview and returns to
the same selection.

Edit detail requires its own approval and verifies the selected event,
session, turn and operation against the retained source receipt. Set `db`
to the history file, `config` to the collection configuration, and
`source_id` to the exact recorded source ID.

```sh
"$agenturm" content grant --kind codex.edit_detail --db "$db" \
  --config "$config" --source-id "$source_id"
"$agenturm" content revoke --kind codex.edit_detail --db "$db" \
  --source-id "$source_id"
```

Select a session and press `p` for eligible user prompts. Press Enter to
reveal one and Escape to clear it. The view requires a current grant for a
pinned source and uses `sources.json` next to the history database or the
path set in `AGENTURM_CONTENT_CONFIG`. It verifies retained JSONL rows by
their committed receipts. OpenCode SQLite prompts with an admission digest
can reopen after later sweeps or unrelated committed writes. The original
row identity, turn, timestamp and bytes must still verify in the native
database. Legacy rows without a digest require the exact committed sweep.
New prompt references carry a logical history ID and snapshot. Reopen the prompt list after
restore or snapshot expiry to make a fresh selection.
The default cockpit uses its collector's writer lease for reveal. Establish
or revoke grants from the CLI while the history writer is available.

Use `"$agenturm" cockpit --db <path>` or `"$agenturm" inspect --db <path>`
to open a specific existing Rust store without starting local collection.
For collection between cockpit sessions, set up the managed collector below.

## Export a Review receipt

Select one exact source, session, and turn. Add `--worker-id` for the
recorded worker. Omitting it selects workerless evidence. The receipt
records the store snapshot ID, observation revisions, and any cursor for
older evidence. Each receipt covers one page.

```sh
"$agenturm" review --db "$db" --harness-id "$source_id" \
  --session-id "$session" --turn-id "$turn" --limit 200 \
  --format json --output ./review-receipt
```

The new private directory contains `review.md`, `review.html`, and `review.json`.
Use `--format text`, `--format markdown`, or `--format html` for readable
stdout output. Omitting `--format` preserves the history query JSON on
stdout. Review receipts contain retained observation metadata. Use the
content-grant commands separately to inspect protected bodies.

### Read an explicit Python 0.2.8 receiver

An already-running authenticated Python receiver is selected explicitly;
Agenturm's Rust client does not launch Python. Use a literal loopback endpoint
and the receiver's token file. Review rechecks the exact receiver, scope and
capture policy around bounded evidence reads and before output:

```sh
"$agenturm" review --receiver-protocol python-v0.2.8 \
  --endpoint "$endpoint" --token-file "$token_file" --service "$service" \
  --session-id "$session" --trace-id "$trace" --pages 5 --format json \
  -o ./captured-review.json
"$agenturm" investigate command 'cargo test' --failed \
  --receiver-protocol python-v0.2.8 --endpoint "$endpoint" \
  --token-file "$token_file" --service "$service" --pages 5 --format json
```

Use `--turn-of-trace` instead of `--trace-id` to select the recorded whole turn.
Review exports use new private destinations and include Markdown and HTML.
Investigation cursors are opaque; keep the query, receiver and scope unchanged
when continuing a page. Source references identify one exact occurrence and
are revalidated on reveal. Capture can be metadata-only, in which case private
text is unavailable. In the explicit Python receiver cockpit, choose an exact trace and press
Ctrl+R for run Review or Ctrl+T for an explicitly recorded whole turn. The
private viewer has a fixed lifetime and no copy/export action; Escape restores
the exact trace. Receiver authorization is polled with bounded checks rather
than a server-held display lock. Private output backpressure and turn-member
changes are part of candidate qualification.

## Edit tasks from the CLI

Set `db` to an existing Rust history database and get a task's ID and
revision from `task list` or `task show`. Pass `--expected-revision` when
updating or archiving a task and when changing its brief, decisions, exact
links, or Git snapshot. The local task CLI returns exit code 3 on a revision
conflict and 2 on invalid input or storage failure.

```sh
"$agenturm" task create --db "$db" --title "Review the release" --workspace .
"$agenturm" task update "$task" --db "$db" --notes-file notes.txt --expected-revision "$revision"
"$agenturm" task brief "$task" --db "$db" --next-action "Check the upgraded store"
"$agenturm" task decide "$task" --db "$db" --kind accepted --text "Keep the current format"
"$agenturm" task decisions "$task" --db "$db"
"$agenturm" task handoff "$task" --db "$db" --output ./release-handoff
```

`task brief --file` reads a JSON object with `intent`, `acceptance_criteria`,
`constraints`, `open_questions`, and `next_action`. Omitted brief fields keep
their previous values and an empty string clears a field. Brief and decision
text fields take up to 4 KiB of UTF-8 each. To replace a decision, add a new
version with `--supersedes` and the current decision ID. `task decisions`
shows current and superseded versions.

`task snapshot` captures a summary of the selected local Git worktree. Set
or change the directory with `task update --workspace`. Changing it clears
the prior Git summary. `task handoff` uses a single saved task revision and
defaults to Markdown with current decisions. Use `--format html` or
`--format json` for another stdout format, and `--include-decision-history`
to include superseded decisions. With `--output`, a new private directory
contains both `handoff.md` and `handoff.html`.

`task archive` hides the task from the default list while preserving its
history. Use `task list --all` to include it and `task archive --restore`
to return it to the default list. The workspace backup includes these task
records and their decision history.

### Prepare a source-backed continuation

Use `content list` after granting user prompt access to find the event,
revision, turn, and worker if recorded. `task draft` captures one eligible
prompt from that exact scope. Give `--output` a new path to write a private
JSON file. Without `--output`, the excerpt and draft go to stdout.

```sh
"$agenturm" content list --db "$db" --config "$config" \
  --source-id "$source_id" --session-id "$session"
"$agenturm" task draft --db "$db" --config "$config" \
  --source-id "$source_id" --session-id "$session" --turn-id "$turn" \
  --event-id "$event" --revision "$event_revision" --task-id "$task" \
  --output ./task-draft.json
"$agenturm" task approve-draft ./task-draft.json --db "$db" --config "$config"
"$agenturm" task continuity "$task" --db "$db"
"$agenturm" task continuity "$task" --db "$db" --config "$config" \
  --approval-id "$approval"
```

Include `--worker-id` when the selected prompt belongs to a worker. Edit
the JSON `continuation` field before approving. Omit `--task-id` at draft
creation to approve the draft as a new task, using the JSON `title` or
`--title` on `approve-draft`. Approval checks the source bytes, event
revision, and original grant again. A draft for an existing task also
checks the task revision captured during preparation.

Use `task decide --source-file ./task-draft.json --config "$config"` with
the usual task ID, kind, and decision text to cite its source. For a draft
created without `--task-id`, also pass the current task `--expected-revision`.
Use an approval ID from `task continuity` to reopen the original excerpt.
The original grant must still be active. Task handoffs and backups retain
the exact source reference and the text you wrote for the task.

For broker history, use `agenturm broker task` with `--broker-config` and
the broker's configured source and permission. If the connection breaks
after an approval is saved, the command can return an error. Check the
task's `continuity` list for the draft digest and the task's revision before
retrying. A draft digest can receive one approval.

## Open native traces

Point an OTLP/HTTP producer at the loopback receiver. Each span needs a
`session.id` or `gen_ai.conversation.id` string attribute. The receiver
accepts uncompressed JSON or protobuf at
`http://127.0.0.1:4318/v1/traces`. To attach the span to a native session,
also set `agenturm.source.id` to the exact collected harness source ID.
Press `4` from a native session to see its linked spans.

For turn-level evidence, set the span's `agenturm.turn.id` to the exact
native turn ID. When the turn belongs to a worker, also set the span's
`agenturm.worker.id` to its native worker ID. Use string values and put all
IDs on each span you want to link. A turn-scoped span requires its own
source and session attributes. Agenturm checks the native turn and worker
against the same history snapshot as the span.

```sh
"$agenturm" serve-traces --listen 127.0.0.1:4318 --db traces.sqlite --source-id otlp.local
```

You can also load an export directly.

```sh
"$agenturm" import-traces --file export.json --db traces.sqlite --source-id otlp.local
```

To list trace spans for a known native turn, add `--linked-traces` to
the exact Review command. Supply `--worker-id` if the turn belongs to
a worker. To open the native turn of a known trace, supply the trace ID,
its OTLP source, and the expected native source and session.

```sh
"$agenturm" review --db traces.sqlite --harness-id "$source_id" \
  --session-id "$session" --turn-id "$turn" --linked-traces --format json
"$agenturm" review --db traces.sqlite --harness-id "$source_id" \
  --session-id "$session" --turn-of-trace "$trace_id" \
  --trace-source-id otlp.local --format json
```

Trace-to-turn selection requires one explicit native scope among the
trace's turn-scoped spans and a retained anchor for that scope. It checks
up to 4,096 retained span rows for one trace. Spans with
session-only attributes remain available in the session trace view. If a
trace names conflicting turns or its native anchor has expired, choose an
explicit source, session, and turn for Review.

After the first stored span, open the cockpit in another terminal and press
`8` in Agents.

```sh
"$agenturm" cockpit --db traces.sqlite
```

If you target a database owned by an active history writer, the receiver
returns HTTP 503 until the writer releases its lease.

Use `run` to start a program whose OpenTelemetry exporter already honors
OTLP environment configuration. It starts a loopback listener on an
ephemeral port, sets the program's trace exporter to HTTP/protobuf, and
closes the listener after the program exits. The default source is
`otlp.run`. The program keeps its standard streams and exit status. Spans
without native session identity are grouped under the actual launch.

```sh
"$agenturm" run --db traces.sqlite --source-id otlp.local -- ./instrumented-program
```

The launcher replaces inherited OTLP endpoint, transport, and header
settings, disables metrics and log exporters for this run, and adds
loopback addresses to the proxy exclusions. Configure scope on the
producer as described above. The launcher is for existing instrumentation
and SDKs that honor these settings.
[OpenTelemetry exporter configuration](https://opentelemetry.io/docs/specs/otel/protocol/exporter/)

### Follow a span JSONL file or instrument Python

```sh
"$agenturm" watch-jsonl --file ./spans.jsonl --db "$db" \
  --source-id otlp.local --session-id "$session" --interval-ms 1000
"$agenturm" run --db "$db" --source-id otlp.local \
  --instrument python --service my-app -- python app.py
```

The follower checkpoints complete lines and committed prefixes; restart resumes
without publishing the same batch twice. Rotation and truncation begin a new
verified generation. Python instrumentation requires a compatible Agenturm SDK
and its selected integrations already installed in that interpreter. The Rust
launcher installs no dependencies and starts no Python receiver. Captured text
requires explicit `--capture-content` and the corresponding payload scope;
metadata is the default.

### Inspect reported error logs

The candidate accepts ordinary OTLP error/fatal records (severity 17–24) over
JSON or protobuf. Metadata records source, reported time, exact supplied scope
and genuine trace/span correlation. Missing session identity stays source-scoped.
Bodies, event labels and arbitrary attributes stay outside public history.
Existing `agent.activity` records retain their separate usage cohort.

To retain bounded scalar log text, explicitly enable it on the authenticated
receiver and separately grant display:

```sh
"$agenturm" serve-traces --listen 127.0.0.1:4318 --db "$db" \
  --source-id otlp.local --payload-scope standalone --token-file producer.token \
  --capture-log-text
# After stopping a local writer, or through the broker's maintenance grant:
"$agenturm" log-text grant --db "$db" --source-id otlp.local --payload-scope standalone
"$agenturm" log-text snapshot --db "$db" --source-id otlp.local
```

A source-only log needs no invented session flag. In Findings, inspect an exact
error-log occurrence and press `s` to open producer-captured text under its
current log grant. Escape redacts it and returns to that occurrence. A span
payload grant does not authorize log text. `log-text revoke` uses the same
binding flags as grant. `binding` prints the distinct `telemetry.log_text`
fingerprint for broker maintenance configuration; broker `log-text snapshot`
and `preview --selection` use the authenticated reader. Private bodies stay
outside ordinary query, copy and export.

Text must be a validated scalar of at most 2 KiB; a batch carries at most 64 KiB.
No text is reconstructed from older metadata-only receipts. When a producer
omits its record ID, public metadata forms the replay identity. Identical
metadata coalesces, and different captured bodies under that identity conflict;
provide a unique record ID to distinguish those records.

The log grant requires schema 20. A writer upgrades supported existing histories;
read-only use does not upgrade them. Schema 19 span grants and receipts remain
valid. Older executables refuse schema 20, so retain a pre-upgrade backup for
rollback. Restore requires fresh private-content approval as before.

### Send a Python launch to an existing receiver

```sh
"$agenturm" run --endpoint http://127.0.0.1:4318 \
  --token-file producer.token --instrument python --service my-app -- python app.py
```

This candidate route uses an already running authenticated native receiver or
an explicit loopback SSH tunnel. The receiver owns the source identity; the
launcher assigns a fresh standalone session and creates no local database or
listener. It validates the owner-only producer token and authenticated receiver
before starting the program. The receiver remains running when the program
exits or is interrupted. Choose either this destination or `--db`. Content
capture and source overrides are unavailable on the existing-receiver route.

## Use the workspace from scripts

Use `attention status --db <path> --workflow` or
`attention watch --db <path> --once --workflow` for a read-only JSON snapshot
of current typed incidents, saved notification settings and matching delivery
receipts. This report never starts a notifier or changes stored state. Exact
64-bit values are decimal strings. `delivery_scope` identifies current matched
evidence; absent history does not acquire an invented receipt scope.


`agenturm task --help` lists the native task and handoff commands. They use
the same task records as the cockpit. Run `agenturm attention status --db
<path>` for one pending-request snapshot or `agenturm attention watch --db
<path>` for changes.

Explicitly enable installed capability executables in
`${XDG_CONFIG_HOME:-$HOME/.config}/agenturm-rust/extensions.json`. The file
uses the inspector extension catalog with an additional `enabled` array
of selected plugin IDs. Agenturm verifies the pinned SHA-256 and the
declared data grants before starting them. An empty selection starts no
capability processes. A cockpit opened with `--db` accepts explicit
`--extension-catalog` and `--extension` options.

## Discover, collect, and inspect

The bundle discovers Codex, Pi, Claude Code, Kiro, and OpenCode in their
default local data directories. Discovery also honors `CODEX_HOME`,
`PI_CODING_AGENT_DIR`, `CLAUDE_CONFIG_DIR`, `KIRO_HOME`, and
`OPENCODE_DB`. It selects up to 16 sources, giving each available harness a
slot before taking additional recent transcript files. OpenCode contributes
one read-only SQLite source with a 1 GiB database limit, a 512 MiB WAL
limit, and a 50,000-row limit. Existing follow configurations retain the
roots they declare.

```sh
state="${XDG_STATE_HOME:-$HOME/.local/state}/agenturm-rust"
mkdir -m 700 -p "$state"
"$agenturm" discover > "$state/sources.json"
"$agenturm" service install --config "$state/sources.json" --db "$state/history.sqlite"
"$agenturm" service status
"$agenturm" query --db "$state/history.sqlite"
"$agenturm" inspect --db "$state/history.sqlite"
```

Discovery looks in `$CODEX_HOME/sessions` and
`$PI_CODING_AGENT_DIR/sessions`, defaulting to `~/.codex/sessions` and
`~/.pi/agent/sessions`. It selects up to four recent JSONL files per
harness by modification time. Use `--sessions-per-harness <1..8>` to set
the limit. Each file gets a source ID derived from its canonical path so an
unchanged path resumes its stored cursor. Discovery prints a configuration
snapshot. The collector watches the selected files, and newly created session
files become eligible when you regenerate the config and restart the service.
Previously collected observations remain available in the database. For rolling
discovery, run `discover --follow` when creating the config. A running collector
rescans the captured roots at most once every five seconds and watches the
same bounded selection of newest files. It starts even when both roots are
empty. A file that leaves the selection keeps its durable cursor and history.
To change the captured roots or replace a static config, write a temporary file
first and replace the config only after discovery succeeds.

To restart the managed collector, run
`systemctl --user restart agenturm-collector.service`. If there is no user
manager, use `"$agenturm" collect --config "$state/sources.json" --db
"$state/history.sqlite" --watch` instead of installing the service.

The turn picker lists the recorded turns on the current history page.
Use the arrow keys to choose a turn, then press Enter to open its scoped
observations. A recorded pull request appears as a work result alongside
the native lifecycle state.

Use `review` to read the typed history for an exact recorded turn as JSON.
Omit `--worker-id` to select its workerless records. Use the same scope and
the returned cursor fields to load an older page.

```sh
"$agenturm" review --db "$state/history.sqlite" \
  --harness-id <source-id> --session-id <session-id> --turn-id <turn-id>
```

## Back up and restore

Create a history-only snapshot while collection is running or stopped.

```sh
"$agenturm" backup --db "$state/history.sqlite" --output "$state/history-backup.sqlite"
```

Use a workspace backup to include saved tasks, handoffs, request seen state,
and review acknowledgements and notes. The destination directory must be new.

```sh
"$agenturm" backup --db "$state/history.sqlite" --bundle "$state/workspace-backup"
"$agenturm" restore --bundle "$state/workspace-backup" --output "$state/restored"
"$agenturm" cockpit --db "$state/restored/history.sqlite"
```

Private-content approvals require renewal after restore. Connector delivery
state lives in its explicitly configured ledger. See the [trusted connector
runtime](rust-connector-api.md).

## Uninstall

Run `"$agenturm" service uninstall` to stop and remove the systemd unit.
Remove the extracted bundle directory afterward if desired. The configuration
and SQLite database stay in the state directory. The extracted or a compatible
Agenturm CLI can query or inspect the database after uninstall.

## Qualify a release candidate

Run the archive checks on each supported platform after building from the
release tag. The Linux CI workflow accepts Rust tags in the form
`rust-v<version>`. The version comes from `rust/agenturm-app/Cargo.toml`.
The workflow verifies the tag matches the version in the archive and retains
the qualified archive and checksum as workflow artifacts. The arm64 job runs
on native Ubuntu 24.04 arm64 and checks install, restart, upgrade from Rust
0.2.0, backup and uninstall through a systemd user service. On Rust tags,
a separate job attaches that exact arm64 archive and checksum only after
both architectures pass. Existing release assets are never overwritten. If
the release does not exist yet, that job creates a draft for the release owner.
A candidate workflow run alone does not advertise a published arm64 asset.
Python releases use
the separate `v<version>` tag family.

```sh
scripts/verify-rust-linux-bundle.sh "./dist/$archive"
scripts/verify-rust-installer.sh "./dist/$archive"
```

The verifier extracts the checksum-verified archive into a fresh state
directory. It discovers and collects fixture sessions with an empty PATH,
checks that reopening and replay preserve history, opens recorded turn
activity in a PTY, and reopens a backup. A narrow-PTY run starts the cockpit
with empty state, adds fixtures while it is open, refreshes the roster, and
verifies the next collector can reopen the history after quit. Qualification
uses Bash, jq, GNU coreutils, GNU tar, and util-linux `script`.

Both native jobs also run the offline installer verifier before uploading the
archive. It checks the companion executables and catalog pins, same-version
replay, and uninstall with both normal and version-local SQLite history
preserved. This uses the built archive and its sidecar on the native runner.

On a dedicated Ubuntu 24.04 x86-64 account with a running systemd user
manager and no Agenturm collector unit, pass `--systemd-user` to verify
managed install, restart, inspection, and retained data after uninstall.
Also repeat those steps with real local Pi and Codex data. When a prior
standalone Rust artifact is available, pass
`--previous /path/to/agenturm-<previous>-linux-x86_64.tar.gz`. The verifier
uses that artifact and its adjacent checksum to create a database, then
checks that the current bundle retains its history and resumes its cursors.

## Publish a qualified archive

After the tag run succeeds, download the archive and checksum from that
specific run. Set `run_id` to the successful run for the release tag.

```sh
tag=rust-vVERSION
asset="agenturm-${tag#rust-v}-linux-x86_64.tar.gz"
gh run download "$run_id" --name agenturm-rust-linux-x86_64 --dir ./release
scripts/verify-rust-linux-bundle.sh "./release/$asset" --systemd-user
```

Complete the real-record checks above using the same extracted archive.
Record the tag, OS and architecture, artifact checksum, and results. Attach
the tested files to the existing tag with release notes identifying the supported
platforms and linking to these instructions.

```sh
gh release create "$tag" "./release/$asset" "./release/$asset.sha256" \
    --verify-tag --latest=false --title "Agenturm Rust ${tag#rust-v}" \
    --notes-file "$release_notes"
```
