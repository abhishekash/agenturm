"""Private local receiver configuration and explicit, non-destructive migration."""

from __future__ import annotations
import ipaddress
import math
import os
from pathlib import Path
import secrets
import sqlite3
import tempfile
import uuid
from contextlib import closing
from urllib.parse import urlsplit

DEFAULT_RETENTION_DAYS = 14


def state_directory():
    if os.environ.get("XDG_STATE_HOME"):
        return Path(os.environ["XDG_STATE_HOME"]).expanduser() / "agenturm"
    if os.name == "nt" and os.environ.get("LOCALAPPDATA"):
        return Path(os.environ["LOCALAPPDATA"]) / "agenturm"
    return Path.home() / ".local" / "state" / "agenturm"


def telemetry_path():
    return state_directory() / "observatory.db"


def prepare_state_directory():
    path = state_directory()
    if path.is_symlink():
        raise ValueError("Agenturm state directory must not be a symbolic link.")
    _private_directory(path)
    if os.name != "nt":
        path.chmod(0o700)
    return path


def _private_directory(path):
    path.mkdir(parents=True, exist_ok=True, mode=0o700)


def _token_path(port):
    return state_directory() / ("receiver-" + str(int(port)) + ".token")


def local_token(port, *, create=False):
    path = _token_path(port)
    if create:
        prepare_state_directory()
        descriptor, staged = tempfile.mkstemp(
            prefix=".receiver-token-", dir=path.parent
        )
        try:
            with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
                stream.write(secrets.token_urlsafe(32) + "\n")
                stream.flush()
                os.fsync(stream.fileno())
            try:
                os.link(staged, path)
            except FileExistsError:
                pass
        finally:
            Path(staged).unlink(missing_ok=True)
    try:
        descriptor = os.open(
            path,
            os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0),
        )
    except FileNotFoundError:
        return None
    with os.fdopen(descriptor, "r", encoding="utf-8") as stream:
        import stat

        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError("Local receiver token must be a regular file.")
        if os.name != "nt" and os.fstat(stream.fileno()).st_mode & 0o077:
            raise ValueError(
                "Local receiver token permissions are too broad; restore owner-only permissions."
            )
        token = stream.read(257).strip()
    if not 32 <= len(token) <= 256 or any(
        c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_"
        for c in token
    ):
        raise ValueError(
            "Local receiver token is malformed; inspect the private state directory."
        )
    return token


def endpoint_token(endpoint, explicit=None):
    if explicit is not None:
        return explicit
    if os.environ.get("OBSERVATORY_TOKEN"):
        return os.environ["OBSERVATORY_TOKEN"]
    parsed = urlsplit(endpoint)
    if loopback_host(parsed.hostname):
        return local_token(parsed.port or (443 if parsed.scheme == "https" else 80))
    return None


def loopback_host(host):
    if not host:
        return False
    if host.casefold() in {"localhost", "localhost."}:
        return True
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return False


def request_authority(headers, allowed_hosts=()):
    """Return a safe authority or raise before authentication, reads or writes."""
    hosts = headers.get_all("Host", [])
    if (
        len(hosts) != 1
        or not hosts[0]
        or any(
            c in "/@?#,\\" or c.isspace() or ord(c) < 33 or ord(c) > 126
            for c in hosts[0]
        )
    ):
        raise ValueError("One valid Host header is required.")
    try:
        parsed = urlsplit("http://" + hosts[0])
        host = parsed.hostname
        port = parsed.port or 80
    except ValueError:
        raise ValueError("Invalid Host header.") from None
    if (
        not host
        or not 0 < port <= 65535
        or not (
            loopback_host(host)
            or host.casefold() in {str(h).casefold() for h in allowed_hosts}
        )
    ):
        raise ValueError("Host is not an allowed receiver authority.")
    origins = headers.get_all("Origin", [])
    if origins:
        if len(origins) != 1 or origins[0] == "null":
            raise ValueError("Origin is not allowed.")
        try:
            origin = urlsplit(origins[0])
            origin_port = origin.port or (443 if origin.scheme == "https" else 80)
        except ValueError:
            raise ValueError("Invalid Origin header.") from None
        if (
            origin.scheme not in {"http", "https"}
            or origin.username is not None
            or origin.password is not None
            or origin.path not in {"", "/"}
            or origin.query
            or origin.fragment
            or origin.hostname != host
            or origin_port != port
        ):
            raise ValueError("Cross-origin receiver access is not allowed.")
    return hosts[0]


