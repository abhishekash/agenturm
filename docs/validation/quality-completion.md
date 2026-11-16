# Agenturm 0.2.4 validation

The user changed the plan on 16 November: extended validation was too token consuming, and 100 runs are not required. This delivery uses bounded checks. No further manual soak or resource-comparison loop is required.

## Bounded checks

| Check | Result |
| --- | --- |
| Focused shutdown, queued-receipt, storage, concurrent-read and provenance tests | 31 passed |
| Real terminal navigation smoke | 1 warmup + 5 measured cycles across five sources passed on the earlier application |
| Fresh Python 3.13 installed wheel | Public CLI and Review help passed; all 86 runtime modules match final code `d6a8320` |
| Distribution file boundaries | Passed; no telemetry databases, auth files or private review notes included |
| Hosted CI | All 12 checks passed at earlier code head `46a89d0`; final delivery uses the existing automated CI/publishing workflow |
| Extended modal attempt | Stopped on user request after 81 measured cycles; receiver and cockpit exited normally |
| Queued cold, idle and HTTP comparisons | Cancelled before launch; no performance pass claimed |

## What changed

Conservative history-preparation checks reduce repeated safety scanning and operation parsing. Native producers can skip exact unchanged stored rows inside the existing capture-policy and source-generation transaction. Diagnostics distinguish healthy writes from retries. Source ownership, repair work, SDK acceptance counts and capture fences remain covered by regression tests.

The later shutdown fix stops new observations before Textual cancels workers. If an accepted receipt's scheduled worker never starts, an independently claimed shutdown drainer persists it without duplicate acknowledgement writes. A real Textual regression reproduces the old lost receipt, and a synchronized race test verifies one write. Private shutdown phase timings retain the existing deadlines.

Modal validation now checks the actual SearchInput widget and explicitly selects History so fixed sources remain available when Today expires. These are test-driver changes, not new product features.

## Retained evidence and limits

The earlier application `4beb84a` reconstructed 297/297 turns during 3,622.110 active seconds but missed its eight-second final cockpit shutdown deadline. Both cockpits eventually reported code 0. That attempt remains failed; the new queued-receipt fix is not claimed as the unique explanation for its timeout, and no new hour is required under the revised plan.

A two-cockpit/headless-watcher native notification fixture passed with one delivery, durable acknowledgement across restarts and exact resolution. An early-frame diagnostic observed metadata at 8.223 seconds, full readiness at 125.138 seconds and a coherent frame at 126.995 seconds. An earlier first-frame attempt failed; these observations do not certify a startup target or universal runtime behavior.

Earlier failed HTTP schedules, modal deadlines and CI fixture/provenance failures remain recorded. The user-stopped attempt is a cancelled extended protocol, not a new product regression. Performance targets, actual live-producer qualification and human reconstruction/repeat-use remain unproved follow-up. No synthetic participant or filtered latency result substitutes for them.

## Further depth

Investigate intermittent shutdown timing, improve retained-history startup and measure resource behavior when useful. Qualify current live producer records and real reconstruction/resumption workflows. These are follow-up opportunities, not instructions to restart extended validation now.

See the [bounded plan](../rfcs/final-quality-gates.html), [retained protocol history](../rfcs/quality-gate-followup.html), [focused concurrent-read and receipt proof](concurrent-read-queued-receipt-proof.html), [acceptance ledger](review-notes-acceptance.html), [machine evidence](quality-completion.json) and [release notes](../releases/0.2.4.html).
