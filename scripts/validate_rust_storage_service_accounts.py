"""Root-required, disposable Linux account-separation proof for the probe service.

Run in an isolated CI VM or container. This creates no system accounts and
changes ownership only below its own temporary directory.
"""
from __future__ import annotations

import argparse
import errno
import grp
import json
import os
import pwd
import signal
import socket
import sqlite3
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path


RECEIVER = "receiver:account-proof"
REQUEST_ID = "0123456789abcdef0123456789abcdef"
SOCKET_NAME = "agenturm-storage-v1.sock"


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def identities() -> tuple[int, int, int, int, int]:
    """Choose unused numeric IDs; do not add host users or groups."""
    unused_uids = []
    unused_gids = []
    for candidate in range(45000, 46000):
        try:
            pwd.getpwuid(candidate)
        except KeyError:
            unused_uids.append(candidate)
        try:
            grp.getgrgid(candidate)
        except KeyError:
            unused_gids.append(candidate)
        if len(unused_uids) >= 3 and len(unused_gids) >= 3:
            break
    check(len(unused_uids) >= 3 and len(unused_gids) >= 3, "no spare numeric identities")
    return (*unused_uids[:3], *unused_gids[:2])


def drop_to(uid: int, gid: int, supplementary: tuple[int, ...] = ()):
    def prepare() -> None:
        os.setgroups(supplementary)
        os.setgid(gid)
        os.setuid(uid)
    return prepare


def request(receiver: str = RECEIVER) -> bytes:
    raw = json.dumps({
        "version": 1,
        "request_id": REQUEST_ID,
        "receiver_id": receiver,
        "operation": {"op": "probe", "topic": "protocol"},
    }, separators=(",", ":")).encode()
    return struct.pack(">I", len(raw)) + raw


def read_exact(client: socket.socket, length: int) -> bytes:
    result = bytearray()
    while len(result) < length:
        chunk = client.recv(length - len(result))
        check(bool(chunk), "short reply frame")
        result.extend(chunk)
    return bytes(result)


def client_case(socket_path: Path, kind: str) -> None:
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(3)
        client.connect(str(socket_path))
        if kind in ("valid", "wrong_receiver"):
            client.sendall(request("receiver:other" if kind == "wrong_receiver" else RECEIVER))
            client.shutdown(socket.SHUT_WR)
        if kind == "valid":
            length = struct.unpack(">I", read_exact(client, 4))[0]
            check(0 < length <= 65536, "invalid reply length")
            reply = json.loads(read_exact(client, length))
            check(reply == {
                "version": 1,
                "request_id": REQUEST_ID,
                "receiver_id": RECEIVER,
                "result": "protocol_ready",
            }, f"unexpected probe reply: {reply!r}")
            check(client.recv(1) == b"", "reply connection did not close")
        else:
            check(client.recv(1) == b"", f"{kind} received a reply")


def store_case(data: Path, *, existing: bool = False) -> None:
    # The old Python Store path is intentionally exercised as a fresh open.
    # It must fail at the private directory, regardless of Store marker logic.
    from observatory.core import Store
    from observatory.store_authority import StoreAuthorityError

    database = data / "canonical.db"
    try:
        Store(database)
    except (PermissionError, StoreAuthorityError, OSError) as error:
        if isinstance(error, OSError):
            check(error.errno in (errno.EACCES, errno.EPERM), f"wrong Store failure: {error}")
        if isinstance(error, StoreAuthorityError):
            cause = error.__cause__
            check(isinstance(cause, OSError) and cause.errno in (errno.EACCES, errno.EPERM),
                  f"Store failed for an unrelated reason: {error}")
    else:
        raise AssertionError("old Python Store opened protected data root")
    try:
        flags = os.O_RDONLY if existing else os.O_CREAT | os.O_WRONLY
        os.open(database, flags, 0o600)
    except PermissionError:
        pass
    else:
        raise AssertionError("client opened a file in protected data root")


def child(args: argparse.Namespace) -> None:
    if args.case in ("store", "store_existing"):
        store_case(Path(args.data), existing=args.case == "store_existing")
    else:
        client_case(Path(args.socket), args.case)


def run_child(case: str, *, uid: int, gid: int, socket_path: Path,
              data: Path, supplementary: tuple[int, ...] = ()) -> None:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(Path(__file__).resolve().parents[1])
    result = subprocess.run(
        [sys.executable, __file__, "--case", case, "--socket", str(socket_path),
         "--data", str(data)], preexec_fn=drop_to(uid, gid, supplementary),
        env=env, text=True, capture_output=True, timeout=8,
    )
    check(result.returncode == 0, f"{case} failed: {result.stdout}{result.stderr}")
    print(f"PASS {case}: uid={uid} gid={gid}", flush=True)


def wait_socket(service: subprocess.Popen, socket_path: Path) -> None:
    until = time.monotonic() + 5
    while time.monotonic() < until:
        if service.poll() is not None:
            _, stderr = service.communicate(timeout=1)
            raise AssertionError(f"service exited before readiness: {stderr}")
        if socket_path.exists():
            return
        time.sleep(0.01)
    raise AssertionError("service socket did not appear")


