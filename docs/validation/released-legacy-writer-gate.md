# Released v0.2.8 writer: Linux artifact gate

20 November 2026. This feature-off fixture executes a wheel rebuilt from the annotated local `v0.2.8` tag, whose target commit is `2802d79930728bb331db1d4d718d6dea1c9c374d`. The checked-in wheel is `tests/fixtures/legacy/agenturm-0.2.8-py3-none-any.whl`, SHA-256 `4fda5471d0a6993e61908949ffc86d7ff28322928071c2463deea774848fba2e`. It was built from `git archive v0.2.8` with the tag's declared setuptools backend using an offline cached backend. A local byte comparison found all 89 wheel Python files identical to the tagged source. The fixture pins its bytes and asserts that the old `Store` module is imported from the unpacked wheel, not the current source tree. This is a locally rebuilt release-source artifact, **not** an assertion about the PyPI upload's bytes.

## Adversarial result

The root-only disposable fixture uses separate unused legacy and service UIDs/GIDs inside an isolated Linux container or CI VM. A `v0.2.8` `Store` opens and commits a WAL-mode database. After a detached backup is placed in a service-only directory, the old process begins a write transaction. Moving the old main/WAL/SHM files into the private archive and changing their owner **does not revoke** that open connection: the released Store still commits DML, DDL, and a changed receiver identity to the archived state. The fixture verifies those changes after the process exits.

Fresh released `Store`, released `agenturm --db ... prune`, and direct SQLite writes are denied on both the protected candidate and archive paths. The child checks that its `Store` came from the pinned wheel; the CLI uses that wheel for Agenturm imports. The candidate's main/WAL/SHM byte state, original receiver ID, and pre-archive row remain unchanged. The backup includes the committed WAL row. This reproduces the held-open hazard with released code rather than the current Store stand-in used by the earlier [negative gate](legacy-held-open-writer-gate.md).

Run with the existing image and no network or image pull:

```sh
docker run --rm --pull=never --network none --read-only --tmpfs /tmp:rw,exec,size=128m \
  -v "$PWD":/src:ro -w /src python:3.12.15-slim-bookworm \
  python scripts/validate_legacy_held_open_writer.py \
  --release-wheel /src/tests/fixtures/legacy/agenturm-0.2.8-py3-none-any.whl
```

The local Docker run passed the released and current-Store modes. The Rust CI job invokes both on an isolated Ubuntu runner; its queued status is separate from these local results. The wheel is a disposable test fixture and is not installed into Agenturm's runtime.

## Cutover boundary

The protected candidate/archive are test-only; no Rust service opens a production canonical Store. One pinned release does not cover every supported older wheel, CLI, direct SQLite caller, or held-open process. The old process must be stopped **before** backup and archive; permission changes alone cannot establish that. Production Rust authority still requires a protected new canonical path and service principal, complete mutation routing, synchronized backup and recovery, and adversarial tests for the supported old-artifact fleet. See the [writer cutover gate](rust-writer-cutover-gate.md). Python `Store` remains the production writer, and this fixture makes no performance claim.

The [HTML checkpoint](released-legacy-writer-gate.html) is a self-contained review copy.
