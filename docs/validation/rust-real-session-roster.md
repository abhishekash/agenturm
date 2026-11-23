# Rust cockpit acceptance with local native sessions

The reported Rust 0.3.8 cockpit showed three rows. The running executable identified on the host was a bare launch of the extracted 0.3.8 bundle with its own `XDG_STATE_HOME`. Its schema 17 history held 21 source and session pairs after polling. The launch state was separate from the older default database. The temporary launch directory closed before a current-cursor comparison could be made.

## Released baseline

Replaying discovery with the exact 0.3.8 binary and isolated state selected four Codex transcripts. In one sample three of the four were ordinary subagents of one native session. The same default cap selected three old main Claude transcripts and one sidechain. An explicitly configured collector admitted eight source and session pairs, including two distinct Claude sources with the same native session ID. The default managed history admitted four Codex sessions in the same recurring sample. The visible Claude recordings were older than the 14-day managed retention window. Subagent classification came from native header metadata. File times and retained rows did not establish which agent processes were still open. Two actual Codex files contained only a native session header. Their IDs were preserved in the recorded inventory, and neither produced a durable session row during acceptance. The retained roster currently needs at least one admitted session observation. Displaying an open, header-only session remains a gap.

The actual Claude historical control carries a native `custom-title` and a current recorded `cwd` within its bounded transcript. Its file had 259,858 bytes and its matching title records ended at offsets 71,140 and 259,629. None of the 17 available Claude project transcripts exceeded 8 MiB. The historical fixture establishes real native admission and can check the title and project projection. It does not establish contemporaneous live Claude collection or qualify a prefix beyond 8 MiB.

## Run the checks

Use an installed bundle with its sibling plugin host and adapter catalog. Pass alternate native homes only when needed. The processes under test use disposable state and an isolated configuration. The checks never edit live native logs or the running cockpit database and do not call a model provider.

```sh
python3 scripts/validate_rust_real_session_roster.py --app /path/to/bundle/bin/agenturm audit
python3 scripts/validate_rust_real_session_roster.py --app /path/to/bundle/bin/agenturm verify --expected /path/to/private-codex-manifest.json
python3 scripts/validate_rust_real_session_roster.py --app /path/to/bundle/bin/agenturm historical --expected /path/to/private-claude-manifest.json
python3 scripts/validate_rust_real_session_roster.py --app /path/to/bundle/bin/agenturm mutation
```

For `verify`, the manifest has a `sessions` array of five to twenty expected source and native session pairs and an `excluded` array with at least one exact guardian review. A session record needs `harness`, `path` relative to the provider’s `sessions` or `projects` directory, its native `session` UUID, `title`, and absolute `project`. Supply `kind` as `subagent` for at least one ordinary worker. An optional `agent` checks the supporting name on the roster. An optional `status` checks an independently established display state. Guardian controls have `harness`, `path`, `session`, and `kind` set to `review`. Obtain titles from native display metadata rather than from unread or captured messages. Keep the manifest private and outside the checkout.

The recorded-native check uses the real default cockpit and a fresh managed history, then verifies retained exact sources across reopen at 120×32, 80×24 and 60×18. It compares full title and project fields from a temporary loaded-view export and opens sessions by human name or context. The inspector must identify the exact source and session again after returning with `Esc`. Regular named subagents remain eligible. Guardian reviews must stay out of new admission and the roster.

Add `--require-live` only with independently observed `pid` values for at least five distinct source and session pairs. The checker requires exact open device and inode matches. A single native process may hold several transcripts. The observed host sample had 12 non-guardian transcripts held by two Codex processes. That sample establishes open transcript ownership without deciding whether each session was Working or Inactive. Use an independently established `status` to check the roster state. Without `--require-live`, the result covers recorded native roots.

`historical` accepts one ordinary Claude record with `harness`, `path`, and `session`. Include `title` and `project` to check the full presentation at all three widths, and optionally `status`. It uses a separately configured store so managed retention does not retroactively admit stale sessions into the live cockpit. The full display check advances the committed Claude checkpoint to the complete unchanged source before comparing the title and project. The observed case reached 259,858 bytes in 15 collection passes. `mutation` creates disposable metadata records and checks an exact native title change, distinct sources with the same native ID, state catch-up, and restart. Its results do not count as real concurrent sessions.

To examine an identified running store, use `inspect-existing --db /path/to/history.sqlite`. It reports source and session IDs, counts and verified native classifications. It leaves private message bodies, native title text, credentials and full terminal frames outside its output.


## Qualified 0.3.9 candidate — 23 November 2026

