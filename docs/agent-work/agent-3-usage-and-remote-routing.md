# Agent 3: Usage totals and remote workflow gaps

## Goal

Finish only the concrete remaining usage and cross-host workflow gaps. The
combined usage slice and initial remote terminal route already exist; do not
rebuild them or add speculative aggregation behavior.

## Own

- Review FD3 and FD4 in
  [`rust-migration-remaining-plan.md`](../rfcs/rust-migration-remaining-plan.md#remaining-feature-development--current-scheduling).
- For usage, confirm the implemented exact-identity overlap exclusions,
  currency separation, unattributed telemetry disclosure, and old attached
  receiver compatibility. Change code only for a reproducible defect or a
  required Python parity gap.
- For routing, assess the listed gaps: per-host route persistence, argument
  transport semantics, and fail-closed behavior for stale/ambiguous targets.
  Implement a narrow improvement only when it is required by an agreed
  workflow and can be validated without weakening target identity checks.
- Keep route-file and transport configuration explicit; never have a route
  select, answer, or approve a remote request on the operator's behalf.

## Do not own

- Private evidence/grant flows (Agent 2), provider-native decoding (Agent 1),
  or macOS packaging (Agent 4).
- New usage metrics or inferred cross-source identity. If overlap cannot be
  established from recorded identity, disclose the limitation; do not guess.
- Final installed/paired acceptance. CF6/CF7 broad checks remain with the
  independent test owner.

## Parallel boundaries

Keep usage changes in usage aggregation/query/rendering modules and routing
changes in the route configuration/remote workflow modules. Avoid shared
workspace dispatch and shared evidence DTOs. If neither area has a confirmed
code gap, deliver a source-backed audit instead of inventing implementation.

## Deliverable

Report the baseline SHA, audited user journey, confirmed gap (if any), changed
files, focused checks, and exact unresolved acceptance items. Clearly separate
implemented behavior from test-only acceptance still pending.

## Done when

Any changed behavior has focused regression coverage and preserves the stated
identity/privacy boundaries. Otherwise, the audit establishes that the current
implementation meets the scoped requirement and identifies only delegated
acceptance work.
