# Rust host responsive writes and group cleanup

Recorded 20 November 2026. This fixes two process-lifecycle defects found during independent review of the opt-in Rust host. It makes no performance claim.

## Failure and change

The Rust relay previously wrote synchronously to the child control pipe and parent data pipe. If either pipe filled, the relay could no longer observe its readers' failure flags. A maximum-size control frame sent to a nonreading child, or a large response sent to a nonreading parent, could leave the host and child running after parent control EOF.

Each pipe now has one dedicated writer with a bounded one-frame channel. The relay waits for write completion while checking peer health every 100 ms, with a five-second write deadline. A failed peer, stalled write, or broken pipe terminates the host; the supervisor still owns and reaps the direct child. There is no retry on a partially written frame, no second writer, and no queue growth. A blocked writer thread ends with its host process. Python remains responsible for poll/response deadlines, artifact verification, grants, source leases, capture policy, durable Store commits, and acknowledgments.

Python's successful shutdown previously cleaned up the Rust host and its direct child but skipped the remaining process group when the host returned zero. An ordinary adapter helper could survive. Group cleanup now runs on successful shutdown too. On platforms with `waitid(WNOWAIT)`, Python observes leader exit without reaping it, kills remaining group members while the leader PID is still reserved, then reaps the leader and clears group ownership. This avoids a normal-shutdown process-group ID reuse window.

## Verification

The regression cases use freshly built binaries and temporary installed artifacts. Raw transport fixtures intentionally bypass Python's reader and timeout assistance to verify Rust itself remains responsive.

| Check | Result |
| --- | --- |
| Rust workspace formatting, Clippy with warnings denied, and locked tests | Passed; 17 Rust tests. |
| Python host, installed Rust sample adapter, and cross-language fixture tests | Passed; 20 tests using freshly rebuilt binaries, including backlog corrections and structure-bounded paging from the preceding fix. |
| Adjacent Python process, runtime, supervisor, and frame tests | Passed; 21 tests. |
| Child and parent pipe backpressure | Both directions terminate and reap the direct child after parent control EOF; both also terminate on the five-second write deadline while the parent keeps its pipe open. |
| Successful host exit with a non-detached helper | Helper stops; the test verifies group cleanup occurs before the leader is reaped. |

The new child-write fixture reduces the child's pipe capacity explicitly; the parent-write fixture emits a transport-bounded response larger than a normal pipe. Neither test relies on the receiver accepting that synthetic payload as evidence. Existing commit-before-ack, restart, revocation, crash, receiver-health, and timeout tests remain applicable.

## Limits

The process-lifecycle assertions use Linux `/proc`, Linux pipe controls, and `waitid(WNOWAIT)`. Other platforms require their own qualification. When another caller has already reaped a crashed host, or `WNOWAIT` is unavailable, group cleanup remains best effort. Plugins that detach into a new session escape ordinary process-group cleanup; subprocess isolation is not a same-user security sandbox. General poll deadlines still belong to Python. This change does not establish built-in harness parity, change database ownership, or claim a speed improvement.
