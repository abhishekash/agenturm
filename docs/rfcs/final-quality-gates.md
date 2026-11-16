# Bounded completion plan

Updated 16 November at the user's request: the extended validation was too token consuming, and 100 modal runs are no longer required. This plan supersedes the earlier long-run protocol.

1. Keep the implemented application and reviewed harness fixes. All 12 hosted CI checks passed at code head `46a89d0`.
2. Use the completed focused checks: 31 focused tests, a six-cycle real terminal smoke (one warmup plus five measured cycles across all five sources), and a fresh Python 3.13 installed-wheel check. The public CLI works, all 86 runtime modules match the final delivery code, and distribution file boundaries pass.
3. Preserve existing evidence and state its limits. The long modal run was stopped on request after 81 measured cycles; both owned processes exited normally. Queued cold/idle/HTTP comparisons were cancelled. Neither is required for this bounded delivery, and no performance target pass is claimed.
4. Reconcile the documentation, merge the verified PR and prepare the 0.2.4 release. Use the existing automated publishing workflow; do not start further manual soak loops or repeat the local full suite.

The earlier hour reconstructed 297/297 turns but missed the eight-second final cockpit shutdown deadline. Short instrumented checks did not reproduce it. The early-frame diagnostic observed native metadata at 8.223 seconds and coherent history at 126.995 seconds, while an earlier first-frame attempt failed. These observations remain qualified. Current live-producer and human reconstruction/repeat-use evidence remains unproved.

See the [quality report](../validation/quality-completion.html) and [retained protocol history](quality-gate-followup.html). Historical registrations are evidence of what was attempted, not requirements to keep running after the user changed the scope.