def retention_policy(store, *, managed=False, requested=None):
    """Only genuinely new managed stores auto-enrol; existing policies persist."""
    if requested is not None and (
        not math.isfinite(float(requested)) or float(requested) <= 0
    ):
        raise ValueError("retention days must be positive and finite")
    with store._connect() as db:
        row = db.execute(
            "SELECT value FROM schema_meta WHERE key='retention_days'"
        ).fetchone()
        chosen = (
            float(requested)
            if requested is not None
            else float(row[0])
            if row
            else DEFAULT_RETENTION_DAYS
            if managed and store.created_new
            else None
        )
        if chosen is not None:
            db.execute(
                "INSERT OR REPLACE INTO schema_meta(key,value) VALUES('retention_days',?)",
                (str(chosen),),
            )
    store.retention_days = chosen
    return chosen


def migrate_database(
    source, destination=None, *, preserve_identity=False, stopped_receiver=False
):
    """Consistent SQLite backup; never overwrite/delete source or destination.

    Default copies receive a new namespace. Preserving an identity is an explicit
    stopped-old-receiver cutover, never permission for concurrent live copies.
    """
    source = Path(source).expanduser().resolve()
    target = Path(destination or telemetry_path()).expanduser().absolute()
    if not source.is_file():
        raise ValueError("Source database does not exist.")
    if source == target.resolve():
        raise ValueError("Source and destination must differ.")
    if preserve_identity and not stopped_receiver:
        raise ValueError(
            "Preserving receiver identity requires --stopped-receiver and a stopped old receiver."
        )
    for name in (target, Path(str(target) + "-wal"), Path(str(target) + "-shm")):
        if name.exists() or name.is_symlink():
            raise FileExistsError("Destination or SQLite sidecar already exists.")
    _private_directory(target.parent)
    descriptor, temporary = tempfile.mkstemp(
        prefix=".agenturm-migrate-", dir=target.parent
    )
    os.close(descriptor)
    temporary = Path(temporary)
    try:
        with (
            closing(
                sqlite3.connect(source.as_uri() + "?mode=ro", uri=True)
            ) as original,
            closing(sqlite3.connect(temporary)) as copied,
        ):
            original.backup(copied)
            check = copied.execute("PRAGMA quick_check").fetchone()
            if not check or check[0] != "ok":
                raise ValueError("Source backup failed its integrity check.")
            old = copied.execute(
                "SELECT value FROM schema_meta WHERE key='receiver_id'"
            ).fetchone()
            if not old:
                raise ValueError("Source is not an Agenturm telemetry database.")
            if not preserve_identity:
                copied.execute(
                    "INSERT OR REPLACE INTO schema_meta(key,value) VALUES('migrated_from_receiver',?)",
                    (old[0],),
                )
                copied.execute(
                    "UPDATE schema_meta SET value=? WHERE key='receiver_id'",
                    (uuid.uuid4().hex,),
                )
            copied.commit()
        for name in (target, Path(str(target) + "-wal"), Path(str(target) + "-shm")):
            if name.exists() or name.is_symlink():
                raise FileExistsError(
                    "Destination or SQLite sidecar appeared during migration."
                )
        os.link(temporary, target)
        return {
            "source": str(source),
            "destination": str(target),
            "identity_preserved": preserve_identity,
            "source_unchanged": True,
        }
    except sqlite3.Error as exc:
        raise ValueError(
            "Migration failed; the source could not be read as an Agenturm SQLite database."
        ) from exc
    finally:
        temporary.unlink(missing_ok=True)
        for suffix in ("-wal", "-shm"):
            Path(str(temporary) + suffix).unlink(missing_ok=True)
