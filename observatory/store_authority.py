"""Point-in-time authority preflight before Python Store initialization.

This guard prevents this version of Store from mutating a database that
declares another storage authority. It is not a process lock or a migration
protocol, and older Python binaries do not know about it.
"""
from __future__ import annotations

import os
import sqlite3
import stat
from contextlib import closing
from pathlib import Path


MARKER = "storage_authority_v1"
PYTHON_OWNER = "python-v1"


class StoreAuthorityError(RuntimeError):
    """An existing Store cannot be opened by this Python writer."""


def _target(path: Path) -> tuple[int, int]:
    try:
        info = path.stat()
    except OSError as exc:
        raise StoreAuthorityError("Store authority target is unavailable") from exc
    if not stat.S_ISREG(info.st_mode):
        raise StoreAuthorityError("Store authority target is not a regular file")
    return info.st_dev, info.st_ino


def require_python_owner(db: sqlite3.Connection) -> None:
    """Reject an existing foreign marker on the connection about to migrate."""
    metadata = db.execute(
        "SELECT type FROM sqlite_master WHERE name='schema_meta'").fetchone()
    if metadata is None:
        return
    if metadata[0] != "table":
        raise StoreAuthorityError("Store authority metadata is invalid")
    rows = db.execute(
        "SELECT length(CAST(value AS BLOB)),substr(value,1,32) "
        "FROM schema_meta WHERE key=? LIMIT 2", (MARKER,)).fetchall()
    if len(rows) > 1:
        raise StoreAuthorityError("Store authority marker is ambiguous")
    if not rows:
        return
    length, owner = rows[0]
    if type(length) is not int or not 1 <= length <= 32:
        raise StoreAuthorityError("Store authority marker is invalid")
    if owner != PYTHON_OWNER:
        raise StoreAuthorityError("Store is not owned by this Python writer")


def preflight(path: str | Path) -> tuple[int, int] | None:
    """Read an existing database without creating or modifying its main file.

    ``None`` means the path did not exist. Missing metadata on a valid SQLite
    database is accepted as a legacy Python Store; only the exact current
    Python marker permits an explicitly marked database.
    """
    target = Path(path).absolute()
    try:
        target.lstat()
    except FileNotFoundError:
        return None
    except OSError as exc:
        raise StoreAuthorityError("Store authority path is unavailable") from exc
    identity = _target(target)
    try:
        with closing(sqlite3.connect(target.as_uri() + "?mode=ro", uri=True,
                                     timeout=2)) as db:
            db.execute("PRAGMA query_only=ON")
            db.execute("BEGIN")
            require_python_owner(db)
    except sqlite3.Error as exc:
        raise StoreAuthorityError("Store authority database is unreadable") from exc
    if _target(target) != identity:
        raise StoreAuthorityError("Store authority target changed during preflight")
    return identity


def require_target(path: str | Path, expected: tuple[int, int] | None) -> None:
    """Recheck the pathname immediately before a constructor mutation."""
    target = Path(path).absolute()
    if expected is None:
        try:
            target.lstat()
        except FileNotFoundError:
            return
        except OSError as exc:
            raise StoreAuthorityError("Store authority path is unavailable") from exc
        raise StoreAuthorityError("Store appeared after authority preflight")
    if _target(target) != expected:
        raise StoreAuthorityError("Store authority target changed after preflight")


def chmod_target(path: str | Path, expected: tuple[int, int]) -> None:
    """Change mode only on an opened inode that matches the preflight target."""
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NONBLOCK", 0)
    try:
        fd = os.open(path, flags)
    except OSError as exc:
        raise StoreAuthorityError("Store authority target is unavailable") from exc
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or (info.st_dev, info.st_ino) != expected:
            raise StoreAuthorityError("Store authority target changed before chmod")
        os.fchmod(fd, 0o600)
    finally:
        os.close(fd)
