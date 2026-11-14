# Full-corpus startup and short active smoke: failed

Frozen application `3fb82549d08730c753bd76553b19b41e48545bf8` and harness `883958949c024784c5b01c4afa35bc65dc2359a8` used the registered common synthetic seed (31,000 spans,41,000 logs,5,751 traces). Source/module hashes remained unchanged. The 4 GiB reserve was not lowered. The configured workload was 12 seconds with 2-second phases; this is neither a one-hour result nor performance acceptance.

## Cold startup succeeded

The receiver reached full readiness with no indexing error after an observed 221.505 seconds, reporting 52,002 processed jobs and 5,752 prepared runs. Native collection completed its first poll with four synthetic observations. This single-arm elapsed time is descriptive, not a cold-start target result.

## Active workload failed

At 7.305 seconds in cycle 0/phase 2, `GET /api/mcp?limit=20` returned HTTP 503 after health, Sessions, Activity and Metrics requests returned 200. The previous published sample was phase 1. The existing harness retained the traceback/status and server access log but not this 503 response body, so the exact dependency/error classification is not yet proved.

No three-worker cycle or turn reconstruction completed. Receiver/cockpit restarts did not run. One approval request and one rendered approval inbox were observed. The run exited 1 and remains failed; no retry, cadence change or filtered error was applied.

## Cleanup and retained evidence

The receiver exited 0 normally and drained pending work 2→0. Failure cleanup terminated the actor and isolated tmux server. No graceful cockpit-exit event was recorded; this remains a failure. All registered driver, receiver, actor and cockpit PIDs are absent. No artifact or clone was removed. The earlier failed 473 artifacts were untouched.

`full-corpus-startup-3fb.json` retains every harness error, exact commits, seed registration, raw result/log hashes and process cleanup evidence. The exact original results are retained in `full-corpus-startup-3fb-results.json.gz` (uncompressed SHA256 `093927512e524be8f791f01b7849042ba2880deba48a13d220c16f02b71d390b`). The private `failure.txt` and generated logs remain in the original owned directory; they are not copied into this report. The directory also contains private generated receiver credentials and should not be published wholesale.

The full startup/active acceptance track, loaded API behavior, the complete workflow/restart/drain smoke, CPU/RSS/WAL/storage outcomes and the one-hour run remain open. Next work is a read-only MCP dependency/coherence proof; no readiness relaxation is authorized by this failure.
