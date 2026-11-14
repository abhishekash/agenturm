# Deterministic source review fixtures

## What failed

PR96 failed Python 3.12 and 3.14 on an exact revocation-status phrase. Main 96 failed Python 3.13 on the same assertion. The policy timer can legitimately replace the approval failure with a source-withholding notice. The test still requires the editor to remain open and the task count to remain zero; it now accepts either revocation or withholding language.

## HTTP readiness evidence

PR96 Python 3.13 separately returned the CLI’s generic receiver/filesystem error. That CI run did not record HTTP statuses, so its exact cause remains unknown. A controlled scheduling check held only the actual migration thread: after ingest and manual summary refresh, health returned 200 while Sessions returned 503, producing the same CLI error. Releasing migration and observing true readiness made the unchanged command succeed.

## Fixture changes

The authenticated HTTP journey now waits at most five seconds for actual asynchronous index readiness, failing immediately on a reported migration or worker error. It does not repair or retry the CLI command. A test-only observer records actual response statuses and API paths without query strings, tokens, or bodies, so future failures are diagnosable. All worker-scope, request, check chronology, unavailable-usage, stdout/file parity, three-format export, and authentication assertions remain unchanged.

## Validation and limits

The two affected checks passed with ResourceWarning treated as an error on Python 3.12.3 (5.501 seconds) and Python 3.13.5 (5.275 seconds). No product behavior, timeout, source policy, immediate revocation assertion, or performance protocol changed. The failed CI runs remain failed; corrected integrated CI is a separate gate.

```sh
python -W error::ResourceWarning -m unittest tests.test_review_continuity_ui.ReviewContinuityJourney.test_source_revocation_blocks_draft_approval_without_task_mutation tests.test_work_receipts.HeadlessReceiverJourney
```

[PR96 failed run](https://github.com/abhishekash/agenturm/actions/runs/43606287537) · [main 96 failed run](https://github.com/abhishekash/agenturm/actions/runs/43606443606)

[Standalone HTML](ci96-fixture-readiness.html) · [Controlled HTTP evidence](ci96-fixture-readiness.json)
