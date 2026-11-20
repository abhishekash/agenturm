"""Root-only two-UID proof for feature-off real Store clone activation."""
from __future__ import annotations

from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import struct
import subprocess
import sys
import tempfile

from validate_rust_storage_service_accounts import check, drop_to, identities, read_exact, wait_socket


def send(socket_path: Path, request: dict, *,
         expect_reply: bool = True, discard: bool = False) -> dict | None:
    raw = json.dumps(request, separators=(",", ":")).encode()
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(4)
        # This helper is called in a forked process below; do not change the
        # test coordinator's identity.
        connection.connect(str(socket_path))
        try:
            connection.sendall(struct.pack(">I", len(raw)) + raw)
            connection.shutdown(socket.SHUT_WR)
        except (BrokenPipeError, ConnectionResetError):
            check(not expect_reply, "accepted activation peer disconnected")
            return None
        if discard:
            return None
        try:
            prefix = connection.recv(4)
        except ConnectionResetError:
            prefix = b""
        if not expect_reply:
            check(prefix == b"", "rejected activation received a reply")
            return None
        check(len(prefix) == 4, "activation reply missing")
        size = struct.unpack(">I", prefix)[0]
        check(0 < size <= 65536, "activation reply exceeds contract")
        reply = json.loads(read_exact(connection, size))
        check(reply["request_id"] == request["request_id"]
              and reply["receiver_id"] == request["receiver_id"], "unbound activation reply")
        return reply


def client_case(args) -> None:
    request = json.loads(Path(args.request).read_text())
    if args.case == "conflict":
        request["operation"]["generation"] = "b" * 64
    elif args.case == "existing":
        request["request_id"] = "3" * 32
        request["operation"]["source_id"] = "existing-source"
    elif args.case == "revoked":
        request["request_id"] = "4" * 32
        request["operation"]["source_id"] = "existing-source"
    elif args.case == "private_fingerprint":
        request["request_id"] = "5" * 32
        request["operation"]["source_id"] = "private-source"
    elif args.case == "private_owner":
        request["request_id"] = "6" * 32
        request["operation"]["source_id"] = "owned-source"
    elif args.case == "wrong_receiver":
        request["receiver_id"] = "receiver:wrong"
    elif args.case == "non_activation":
        request["operation"] = {"op": "revoke_source", "lease": {
            "receiver_id": request["receiver_id"], "plugin_id": "fixture.plugin",
            "source_id": "real-source", "generation": "a" * 64,
            "epoch": 1, "policy_epoch": 1}, "cursor": "", "expected_dirty_seq": 0}
    elif args.case in {"drop_reply", "replay_second"}:
        request["request_id"] = "2" * 32
    try:
        reply = send(Path(args.socket), request,
                     expect_reply=args.case in {"valid", "replay", "replay_second", "existing"},
                     discard=args.case == "drop_reply")
    except (ConnectionRefusedError, FileNotFoundError):
        if args.case != "path_swap":
            raise
        reply = None
    if reply is not None:
        expected = "committed" if args.case in {"valid", "existing"} else "replay"
        check(reply["result"]["disposition"] == expected, "wrong activation disposition")
        check(reply["result"]["lease"]["epoch"] > 0, "invalid activation lease")
        check(reply["result"]["cursor"] == "", "unexpected activation cursor")


