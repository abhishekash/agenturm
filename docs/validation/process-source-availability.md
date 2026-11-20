# Process source availability

An installed process adapter can stop reading while its previously recorded sessions remain in the Store. A completed session is still a recorded outcome, but it is no longer evidence that the source is currently being observed. The health overlay keeps these facts separate.

## Health contract

`/api/health` reports `local_agents.process_source_availability` with the exact configured `source_id`, `availability`, `reason_code`, and the number of retained projected sessions. The values are `ready`, `pending`, and `unavailable`:

- **Ready:** the worker is running and a successful poll followed its latest failure.
- **Pending:** a worker is configured or starting, or has restarted without a successful poll yet.
- **Unavailable:** the worker has failed, stopped, or has a failure newer than its last successful poll. A retained source with no active worker is also unavailable and has reason `unmonitored`.

The `Process adapters` source group is partial while any configured or retained process source is not ready. The API annotates affected session copies with `source_availability` and `source_health_code`. It does not rewrite the stored run state or remove the recorded session. A successful poll after the failure clears the annotation. The worker's own failure code remains available in `plugin_workers` for diagnosis.

The terminal session inspector says the outcome is last recorded and identifies the exact source. The Host pane and plain output surface a bounded source warning. Long source IDs are retained in the API and shortened only for display. Counts and visible source labels are capped to keep narrow terminals readable; the API currently projects at most 200 retained sessions and accepts at most four configured workers.

## Validation

The tests cover a completed retained session across worker failure, restart before a successful poll, and recovery; a receiver without a matching worker; exact API ownership versus bounded display; and rendered 80×24 and 60×18 terminal layouts. The installed Codex directory fixture also renames its root during polling, confirms the worker becomes unavailable while the completed outcome remains, restores the root, and checks that coverage returns to ready after a successful poll.

```sh
uv run python -m unittest tests.test_process_source_availability tests.test_session_view -q
AGENTURM_RUST_CODEX_BIN=/path/to/agenturm-codex-harness uv run python -m unittest tests.test_rust_codex_directory_plugin.CodexDirectoryTests.test_root_unavailable_then_restored_keeps_last_outcomes_and_recovers_coverage -q
```
