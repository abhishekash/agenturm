"""Exercise retained Rust Host metrics and the real process drill-down in a Linux guest."""

import argparse
import fcntl
import grp
import json
import os
from pathlib import Path
import pty
import pwd
import re
import secrets
import select
import stat
import struct
import subprocess
import sys
import tempfile
import termios
import time

from validate_rust_remote_history import credentials, private_directory, request, stop, wait_for, write_token


SOURCE = "host.guest"
PRIVATE_SENTINEL = "PRIVATE_HOST_TEST_ARG_ENV"


def read_line(process):
    assert select.select([process.stdout], [], [], 10)[0], "Expected a committed host sample"
    return process.stdout.readline().strip()


def title_line(output):
    # Ratatui writes only changed cells. Reconstruct the title across frames.
    title = [' '] * 60
    row = column = 1
    tokens = re.split(rb'(\x1b\[[0-?]*[ -/]*[@-~])', output)
    for token in tokens:
        cursor = re.fullmatch(rb'\x1b\[(\d+);(\d+)H', token)
        if cursor:
            row, column = map(int, cursor.groups())
        elif token == b'\x1b[2J':
            title = [' '] * 60
        elif row == 1 and not token.startswith(b'\x1b'):
            for character in token.decode(errors='replace'):
                if 1 <= column <= len(title) and character.isprintable():
                    title[column - 1] = character
                column += 1
    return ''.join(title)


def read_screen(master, process, output, text):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if select.select([master], [], [], 0.1)[0]:
            output.extend(os.read(master, 8192))
        assert PRIVATE_SENTINEL.encode() not in output
        if text in title_line(output):
            return
        assert process.poll() is None, output
    raise AssertionError((text, output))


def narrow_cockpit(command, root, uid, gid, viewer_home):
    env = {**os.environ, "HOME": str(viewer_home), "XDG_STATE_HOME": str(viewer_home / "state"),
           "TERM": "xterm-256color"}
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 18, 60, 0, 0))
    original = termios.tcgetattr(slave)
    process = subprocess.Popen(command, cwd=root, env=env, stdin=slave, stdout=slave, stderr=slave,
                               user=uid, group=gid, extra_groups=[])
    try:
        output = bytearray()
        read_screen(master, process, output, "Agenturm")
        os.write(master, b"6")
        read_screen(master, process, output, "Host history")
        os.write(master, b"\r")
        read_screen(master, process, output, "Host processes")
        os.write(master, b"\r")
        read_screen(master, process, output, "Process history")
        os.write(master, b"\x1b")
        read_screen(master, process, output, "Host processes")
        os.write(master, b"\x1b")
        read_screen(master, process, output, "Host history")
        os.write(master, b"q")
        assert process.wait(timeout=5) == 0
        assert termios.tcgetattr(slave) == original
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        os.close(master)
        os.close(slave)


