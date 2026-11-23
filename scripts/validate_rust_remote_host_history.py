"""Verify opt-in recorded Host history, its native grants and restored bindings in Linux."""

import argparse
import grp
import json
import os
from pathlib import Path
import pwd
import secrets
import stat
import subprocess
import tempfile
from urllib.parse import urlencode, urlsplit

from validate_rust_host_history import PRIVATE_SENTINEL, fake_harness, narrow_cockpit, read_line
from validate_rust_remote_history import private_directory, request, request_get, stop, wait_for, write_token


SOURCE = "host.remote-fixture"
HIDDEN = "host.revoked-fixture"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("Run inside a disposable Linux guest with support for separate UIDs.")
    used_uids = {entry.pw_uid for entry in pwd.getpwall()}
    used_gids = {entry.gr_gid for entry in grp.getgrall()}
    broker, collector, reader, maintainer, viewer = [
        uid for uid in range(45000, 46000) if uid not in used_uids
    ][:5]
    client_gid, viewer_gid = [gid for gid in range(45000, 46000) if gid not in used_gids][:2]
    app = args.bin_dir.resolve(strict=True) / "agenturm"
    with tempfile.TemporaryDirectory(prefix="agenturm-remote-host-") as path:
        root = Path(path)
        root.chmod(0o755)
        data, transport, home, backups = (root / item for item in ("data", "transport", "home", "backups"))
        private_directory(data, broker, broker)
        private_directory(backups, broker, broker)
        private_directory(transport, broker, client_gid, 0o2710)
        private_directory(home, viewer, viewer_gid)
        socket = transport / "agenturm-native-storage-v1.sock"
        database = data / "history.sqlite"
        config = root / "broker.json"
        config.write_text(json.dumps({"version": 1, "socket": str(socket),
            "receiver_id": "receiver:remote-host", "service_uid": broker, "service_gid": broker}))
        config.chmod(0o644)
        token = secrets.token_hex(32)
        server_token, client_token = root / "reader-token", home / "reader-token"
        write_token(server_token, token, reader, client_gid)
        write_token(client_token, token, viewer, viewer_gid)

        def command(*argv):
            return [app, "broker", *argv, "--broker-config", config]

        def checked(uid, argv, gid=client_gid):
            result = subprocess.run(argv, cwd=root, capture_output=True, text=True,
                                    user=uid, group=gid, extra_groups=[], timeout=12)
            assert result.returncode == 0, (argv[1:3], result.stderr)
            assert PRIVATE_SENTINEL not in result.stdout
            return result.stdout

        def service_command(sources):
            return [app.with_name("agenturm-storage-service"), "--data-root", data,
                    "--transport-dir", transport, "--receiver-id", "receiver:remote-host",
                    "--client-uid", str(collector), "--client-gid", str(client_gid),
                    "--native-reader", f"{reader}:{client_gid}",
                    "--native-maintenance", f"{maintainer}:{client_gid}",
                    "--native-sources", sources, "--native-backup-root", backups]

        def start_service(sources):
            process = subprocess.Popen(service_command(sources), cwd=root,
                                       user=broker, group=broker, extra_groups=[])
            try:
                wait_for(lambda: socket.exists() and database.exists() and
                         stat.S_IMODE(socket.stat().st_mode) == 0o660,
                         process, "Remote Host broker unavailable")
                return process
            except BaseException:
                stop(process)
                raise

        def start_gateway(include_host, listen="127.0.0.1:0"):
            flags = ["--include-host"] if include_host else []
            process = subprocess.Popen(command("serve-history", "--listen", listen,
                "--token-file", server_token, *flags), cwd=root, stdout=subprocess.PIPE,
                text=True, user=reader, group=client_gid, extra_groups=[])
            try:
                return process, read_line(process).split()[-1]
            except BaseException:
                stop(process)
                raise

        def record(source):
            return json.loads(checked(collector, command("host", "record", "--source-id", source)))

        def host_request(**query):
            return {"op": "host_history", "query": {"limit": 64, **query}}

        service = start_service(f"{SOURCE},{HIDDEN}")
        gateway = harness = None
        try:
            harness = fake_harness(root, collector, client_gid, "codex")
            first = record(SOURCE)
            second = record(SOURCE)
            record(HIDDEN)
            saved = json.loads(checked(maintainer, command("backup")))
            stop(service)
            service = start_service(SOURCE)

            gateway, endpoint = start_gateway(False)
            _, identity = request(endpoint, token, {"op": "identity"})
            binding = identity["history_identity"]
            assert request_get(endpoint, token, "/api/host", binding)[0] == 403
            assert request(endpoint, token, host_request(), binding)[1]["result"] == {
                "Err": {"code": "host_history_disabled"}}
            assert request(endpoint, token, {"op": "query_host_history"}, binding)[0] == 400
            stop(gateway)
            gateway, endpoint = start_gateway(True)
            assert request_get(endpoint, None, "/api/host", binding)[0] == 401
            assert request_get(endpoint, token, "/api/host", None)[0] == 428
            assert request_get(endpoint, token, "/api/host", "0" * 32)[0] == 409
            assert request_get(endpoint, token, f"/api/host?source_id={HIDDEN}", binding)[0] == 403
            assert request_get(endpoint, token, "/api/host?session_id=unrelated", binding)[0] == 400
            assert request(endpoint, token, host_request(harness_id=HIDDEN), binding)[1]["result"] == {
                "Err": {"code": "unauthorized"}}
            for route in ["activity", "traces", "mcp", "metrics", "hierarchy"]:
                code, output = request_get(endpoint, token, f"/api/{route}", binding)
                assert code == 200 and output["observations"] == []
            assert request_get(endpoint, token, f"/api/activity?source_id={SOURCE}", binding)[0] == 403
            assert request_get(endpoint, token, "/api/sessions", binding)[1]["sessions"] == []
            ordinary = {"op": "history", "query": {"limit": 20}, "selection": {"kind": "observations"}}
            assert request(endpoint, token, ordinary, binding)[1]["result"]["Ok"]["value"]["observations"] == []
            assert request(endpoint, token, {"op": "host_record", "source_id": SOURCE}, binding)[0] == 400

            code, page = request_get(endpoint, token, f"/api/host?source_id={SOURCE}&limit=1", binding)
            assert code == 200 and page["has_more"] and page["next_cursor"]
            assert page["samples"][0]["event_id"] == second["event_id"]
            data_sample = page["samples"][0]["kind"]
            assert isinstance(data_sample["memory_total_bytes"], str)
            assert isinstance(data_sample["clock_ticks_per_second"], str)
            assert isinstance(data_sample["cpu"]["user_ticks"], str)
            assert PRIVATE_SENTINEL not in json.dumps(page)
            position = urlencode({"source_id": SOURCE, "cursor": page["next_cursor"], "limit": 1})
            older = request_get(endpoint, token, f"/api/host?{position}", binding)[1]
            assert older["snapshot_id"] == page["snapshot_id"]
            assert older["samples"][0]["event_id"] == first["event_id"]
            assert not older["has_more"]
            _, read = request(endpoint, token, host_request(), binding)
            retained = read["result"]["Ok"]["value"]["observations"]
            assert {row["event_id"] for row in retained} == {first["event_id"], second["event_id"]}
            assert all(row["harness_id"] == SOURCE and row["identity"]["session_id"] is None
                       for row in retained)
            assert PRIVATE_SENTINEL not in json.dumps(retained)
            narrow_cockpit([app, "cockpit", "--endpoint", endpoint, "--token-file", client_token],
                           root, viewer, viewer_gid, home)

            rolled_back = record(SOURCE)
            _, old = request(endpoint, token, host_request(), binding)
            watermark = old["result"]["Ok"]["value"]["snapshot_id"]
            stop(service)
            data.rename(root / "before-restore")
            private_directory(data, broker, broker)
            checked(broker, [*service_command(SOURCE), "--native-restore", saved["name"]], gid=broker)
            service = start_service(SOURCE)
            regrown = record(SOURCE)
            assert request_get(endpoint, token, "/api/host", binding)[0] == 409
            assert request(endpoint, token, host_request(), binding)[1]["result"] == {
                "Err": {"code": "snapshot_replaced"}}
            stop(gateway)
            gateway, endpoint = start_gateway(True, urlsplit(endpoint).netloc)
            _, new_identity = request(endpoint, token, {"op": "identity"})
            new_binding = new_identity["history_identity"]
            assert new_binding.startswith(binding + ":")
            assert request_get(endpoint, token, "/api/host", binding)[0] == 409
            assert request_get(endpoint, token, f"/api/host?snapshot_id={watermark}", new_binding)[0] == 409
            assert request(endpoint, token, host_request(snapshot_id=watermark), new_binding)[1]["result"] == {
                "Err": {"code": "snapshot_replaced"}}
            code, fresh_page = request_get(endpoint, token, "/api/host", new_binding)
            assert code == 200
            assert fresh_page["snapshot_id"] == f'r1:{new_binding.split(":")[1]}:{watermark}'
            events = {row["event_id"] for row in fresh_page["samples"]}
            assert events == {first["event_id"], second["event_id"], regrown["event_id"]}
            assert rolled_back["event_id"] not in events
            print("PASS opt-in Host GET/POST, exact grants, remote PTY drill-down and restored watermark fencing",
                  flush=True)
        finally:
            if harness is not None and harness.poll() is None:
                harness.terminate()
                harness.wait(timeout=5)
            if gateway is not None:
                stop(gateway)
            stop(service)


if __name__ == "__main__":
    main()
