# Agenturm Rust bundle for Linux

The bundle contains the Agenturm CLI and inspector, the process host, the
Codex, Pi, Claude Code, Kiro, and OpenCode adapters and their
SHA-256-pinned catalog, plus the trusted connector driver. The runtime
requires a compatible Linux system and does not require Python or Rust.
The managed collector uses a running systemd user manager. Foreground
collection is available on Linux systems without a user manager.

| Platform | Build | Collector |
| --- | --- | --- |
| Ubuntu 24.04 x86-64 | Local bundle and CI artifact | Foreground and systemd user service verified |
| Other Linux releases and architectures | Host build script available | Qualification open |
| macOS and Windows | Bundle qualification open | Service integration open |

## Build and install

From a source checkout on Linux with the pinned Rust toolchain and a native C
build toolchain, run `scripts/build-rust-linux-bundle.sh ./dist`. The script
builds for the local Linux architecture and emits a tarball and checksum.

From the directory containing the archive and its checksum, set the version
and use the archive name for your architecture.

```sh
archive="agenturm-VERSION-linux-x86_64.tar.gz"
sha256sum -c "$archive.sha256"
mkdir -p "$HOME/.local/opt"
tar -xzf "$archive" -C "$HOME/.local/opt"
agenturm="$HOME/.local/opt/${archive%.tar.gz}/bin/agenturm"
"$agenturm" --version
```

Use the archive and directory names produced by the build for your version and
architecture. Keep the catalog and binaries in the extracted directory.

## Open the cockpit

From a terminal, run the installed binary.

```sh
"$agenturm"
```

The cockpit opens on Agents. It uses
`${XDG_STATE_HOME:-$HOME/.local/state}/agenturm-rust/history.sqlite`.
It starts session-owned collection of supported local harnesses when the default database is
available for writing. When another writer owns the database, the cockpit
reads the same history. When the cockpit owns collection, it also receives
OTLP/HTTP traces at `http://127.0.0.1:14318/v1/traces`. Set `AGENTURM_OTLP_PORT`
to use another loopback port. Both stop on quit.

Use `↑` and `↓` to choose a session, `/` to search the loaded roster,
and Enter to open its Overview. The newest roster page refreshes while open.
Press `r` in Agents to refresh it immediately. Open Activity with `a` or
Review with `v`, and open the selected session’s Usage with `u`. Press
`y` in Review to acknowledge the displayed evidence. The acknowledgement
is stored next to the history database and applies to the reviewed version.

From Agents, `g` opens the cross-source Overview. Use `2` for Activity,
`3` for MCP, `4` for Operations, `5` for Findings, `6` for Host, and
`7` for Usage or `8` for Traces. Press `t` in a history view to select a
recorded turn, `F1` for keyboard help, and `q` to quit. The available
evidence varies by source. For Codex MCP calls the view includes the server,
tool, method, duration and outcome when recorded.

From Agents or a session or turn detail, press `b` to open saved tasks.
Use `n` to create a task linked to the selection, `r` to see its linked
runs, and `h` to create a handoff using the current selection. Open the
handoff list with `o` and export the selected handoff with `x`. Tasks can
also record a Git snapshot from a directory you choose with `G`.

From Agents, press `w` to see recorded requests. Use `s` to mark an item
seen and `l` to focus the matching tmux pane when Agenturm verifies the
route. Respond in the harness terminal.

In Review, press `n` for saved decision notes. Press `e` to write a note
against the selected evidence version and Ctrl+S to save it.

Select a session and press `p` for eligible user prompts. Press Enter to
reveal one and Escape to clear it. The view requires a current grant for a
pinned source and uses `sources.json` next to the history database or the
path set in `AGENTURM_CONTENT_CONFIG`. It verifies retained JSONL rows by
their committed receipt and SQLite rows against the current source sweep.
The default cockpit uses its collector's writer lease for reveal. Establish
or revoke grants from the CLI while the history writer is available.

Use `"$agenturm" cockpit --db <path>` or `"$agenturm" inspect --db <path>`
to open a specific existing Rust store without starting local collection.
For collection between cockpit sessions, set up the managed collector below.

## Open native traces

Point an OTLP/HTTP producer at the loopback receiver. Each span needs a
`session.id` or `gen_ai.conversation.id` string attribute. The receiver
accepts uncompressed JSON or protobuf at
`http://127.0.0.1:4318/v1/traces`. To attach the span to a native session,
also set `agenturm.source.id` to the exact collected harness source ID.
Press `8` from a native session to see its linked spans.

```sh
"$agenturm" serve-traces --listen 127.0.0.1:4318 --db traces.sqlite --source-id otlp.local
```

You can also load an export directly.

```sh
"$agenturm" import-traces --file export.json --db traces.sqlite --source-id otlp.local
```

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

## Use the workspace from scripts

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
the qualified archive and checksum as workflow artifacts. Python releases use
the separate `v<version>` tag family.

```sh
scripts/verify-rust-linux-bundle.sh "./dist/$archive"
```

The verifier extracts the checksum-verified archive into a fresh state
directory. It discovers and collects fixture sessions with an empty PATH,
checks that reopening and replay preserve history, opens recorded turn
activity in a PTY, and reopens a backup. A narrow-PTY run starts the cockpit
with empty state, adds fixtures while it is open, refreshes the roster, and
verifies the next collector can reopen the history after quit. Qualification
uses Bash, jq, GNU coreutils, GNU tar, and util-linux `script`.

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
