# Concurrent read and queued receipt regression proof

Generated fixtures only. Baseline: `46a89d070353a7af568a75bcda584a68edb35d7a`. This is a focused regression proof, not a resource benchmark or an hour-soak acceptance certificate.

## Concurrent native reads

`tests/test_native_concurrent_read_certificate.py` replays seven native traces with one changed trace while the derived worker is barred. An explicit writer barrier checks all four authenticated HTTP paths: global Activity, trace-scoped Activity, Metrics, and Sessions. Entire old payloads remain visible before commit; entire new payloads match Store results after commit. The test also checks cache contents and commit invalidation, receiver response headers, Activity receiver identity, unauthorized requests, and exact no-op replay counts.

A separate 65-record control exceeds the existing atomic publication limit. Current global Activity remains available because its source index is already committed. Scoped Activity, Metrics, and Sessions correctly return 503 while summary dependencies remain dirty, then return 200 after worker repair. No stale summary is accepted as current. Limits, catch-up budgets, and readiness certificates remain unchanged.

Result: both tests pass on the baseline and on the shutdown candidate. This does not reproduce an unresolved current Activity 503 defect. The older Metrics 503 hour failure predates the exact-native-replay change; the current retained hour had no 503 and failed only its shutdown deadline. These observations do not prove universal availability.

## Cancelled queued workflow receipt

`tests/test_queued_receipt_shutdown.py` exercises a real minimal Textual app. A single occupied executor slot prevents an accepted workflow worker from starting. The app exits, cancelling that queued worker. A generated durable WorkflowStore is explicitly opened before accepting the displayed-error receipt; headless test defaults therefore cannot turn this into a memory-only receipt test.

Baseline result: the regression fails in 3.229 seconds. Shutdown prints the existing receipt-flush timeout and the accepted error receipt is absent from durable storage. The controller still reserves a worker which will never run.

Candidate result: both shutdown tests pass in 0.415 seconds. The accepted error receipt is persisted and the queue drains without a warning. A second deterministic barrier starts the original worker while the rescue drainer is writing a reversible review acknowledgement: exactly one acknowledgement write occurs and the persisted review remains enabled.

The candidate uses an independent shutdown drainer with an atomic claim. It preserves the existing three-second flush deadline and accepted receipt types. Independent review found the claim prevents duplicate rescue/original writes. This concrete defect is not established as the cause of the retained hour's separate eight-second cockpit shutdown failure; that acceptance gate still requires its own successful run.

## Validation scope

Python 3.12.3, ResourceWarning promoted to error. Tests use temporary generated databases, an authenticated loopback server, and explicit synchronization barriers. No personal provider, telemetry, preference, or capture configuration is used. Previously failed evidence remains failed. Performance gates and human validation are outside this proof.
