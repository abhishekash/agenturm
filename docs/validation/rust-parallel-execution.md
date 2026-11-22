# Rust execution checkpoint

The 22 November 2026 checkpoint starts from `14be2845e4c7`. The completed checks below apply to individual slices. The distribution cutover still requires the paired workflow checks and the release owner's remaining scope decisions.

| Track | Implementation and verification | Remaining gate |
| --- | --- | --- |
| A1–A6 | Actual Pi, Codex, Claude Code and OpenCode records reopened and passed installed PTY checks at 60×18. Pi preserves explicit shell exit results. Codex preserves structured failure and independent check results. OpenCode covers direct checks and scoped activity after idle. Claude Code keeps worker identity when immediate parentage is absent. | Real Kiro transcript, actual Pi relationship evidence and an exact same-session Claude worker relationship contract. [Harness matrix](rust-harness-evidence-coverage.md) |
| B1 | Audited the Python 0.2.8 commands, SDKs, payload and metrics gaps, and specified paired keyboard journeys with owners. | Resolve legacy CLI, automatic SDK capture, cost, and host-history scope before the default switch. [Cutover inventory](../rfcs/rust-cutover-workflows.md) |
| B2–B4 | OpenCode retained prompt proofs survived subsequent sweeps and reopen. The tests deny disclosure after changed bytes, source replacement, expiry, revocation and restore. Codex Review renders typed edit counts tied to the recorded operation and turn. MCP has a separate approval and an exact, byte-verified operation picker. PTY and CLI checks cover selection, missing payload, mismatched identity, recorded null versus absent fields, and revocation. | OpenCode history recorded before these proofs may require recollection. File paths and diffs need a separate grant-backed proof. MCP support covers bounded Codex completion records with explicit thread and turn identity. |
| B5–B6 | A real OpenTelemetry SDK sent nested HTTP/protobuf spans through `run` and `serve-traces`. Both retained parent/child and exact source/session identity after reopen, and the launcher propagated the producer's exit status. The installed `review` command returned only the selected native turn, separate from a later turn. | Qualify additional SDKs after scope decisions and finish retained command workflows, including exact review receipt export and typed investigation. |
| B7–B8 | The packaged bare cockpit rendered the independent Prompt signals pane and Metrics at 60×18. Disabled configuration omitted the capability and a changed pin failed before opening history. Decoder installation design proposes a trusted local manifest and atomic catalog publication. | Approve decoder installation trust and activation. [Decoder installation proposal](../rfcs/rust-decoder-installation.md) |
| C1–C5 | Specified roles and native store operations. The separate opt-in admission listener exercised peer authentication, cursor reads, commit, lost-checkpoint recovery and exact replay as distinct Linux users. Scoped query core preserves the snapshot and reports retention expiry. | Wire collector ACK and cockpit queries through the broker, then qualify authorization, grants, retention, backup and restore end to end. [Storage broker contract](../rfcs/rust-storage-broker.md) |
| C6–C8 | A pinned static connector ran under the opt-in Linux x86-64 isolation profile. Tests exercise denial of host-file, network and inherited credential access and fail-closed launch. | Host-mediated credential and destination enforcement, followed by production endpoint qualification. [Connector runtime](../rfcs/rust-connector-api.md) |
| D1–D3 | The Ubuntu x86-64 candidate passed the archive checks below. Native arm64 qualification is configured in CI. | Arm64 support requires the exact archive and service checks to pass. D3 awaits the required workflow decisions and paired keyboard runs. |

## Ubuntu x86-64 archive

On Ubuntu 24.04.5 LTS, the local integrated 0.3.1 candidate archive had SHA-256
`7a1c87b4736ae0f7c3c6080cd2bed04a32168b4c3761fcfb8e21c3f1b3568866`.
Its extracted binary passed startup and collection without a Python
executable on PATH, rolling discovery, foreground restart, exact-turn
Review, inspection, backup and upgrade from the published 0.3.1 archive.
The temporary systemd user collector restarted with a new PID and retained
queryable history after uninstall. The independent capability passed on
both the published archive and this candidate. [Linux bundle verifier](../../scripts/verify-rust-linux-bundle.sh)

The real SDK launch and standalone receiver checks ran on the earlier
integrated archive with SHA-256
`87a11b63f765d69669152d5bd24455a7b994bb0abf7e612b77e5718378821912`.
The actual-record harness checks used that same archive.

## Distinct Linux users

An isolated, network-free x86-64 QEMU guest booted Alpine Linux
`6.12.115-0-virt`. The native service binary had SHA-256
`7e5222f6944c173ad2a355b9cb43ead2be76fc5b2c67324364a565191f03ced5`.
The broker ran under UID/GID 45000 and the test client under 45001.
The client established the broker peer identity, sent cursor and admission
requests, and could neither read nor write the database directly. A different
UID and a wrong primary GID with the authorized supplementary group reached
the socket and received no reply. An authenticated request for an unregistered
source was denied. SIGKILL after a partial request left the source cursor
unset. SIGKILL after a committed batch preserved exact replay across restart.
A later batch advanced the cursor, and the first request still replayed with
no extra observation. The database reopened with exactly two rows. Clean
termination removed the socket.