def fake_harness(root, uid, gid, name):
    process = subprocess.Popen([sys.executable, "-c",
        "from pathlib import Path; import sys,time; Path('/proc/self/comm').write_text(sys.argv[1]); "
        "print('ready', flush=True); time.sleep(60)", name, PRIVATE_SENTINEL],
        cwd=root, stdout=subprocess.PIPE, text=True, user=uid, group=gid, extra_groups=[],
        env={**os.environ, "PRIVATE_HOST_TEST": PRIVATE_SENTINEL})
    assert read_line(process) == "ready"
    return process


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("Run inside a disposable Linux guest with separate UID support.")
    occupied_uids = {entry.pw_uid for entry in pwd.getpwall()}
    occupied_gids = {entry.gr_gid for entry in grp.getgrall()}
    broker, collector, reader = [uid for uid in range(45000, 46000) if uid not in occupied_uids][:3]
    client_gid = next(gid for gid in range(45000, 46000) if gid not in occupied_gids)
    app = args.bin_dir.resolve(strict=True) / "agenturm"
    with tempfile.TemporaryDirectory(prefix="agenturm-host-") as name:
        root = Path(name)
        root.chmod(0o755)
        data, transport, viewer = (root / child for child in ["data", "transport", "viewer"])
        private_directory(data, broker, broker)
        private_directory(transport, broker, client_gid, 0o2710)
        private_directory(viewer, reader, client_gid)
        socket = transport / "agenturm-native-storage-v1.sock"
        database = data / "history.sqlite"
        config = root / "broker.json"
        config.write_text(json.dumps({"version": 1, "socket": str(socket),
            "receiver_id": "receiver:host-fixture", "service_uid": broker, "service_gid": broker}))
        config.chmod(0o644)
        token_path = root / "reader-token"
        token = secrets.token_hex(32)
        write_token(token_path, token, reader, client_gid)

        def command(*args):
            return [app, "broker", *args, "--broker-config", config]

        def run(uid, *args, code=0):
            result = subprocess.run(command(*args), cwd=root, capture_output=True, text=True,
                                    user=uid, group=client_gid, extra_groups=[], timeout=10)
            assert result.returncode == code, (args, result.stdout, result.stderr)
            assert PRIVATE_SENTINEL not in result.stdout
            return result.stdout

        service = subprocess.Popen([
            app.with_name("agenturm-storage-service"), "--data-root", data,
            "--transport-dir", transport, "--receiver-id", "receiver:host-fixture",
            "--client-uid", str(collector), "--client-gid", str(client_gid),
            "--native-reader", f"{reader}:{client_gid}", "--native-sources", SOURCE,
        ], cwd=root, preexec_fn=credentials(broker, broker))
        watcher = gateway = shared_process = other_process = None
        try:
            wait_for(lambda: socket.exists() and database.exists() and
                     stat.S_IMODE(socket.stat().st_mode) == 0o660, service, "Host broker unavailable")
            shared_process = fake_harness(root, collector, client_gid, "codex")
            other_process = fake_harness(root, reader, client_gid, "claude")
            assert not run(reader, "host", "record", "--source-id", SOURCE, code=1)
            assert not run(collector, "host", "record", "--source-id", "host.outside", code=1)
            assert json.loads(run(reader, "host", "history"))["samples"] == []

            watcher = subprocess.Popen(command("host", "record", "--source-id", SOURCE,
                "--watch", "--interval-ms", "1000"), cwd=root, stdout=subprocess.PIPE, text=True,
                user=collector, group=client_gid, extra_groups=[])
            receipts = [json.loads(read_line(watcher)) for _ in range(2)]
            stop(watcher)
            assert all(receipt["inserted"] == 1 for receipt in receipts)
            result = json.loads(run(reader, "host", "history", "--source-id", SOURCE))
            assert len(result["samples"]) >= 2
            process_keys = set()
            for row in result["samples"]:
                assert row["identity"]["scope"] == "source" and row["identity"]["session_id"] is None
                sample = row["kind"]
                assert sample["kind"] == "host_sample"
                assert sample["memory_total_bytes"] > 0
                assert sample["clock_ticks_per_second"] > 0
                assert sample["cpu"]["user_ticks"] is not None
                assert other_process.pid not in [item["pid"] for item in sample["processes"]]
                match = next(item for item in sample["processes"] if item["pid"] == shared_process.pid)
                assert match["runtime"] == "codex" and match["resident_bytes"] > 0
                process_keys.add((sample["boot_id"], match["pid"], match["start_ticks"]))
            assert len(process_keys) == 1
            first = json.loads(run(reader, "host", "history", "--limit", "1"))
            older = json.loads(run(reader, "host", "history", "--limit", "1",
                                   "--before", json.dumps(first["older"])))
            assert first["snapshot_id"] == older["snapshot_id"]
            assert first["samples"][0]["event_id"] != older["samples"][0]["event_id"]
            assert json.loads(run(reader, "query"))["observations"] == []

            managed = root / "host-only.json"
            managed.write_text(json.dumps({"version": 2, "host": {
                "source_id": SOURCE, "interval_ms": 1000}}))
            assert "host capture failed" in run(reader, "collect", "--config", managed, code=1)

            def sample_ids():
                samples = json.loads(run(reader, "host", "history", "--source-id", SOURCE))["samples"]
                return {row["event_id"] for row in samples}

            initial_ids = {row["event_id"] for row in result["samples"]}
            assert sample_ids() == initial_ids
            for _ in range(2):
                before = sample_ids()
                watcher = subprocess.Popen(command("collect", "--config", managed, "--watch",
                    "--interval-ms", "100"), cwd=root, stdout=subprocess.DEVNULL,
                    user=collector, group=client_gid, extra_groups=[])
                wait_for(lambda: len(sample_ids() - before) >= 2, watcher,
                         "Managed collection did not record on schedule")
                stop(watcher)
                assert before < sample_ids()

            gateway = subprocess.Popen(command("serve-history", "--listen", "127.0.0.1:0",
                "--token-file", token_path), cwd=root, stdout=subprocess.PIPE, text=True,
                user=reader, group=client_gid, extra_groups=[])
            endpoint = read_line(gateway).split()[-1]
            _, identity = request(endpoint, token, {"op": "identity"})
            read = {"op": "history", "query": {"limit": 20}, "selection": {"kind": "observations"}}
            key = identity["history_identity"]
            _, unscoped = request(endpoint, token, read, key)
            assert unscoped["result"]["Ok"]["value"]["observations"] == []
            read["query"]["harness_id"] = SOURCE
            assert request(endpoint, token, read, key)[1]["result"] == {"Err": {"code": "unauthorized"}}
            assert request(endpoint, token, {"op": "query_host_history"}, key)[0] == 400

            probe = subprocess.run([sys.executable, "-c", "import sys; open(sys.argv[1])", database],
                                   user=reader, group=client_gid, extra_groups=[], capture_output=True, timeout=5)
            assert probe.returncode != 0 and b"PermissionError" in probe.stderr
            narrow_cockpit(command("cockpit"), root, reader, client_gid, viewer)
            print("PASS Host watch and managed restart, separate reader, process identity, HTTP exclusion and PTY drill-down", flush=True)
        finally:
            for process in [shared_process, other_process]:
                if process is not None and process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
            for process in [watcher, gateway, service]:
                if process is not None:
                    stop(process)


if __name__ == "__main__":
    main()
