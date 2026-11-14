# Final authenticated HTTP and raw-history protocol

This harness distinguishes earlier `Store.sessions`/`Store.mcp_page` method timings from actual authenticated HTTP endpoint timings. It is an API-only receiver benchmark. It starts no native discovery, provider, agent launcher, watcher or notification. Whole-monitor CPU, memory, cold readiness and useful Agents frames remain separate measurements.

`benchmark_http_tails.py` runs the complete baseline **5e0c397** and candidate **55c6244** source trees in separate receiver processes. An external coordinator issues direct loopback HTTP requests and paced writes. It does not patch older methods into a newer Store. Each source imports its own Store/server and builds its own normal production derived schema. The same committed harness, Python environment and immutable generated seed serve both versions.

## Registered preparation

The normal run requires the common generated corpus: 5,751 trace-activity rows, 31,000 spans and 41,000 logs. The seed is opened read-only, hashed at every clone/preparation/run boundary and never passed to a Store. Only one working clone is retained at a time. Disk checks preserve at least 4 GiB reserve and the existing eight-times-seed preparation allowance.

The shared `clean-raw-schema-v1` preparation is applied **only to registered owned clones**, before receiver startup and before the idle harness's cold-readiness clock. It keeps exact `spans`, `logs` and `trace_activity` rows, table definitions and constraints. It removes inherited derived tables, views, triggers and named indexes, recording their SQL. Only a fresh same-version `schema_meta` table definition and `trace_activity_v1` marker remain: the raw trace-activity inventory is already present. Empty-store readiness and backfill markers are never copied.

Nonempty progress, metrics, outcomes, explicit agent connections/nodes/identities, durable native policy and unknown input tables refuse preparation. Derived dirty queues are permitted and are rebuilt normally. Symlink/seed aliases, changed registered clones or changed seeds also refuse preparation. The preparation record includes raw/file hashes, counts, retained raw DDL, removed schema and elapsed preparation time. After normal initialization and strict readiness, the full named-index SQL manifest must equal an empty, normally initialized same-version Store. Indexes are not adjusted inside timed phases.

The idle harness uses the same preparation for `--seed` runs. Its earlier interfaces and functional `--history` mode remain available. Earlier smoke/resource artifacts using another preparation are not relabelled as this protocol.

## Registered HTTP workload

Both endpoints use new authenticated HTTP connections, with the exact selected limits:

- `/api/sessions?limit=500`
- `/api/mcp?limit=100`

Each version first proves unauthenticated API access returns 401. Authenticated health must have a stable receiver, a matching namespace header, authentication enabled, native monitoring disabled and `indexing.ready` exactly true. Warmup and timed responses must have coherent nonempty object rows; MCP inventory must be an integer at least as large as its returned call page. Every GET requires the exact receiver header, and any receiver in the JSON body must agree.

Preparation, initial readiness, schema verification and warmup occur outside endpoint sample timings. Five warmup pairs precede twenty idle pairs. Endpoint order alternates on every pair.

The loaded phase schedules forty read pairs one second apart, beginning 0.3 seconds after the writer's first scheduled slot. The external writer sends exactly 420 authenticated two-span batches at absolute 100 ms intervals. Each batch updates one generated root and adds one generated MCP operation in one fixed trace/session/actor. Bodies, IDs and source timestamps are deterministic and identical across versions. Thus 840 accepted records represent 421 unique spans, rather than 840 new operations.

No slots are skipped to catch up and no burst catchup is permitted. Lateness over one respective slot period invalidates the run; missed slots and all actual attempts remain recorded. Every loaded read must complete within the fixed scheduled writer window. Accepted write progress must occur before, during and after each endpoint's sample window.

Literal write-attempt intersections and accepted writes finishing during each read are recorded individually. A fast read with zero literal intersections remains in the sample. Selecting only requests overlapping a write would bias the distribution toward slower requests.

The `/api/spans` POST does not atomically enforce the expected-receiver header in these versions. The harness makes no such claim. It uses privately owned receiver databases and processes, verifies pre/post health identity, and binds every GET to its observed namespace. POST status, accepted count and all write errors remain explicit.

## Quantiles and failure handling

A latency sample begins immediately before issuing a request and ends when its full response body has arrived. It includes server dispatch, normal Store work, server JSON encoding and HTTP/body transfer. Client JSON validation follows timing. The coordinator's client/writer threads share a process with each other, while the receiver and its Store workers are in a different process.

Each raw sample retains monotonic timestamps, status, body size/hash, receiver header, row keys, returned counts and selected nonnull field counts. Writer samples additionally retain scheduled/actual timestamps, accepted counts, deterministic request hashes and lateness. Source-tree hashes, harness hashes, process/import identities, before/after seed manifests, schema manifests and drains are retained.

The empirical p95 is nearest rank: sorted element `ceil(0.95*n)`, without interpolation. All actual attempts remain in the distribution. A 503, authentication failure, wrong receiver, malformed/oversized JSON or missing/empty expected data invalidates the run. It is not retried, replaced by an empty success or removed from quantiles. Statistics from a failed run do not establish a successful-response latency budget; budget verdicts remain unavailable for failed or smoke runs.

Raw results are written before registered clone cleanup, after server requests and Store workers have stopped. Seed files, source trees, credentials, logs and result artifacts are not cleanup targets. The private token is generated per receiver, stored only inside the owned directory, and omitted from result JSON.

## Invocation and evidence boundary

Use an already prepared Python environment, clean committed harness and clean detached source trees. Coordinate other local workloads before an explicit normal run; dependency installation, fixture preparation and profiler runs are not part of the timed interval.

```sh
python scripts/benchmark_http_tails.py run \
  --seed /tmp/agenturm-final-common-seed/raw-seed.db \
  --verified-synthetic-source \
  --baseline-root /tmp/agenturm-http-baseline-5e0 \
  --candidate-root /tmp/agenturm-http-candidate-55c \
  --output /tmp/new-http-tails
```

`--smoke` uses a tiny generated fixture, one warmup pair, two idle pairs, two loaded pairs and ten batches. It is always nonpublishable. The actual tiny B/C functional check proved separate correct source imports, normal named indexes, authentication/namespace binding, unchanged seeds, accepted 10/10 batches and clean shutdown/cleanup. Its loaded requests returned transient 503 while incoming history was indexed. Those responses are retained as invalid, and establish **no API latency or availability pass**. No full-corpus HTTP measurement has been performed for this report.

The shared heartbeat now records `visible_agent_rows` from the already published `app.table_data['agent-table']`, without preparing sources or querying widgets. The idle first-useful gate requires at least one such row in addition to four fresh known fixture sources. It saves the count and first useful terminal frame. An internal source map with an empty visible table cannot establish a useful Agents frame.

The final touched/common contract suite passed 21 tests in 13.295 seconds. It checks actual HTTP 503 retention, strict namespace/shape checks, nearest-rank p95, no catchup requests, atomic handshake publication, raw-schema ownership/preservation/refusals, fresh production indexes, external receiver/client identities, explicit negative-smoke failures and the published visible-row gate. These are functional validations, not controlled resource measurements.
