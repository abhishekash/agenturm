# Storage mutation route inventory

20 November 2026. This is a feature-off review gate for the planned Rust writer cutover. Python `Store`, `WorkflowStore`, and `TaskStore` remain authoritative. The checked-in [site inventory](../../tests/storage_mutation_routes.json) describes 143 access calls at 122 source locations in the current tree. It is not a runtime fence.

## Current paths that can change durable state

| Authority | Entry routes and bypasses | Current boundary |
| --- | --- | --- |
| Telemetry `Store` | Receiver and cockpit `Store` constructors; OTLP/native ingest, logs, metrics, outcomes, progress, pruning, retention policy, native backfills, plugin admission/projection, and direct CLI `import-jsonl`, `import-evals`, `prune` | One `Store` instance has a feature-off handoff drain. Other instances and legacy binaries can still write. Its `_connect()` remains a writable Python SQLite connection. |
| Workflow | `WorkflowStore` constructor and transactions for observe, acknowledgement, claims, delivery, error state, native lifecycle enqueue/flush/uncertain; attention CLI and UI controller create separate instances | Handoff tracks one instance. Direct CLI/UI instances are not covered by another instance's drain. |
| Saved tasks | `TaskStore` constructor, create/edit/decision/link operations, instance backup, task CLI and UI instances; module-level `backup_task_database()` opens SQLite outside a `TaskStore` instance | Handoff tracks one instance and its backup. The module-level backup and other instances are outside that fence. |
| Outbound | `grant`, `revoke`, and `enqueue` use `Store` writes. Dispatch, recovery, and reconcile write before/after external process calls; dispatch may be in flight while no `Store` connection is open | The feature-off outbound guard excludes cooperating dispatch/reconcile while held. It does not settle uncertain deliveries or fence an older binary. |
| Direct SQLite and offline tools | `receiver_state.migrate_database()` copies into a new telemetry target; `migration_candidate` writes detached clones and backups. Read-only connections include source readers, Store authority preflight, response cache, and capability watch | These do not transfer canonical authority. The offline producer requires externally stopped writers; ordinary read-only opens are catalogued because a changed mode could become a bypass. |

The `agenturm migrate-db` command uses `receiver_state.migrate_database()`. `agenturm task backup` reaches module-level `backup_task_database()` directly. `agenturm serve` and cockpit construct telemetry Stores; `agenturm task`, native lifecycle, attention CLI, and UI controllers construct their own task/workflow instances. This inventory includes these constructor sites, not only class definitions.

## Machine check

`python -m unittest tests.test_storage_mutation_route_inventory -v` parses every Python module under `observatory/`. For each lexical owner it compares the exact count of `sqlite3.connect`, direct `sqlite3.Connection`, `_connect`, `_transaction`, `_write_db`, `_source_write`, and Store/WorkflowStore/TaskStore/OutboundQueue constructor calls to the reviewed JSON inventory. It recognizes the standard `sqlite3.dbapi2` spelling and called `Connection` imports. Aliased SQLite imports, escaped connection callables and imports of known alternate drivers (`aiosqlite`, `apsw`, `pysqlite3`, `sqlcipher3`, `pysqlcipher3`) fail for explicit review. A new site, removed site, or changed count fails until its authority and effect are classified. It also checks the top-level, task, attention, and native lifecycle CLI parser names against an effect classification, so a new CLI route cannot silently reuse an existing Store constructor. Synthetic tests exercise direct constructors, import aliases, function-local scope and reassignment. CI runs this check.

The `effect` field is conservative: `may_write` means the site has or can reach a writable handle, not that every call writes. `external_effect` marks outbound dispatch/reconcile with a network action between database operations. `backup_target` includes read-only backup sources paired with new destinations. The registry is intended to make code review visible; editing the JSON can acknowledge a route but cannot make it safe.

## Limits and next gate

This AST gate cannot detect dynamic `getattr` or `importlib`, arbitrary extension code, an unrecognized alternate SQLite driver, a separately installed old wheel, an already-open handle, or SQL that a later change executes through an existing connection site. It does not inspect every UI action, HTTP route, or dynamic CLI argument; the catalogued constructors and connection sites are the current boundary. It does not prove all current routes are drained or routed through the Rust service. Before production cutover, every listed **mutable** route needs a service API or a proven stop/fence, including constructor migration, separate CLI/UI instances, task backup, outbound uncertainty, and old held-open processes. Then run the adversarial [writer cutover gate](rust-writer-cutover-gate.md) and a complete migration/recovery drill. No performance gain is claimed.

The [HTML inventory](storage-mutation-route-inventory.html) is a self-contained review copy.
