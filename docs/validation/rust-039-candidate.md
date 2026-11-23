# Rust 0.3.9 candidate qualification

23 November 2026. Native source `42712dd5bf14f3d80fb9f87d7f648b89d9532d5e`
is qualified as an opt-in candidate in PR #379. This record does not publish a
release or switch the default entry point. The explicitly attached Python
receiver Findings journey is a separate integration still in progress.

## Qualified artifacts

| Artifact | SHA-256 |
| --- | --- |
| Optimized Linux x86-64 archive `agenturm-0.3.9-linux-x86_64.tar.gz` | `0c834ebc70bee037268f9935901ec816a20c8ff56cab2abac4cd264d03b87ed5` |
| App inside that archive | `bfd59348d9bceefb682d9cd12b080ace82eb61ce43ba6651833181a7bf5b7ed5` |
| Frozen nine-binary development app | `dc3624f59efb235be8460a558b33f365be4d4a64a33bbe62af743e9d63ce46d7` |

The local archive and checksum are in `/tmp/agenturm-039-final-native-release`.
The extracted archive is in `/tmp/agenturm-039-final-native-extracted`.
The development bundle is `/tmp/agenturm-qualified-039-dev`. Its pinned catalog
uses the hashes of the five bundled harness executables. All nine development
executable hashes matched the completed joined build.

## Results

- Offline locked workspace: **813 tests across 96 suites**, zero failed or
  ignored; strict whole-workspace all-target Clippy and all-binary build passed.
- The exact optimized archive passed the offline installer, retained-history
  uninstall, Python-free startup, discovery, collection/restart, inspection,
  backup, bare cockpit, follow discovery, and prior-0.3.8 history/cursor upgrade.
- All **12 separate-UID Linux KVM cases** passed on the exact extracted archive:
  native private evidence, telemetry payloads, private evaluation details,
  Host history, remote history, remote Host, evaluation inventory, broker
  span/usage, native attention, execution clocks, Finding span payload, and
  captured error-log text. Boot verified all nine executable hashes.
- The development bundle passed real PTY checks at **120×32, 80×24 and 60×18**
  for MCP inventory/trace return, metric points/exemplars/usage, protected log
  Finding source and current failed evaluation source. Log checks include
  source-only records, capture off, separate span/log grants, exact body,
  no sibling/clipboard/export, revoked selection denial and redacted return.
  A passing evaluation correction denied the stale selected body.
- Actual cached OpenTelemetry Logs SDK exported an ordinary error log without
  a producer ID. JSON/protobuf metadata, exact replay, authenticated opt-in
  text and divergent text replay denial passed against the frozen app.
- The joined Workflow implementation passed settings persistence, exact
  token/USD/minute values, desktop state, snooze/end snooze, typed incidents,
  Failed/Unavailable retry, native resume and terminal restoration at all three
  widths. Two actual cockpits proved an old failed-attempt receipt cannot
  launch a new notification after another attempt has failed.
- The existing-receiver launcher passed ten process tests and a cached actual
  OpenAI SDK witness: four exact spans, an explicit launch session, preserved
  exit 23, no launcher database, and the existing receiver left running.
  The read-only Workflow report passed seven process tests, including no
  notifier, no new sidecars and unchanged history/state.
- Tagged OpenCode whole-turn task draft, explicit approval and protected reveal
  passed using Python 0.2.8 and Rust Review receipts. Ordinary-only substitution,
  changed second context reads and old step proof regressions were checked.
- Rust, all five selected SDK jobs, and native Linux x86-64/ARM64 bundle jobs
  passed on source `42712dd` in CI runs `45203734924` and `45203734795`.
  The broader Python version matrix was still running when this record was
  written. ARM64 has native CI qualification; the separate-UID KVM proof here
  is x86-64.

The terminal check found a durable source-only log reload rejection. The exact
log kind was added to the kernel source-scope allowance, while absent identity
fields and kind-specific validation remain enforced. A regression reproduces
the rejection before the fix and verifies invalid collector/native-link/scope
denials afterward. A hand-built development catalog path and repeatability
helper assumptions were also corrected; runtime disclosure guards were retained.

## Repeatable checks and logs

- [Ordinary error logs and optional actual SDK](../../scripts/validate_rust_error_logs.py)
- [Protected log Finding source and terminal restoration](../../scripts/validate_rust_log_finding_source.py)
- [Current failed evaluation Finding source](../../scripts/validate_rust_finding_eval_source.py)
- [Separate-UID log capture/disclosure/restore](../../scripts/validate_rust_log_text_broker.py)
- [Settings, snooze and typed incidents](../../scripts/validate_rust_workflow_settings.py)
- [Concurrent retry attempt compare-and-swap](../../scripts/validate_rust_workflow_retry_cas.py)
- [Authenticated execution clocks and restore fencing](../../scripts/validate_rust_execution_clocks.py)
- [MCP inventory and exact traces](../../scripts/validate_rust_mcp_inventory.py)
- [OTLP metrics, usage and exemplars](../../scripts/validate_rust_native_analytics.py)

Joined tests are recorded in
`/tmp/agenturm-error-log-source-scope-validation.log`. Exact package results are
in `/tmp/agenturm-final-native-all-guests.log`,
`/tmp/agenturm-final-native-bundle-verify.log` and
`/tmp/agenturm-final-native-installer-verify.log`. Frozen development terminal
and SDK evidence is in `/tmp/agenturm-039-qualified-*.log`.

## Remaining gates

The Python receiver Findings tab/source integration is being verified against
the actual tagged receiver, including current scope, capture policy, member
changes, bounded private display and redaction. It is not certified by the
native results above. Current remote Host inventory, retained metric description
fields and other documented paired-journey differences remain explicit in the
[screen comparison](rust-ui-journey-parity.md); no workflow was retired.

A real Kiro transcript and an approved production connector endpoint remain
external inputs. macOS requires its D2-Mac native platform acceptance and is
unsupported. Default cutover remains gated on required workflow acceptance.
The next public batch is eligible no earlier than 24 November 2026 at
06:03:24 UTC, subject to rechecking the latest publication. No public 0.3.9
release has been created.
