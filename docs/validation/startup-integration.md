# Startup writer and readiness harness integration

Retained-history startup could compete with health-side hierarchy persistence and fail before the timed workload. The reviewed fix coordinates writer admission before SQLite, commits bounded migration/backfill pages and yields outside locks. This integration preserves caller-owned transactions, exact source snapshots and visible fatal errors. The README installation change from main `7c80dc3` is unchanged.

The readiness harness stops on an explicit indexing error instead of waiting for a later timeout. Valid incomplete startup retains the existing deadlines/cadence; actual boolean readiness is required. At most eight observations retain allowlisted metadata, not captured health/session/error text. A follow-up records bounded metadata for failed active HTTP responses, attempts to close them and re-raises the original failure without retries or filtering. Failed-request duration includes that diagnostic read/close work; it is not an accepted API percentile.

## Exact integration

| Delivered commit | Integrated commit | Scope |
| --- | --- | --- |
| `3fb8254` | `86fed26` | Reviewed core writer admission |
| `8839589` | `7e9f202` | Reviewed readiness harness |
| `adb1350` | `66a612b` | Independent authenticated startup tests |
| `2354ecf` | `a9665e1` | Actual MCP readiness fixture |
| `eace36b` | `b5590dc` | Preserved full-corpus failed smoke |
| `9e0b3ac` | `1690857` | Next-run safe HTTP failure diagnostics |

The runtime core SHA-256 is `a91eb782970314004302e40177469f23875867f4f2388dceee8061e80b399d20`. The independent native/HTTP regression and metadata-safe harness bytes match the reviewed delivery boundaries. The test-only connection closure is `b46e8a9`: it adds deterministic closing around the existing commit/rollback context, with all evidence assertions intact. The MCP fixture calls the actual readiness method rather than stubbing its result. No additional product logic was introduced during integration.

## Validation

| Gate | Actual result | Scope |
| --- | --- | --- |
| Combined normal-module gate, Python 3.13.5 | **60 passed in 38.757 s**; explicit stderr check found no ResourceWarning or unraisable output | Writer admission, full endpoint/outcome/node parity, P1 snapshots, exact Activity worker scope, storage derivation, real HTTP/native startup, stop/resume and readiness privacy |
| Targeted gate after HTTP diagnostic helper, Python 3.13.5 | **22 passed in 0.874 s**, same clean-warning check | Eight new failed-response contracts plus existing readiness/soak tests |
| Affected fixture gate, Python 3.13.5 | **9 passed in 5.173 s**, no warning observed | Nested native snapshot and complete native-turn proof module |
| Coordinator's separately reported UI/model gates | 28 passed in 34.654 s; overlapping seven-model Python 3.13 gate passed in 2.022 s | Actual MCP readiness fixture and native bootstrap |

These overlapping gates are not added into a unique test count. The combined 60 gate predates only the later harness HTTP-diagnostic helper; the targeted 22 gate validates that helper and its readiness/soak compatibility. Runtime core bytes did not change between them. Python 3.13 used the project dependency path; the coordinator's earlier bare-interpreter Textual import failure was an environment failure, not a runtime regression.

The exact combined command is:

```sh
python -W error::ResourceWarning -m unittest \
  tests.test_startup_writer_admission tests.test_startup_receiver_health \
  tests.test_soak_readiness_errors tests.test_soak_monitor \
  tests.test_derived_readiness_snapshot tests.test_activity_worker_scope \
  tests.test_storage_derived -q
```

### First warning and diagnostic limitations retained

The first combined run had 60 passing assertions in 39.417 s but emitted an unraisable ResourceWarning, so it was **not warning-free acceptance**. A lightweight SQLite allocation audit identified `tests/test_opencode_turn_proof.py:21`, `TurnProofTests.prepare`: sqlite's transaction context committed but did not close its connection. The two-line fixture correction preserves that transaction and closes it deterministically. The affected nine-case check and final normal 60-case gate then passed without warning output. No warning was globally suppressed.

Two diagnostic limitations remain recorded: a full-tracemalloc two-test attempt hit the existing HTTP timeout without establishing allocation, and the allocation-audit runner invoked from stdin could not satisfy one multiprocessing spawn path. Those diagnostic failures are not relabelled as product failures or successful gates. The final ordinary `-m unittest` run has neither diagnostic wrapper and includes the cross-process case. Raw diagnostic hashes are retained in the [machine-readable integration record](startup-integration.json); raw directories are not publication material.

## Full-corpus functional smoke: startup ready, active phase failed

The separately owned run used app `3fb8254`, harness `8839589` and the common generated 31,000-span/41,000-log/5,751-trace seed. It reached complete readiness after 52,002 indexed jobs and 5,752 prepared runs, with no fatal index error, native initial polling complete and four native rows. Observed readiness time 221.505 s is **descriptive**, not cold-start budget acceptance.

The run then **failed** on `/api/mcp?limit=20` HTTP 503 in phase 2/cycle 0, with stored active elapsed 7.305 s. Three samples were published, but no turn/cycle completed or restart was exercised. Receiver exit 0 and pending 2→0 are recorded; no graceful cockpit exit record exists from failure cleanup. All registered child PIDs were absent afterward. All seven errors remain in the [failed-smoke report and screened original results](full-corpus-startup-3fb.html).

The original 503 response body was not retained. The exact gate/cause is **unknown**; later metadata diagnostics cannot reconstruct that response. Startup repair does not solve active MCP availability. The original frozen473 failure—600-second deadline, zero timed samples—also remains unchanged.

### Post-snapshot artifact handling

After the failed-smoke report was frozen, the coordinator rechecked that all four registered child PIDs were absent, created a private byte-verified reversible archive of the stopped failed working database/WAL/SHM (37,779,168 archive bytes versus 525,127,264 original bytes), and removed only that registered working clone. Logs, results, configuration, common seed and source remained untouched. Archive/cleanup metadata hashes are retained in the integration JSON. The original result and failed-smoke report's earlier “no clone removed” observation remain unchanged; this later action does not alter the failure or establish a performance result. The archive itself is not publication material.

## Remaining acceptance

Strict 420-write HTTP availability/cadence, loaded p95, cold <30 s, whole-monitor idle <5% of one core, sustained RSS/WAL/storage and the full one-hour lifecycle workload remain open. No benchmark, provider, native agent process or personal data was used by this integration track. Code delivery, focused correctness tests, remote CI and valid resource/external outcomes remain separate. No current all-green CI, push or merge is claimed by this report.
