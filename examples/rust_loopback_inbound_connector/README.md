# Rust loopback inbound connector fixture

This separately built, opt-in Rust connector reads metadata-only pages from one configured loopback HTTP endpoint. It reuses Agenturm's `connector-adapter/1` and `source-adapter/1` contracts and the generic plugin supervisor. The child never opens the receiver Store: the host owns admission, capture lease, durable cursor, commit-before-ACK, replay and projection.

## Build and explicitly install

```sh
cargo build --locked --release --bin agenturm-loopback-inbound-connector
python examples/rust_loopback_inbound_connector/install.py \
  --binary "$PWD/target/release/agenturm-loopback-inbound-connector" \
  --destination /absolute/private/inbound-plugin \
  --endpoint http://127.0.0.1:8123/events \
  --source-id rust-loopback-source
agenturm serve --plugin-config /absolute/private/inbound-plugin/plugins.toml
```

The installer copies an already-built executable, pins the copied bytes in the manifest, and writes a configured endpoint file and explicit plugin config. It does not compile, launch or activate the connector. Agenturm does not enable this example by default.

The endpoint in the command is illustrative. A compatible local HTTP service must already be listening at that address; this example does not launch one.

## Scope and limits

Only the literal URL shape `http://127.0.0.1:<port>/events` is accepted. The adapter opens a direct loopback TCP connection; it does not consult proxy settings, follow redirects, send credentials or perform writes. It reads at most 1 MiB per HTTP body, 256 records per poll, 16 KiB per projected record, and the shared 4 MiB process frame limit. Remote pages are checked for duplicate/unknown fields and a maximum of 8,192 structural tokens. It accepts lifecycle and usage metadata only; private content and arbitrary text fail before Store admission. The configured grants are admission declarations, not an OS network sandbox, and the process runs with the user's OS permissions.

A `429` creates an idle batch with the same cursor and bounded in-process retry delay. A `410` for a nonfresh cursor yields one durable `cursor_expired` gap diagnostic. The host ACKs only after its Store transaction succeeds, so a restart resumes at the committed cursor. Capture revocation fences the active lease and removes this source's admitted projection through the existing host path.

This is a connector capability proof, not a credentialed external connector, stable SDK/ABI, or performance claim. Do not put credentials in the endpoint or source config.
