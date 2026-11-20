# Trusted private Activity read-provider checkpoint

The receiver has a bounded, immutable `ReferenceReadRegistry` for explicit private-content previews. A reader is installed by trusted host code. For every request the host checks the exact Activity event, durable source ownership by that reader, receiver identity, capture enablement, current policy and lease, and a fully projected source revision. It repeats the binding check after the provider read and guards the response immediately before writing it.

## Read boundary

The local HTTP caller may select a provider ID and one Activity event ID. Those values only select a host-installed reader and an exact record. They do not grant access by themselves. The store must name the provider as an owner of the record's source. Unknown readers, stale or revoked ownership, mismatched receivers, disabled capture, and incomplete projections fail closed.

The existing Pi user-text and requested-command readers remain the providers for Pi content. They retain their independent file fingerprint, provenance, parsing, redaction, and authorization proofs. The registry wraps their already-redacted output, enforces an additional 16 KiB reader bound and safe-text check, and the HTTP response clips to 4 KiB. Responses are local-only, `no-store`, bound to receiver and source revision, and cancelled if the response guard detects a change. The UI holds previews transiently and expires them after 20 seconds.

## Extension boundary

Only trusted receiver composition may install a read provider or a UI preview declaration. An untrusted child, TOML file, manifest, or arbitrary plugin cannot register a reader or choose its proof implementation. A future reader must provide its own trusted proof and redaction logic, be durably granted for an exact source, and preserve these revision and revocation checks. This does not mean arbitrary plugins can read private content.

The synthetic provider tests use only fixed synthetic text. They exercise exact owner/source/receiver binding, unknown provider rejection, revocation during a read, stale projection, and output bounds. Existing Pi route tests cover legacy status behavior (400 malformed query, 403 nonlocal access, 404 unavailable or revoked content, 409 mid-read change, 503 unavailable revision), no-store delivery, and response cancellation. UI tests cover the transient preview lifecycle and expiry.

## Verification

- Broad plugin-family suite after rebase: **247 tests passed** (`uv run python -m unittest discover -s tests -p 'test_plugin_*.py'`).
- Backend and HTTP route subset after rebase: **23 tests passed** (`test_plugin_reference_read`, `PluginContentDeliveryTests`, and `PiCommandHTTPDeliveryTests`).
- Combined registry, route, UI preview, and Pi read-proof suite before rebase: **78 tests passed**. It was rerun after rebase for the backend and route subset above.

No performance claim or production cutover is made.

The [HTML checkpoint](plugin-reference-read-spi.html) presents the same review record in a responsive light/dark layout.
