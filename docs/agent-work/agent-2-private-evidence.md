# Agent 2: Private evidence and provider detail

## Goal

Preserve supported private edit and MCP investigation journeys with exact
scope, proof, and grant checks. Extend provider coverage only where retained
source evidence supports it.

## Own

- Audit the remaining FD2 cases in
  [`rust-migration-remaining-plan.md`](../rfcs/rust-migration-remaining-plan.md#f2-close-the-remaining-required-python-028-workflow-differences)
  and the private-evidence parity rows in
  [`rust-ui-parity-plan.md`](../rfcs/rust-ui-parity-plan.md#ui-04-prompts-session-activity-and-cross-session-history).
- Identify provider-specific edits or MCP details that are actual retained
  Python workflows and have an exact Rust-side content proof and grant path.
- Implement only confirmed gaps, preserving source/session/worker/turn
  identity, snapshot binding, revocation, expiry, and clear unavailable states.
- Keep content out of ordinary telemetry, summaries, logs, and ungranted views.

## Do not own

- Native worker/check/turn attribution; Agent 1 owns provider fact collection.
- General cockpit shell, shared keyboard dispatch, or broad visual parity.
- New capture types, grants, or retention of private content without a
  documented source proof and privacy boundary.
- Broad paired UI or release qualification.

## Parallel boundaries

Prefer the existing content/query modules and their view components. Do not
edit shared workspace navigation unless the integration owner approves a small
interface first. Agent 1 may add typed source evidence; consume only facts that
are already present or explicitly agreed in that contract.

## Deliverable

Report the exact Python reference journey, source proof, grant boundary,
changed files, focused checks, and any cases left unavailable. Include a
negative case proving that missing, stale, or revoked authorization does not
reveal content.

## Done when

Each implemented detail is bound to the exact retained evidence and current
grant, or the UI explains why it is unavailable. Broad installed acceptance
remains with the independent test owner.

## Audit result — 2026-11-26

### Python reference journey and source proof

- Python's general private Activity preview is intentionally only wired to Pi
  user messages: `observatory/ui_plugin_content.py` registers
  `PI_USER_TEXT` at `/api/plugin-content`, and `observatory/plugin_pi_content_read.py`
  resolves one selected activity row back to its exact retained `pi.user_text`
  reference. The reader rechecks receiver/source identity, policy and lease
  epochs, source dirty sequence, member/session projection, evidence revision,
  file identity and row hash around source-file I/O. Python exposes no
  corresponding provider for private edit or MCP payload details.
- Rust's native edit journey is the existing Codex Review edit list → select
  one occurrence → explicit detail reveal. `agenturm-content` binds the read to
  the configured, pinned Codex source plus exact session, turn, operation,
  event and revision. Only `CodexEditDetail` detail is eligible. The related
  Codex MCP journey selects one recorded call and uses the distinct
  `CodexMcpPayload` grant with exact session, operation, event and revision.
- Pi retained proof is only `pi.user_text`; the Pi emitter strips other source
  content. Claude, OpenCode and Kiro admission retain content-free observations
  for these routes. Their native files may contain private text, but that text
  is not an admitted retained reference or authorization to read it. No
  provider-specific edit or MCP payload gap has both retained proof and a
  supported grant path, so no new reader or grant is justified in this lane.

### Grant boundary and negative cases

Existing Rust operation previews require a separate per-source, per-kind
approval and recheck the current grant, pinned adapter/artifact, exact
observation and current source proof before delivery. Edit and MCP approvals
are independent; revocation invalidates the relevant approval. Missing detail,
changed source checkpoints, mismatched operation/event/turn selections,
stale/restored history snapshots and revoked grants fail closed. These grants
are revocable epoch grants; there is no grant TTL to claim as an expiry
boundary. The private-display controller separately clears transient text on
close/revoke/timeout.

### Files and focused checks

- Changed file: this assignment report only. No runtime files changed because
  the audit found no additional provider route with retained proof.
- `cargo test -p agenturm-content --locked` passed (2 unit tests).
- `cargo test -p agenturm-app --test prompt_content codex_edit_and_command_details_require_exact_occurrences_and_independent_approvals --locked`
  passed (1 test). This existing integration case checks independent grants,
  exact-turn rejection and unavailable oversized detail.
- The adjacent `prompt_content.rs` cases include missing/revoked Pi and MCP
  approvals, changed-source/checkpoint rejection and stale/restored history
  rejection (`preview_requires_a_current_grant_and_exact_committed_pi_bytes`,
  `codex_preview_reveals_only_exact_completed_native_user_items`, and the
  Codex edit/command case). The broad integration suite was not run.
