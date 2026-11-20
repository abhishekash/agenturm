"""Disposable Linux two-UID proof of the held-open legacy SQLite writer hazard.

Run as root only inside an isolated CI VM or container. The old Python Store
keeps a writable SQLite connection across backup and archive. This is a
negative cutover gate: moving/chowning the old file cannot revoke that handle.
No production database, system account, or Rust writer is changed.
"""
from __future__ import annotations

import argparse
from contextlib import closing
import errno
import hashlib
import os
from pathlib import Path
import pwd
import grp
import select
import sqlite3
import stat
import subprocess
import sys
import tempfile


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def identities() -> tuple[int, int, int, int]:
    uids, gids = [], []
    for value in range(45000, 46000):
        try:
            pwd.getpwuid(value)
        except KeyError:
            uids.append(value)
        try:
            grp.getgrgid(value)
        except KeyError:
            gids.append(value)
        if len(uids) >= 2 and len(gids) >= 2:
            break
    check(len(uids) >= 2 and len(gids) >= 2, "two unused identities required")
    return uids[0], uids[1], gids[0], gids[1]


def drop_to(uid: int, gid: int):
    def drop() -> None:
        os.setgroups(())
        os.setgid(gid)
        os.setuid(uid)
    return drop


def checksum(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def line(process: subprocess.Popen, expected: str) -> None:
    ready, _, _ = select.select([process.stdout], [], [], 10)
    check(bool(ready), f"legacy writer timed out waiting for {expected!r}")
    result = process.stdout.readline().strip()
    check(result == expected, f"legacy writer expected {expected!r}, got {result!r}")


def send(process: subprocess.Popen, command: str) -> None:
    process.stdin.write(command + "\n")
    process.stdin.flush()


def legacy_holder(path: Path) -> None:
    from observatory.core import Store

    store = Store(path)
    try:
        with store._connect() as db:
            db.execute("CREATE TABLE held_open_legacy_v1(value TEXT NOT NULL)")
            db.execute("INSERT INTO held_open_legacy_v1 VALUES('before')")
            db.commit()
            print("READY", flush=True)
            check(input() == "BEGIN", "unexpected first holder command")
            db.execute("BEGIN IMMEDIATE")
            print("TRANSACTION_OPEN", flush=True)
            check(input() == "WRITE", "unexpected second holder command")
            db.execute("INSERT INTO held_open_legacy_v1 VALUES('after')")
            db.execute("CREATE TABLE legacy_ddl_after_archive(value TEXT)")
            db.execute("UPDATE schema_meta SET value='receiver:legacy-second' "
                       "WHERE key='receiver_id'")
            db.commit()
            print("COMMITTED", flush=True)
            check(input() == "EXIT", "unexpected final holder command")
    finally:
        store.close()


def fresh_legacy_denials(data: Path) -> None:
    from observatory.core import Store
    from observatory.store_authority import StoreAuthorityError

    for name in ("canonical.db", "archived.db"):
        path = data / name
        try:
            Store(path)
        except StoreAuthorityError as error:
            cause = error.__cause__
            check(isinstance(cause, OSError) and cause.errno in (errno.EACCES, errno.EPERM),
                  f"Store failed for an unrelated reason: {error}")
        except OSError as error:
            check(error.errno in (errno.EACCES, errno.EPERM),
                  f"Store failed for an unrelated reason: {error}")
        else:
            raise AssertionError(f"legacy Store opened protected {name}")
        try:
            with sqlite3.connect(path.as_uri() + "?mode=rw", uri=True) as db:
                db.execute("CREATE TABLE forbidden_ddl(value TEXT)")
                db.execute("INSERT INTO forbidden_ddl VALUES('forbidden')")
                db.execute("UPDATE schema_meta SET value='receiver:legacy-second' "
                           "WHERE key='receiver_id'")
        except sqlite3.OperationalError as error:
            check("unable to open database file" in str(error),
                  f"unexpected SQLite denial: {error}")
        else:
            raise AssertionError(f"legacy SQLite changed protected {name}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=("holder", "fresh"))
    parser.add_argument("--database", type=Path)
    parser.add_argument("--data", type=Path)
    args = parser.parse_args()
    if args.case == "holder":
        legacy_holder(args.database)
        return
    if args.case == "fresh":
        fresh_legacy_denials(args.data)
        return
    check(sys.platform == "linux" and os.geteuid() == 0,
          "run as root on an isolated Linux VM or container")
    legacy_uid, service_uid, legacy_gid, service_gid = identities()
    with tempfile.TemporaryDirectory(prefix="agenturm-held-open-") as temporary:
        root = Path(temporary)
        root.chmod(0o755)
        old = root / "old"
        private = root / "service-private"
        old.mkdir(mode=0o700)
        private.mkdir(mode=0o700)
        os.chown(old, legacy_uid, legacy_gid)
        os.chown(private, service_uid, service_gid)
        database = old / "legacy.db"
        env = os.environ.copy()
        env["PYTHONPATH"] = str(Path(__file__).resolve().parents[1])
        holder = subprocess.Popen(
            [sys.executable, __file__, "--case", "holder", "--database", str(database)],
            preexec_fn=drop_to(legacy_uid, legacy_gid), env=env,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, bufsize=1,
        )
        try:
            line(holder, "READY")
            canonical = private / "canonical.db"
            with closing(sqlite3.connect(database.as_uri() + "?mode=ro", uri=True)) as source:
                with closing(sqlite3.connect(canonical)) as target:
                    source.backup(target)
            os.chown(canonical, service_uid, service_gid)
            canonical.chmod(0o600)
            with closing(sqlite3.connect(canonical)) as copied:
                check(copied.execute("SELECT value FROM held_open_legacy_v1").fetchall() ==
                      [("before",)], "WAL-inclusive backup missed committed row")
                receiver = copied.execute("SELECT value FROM schema_meta "
                                          "WHERE key='receiver_id'").fetchone()[0]
            baseline = checksum(canonical)
            send(holder, "BEGIN")
            line(holder, "TRANSACTION_OPEN")

            # A held writer has already entered its transaction. Transfer the
            # old main file and sidecars to a private service-owned archive.
            archived = private / "archived.db"
            moved = []
            for suffix in ("", "-wal", "-shm"):
                path = Path(str(database) + suffix)
                if path.exists():
                    destination = Path(str(archived) + suffix)
                    path.rename(destination)
                    os.chown(destination, service_uid, service_gid)
                    destination.chmod(0o600)
                    moved.append(destination)
            check(archived.exists(), "legacy main file was not archived")
            before_archive = {p.name: checksum(p) for p in moved}
            send(holder, "WRITE")
            line(holder, "COMMITTED")
            check(checksum(canonical) == baseline,
                  "held legacy writer changed protected new canonical bytes")
            with closing(sqlite3.connect(canonical)) as copied:
                check(copied.execute("SELECT value FROM schema_meta "
                                     "WHERE key='receiver_id'").fetchone()[0] == receiver,
                      "new canonical receiver identity changed")
            check(any(checksum(p) != before_archive[p.name] for p in moved),
                  "held writer did not modify an archived inode or sidecar")
            check(stat.S_IMODE(private.stat().st_mode) == 0o700 and
                  private.stat().st_uid == service_uid,
                  "private target permissions changed")
            fresh = subprocess.run(
                [sys.executable, __file__, "--case", "fresh", "--data", str(private)],
                preexec_fn=drop_to(legacy_uid, legacy_gid), env=env,
                capture_output=True, text=True, timeout=10,
            )
            check(fresh.returncode == 0,
                  f"fresh legacy denial failed: {fresh.stdout}{fresh.stderr}")
            check(checksum(canonical) == baseline,
                  "fresh legacy attempts changed new canonical bytes")
            print("PASS fresh old Store/SQLite denied canonical and archive", flush=True)
            print("PASS held-open old transaction mutated archived inode after chmod", flush=True)
            print("PASS protected canonical preserved receiver identity and WAL snapshot", flush=True)
            send(holder, "EXIT")
            _, stderr = holder.communicate(timeout=10)
            check(holder.returncode == 0, f"legacy holder failed: {stderr}")
            with closing(sqlite3.connect(archived.as_uri() + "?mode=ro", uri=True)) as old_copy:
                check(old_copy.execute("SELECT value FROM held_open_legacy_v1 ORDER BY rowid"
                                       ).fetchall() == [("before",), ("after",)],
                      "archived DML from held writer was not durable")
                check(old_copy.execute("SELECT name FROM sqlite_master WHERE "
                                       "name='legacy_ddl_after_archive'").fetchone() is not None,
                      "archived DDL from held writer was not durable")
                check(old_copy.execute("SELECT value FROM schema_meta WHERE key='receiver_id'"
                                       ).fetchone()[0] == "receiver:legacy-second",
                      "held writer did not change archived receiver identity")
            with closing(sqlite3.connect(canonical.as_uri() + "?mode=ro", uri=True)) as new_copy:
                check(new_copy.execute("SELECT value FROM held_open_legacy_v1").fetchall() ==
                      [("before",)], "canonical took a post-backup legacy write")
        finally:
            if holder.poll() is None:
                holder.kill()
                holder.communicate(timeout=5)


if __name__ == "__main__":
    main()
