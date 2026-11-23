# Host inventory and metric metadata qualification

23 November 2026. This joined follow-up builds on merged Findings PR #380
(`39474ddabfca3bc448cf8f8a0b71fda42b9560f1`). It does not publish a release or
switch the default. The source and fixtures accompany this record.

## Source validation

- **833 tests across 96 suites**, zero failed or ignored; strict whole-workspace
  all-target Clippy; all workspace binaries; Rust formatting check passed.
- The final Host chooser correction is included in that joined run. Its 14
  focused tests are a subset of the aggregate, not an additional test count.
- Independent review found no source authorization or compatibility blocker.

The frozen nine-binary development bundle is
`/tmp/agenturm-final-merge-qualified-dev`. Its app SHA-256 is
`a3fd5ae6503c4a7529e36d8ed0783ca6f457cf76a7e4b42ff2121bf9261be3dd`;
storage service SHA-256 is
`8f28f38c3cd586531e88569a8bb8982116166f8092c71b3cf2775d25e85a09b5`.
These identify the development executables exercised below, not a published
archive or an optimized release qualification.

## Metrics

Schema 21 adds a separately validated public metric-description companion and
index/format marker. Existing point IDs, payloads, series keys, receipts and
private grants are not rewritten. A descriptor must be anchored to an admitted
point in the same batch and source. Descriptions are bounded to 512 bytes and
unsafe or ambiguous text is omitted without discarding valid metric points.

Catalog queries search name, service, kind, unit, scope, source and description
before pagination. Source authorization is applied before grouping and limits.
The bounded literal query is at most 256 bytes and uses ASCII-insensitive
matching. Search, source filter and read snapshot bind the catalog cursor.
Existing point wire format/cursors are unchanged; older source-scoped catalog
cursors require a fresh first page.

The frozen app passed authenticated JSON/protobuf HTTP ingestion, auth denial,
replay, description-only admission, revision and unsafe-text omission. All seven
old point rows retained identical IDs, serialized values, digests and receipt
sequences. Unit/store checks include schema 20-to-21 compatibility.

At 120×32, 80×24 and 60×18, the actual terminal found an older matching series
beyond the first 32 entries, opened its description, entered exact points,
returned to the same series/search/snapshot and cleared the filter.

## Recorded Host inventory

The local Linux writer records a bounded validated hostname and explicit Linux
platform when available. Older/remote records without those fields remain
unreported. The reader never infers the remote OS from its own host.

Host inventory selects the latest authorized sample per source before limiting
results. A remote `H` reads that inventory without probing the viewer's process
filesystem or requesting local capture. Multiple/partial streams open a
`Recorded host streams` chooser rather than selecting an arbitrary source.
Refresh keeps inventory mode and exact selected source through reordering.
Process drilldown shows recorded source, timestamp and age, and Escape returns
to the same stream. Host chronology remains a separate mode.

The exact app and service passed the updated separate-UID Linux KVM fixture:
opt-in GET/typed POST, hidden-source exclusion, more than 64 records for one
source, multiple streams, metadata/grant denial, and restored binding fences.
At all three terminal sizes, a newer sample was published for the previously
older selected stream; current settled terminal cells verified refreshed time,
exact source/process drilldown and selection return. Collector and reader used
different identities; the reader did not reacquire a local sample.

A stale typed history POST returns HTTP 200 with `Err(snapshot_replaced)`.
A metadata GET with a stale binding header returns HTTP 409. The fixture was
corrected to assert those distinct existing protocol contracts.

## Reproduce

From the checkout with the corresponding built executable:

```sh
python3 scripts/validate_rust_metric_descriptions.py --app /path/to/bin/agenturm
python3 scripts/validate_rust_metric_catalog.py --app /path/to/bin/agenturm
```

The remote fixture requires the documented broker/collector/reader account
setup or its equivalent separate-UID guest:

```sh
python3 scripts/validate_rust_remote_host_history.py --bin-dir /path/to/bin
```

Working-session logs are `/tmp/agenturm-final-metadata-merge-validation.log`,
`/tmp/agenturm-final-merge-metric-http.log`,
`/tmp/agenturm-final-merge-metric-pty.log` and
`/tmp/agenturm-final-merge-host-kvm.log`. Local `/tmp` paths are evidence handles,
not durable artifacts. The two metric fixtures are also wired into CI.

## Remaining acceptance

Final optimized combined-archive install/0.3.8 upgrade, all packaged guest and
paired UI journeys, and current-head Linux/SDK CI remain separate gates. Earlier
native-only archive checks do not certify this schema 21 follow-up. Real Kiro,
simultaneous native Claude coverage, header-only roster behavior, production
connector acceptance and native macOS support retain their documented scope in
the [remaining-work plan](../rfcs/rust-migration-remaining-plan.md).