def main() -> None:
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", type=Path)
    parser.add_argument("--case")
    parser.add_argument("--socket")
    parser.add_argument("--request")
    args = parser.parse_args()
    if args.case:
        client_case(args)
        return
    check(sys.platform == "linux" and os.geteuid() == 0, "run as root on isolated Linux")
    binary = args.bin.resolve(strict=True)
    service_uid, client_uid, wrong_uid, service_gid, client_gid = identities()
    with tempfile.TemporaryDirectory(prefix="agenturm-real-clone-") as temporary:
        root = Path(temporary)
        root.chmod(0o755)
        source = root / "source.db"
        from observatory.core import Store
        from observatory.plugin_admission import activate_source
        original = Store(source)
        receiver = original.receiver_id
        activate_source(original, plugin_id="fixture.plugin", source_id="existing-source",
                        generation="a" * 64)
        activate_source(original, plugin_id="fixture.plugin", source_id="private-source",
                        generation="a" * 64, content_fingerprint="b" * 64)
        activate_source(original, plugin_id="fixture.plugin", source_id="owned-source",
                        generation="a" * 64)
        with original._connect() as db:
            db.execute("INSERT INTO plugin_reference_owner_v1 VALUES('owned-source','pi-content')")
        original.close()
        data = root / "private-data"
        data.mkdir(mode=0o700)
        clone = data / "telemetry-clone.db"
        oracle = root / "oracle.db"
        with closing(sqlite3.connect(source)) as db:
            for target in (clone, oracle):
                with closing(sqlite3.connect(target)) as copied:
                    db.backup(copied)
        with closing(sqlite3.connect(clone)) as db:
            db.execute("INSERT INTO schema_meta VALUES('storage_activation_clone_v1','enabled')")
            db.commit()
        def source_state():
            return tuple((path.stat().st_ino, path.stat().st_mtime_ns,
                          hashlib.sha256(path.read_bytes()).hexdigest()) if path.exists() else None
                         for path in (source, Path(f"{source}-wal"), Path(f"{source}-shm")))
        before = source_state()
        reference = Store(oracle)
        try:
            python_lease = activate_source(reference, plugin_id="fixture.plugin",
                                           source_id="real-source", generation="a" * 64)
            activate_source(reference, plugin_id="fixture.plugin",
                            source_id="existing-source", generation="a" * 64)
        finally:
            reference.close()
        for path in data.iterdir():
            os.chown(path, service_uid, service_gid)
            path.chmod(0o600)
        os.chown(data, service_uid, service_gid)
        transport = root / "transport"
        transport.mkdir(mode=0o700)
        os.chown(transport, service_uid, client_gid)
        transport.chmod(0o2710)
        request = {"version": 2, "request_id": "1" * 32, "receiver_id": receiver,
                   "operation": {"op": "activate_source", "plugin_id": "fixture.plugin",
                                 "source_id": "real-source", "generation": "a" * 64}}
        request_file = root / "request.json"
        request_file.write_text(json.dumps(request))
        request_file.chmod(0o644)
        socket_path = transport / "agenturm-storage-v1.sock"
        service = subprocess.Popen([str(binary), "--data-root", str(data),
            "--transport-dir", str(transport), "--receiver-id", receiver,
            "--client-uid", str(client_uid), "--client-gid", str(client_gid),
            "--experimental-real-clone-activation", "enabled"],
            preexec_fn=lambda: (os.umask(0), drop_to(service_uid, service_gid)()),
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        def run_case(case, uid=client_uid, gid=client_gid):
            result = subprocess.run([sys.executable, __file__, "--case", case,
                "--socket", str(socket_path), "--request", str(request_file)],
                preexec_fn=drop_to(uid, gid), capture_output=True, text=True, timeout=8)
            check(result.returncode == 0, f"{case}: {result.stdout}{result.stderr}")
        try:
            wait_socket(service, socket_path)
            run_case("valid")
            run_case("replay")
            run_case("conflict")
            run_case("wrong_receiver")
            run_case("non_activation")
            run_case("drop_reply")
            run_case("replay_second")
            run_case("existing")
            with closing(sqlite3.connect(clone)) as db:
                protected_before = db.execute(
                    "SELECT source_id,lease_epoch,cursor,dirty_seq,content_fingerprint "
                    "FROM plugin_source_cursor_v1 WHERE source_id IN "
                    "('private-source','owned-source') ORDER BY source_id").fetchall()
            run_case("private_fingerprint")
            run_case("private_owner")
            with closing(sqlite3.connect(clone)) as db:
                check(db.execute(
                    "SELECT source_id,lease_epoch,cursor,dirty_seq,content_fingerprint "
                    "FROM plugin_source_cursor_v1 WHERE source_id IN "
                    "('private-source','owned-source') ORDER BY source_id").fetchall() ==
                    protected_before, "private source changed after denied activation")
            # The wrong UID cannot authenticate even with the exact request.
            run_case("wrong_uid", uid=wrong_uid)
            with closing(sqlite3.connect(clone)) as db, closing(sqlite3.connect(oracle)) as py:
                sql = ("SELECT source_id,plugin_id,generation,cursor,dirty_seq,policy_epoch,"
                       "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                       "WHERE source_id IN ('real-source','existing-source') ORDER BY source_id")
                check(db.execute(sql).fetchall() == py.execute(sql).fetchall(),
                      "Rust activation differs from Python Store state")
                lease = db.execute("SELECT lease_epoch FROM plugin_source_cursor_v1 "
                                   "WHERE source_id='real-source'").fetchone()[0]
                check(lease > 0 and python_lease.epoch > 0, "missing fresh lease")
                check(db.execute("SELECT COUNT(*) FROM storage_source_activate_v2").fetchone()[0] == 3,
                      "request replay created duplicate receipt")
                db.execute("UPDATE plugin_source_cursor_v1 SET capture_enabled=0 "
                           "WHERE source_id='existing-source'")
                db.commit()
            run_case("revoked")
            check(source_state() == before,
                  "real canonical source changed")
            print("PASS detached real Store clone activation, replay, denial and Python parity")
        finally:
            service.send_signal(signal.SIGTERM)
            try:
                _, stderr = service.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                service.kill(); _, stderr = service.communicate(timeout=5)
            check(service.returncode == 0, f"service failed: {stderr}")
        # A new process must recover the same durable activation receipt,
        # The receipt must survive a clean service restart.
        restarted = subprocess.Popen([str(binary), "--data-root", str(data),
            "--transport-dir", str(transport), "--receiver-id", receiver,
            "--client-uid", str(client_uid), "--client-gid", str(client_gid),
            "--experimental-real-clone-activation", "enabled"],
            preexec_fn=lambda: (os.umask(0), drop_to(service_uid, service_gid)()),
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        try:
            wait_socket(restarted, socket_path)
            run_case("replay")
            # Same bytes at a new inode must never be adopted by the process.
            moved = root / "moved-clone.db"
            clone.rename(moved)
            clone.write_bytes(moved.read_bytes())
            os.chown(clone, service_uid, service_gid)
            clone.chmod(0o600)
            replacement_digest = hashlib.sha256(clone.read_bytes()).hexdigest()
            run_case("path_swap")
            restarted.wait(timeout=5)
            check(restarted.returncode != 0, "service accepted replaced clone inode")
            check(hashlib.sha256(clone.read_bytes()).hexdigest() == replacement_digest,
                  "service mutated replacement clone")
        finally:
            if restarted.poll() is None:
                restarted.send_signal(signal.SIGTERM)
            try:
                _, stderr = restarted.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                restarted.kill(); _, stderr = restarted.communicate(timeout=5)
            check(restarted.returncode is not None, f"restarted service failed: {stderr}")


if __name__ == "__main__":
    main()