PR [#377](https://github.com/abhishekash/agenturm/pull/377) merged as
`ed03815fdfe0a6bfea88ee07ce69e9ff227c2b89`. The frozen runtime source is
`b2c1328f8f9f0daa9d3dfdf1376675e05532189e`; `13e3bb6` changes only the
terminal qualification scripts. The merge has the same source tree as that
qualified head. No 0.3.9 tag or release was published.

The exact local x86-64 candidate archive SHA-256 is
`b17a10d382d3e2604fb528db05992e7dbf6a56b6738f99f5e2cd55c76c3f6a45`.
Its `bin/agenturm` SHA-256 is
`508482fdd3ea02272fd2800cec97c7f8401b083e8d3c021c859fbda62e065ca2`.
These identify this candidate, not a future daily batch that includes further
changes. Its bundled README predates the publication hold; the current guide
above documents the published/candidate distinction and local-archive install.

| Check | Evidence | Result |
| --- | --- | --- |
| Workspace | 652 tests across 93 suites, including the host socket case | Passed; zero failed or ignored |
| Formatting and lint | Full workspace, strict all-target Clippy | Passed |
| Actual open Codex cohort | 12 non-guardian transcript handles across two recognized native processes; exact titles, projects, Enter/Esc and restart at 120×32, 80×24, 60×18 | All 36 session/size checks passed |
| Actual recorded Codex | Nine retained native sessions, catch-up, restart and the same three sizes | All 27 session/size checks passed |
| Actual historical Claude | Scoped native custom title and cwd after complete 259,858-byte checkpoint, 15 passes | All three size checks passed |
| Actual Claude snapshot prelude | Complete 8,815-byte transcript, exact row-two identity and workspace; no explicit name | All three size checks passed; native bytes and file identity unchanged |
| Disposable native metadata | Rename without identity change, distinct path-bound sources with the same native ID, independent lifecycle catch-up, narrow/reopened UI | Passed |
| Discovery fixture | Seven older synthetic open handles, one late arrival, ten backlog sessions, guardian exclusion and saved-cursor restart | Passed against exact package |
| Installed workflow | Foreground collection, exact two-session/completed-turn inspector, backup, follow, prior 0.3.8 reopen and cursors | Passed |
| Installer | Offline install, update/replay and uninstall with retained history/configuration | Passed |
| Themes | Five palettes, preview/save/cancel/restart, MCP/inspectors and compact terminal layouts | Passed against exact package |
| Multi-user KVM | Native private content, telemetry payloads, eval details, host history, remote history, remote host and eval inventory | All seven passed in the network-isolated guest |

Native candidate CI passed both Linux architectures at `13e3bb6` in
[run 45150637065](https://github.com/abhishekash/agenturm/actions/runs/45150637065),
including ARM64 upgrade and user-service checks. All five SDK jobs passed on
runtime-identical `b2c1328` in
[run 45149465255](https://github.com/abhishekash/agenturm/actions/runs/45149465255).
The current core/SDK rerun subsequently passed at `13e3bb6` in
[run 45150636838](https://github.com/abhishekash/agenturm/actions/runs/45150636838).
The legacy Python matrix was still running when this record was updated.

The old native qualification failure was an assertion over concatenated
incremental terminal writes. It reproduced on this archive; reconstructing
current terminal cells preserved and passed the exact session, source, turn,
completed lifecycle, Escape and terminal-restoration assertions.

### Unreleased visual follow-up

Visual review of captured terminal cells found that task titles inherited a
heading background, overriding the table's row background. Commit `88c0006`
uses foreground-only bold task names and muted supporting context. A rendered
buffer regression checks title, status and context backgrounds at 120 and 60
columns. The updated source passed 653 tests across 93 suites, all 45 UI tests,
strict all-target workspace Clippy and formatting. The earlier exact archive
above predates this display-only follow-up; the next release batch must be built
from and qualified against its selected final source.

The exact archive also passed 30 `NO_COLOR` terminal captures across the five
themes and compact layouts without RGB escape sequences. Theme selection,
Escape, keyboard navigation and state text remained available.

### Remaining acceptance and release limits

Contemporaneous live Claude concurrency was unavailable in the inspected host;
historical native Claude coverage is not a replacement for that acceptance.
Header-only recordings without session facts remain outside retained roster
rows. Names absent from explicit metadata stay unavailable. Discovery beyond
its file/process scan budgets is partial. Mutable Claude names are bounded to
16 MiB per source and 32 MiB per roster read; a snapshot-prefixed opening requires
full committed-prefix verification before any row-two workspace is shown.

The public release cadence is at most one publication per rolling 24 hours.
The previous publication was 23 November 2026 at 06:03:24 UTC, so the next eligible
batch begins 24 November at 06:03:24 UTC (07:03:24 Europe/Berlin), provided there
has been no later publication. Implementation and qualification continue.
The remaining Python view-parity, production endpoint and default-cutover gates
in the execution plan remain open.
