"""Verify the Rust evaluation inventory in a disposable Linux guest."""

import argparse
import grp
import json
import os
from pathlib import Path
import pwd
import secrets
import select
import stat
import subprocess
import tempfile
from urllib.parse import urlsplit

from validate_rust_remote_history import private_directory, request_get, stop, wait_for, write_token

PRIMARY, SIBLING, CAPACITY = "otlp.primary", "otlp.sibling", "otlp.capacity"
PRIVATE = "bounded evaluation fixture text"
TRACE = "1" * 32


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("Run in a disposable Linux guest with support for separate UIDs.")
    binaries = args.bin_dir.resolve(strict=True)
    occupied_uids = {account.pw_uid for account in pwd.getpwall()}
    occupied_gids = {group.gr_gid for group in grp.getgrall()}
    broker_uid, collector_uid, reader_uid, maintenance_uid = [
        uid for uid in range(45800, 46000) if uid not in occupied_uids
    ][:4]
    client_gid = next(gid for gid in range(45800, 46000) if gid not in occupied_gids)
    token = secrets.token_hex(32)

    with tempfile.TemporaryDirectory(prefix="eval-inventory-") as name:
        root = Path(name)
        root.chmod(0o755)
        data, transport, backups = [root / folder for folder in ("history", "transport", "backups")]
        private_directory(data, broker_uid, broker_uid)
        private_directory(transport, broker_uid, client_gid, 0o2710)
        private_directory(backups, broker_uid, broker_uid)
        socket = transport / "agenturm-native-storage-v1.sock"
        config, token_file, fixture = [root / file for file in ("broker.json", "token", "fixture.json")]
        config.write_text(json.dumps({
            "version": 1, "socket": str(socket), "receiver_id": "receiver:eval-inventory",
            "service_uid": broker_uid, "service_gid": broker_uid,
        }))
        write_token(token_file, token, reader_uid, client_gid)

        broker_args = [
            binaries / "agenturm-storage-service", "--data-root", data, "--transport-dir", transport,
            "--receiver-id", "receiver:eval-inventory", "--client-uid", str(collector_uid),
            "--client-gid", str(client_gid), "--native-reader", f"{reader_uid}:{client_gid}",
            "--native-maintenance", f"{maintenance_uid}:{client_gid}",
            "--native-sources", f"{PRIMARY},{SIBLING},{CAPACITY}",
            "--native-backup-root", backups,
        ]

        def command(verb, *args):
            return [binaries / "agenturm", "broker", verb, "--broker-config", config, *args]

        def run(uid, args, code=0, gid=client_gid):
            output = subprocess.run(args, cwd=root, capture_output=True, text=True, user=uid,
                                    group=gid, extra_groups=[], timeout=12)
            assert output.returncode == code, (args, output.stdout, output.stderr)
            if code:
                assert not output.stdout, output.stdout
            return output.stdout

        def start_broker():
            service = subprocess.Popen(
                broker_args, cwd=root, user=broker_uid, group=broker_uid, extra_groups=[],
            )
            processes.append(service)
            wait_for(lambda: socket.exists() and stat.S_IMODE(socket.stat().st_mode) == 0o660,
                     service, "Broker socket unavailable")
            return service

        def start_gateway(listen="127.0.0.1:0"):
            gateway = subprocess.Popen(
                command("serve-history", "--listen", listen, "--token-file", token_file),
                cwd=root, user=reader_uid, group=client_gid, extra_groups=[],
                text=True, stdout=subprocess.PIPE,
            )
            processes.append(gateway)
            assert select.select([gateway.stdout], [], [], 8)[0]
            banner = gateway.stdout.readline().strip()
            assert banner.startswith("Listening for history reads at http://127.0.0.1:"), banner
            return gateway, banner.split()[-1]

        def trace_batch(source, traces, end):
            spans = [{
                "traceId": trace, "spanId": trace[-16:], "name": "fixture",
                "startTimeUnixNano": str(end - 1), "endTimeUnixNano": str(end),
                "status": {"code": 1},
                "attributes": [{"key": "session.id", "value": {"stringValue": "fixture"}}],
            } for trace in traces]
            fixture.write_text(json.dumps({"resourceSpans": [{"scopeSpans": [{"spans": spans}]}]}))
            run(collector_uid, command("import-traces", "--source-id", source, "--file", fixture))

        def eval_batch(source, results, detail=False):
            fixture.write_text(json.dumps({"results": results}))
            args = ["--source-id", source, "--file", fixture]
            if detail:
                args.append("--include-detail")
            return run(collector_uid, command("import-evals", *args))

        def inventory(*sources, code=0):
            flags = [flag for source in sources for flag in ("--source-id", source)]
            return run(reader_uid, command("evals", "--list", "--json", *flags), code=code)

        processes = []
        try:
            broker = start_broker()
            gateway, endpoint = start_gateway()
            target = f"/api/outcomes?source_ids={SIBLING},{PRIMARY}"
            assert request_get(endpoint, None, target)[0] == 401
            assert request_get(endpoint, token, target)[0] == 428
            status, health = request_get(endpoint, token, "/api/health")
            assert status == 200
            binding = health["history_identity"]
            assert request_get(endpoint, token, target + "&limit=1", binding)[0] == 400
            assert request_get(endpoint, token, target + ",otlp.denied", binding)[0] == 403
            trace_batch(PRIMARY, [TRACE], 200)
            trace_batch(SIBLING, [TRACE], 600)
            original = {
                "trace_id": TRACE, "passed": False, "scores": [{"scorer": "zero", "score": 0}],
            }
            correction = {**original, "passed": True, "error": PRIVATE}
            sibling = {
                "trace_id": TRACE, "passed": False, "scores": [{"scorer": "unknown", "passed": False}],
            }
            assert "Imported 1" in eval_batch(PRIMARY, [original])
            assert "Corrected 1" in eval_batch(PRIMARY, [correction], detail=True)
            assert "Imported 1" in eval_batch(SIBLING, [sibling])
            local = json.loads(inventory(PRIMARY, SIBLING))
            inventory(PRIMARY, "otlp.denied", code=1)
            status, received = request_get(endpoint, token, target, binding)
            assert status == 200, (status, received)
            assert received["evaluation_metadata"] == local["evaluation_metadata"] == "current"
            assert received["source_ids"] == [PRIMARY, SIBLING]
            assert received["trace_evidence_snapshot"] == str(local["trace_evidence_snapshot"])
            assert [(row["trace_source_id"], row["revision"]) for row in received["outcomes"]] == [(PRIMARY, "2"), (SIBLING, "1")]
            assert received["outcomes"][0]["outcome"]["scores"][0]["score"] == 0
            assert received["outcomes"][1]["outcome"]["scores"][0]["score"] is None
            for response in [local, received]:
                assert all(marker not in json.dumps(response) for marker in [PRIVATE, "detail_selection", "receipt_id"])
            backup = json.loads(run(maintenance_uid, command("backup")))["name"]
            run(maintenance_uid, command("prune", "--before-unix-ns", "250"))
            assert [row["trace_source_id"] for row in request_get(endpoint, token, target, binding)[1]["outcomes"]] == [SIBLING]
            stop(gateway)
            stop(broker)
            data.rename(root / "before-restore")
            private_directory(data, broker_uid, broker_uid)
            run(broker_uid, [*broker_args, "--native-restore", backup], gid=broker_uid)
            start_broker()
            _, restored = start_gateway(urlsplit(endpoint).netloc)
            assert restored == endpoint
            assert request_get(restored, token, target, binding)[0] == 409
            fresh = request_get(restored, token, "/api/health")[1]["history_identity"]
            status, result = request_get(restored, token, target, fresh)
            assert status == 200 and result["outcomes"] == received["outcomes"]
            assert result["trace_evidence_snapshot"] != received["trace_evidence_snapshot"]
            for start, end in [(1, 513), (513, 1026)]:
                traces = [f"{index:032x}" for index in range(start, end)]
                trace_batch(CAPACITY, traces, 700)
                eval_batch(CAPACITY, [{"trace_id": trace, "passed": True} for trace in traces])
            status, error = request_get(restored, token, f"/api/outcomes?source_ids={CAPACITY}", fresh)
            assert status == 413 and "outcomes" not in error, (status, error)
            inventory(CAPACITY, code=1)
            print(
                "PASS eval inventory: separate UIDs, complete scoped revisions, private exclusion, "
                "bearer and bindings, restore, prune, and HTTP 413", flush=True,
            )
        finally:
            for process in reversed(processes):
                stop(process)


if __name__ == "__main__":
    main()
