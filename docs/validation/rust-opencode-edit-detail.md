# OpenCode patch-path evidence and private reveal

24 November 2026. FD2 continuation from the interrupted implementation;
development branch `feature/opencode-edit-detail`, based on `57b7580` (PR #393).
This is a focused development handoff, not release or installed qualification.

## Native evidence and limits

A read-only SQLite snapshot of the local OpenCode database contained 141
`part` rows of type `patch`. All 141 joined the current `session_message`
table by both message and session ID. The observed patch keys were `type`,
`files` (a string list) and `hash` (40 lowercase hex characters). The earlier
inspection found no nonempty session-diff files. No personal paths, prompts,
patch bodies or native IDs are included here; regression fixtures are synthetic.

The producer hash is treated as opaque. Its presence does **not** prove the
path list, re-derive a diff, or attest the worktree. Session-level aggregate
columns are not exact operation evidence and are not used as a fallback.
The current source must also have the supported V2 session/message/part
layout. A missing optional `part` table simply supplies no patch evidence.

## Implemented contract

- `FileChange { files, digest }` retains only a bounded distinct-file count
  and SHA-256 of the canonical native patch JSON. It contains no path text,
  diff, inferred add/delete counts or command content. The digest is a content
  commitment, not anonymization or an execution attestation.
- The adapter reads at most four patch summaries per assistant message,
  with an extra row to detect overflow. Oversized/malformed records are withheld;
  bounded overflow diagnostics contain no source content. A patch must have
  1–256 distinct nonempty paths and a recognized native hash shape.
- A patch is attached only to its exact message/session and a provable
  preceding idle boundary. No first-turn boundary or worker identity is guessed.
  Its event and operation IDs derive from the database epoch and native IDs;
  time comes from the native part's update timestamp.
- Native polling uses `oc5`; upgrading an unchanged `oc4` cursor forces a
  sweep. Old cursors remain readable for prompt proofs. Legacy Python admission
  continues using `oc4` and does not receive the new fact.
- Schema 24 adds the independent `opencode.edit_detail` approval. The v23
  upgrade preserves prior grants. A prompt, Claude or Codex approval cannot
  authorize OpenCode edit detail. The binding includes source/adapter pins,
  configured path and file identity under `agenturm.opencode-edit-detail/1`.
- Reveal selects the exact retained event, revision, source, session, turn
  and operation under the existing writer/grant fence. It opens SQLite read-only,
  checks one unique native part, timestamp, current turn membership, file count
  and canonical record digest. A same-count path rewrite with unchanged native
  hash and timestamp is denied. A later admitted correction requires a fresh
  selection. Missing, ambiguous, replaced, over-limit or revoked evidence denies.
- Source reads retain the existing database/WAL ceilings and sidecar preflight.
  Patch JSON is capped at 64 KiB before transfer/decode; reveal scans at most
  8,192 session part rows, failing closed above that ceiling. The source stamp,
  including WAL metadata, is rechecked before delivery.
- Revealed paths are at most 512 bytes each, truncated on UTF-8 boundaries,
  credential/control screened and bounded below the shared 8 KiB output limit,
  accounting for JSON escaping. `reported_files` preserves the original count;
  `truncated` identifies partial output. The terminal explicitly says diffs
  and add/delete counts are unavailable.
- Review/turn pickers include eligible OpenCode and Claude edits while keeping
  worker and occurrence fences. Shared `VerifiedEdit` carries an optional
  provider-shaped detail instead of coercing path-only patches or Claude tool
  inputs into Codex diffs. Local and broker consumers use that same proof type.
  Path investigation refuses exact matches from a truncated OpenCode list.

## Operator commands

Use an explicitly configured, SHA-256-pinned OpenCode adapter and native SQLite
source. Substitute real values; no command below approves other content kinds.

```sh
agenturm content grant --db HISTORY --config SOURCES \
  --source-id SOURCE --kind opencode.edit_detail

agenturm content preview --db HISTORY --config SOURCES \
  --source-id SOURCE --kind opencode.edit_detail \
  --session-id SESSION --turn-id TURN --operation-id OPERATION \
  --event-id EVENT --revision REVISION

agenturm content revoke --db HISTORY --source-id SOURCE \
  --kind opencode.edit_detail
```

Revocation prevents subsequent disclosure; it does not erase native records or
previously viewed content. Collection remains content-free for these facts.
No install command, default distribution, public release or support platform
changes as part of this slice.

## Validation and CF10 acceptance

Developer checks completed 25 November 2026, with no failed or ignored tests:

| Developer check | Result |
| --- | --- |
| App, Content, OpenCode adapter, Store and UI library suites | 171 + 2 + 7 + 148 + 76 = **404 passed** |
| App `prompt_content` process suite with all five adapter paths configured | **8 passed**, including existing provider regressions and the new OpenCode path-proof journey |
| OpenCode `configured_checks` process suite with built app and host | **2 passed**, including granted prompts under `oc5` |
| Locked workspace all-target Clippy with warnings denied | Passed |
| Workspace formatting and `git diff --check` | Passed |
| Built App, plugin host and five native adapters | Passed |

These are 414 distinct tests in the selected suites, not a full workspace,
installed-artifact or release qualification run. The regressions cover
native shape recognition, legacy isolation and cursor upgrade, exact commitment
and revision correction, grant isolation, schema migration, occurrence selection,
bounded path display, unsafe-text denial and local investigation source proof.
CI supplies the OpenCode adapter path so the process test actually executes.

| Separate acceptance scenario | Required result | Status |
| --- | --- | --- |
| Installed archive and actual native patch | Exact source/turn/operation path-only reveal; query/export metadata contains no paths | Not run |
| Review/turn picker at 120×32, 80×24 and 60×18 | Eligible occurrence selected; bounded modal and return preserve selection | Not run |
| Broker private delivery | Same provider-shaped proof; current authorization, receipt/snapshot and revocation fences | Not run |
| Live WAL updates and path replacement during delivery | Mixed snapshots, stale source and replacement fail closed | Not run |
| Upgrade, retention and restore on installed history | v23 grants preserved by upgrade; pruned selections and restored stale approvals deny | Not run |
| Missing parts, ambiguous boundaries and over-budget sessions | Visible unavailable/partial outcomes, never neighboring content | Not run |

CF10 remains a separate test-owner gate in the
[core feature test plan](rust-core-feature-test-plan.md). The whole Rust
migration is not complete. Pi/Kiro edit detail, other OpenCode shapes and
non-Codex MCP payload detail remain open.

## Cross-project consistency review

| Destination | Disposition |
| --- | --- |
| App implementation and migration/test plans | Updated together with this handoff and HTML companions |
| Default-install README | In sync for this change: Python/PyPI install remains unchanged; this is opt-in Rust development, not a public release |
| Marketing site | `agenturm-dev/src/pages/index.astro` still advertises Python/PyPI installs, read-only OpenCode input and sensitive local records. No default-install or privacy claim needs changing for this opt-in slice |
| GitHub profile | `github-profile/README.md` retains the same local-monitoring description, `uvx agenturm` and privacy link; no Rust patch-detail claim is advertised |
| Visual identity | No assets changed. Website dark/light tokens and profile generator palette inspected; full logo/artwork parity and rendered visual comparison not checked in this slice |