def wait_exit(service: subprocess.Popen, socket_path: Path) -> None:
    _, stderr = service.communicate(timeout=5)
    check(service.returncode == 0, f"service exit {service.returncode}: {stderr}")
    check(not socket_path.exists(), "service did not remove its socket")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", type=Path)
    parser.add_argument("--case", choices=("valid", "wrong_uid", "wrong_gid",
                                           "same_uid", "wrong_receiver", "store",
                                           "store_existing"))
    parser.add_argument("--socket")
    parser.add_argument("--data")
    args = parser.parse_args()
    if args.case:
        child(args)
        return
    check(sys.platform == "linux" and os.geteuid() == 0,
          "this fixture requires root on Linux inside an isolated VM or container")
    check(args.bin is not None and args.bin.is_file(), "pass a built service with --bin")
    binary_source = args.bin.resolve()
    service_uid, client_uid, wrong_uid, service_gid, client_gid = identities()
    wrong_gid = service_gid  # Distinct from the admitted primary client GID.
    with tempfile.TemporaryDirectory(prefix="agenturm-accounts-") as temporary:
        root = Path(temporary)
        root.chmod(0o755)
        binary = root / "agenturm-storage-service"
        shutil.copy2(binary_source, binary)
        binary.chmod(0o755)
        data = root / "private-data"
        transport = root / "transport"
        data.mkdir(mode=0o700)
        transport.mkdir(mode=0o700)
        os.chown(data, service_uid, service_gid)
        os.chown(transport, service_uid, client_gid)
        transport.chmod(0o2710)
        socket_path = transport / SOCKET_NAME
        command = [str(binary), "--data-root", str(data), "--transport-dir", str(transport),
                   "--receiver-id", RECEIVER, "--client-uid", str(client_uid),
                   "--client-gid", str(client_gid), "--max-connections", "5"]
        service = subprocess.Popen(command, preexec_fn=drop_to(service_uid, service_gid),
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        try:
            wait_socket(service, socket_path)
            mode = transport.stat()
            check(stat.S_IMODE(mode.st_mode) == 0o2710 and mode.st_uid == service_uid
                  and mode.st_gid == client_gid, "transport owner/mode differs")
            mode = socket_path.lstat()
            check(stat.S_ISSOCK(mode.st_mode) and stat.S_IMODE(mode.st_mode) == 0o660
                  and mode.st_uid == service_uid and mode.st_gid == client_gid,
                  "socket owner/mode differs")
            lock = (transport / ".agenturm-storage-v1.lock").stat()
            check(stat.S_IMODE(lock.st_mode) == 0o600 and lock.st_uid == service_uid,
                  "instance lock owner/mode differs")
            mode = data.stat()
            check(stat.S_IMODE(mode.st_mode) == 0o700 and mode.st_uid == service_uid,
                  "data root owner/mode differs")
            check(list(data.iterdir()) == [], "probe service wrote data")
            print("PASS data 0700, transport 2710, socket 0660", flush=True)
            run_child("store", uid=client_uid, gid=client_gid,
                      socket_path=socket_path, data=data)
            run_child("valid", uid=client_uid, gid=client_gid,
                      socket_path=socket_path, data=data)
            run_child("wrong_uid", uid=wrong_uid, gid=client_gid,
                      socket_path=socket_path, data=data)
            # Supplementary client group grants traversal; SO_PEERCRED must still
            # reject the wrong primary GID after the connection is accepted.
            run_child("wrong_gid", uid=client_uid, gid=wrong_gid,
                      supplementary=(client_gid,), socket_path=socket_path, data=data)
            run_child("same_uid", uid=service_uid, gid=service_gid,
                      socket_path=socket_path, data=data)
            run_child("wrong_receiver", uid=client_uid, gid=client_gid,
                      socket_path=socket_path, data=data)
            wait_exit(service, socket_path)
            check(list(data.iterdir()) == [], "probe service wrote data after connections")
            print("PASS bounded exit and socket cleanup", flush=True)
            # A fresh listener must also remove the socket on SIGTERM.
            service = subprocess.Popen(command[:-2], preexec_fn=drop_to(service_uid, service_gid),
                                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                                       text=True)
            wait_socket(service, socket_path)
            service.send_signal(signal.SIGTERM)
            wait_exit(service, socket_path)
            print("PASS SIGTERM cleanup and restart", flush=True)
            # Seed a disposable SQLite file only after the probe service has
            # stopped. This also checks a fresh open of an *existing* database.
            database = data / "canonical.db"
            with sqlite3.connect(database) as connection:
                connection.execute("CREATE TABLE fixture (value INTEGER)")
            os.chown(database, service_uid, service_gid)
            database.chmod(0o600)
            run_child("store_existing", uid=client_uid, gid=client_gid,
                      socket_path=socket_path, data=data)
        finally:
            if service.poll() is None:
                service.kill()
                service.communicate(timeout=5)
    print("PASS distinct-principal installed storage probe", flush=True)


if __name__ == "__main__":
    main()
