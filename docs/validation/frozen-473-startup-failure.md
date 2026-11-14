# Frozen 473: retained-history startup failed

The attempt on frozen `473d3388e6cd6966d5c2fb8e90001990573d224b` failed **before the timed one-hour workload started**. The receiver did not satisfy the unchanged 600-second readiness deadline. Recorded elapsed time was **600.907 seconds**. This is a failed startup result, not a completed one-hour stability run.

No timed UI, heartbeat, resource or turn samples were collected. CPU, memory, API workload latency, reconstruction, approvals, reconnects and one-hour stability therefore remain unmeasured by this attempt. Runtime and performance gates remain **open**.

## Registered scope and observed result

The detached application and harness remained clean at the same full commit before and after the attempt. The common generated raw seed contained **31,000 spans, 41,000 logs and 5,751 trace-activity rows**, occupying 188,383,232 bytes. Its file SHA-256 was `5418206cd39d72e397dbf0104fafe16a1c6ad298e4c3b9dd4b8cdfe9c8d8b682`. Private-clone schema preparation preserved the canonical raw-content hash `a7834f8369c95c9160d16fe1d4d1c23b401e76a58c68507ce8b4bafa43948588`. Preparation took 16.460 seconds and is reported separately from the failed readiness phase.

| Observation | Recorded value | Interpretation |
| --- | --- | --- |
| Configured timed workload | 3,600 seconds | Never entered |
| Receiver readiness deadline | 600 seconds | Exceeded |
| Attempt elapsed time | 600.907 seconds | Startup failure duration, not a soak duration |
| Initial readiness time | Unavailable | Readiness never established |
| UI/resource sample count | 0 | No resource budget conclusion |
| Heartbeat samples | 0 | No UI responsiveness conclusion |
| Completed/verified turns | 0 / 0 | No timed reconstruction opportunity |
| Expected/accepted/persisted review receipts | 0 / 0 / 0 | No timed review opportunity, not successful durability coverage |
| Receiver exit status | 0 | Recorded shutdown succeeded; startup did not |

The result file contains **13 reported errors in total**: the primary readiness timeout and **12 consequential unmet checks**. These include absent keyboard acknowledgements, worker cycles, restarts, heartbeat/process coverage and two-cycle shutdown/drain proof. They are retained individually in the sanitized artifact. They do not establish 12 additional independently diagnosed defects. A single receiver exit and pending-before/after values of zero do not satisfy the required two-receiver/two-cockpit lifecycle gate; remaining source-index work was still recorded after shutdown.

## What failed, and what is not yet proved

The retained receiver log contains `sqlite3.OperationalError: database is locked` on authenticated health requests. Its minimal application stack is:

```text
observatory/server.py:148        _read_GET
observatory/local_agents.py:1444 status
observatory/core.py:1247         upsert_agent_nodes
                                INSERT INTO agent_nodes
sqlite3.OperationalError: database is locked
```

This proves that health-side hierarchy persistence encountered a failing SQLite write. Initial migration failure was also observed during the attempt. The exact blocking transaction, its owner, lock duration and SQLite extended error code were **not established for this frozen run**. Separate later reproductions of a ten-second busy wait and a SQLite 517 snapshot conflict inform subsequent fixes; they do not uniquely attribute this failure to either mechanism. No timing or successful-run claim for those fixes is made here.

The coordinator also observed a live health excerpt with `ready=false`, stage “Indexing incoming source changes,” `processed=9600`, error “database is locked,” completed initial native polling and four native rows. **That response body was not retained.** This is a coordinator-observed excerpt, not a hashable raw-health archive or independent proof of a healthy four-agent frame. The archived timeout, server stack and Feed refusal are the primary retained evidence.

The separate [minimal Feed probe](frozen-473-health-probe.json) retained `coherent_updated=0`, `indexing_ready=false`, `native_bootstrap=false` and no captured content. It reported “History indexing failed; native observations are unavailable.” Rejecting fatal health metadata was the intended fail-closed behavior. Zero rows in that Feed projection mean evidence was withheld/unavailable; they do not establish an empty agent fleet or empty history. The probe was a separate generated-data observation during preparation, not a timed UI sample or full cold-readiness result.

## Process and evidence handling

The receiver's recorded exit status was zero. The generated actor was terminated and waited for normally by the coordinator. A later read-only check of the two original child PIDs found neither `/proc` entry present; neither matched an owned command line. This check does not claim ownership of or act on any reused PID. No personal process was stopped.

The failed database, WAL/config/state, original logs and frozen checkout were retained locally. This report publishes only an allowlisted projection of generated results and a minimal Feed probe. It includes no database, configuration, environment, credential, native source body, complete log or directory archive. Source-file/line references above are sufficient to locate the failing write without publishing unrelated log contents.

- [Sanitized startup results, source/harness hashes and individual checks](frozen-473-startup-results.json.gz)
- [Minimal Feed admission probe and original probe hash](frozen-473-health-probe.json)
- [Native bootstrap contract and independent keyboard validation](native-bootstrap.html)

## Remaining acceptance

Retained-history readiness, a useful current-Agent frame on the full corpus, strict active HTTP availability/writer cadence, resource budgets and the complete one-hour lifecycle workload all remain open. The independent keyboard/lifecycle checks in the native-bootstrap report remain valid within their own scope; they are not substitutes for this failed retained-history startup gate. Subsequent runtime changes require their own frozen-source validation, with every missed deadline and HTTP failure retained.
