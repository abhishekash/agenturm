# Trusted synthetic private-reference end-to-end checkpoint

## Scope

This checkpoint exercises the generic reference envelope, host-selected capture mode, admission provider SPI, Activity projection, generic exact-event read route, and provider ownership lifecycle together. It uses only a fixed fake text fixture under a temporary directory and a separately installed test child. The test does not install a production capture mode or load arbitrary provider code from the child.

## Evidence exercised

- The child process receives only a synthetic SHA-256 digest and submits it with one coarse Activity record through `reference_refs`.
- The trusted host grant reads the configured fixture file, binds its fingerprint to the source lease, and the admission provider reopens and compares the source bytes before commit.
- Reference content, its source digest, ordinary evidence, cursor, and provider ownership are committed in the same writer transaction.
- The Activity projection publishes the event. The generic `/api/plugin-reference-content` route returns only the exact requested event through a host-registered reader.
- The read output redacts the fixed fake password, omits the configured file path, and is capped at 4096 UTF-8 bytes with `clipped=true`.
- Revocation deletes provider content and owner rows and makes the exact-event route return 404. Reopening the receiver database still fails closed.
- Changing the source fingerprint during activation erases provider content before the new lease is issued; the prior event cannot be read afterward.

## Validation

Run the focused fixture with:

```sh
python -m unittest tests.test_plugin_private_reference_e2e -v
```

CI also runs this focused command in the Rust protocol/plugin-adapter job and discovers the test in the full Python suite.
