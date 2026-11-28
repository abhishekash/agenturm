# Default local prompt history

In the Linux live local cockpit, open **Prompts** (or press **P** from the agent
list), select a request, then press **Enter** or **Right**. A previously
unconfigured source receives a durable user-text-only grant when Prompts opens.
It covers that source's sessions, not other sources. Session and worker filters
still limit the selected history. Native prompt reading remains Linux-only.

## What stays explicit

- A revoked grant stays off, even if the revocation preceded first access.
- A changed source binding requires explicit reapproval. Replacing the source
  file or changing the pinned adapter configuration does not inherit approval.
- Disabled grants in a backup stay disabled when reopened.
- Saved-history views, CLI content previews and remote readers do not create
  default grants. An already enabled grant still follows its existing rules.
- Tool inputs/results, commands, edits, telemetry and evaluations retain
  independent permissions. Enabling prompts does not approve them.

Where permission was explicitly withheld or invalidated, the UI still offers
**Enable prompt history** (**a**) for deliberate reapproval.

## Opt out for a source

Stop the collector using that history so the command can acquire its writer
lease, then run with the exact database path and source ID:

```sh
agenturm content revoke --db <history.sqlite> --source-id <source-id>
```

This revokes **all private-content kinds for that source**, not only prompts.
Restarting the monitor or opening Prompts will not undo it. Explicit reapproval
is required. This changes access, not retention: it does not erase history or
the native agent's logs. Revocations made by older versions before any grant
existed left no row, so they cannot be recovered; run the command again with
this version to record that opt-out.

## Privacy boundary

This change stores only authorization state, not another transcript copy.
Prompt text is read from the existing native source on request. It can contain
private project data or secrets; filtering recognizable credentials is
best-effort. Existing source/checkpoint proofs, exact session/worker scope,
snapshot and history identity, source-replacement checks and revocation-fenced
delivery still govern listing and disclosure. There is no new network transfer,
provider call or automatic export. Protect native logs, the local database and
backups as sensitive files. Retention is logical deletion, not secure erasure.

[Self-contained HTML](default-prompt-history.html) · [Privacy](privacy.md)
