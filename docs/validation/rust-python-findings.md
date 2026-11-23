# Python receiver Findings qualification

23 November 2026. Rust can browse Findings from an explicitly attached Python
0.2.8 receiver and display an exact, currently authorized source. The reference
is tag commit `2802d79930728bb331db1d4d718d6dea1c9c374d`. The client starts no
Python receiver and imports no Python history into Rust storage.

## Implemented journey

Press `7` for the bounded receiver-wide Findings list, Enter to inspect a
selected finding, and `s` for its supported source. Escape redacts the private
view and restores the selected finding. Paging, refresh, loading, failure and
compact views retain the selected ID when it still exists. Metadata uses closed
kind/severity labels and exact owner/member IDs; server titles, detail fragments
and source bodies do not become public row labels or exports.

Private display checks authenticated receiver identity, readiness, applicable
current capture policy, exact finding ID, producer/actor/session/trace/turn,
and every selected source reference. It reads the current source twice before
publication and continues checking during display. Only bounded known log,
span-error and evaluation fields are rendered. Arbitrary attributes, resources
and unknown raw record shapes are excluded. Malformed or credential-like text
is withheld. Private copy/export remains unavailable.

The guard uses the existing bounded terminal writer and redaction acknowledgement.
Authorization is sampled by polling, not held by a remote server transaction.
The visible lifetime remains bounded. An evaluation source reference identifies
the current returned record, not an immutable historical evaluation revision.
New policy generations require refreshed metadata before another reveal.

## Evidence

The joined source passed **819 Rust tests across 96 suites**, none failed or
ignored, strict whole-workspace all-target Clippy and an all-binary build.
The frozen development app SHA-256 is
`fffbe7366305fe591aa7b9264e57a4af041ed67c85bc165873ca6ebd411c9ffc`.
Its nine-binary bundle is `/tmp/agenturm-python-findings-qualified-dev`.

The actual tagged receiver passed:

- Error-log and failed-evaluation source display, exact selection return,
  no private clipboard/export and terminal restoration at 120×32, 80×24 and 60×18.
- Source-only evidence with applicable global capture-policy revocation.
- Real same-URL finding-cache removal, owner substitution, same-owner member
  substitution and a changed evaluation between proof reads; each withheld text.
- Stable unknown raw fields and split sensitive key/value injection; each withheld text.
- Policy denial before the source request, with zero source fetches, and policy
  revocation during an already visible private frame.
- A fresh receiver on the same authenticated endpoint with the exact same finding
  ID/body but a different authoritative receiver identity; the old view redacted.
- A saturated terminal before publication, then a policy change before the second
  proof completed; no private frame was published afterward.

Receiver replacement uses a real fresh Store and handler on the same listener.
It does not claim the old server's cached Store identity detects an unannounced
physical database-file swap. Fault hooks asserted that the intended records or
policy actually changed; no passing case relies solely on a timeout.

An older Review comparison assumed equality with Python's six broad turn records.
The tagged fixture actually contains four verified step-member events plus two
unspanned user messages. Rust keeps the latest verified first-page user message
as separate request context and counts four authorized window records. The
corrected oracle proves the exact raw member set and retains all request,
assistant, check, usage, file, policy-revocation, cursor and bearer assertions.
No runtime ownership check was relaxed. Python's stale live-session overlay and
Rust's captured-member completeness are distinct coverage statements.

## Repeatable validation

- [Tagged Findings and adversarial terminal test](../../scripts/validate_rust_python_findings.py)
- [Scoped Review and exact step-membership oracle](../../scripts/validate_rust_python_receiver_review.py)

The full terminal proof is `/tmp/agenturm-python-findings-tagged-root.log`.
Review regression is `/tmp/agenturm-python-review-membership-oracle-root.log`;
joined tests are `/tmp/agenturm-python-findings-joined-validation.log`.
CI checks out the immutable reference commit separately before running both
compatibility tests. This follow-up creates no public release or default cutover.
