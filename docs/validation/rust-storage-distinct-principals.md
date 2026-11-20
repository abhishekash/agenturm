# Rust storage probe: distinct-principal installed gate

20 November 2026. This is a deployment-shape check for the feature-off Rust storage service, not a storage cutover. The service still supports only a protocol probe, opens no database, and writes no data-root files.

## Installed proof

`scripts/validate_rust_storage_service_accounts.py` requires Linux root inside an isolated CI VM or container. It creates temporary directories and chooses unused numeric UIDs/GIDs without creating system users. The service runs as one non-root UID; its client runs as a different non-root UID and primary GID. Only temporary paths are changed in ownership. The local run used the pre-existing `python:3.12.15-slim-bookworm` image, a read-only repository mount, a read-only service-binary mount, no network, and an ephemeral container `/tmp`; it did not change host file ownership or touch a production database.

The real installed binary accepted and replied to one valid framed probe from the admitted client, with the expected request ID, receiver ID and `protocol_ready` result. The same running listener returned no reply to a wrong UID, wrong primary GID, same-UID service client, or wrong receiver ID. The wrong-GID client retained the authorized group as a *supplementary* group, so it reached the socket and exercised `SO_PEERCRED` rejection rather than merely failing a directory permission check. The check also verified a service-owned `0700` empty data root, service-owned `2710` transport directory with client group, service-owned `0660` socket with client group, bounded exit and socket cleanup, and restart followed by SIGTERM cleanup.

The admitted client tried to construct the existing Python `Store` at a fresh database pathname inside the private root. That failed on `EACCES`/`EPERM`, and a direct file-create attempt failed too. After the probe service stopped, the fixture seeded a disposable SQLite file owned by the service UID. The client could neither construct `Store` on that existing file nor open it directly. This verifies **fresh opens** from the old/client identity are denied by filesystem permissions even if that Python Store has no knowledge of the Rust service.

Local command (after building the binary):

```sh
docker run --rm --network none --read-only --tmpfs /tmp:rw,exec,size=64m \
  -v "$PWD":/src:ro \
  -v /absolute/path/to/agenturm-storage-service:/opt/agenturm-storage-service:ro \
  -w /src python:3.12.15-slim-bookworm \
  python scripts/validate_rust_storage_service_accounts.py \
  --bin /opt/agenturm-storage-service
```

The local run passed all account, permission, probe, denial and cleanup cases. The `rust-protocol` CI job now builds the service, runs its existing installed tests, then invokes this root-required fixture with passwordless `sudo` on the `ubuntu-24.04` hosted VM. [GitHub's hosted-runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners#administrative-privileges) documents passwordless `sudo` for Linux VMs. CI results remain pending until that workflow runs on the PR.

## Gates still open

- This test does not revoke an old Python process's **already-open** SQLite file descriptor or transaction. A cutover needs a stop/drain protocol, explicit writer exclusion and evidence that no old process retains an open handle.
- It does not migrate an existing database or test archived copies, WAL files, backups, or other historical data. Their ownership, access and migration behavior need separate gates.
- The probe service has no mutation operation, durable replay receipt, database lifecycle, production caller or default activation. The `protocol_ready` reply says only that the transport and identity checks succeeded.
- The `0700` check is a filesystem boundary for these non-root identities. It does not confine root or a process that already held a usable descriptor before permissions changed.

See [the service skeleton validation](rust-storage-service-skeleton.md) and [the protocol contract validation](rust-storage-service-contract.md).
