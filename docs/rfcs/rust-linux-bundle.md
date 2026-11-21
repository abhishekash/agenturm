# Agenturm Rust bundle for Linux

The bundle contains the Agenturm CLI and inspector, the process host, the
Codex and Pi adapters, and their SHA-256-pinned adapter catalog. The runtime
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

From the directory containing the archive and its checksum, run:

```sh
sha256sum -c agenturm-0.1.0-linux-x86_64.tar.gz.sha256
mkdir -p "$HOME/.local/opt"
tar -xzf agenturm-0.1.0-linux-x86_64.tar.gz -C "$HOME/.local/opt"
agenturm="$HOME/.local/opt/agenturm-0.1.0-linux-x86_64/bin/agenturm"
"$agenturm" --version
```

Use the archive and directory names produced by the build for your version and
architecture. Keep the catalog and binaries in the extracted directory.

## Discover, collect, and inspect

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

In the inspector, press `t` when the header offers Turns. The view
lists recorded turns on the current history page. Use the arrow keys to
choose a turn, then press Enter to open its scoped observations. Press
`Esc` to return to the turn list, then press it again to return to history.
A recorded pull request appears as a work result alongside the native
lifecycle state.

## Back up history

Run `"$agenturm" backup --db "$state/history.sqlite" --output
"$state/history-backup.sqlite"` while collection is running or stopped.
The output path must be new. Query or inspect the backup by passing its
path with `--db`. To resume collection from a snapshot, stop the current
collector and configure the backup file as the new database.

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
scripts/verify-rust-linux-bundle.sh ./dist/agenturm-0.1.0-linux-x86_64.tar.gz
```

The verifier extracts the checksum-verified archive into a fresh state
directory. It discovers and collects fixture sessions with an empty PATH,
checks that reopening and replay preserve history, opens recorded turn
activity in a PTY, and reopens a backup. It uses Bash, jq, GNU coreutils,
GNU tar, and util-linux `script` as qualification tools.

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
tag=rust-v0.1.0
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
