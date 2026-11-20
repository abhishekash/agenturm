"""Root-required, disposable two-account proof for experimental v2 scratch dispatch.

Run only in an isolated CI VM or container. Numeric identities and ownership
changes are confined to a temporary directory; no host accounts are created.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import signal
import socket
import sqlite3
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

from validate_rust_storage_service_accounts import (check, drop_to, identities,
                                                     read_exact, wait_exit, wait_socket)


SOURCE = "scratch-source"
PLUGIN = "fixture.plugin"
GENERATION = "a" * 64
REQUEST_ID = "1" * 32
SOCKET_NAME = "agenturm-storage-v1.sock"


def seed(data: Path, service_uid: int, service_gid: int) -> dict:
    from observatory.core import Store
    from observatory.plugin_admission import _validated, activate_source
    from observatory.plugin_wire import PollBatch

    path = data / "scratch.db"
    store = Store(path)
    try:
        lease = activate_source(store, plugin_id=PLUGIN, source_id=SOURCE,
                                generation=GENERATION)
        with store._connect() as db:
            db.execute("INSERT INTO schema_meta(key,value) VALUES(?,?)",
                       ("storage_admission_scratch_v1", "enabled"))
        batch = PollBatch(SOURCE, "one", ({"event_id": "first", "revision": 0,
            "kind": "lifecycle", "time_ns": "9007199254740993",
            "payload": {"state": "started"},
            "identity": {"session_id": "native-one"}},))
        digest, rows, checked = _validated(batch, SOURCE)
        request = {"version": 2, "request_id": REQUEST_ID,
            "receiver_id": lease.receiver_id,
            "operation": {"op": "admit_lifecycle", "lease": {
                "receiver_id": lease.receiver_id, "plugin_id": lease.plugin_id,
                "source_id": lease.source_id, "generation": lease.generation,
                "epoch": lease.epoch, "policy_epoch": lease.policy_epoch},
                "expected_cursor": "", "cursor": checked.cursor,
                "batch_digest": digest,
                "records": [{"event_id_hash": row[1], "revision": row[2],
                    "kind": row[3], "time_ns": row[4], "payload_json": row[5],
                    "record_digest": row[6], "native_session_id": row[7],
                    "native_worker_id": row[8], "native_turn_id": row[9],
                    "native_operation_id": row[10]} for row in rows]}}
    finally:
        store.close()
    for item in data.iterdir():
        os.chown(item, service_uid, service_gid)
        item.chmod(0o600)
    return request


def frame(payload: dict) -> bytes:
    raw = json.dumps(payload, separators=(",", ":")).encode()
    return struct.pack(">I", len(raw)) + raw


def child(args: argparse.Namespace) -> None:
    if args.case == "store":
        from observatory.core import Store
        from observatory.store_authority import StoreAuthorityError
        try:
            Store(Path(args.data) / "scratch.db")
        except (PermissionError, StoreAuthorityError, OSError):
            pass
        else:
            raise AssertionError("client opened service-owned scratch database")
        try:
            os.open(Path(args.data) / "scratch.db", os.O_RDWR)
        except PermissionError:
            return
        raise AssertionError("client opened protected scratch file directly")

    request = json.loads(Path(args.request).read_text(encoding="utf-8"))
    if args.case == "conflict":
        request["operation"]["cursor"] = "changed"
    elif args.case == "wrong_receiver":
        request["receiver_id"] = "receiver:other"
        request["operation"]["lease"]["receiver_id"] = "receiver:other"
    elif args.case == "v1_probe":
        request = {"version": 1, "request_id": REQUEST_ID,
                   "receiver_id": request["receiver_id"],
                   "operation": {"op": "probe", "topic": "protocol"}}
    elif args.case in ("drop_reply", "replay_second"):
        request["request_id"] = "2" * 32
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(3)
        client.connect(args.socket)
        try:
            client.sendall(struct.pack(">I", 65537) if args.case == "oversized"
                           else frame(request))
            client.shutdown(socket.SHUT_WR)
        except (BrokenPipeError, ConnectionResetError):
            check(args.case not in ("valid", "replay", "drop_reply", "replay_second"),
                  "accepted peer disconnected")
            return
        if args.case == "drop_reply":
            return
        if args.case in ("valid", "replay", "replay_second"):
            length = struct.unpack(">I", read_exact(client, 4))[0]
            check(0 < length <= 65536, "invalid reply size")
            reply = json.loads(read_exact(client, length))
            expected_digest = hashlib.sha256(json.dumps(
                request, sort_keys=True, separators=(",", ":"),
                ensure_ascii=False).encode()).hexdigest()
            check(reply["request_digest"] == expected_digest, "unbound request reply")
            check(reply["request_id"] == request["request_id"],
                  "reply request ID changed")
            check(reply["receiver_id"] == request["receiver_id"],
                  "reply receiver changed")
            check(reply["result"]["disposition"] ==
                  ("committed" if args.case == "valid" else "replay"),
                  f"wrong disposition: {reply!r}")
            check(reply["result"]["cursor"] == "one", "wrong reply cursor")
            check(client.recv(1) == b"", "reply connection remained open")
        else:
            try:
                unexpected = client.recv(1)
            except ConnectionResetError:
                unexpected = b""
            check(unexpected == b"", f"{args.case} received a reply")


def run_child(case: str, *, uid: int, gid: int, root: Path,
              supplementary: tuple[int, ...] = ()) -> None:
    env = os.environ.copy()
    fixture = root / Path(__file__).name
    env["PYTHONPATH"] = str(root)
    result = subprocess.run([sys.executable, str(fixture), "--case", case,
        "--socket", str(root / "transport" / SOCKET_NAME),
        "--data", str(root / "private-data"), "--request", str(root / "request.json")],
        preexec_fn=drop_to(uid, gid, supplementary), env=env,
        text=True, capture_output=True, timeout=8)
    check(result.returncode == 0,
          f"{case} failed: {result.stdout}{result.stderr}")
    print(f"PASS v2 {case}: uid={uid} gid={gid}", flush=True)


def run_rust_client(binary: Path, request: dict, *, socket_path: Path,
                    service_uid: int, service_gid: int,
                    client_uid: int, client_gid: int,
                    expect_denied: bool = False) -> None:
    result = subprocess.run(
        [str(binary), str(socket_path), str(service_uid), str(service_gid)],
        input=json.dumps(request, separators=(",", ":")), text=True,
        preexec_fn=drop_to(client_uid, client_gid), capture_output=True, timeout=8)
    if expect_denied:
        check(result.returncode != 0 and not result.stdout and
              "peer denied" in result.stderr,
              f"Rust client sent to the wrong service: {result.stdout}{result.stderr}")
        print("PASS v2 Rust client denied wrong service identity", flush=True)
        return
    check(result.returncode == 0,
          f"Rust v2 client failed: {result.stdout}{result.stderr}")
    reply = json.loads(result.stdout)
    expected_digest = hashlib.sha256(json.dumps(
        request, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()
    check(reply["request_digest"] == expected_digest and
          reply["request_id"] == request["request_id"] and
          reply["receiver_id"] == request["receiver_id"] and
          reply["result"]["disposition"] == "replay" and
          reply["result"]["cursor"] == "one",
          f"Rust client accepted an unbound reply: {reply!r}")
    print("PASS v2 Rust client across distinct accounts", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", type=Path)
    parser.add_argument("--rust-client-bin", type=Path)
    parser.add_argument("--case", choices=("store", "valid", "replay", "conflict",
                                           "wrong_receiver", "wrong_uid", "wrong_gid",
                                           "same_uid", "v1_probe", "oversized",
                                           "tampered_path", "drop_reply",
                                           "replay_second"))
    parser.add_argument("--socket")
    parser.add_argument("--data")
    parser.add_argument("--request")
    args = parser.parse_args()
    if args.case:
        child(args)
        return
    check(sys.platform == "linux" and os.geteuid() == 0,
          "run this fixture as root in an isolated Linux VM or container")
    check(args.bin is not None and args.bin.is_file(), "pass a built service")
    binary_source = args.bin.resolve()
    rust_client_source = args.rust_client_bin.resolve() if args.rust_client_bin else None
    if rust_client_source is not None:
        check(rust_client_source.is_file(), "pass a built Rust client")
    service_uid, client_uid, wrong_uid, service_gid, client_gid = identities()
    def drop_service() -> None:
        # Deliberately permissive launcher umask: service must impose privacy.
        os.umask(0)
        drop_to(service_uid, service_gid)()

    with tempfile.TemporaryDirectory(prefix="agenturm-v2-accounts-") as temporary:
        root = Path(temporary)
        root.chmod(0o755)
        fixture = root / Path(__file__).name
        shutil.copy2(Path(__file__).resolve(), fixture)
        fixture.chmod(0o644)
        account_fixture = root / "validate_rust_storage_service_accounts.py"
        shutil.copy2(Path(__file__).resolve().with_name(account_fixture.name),
                     account_fixture)
        account_fixture.chmod(0o644)
        shutil.copytree(Path(__file__).resolve().parents[1] / "observatory",
                        root / "observatory")
        binary = root / "agenturm-storage-service"
        shutil.copy2(binary_source, binary)
        binary.chmod(0o755)
        rust_client = None
        if rust_client_source is not None:
            rust_client = root / "agenturm-v2-client"
            shutil.copy2(rust_client_source, rust_client)
            rust_client.chmod(0o755)
        data = root / "private-data"
        transport = root / "transport"
        data.mkdir(mode=0o700)
        transport.mkdir(mode=0o700)
        os.chown(data, service_uid, service_gid)
        os.chown(transport, service_uid, client_gid)
        transport.chmod(0o2710)
        request = seed(data, service_uid, service_gid)
        (root / "request.json").write_text(json.dumps(request), encoding="utf-8")
        (root / "request.json").chmod(0o644)
        canonical = root / "canonical.db"
        canonical.write_bytes(b"canonical-sentinel-untouched")
        before = canonical.read_bytes()
        command = [str(binary), "--data-root", str(data),
            "--transport-dir", str(transport), "--receiver-id", request["receiver_id"],
            "--client-uid", str(client_uid), "--client-gid", str(client_gid),
            "--max-connections", "13" if rust_client else "11",
            "--experimental-v2-scratch", "enabled"]
        socket_path = transport / SOCKET_NAME
        service = subprocess.Popen(command, preexec_fn=drop_service,
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        try:
            wait_socket(service, socket_path)
            check(stat.S_IMODE(data.stat().st_mode) == 0o700, "data root not private")
            check((data / "scratch.db").stat().st_uid == service_uid,
                  "scratch file not service-owned")
            run_child("store", uid=client_uid, gid=client_gid, root=root)
            run_child("valid", uid=client_uid, gid=client_gid, root=root)
            if rust_client is not None:
                run_rust_client(rust_client, request, socket_path=socket_path,
                                service_uid=wrong_uid, service_gid=service_gid,
                                client_uid=client_uid, client_gid=client_gid,
                                expect_denied=True)
                run_rust_client(rust_client, request, socket_path=socket_path,
                                service_uid=service_uid, service_gid=service_gid,
                                client_uid=client_uid, client_gid=client_gid)
            run_child("replay", uid=client_uid, gid=client_gid, root=root)
            run_child("drop_reply", uid=client_uid, gid=client_gid, root=root)
            run_child("replay_second", uid=client_uid, gid=client_gid, root=root)
            run_child("conflict", uid=client_uid, gid=client_gid, root=root)
            run_child("wrong_receiver", uid=client_uid, gid=client_gid, root=root)
            run_child("v1_probe", uid=client_uid, gid=client_gid, root=root)
            run_child("oversized", uid=client_uid, gid=client_gid, root=root)
            run_child("wrong_uid", uid=wrong_uid, gid=client_gid, root=root)
            run_child("wrong_gid", uid=client_uid, gid=service_gid,
                      supplementary=(client_gid,), root=root)
            run_child("same_uid", uid=service_uid, gid=service_gid, root=root)
            wait_exit(service, socket_path)
            with sqlite3.connect(data / "scratch.db") as db:
                cursor = db.execute("SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 "
                                    "WHERE source_id=?", (SOURCE,)).fetchone()
                evidence = db.execute("SELECT COUNT(*) FROM plugin_evidence_v1 "
                                      "WHERE source_id=?", (SOURCE,)).fetchone()[0]
                receipts = db.execute("SELECT COUNT(*) FROM storage_admission_request_v2"
                                      ).fetchone()[0]
            check(cursor == ("one", 1) and evidence == 1 and receipts == 2,
                  "scratch replay state changed unexpectedly")
            for item in data.iterdir():
                metadata = item.lstat()
                check(stat.S_ISREG(metadata.st_mode) and
                      stat.S_IMODE(metadata.st_mode) == 0o600 and
                      metadata.st_uid == service_uid,
                      f"scratch sidecar owner/mode changed: {item.name}")
            check(canonical.read_bytes() == before, "unrelated canonical file changed")
            print("PASS v2 scratch state and unrelated canonical sentinel", flush=True)
        finally:
            if service.poll() is None:
                service.send_signal(signal.SIGTERM)
                service.communicate(timeout=5)
        # A replaced database pathname is rejected before reading another
        # admitted client's bytes, even if the replacement is also private.
        service = subprocess.Popen(command[:-4] + command[-2:],
            preexec_fn=drop_service, stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE, text=True)
        try:
            wait_socket(service, socket_path)
            archived = root / "archived-scratch.db"
            scratch = data / "scratch.db"
            scratch.rename(archived)
            shutil.copyfile(archived, scratch)
            os.chown(scratch, service_uid, service_gid)
            scratch.chmod(0o600)
            run_child("tampered_path", uid=client_uid, gid=client_gid, root=root)
            _, error = service.communicate(timeout=5)
            check(service.returncode != 0 and "identity" in error,
                  "scratch pathname replacement did not stop service")
            check(not socket_path.exists(), "socket survived failed identity check")
            scratch.unlink()
            archived.rename(scratch)
            print("PASS replaced scratch pathname rejected before dispatch", flush=True)
        finally:
            if service.poll() is None:
                service.send_signal(signal.SIGTERM)
                service.communicate(timeout=5)
        # An extra file inside the private root is never treated as a second DB.
        extra = data / "canonical.db"
        extra.write_bytes(b"do-not-touch")
        os.chown(extra, service_uid, service_gid)
        extra.chmod(0o600)
        rejected = subprocess.run(command, preexec_fn=drop_service,
                                  text=True, capture_output=True, timeout=5)
        check(rejected.returncode != 0 and not socket_path.exists(),
              "service accepted an extra data-root file")
        check(extra.read_bytes() == b"do-not-touch", "extra file changed")
        print("PASS extra private-root file rejected before listener", flush=True)
        extra.unlink()
        with sqlite3.connect(data / "scratch.db") as db:
            db.execute("DELETE FROM schema_meta WHERE key=?",
                       ("storage_admission_scratch_v1",))
        rejected = subprocess.run(command, preexec_fn=drop_service,
                                  text=True, capture_output=True, timeout=5)
        check(rejected.returncode != 0 and not socket_path.exists(),
              "service accepted a database without the scratch marker")
        print("PASS missing scratch marker rejected before listener", flush=True)


if __name__ == "__main__":
    main()
