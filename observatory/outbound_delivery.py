"""Explicit outbound action contract proof with host-owned durable delivery state.

This module is deliberately not wired to findings or receiver startup. A caller
must configure a destination, enqueue a bounded fixture action, and invoke the
dispatcher. The only executable in the example is a loopback fixture adapter.
"""
from __future__ import annotations

import hashlib
import json
import os
import queue
import re
import secrets
import stat
import subprocess
import tempfile
import threading
import time
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import urlsplit

from .plugin_frames import FrameError, read_frame, write_frame
from .plugin_manifest import PluginManifest, plan_activation
from .plugin_wire import ProtocolError, decode_message, encode_message

if TYPE_CHECKING:
    from .core import Store


_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,127}\Z")
_CODE = re.compile(r"[a-z][a-z0-9_]{0,63}\Z")
_RECEIPT = re.compile(r"[A-Za-z0-9._:-]{1,128}\Z")
MAX_FRAME = 64 * 1024
MAX_ARTIFACT = 64 * 1024 * 1024
MAX_ATTEMPTS = 3
ACTION = "write_fixture_metric"


def _timestamp(value: int | None) -> int:
    result = time.time_ns() if value is None else value
    if type(result) is not int or not 0 <= result <= 2**63 - 1:
        raise ValueError("invalid delivery timestamp")
    return result


def loopback_endpoint(value: str) -> str:
    if not isinstance(value, str) or len(value) > 300:
        raise ValueError("invalid outbound endpoint")
    try:
        parsed = urlsplit(value)
        port = parsed.port
    except ValueError as exc:
        raise ValueError("invalid outbound endpoint") from exc
    if (parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or not port
            or parsed.netloc != f"127.0.0.1:{port}" or parsed.path != "/deliver"
            or parsed.query or parsed.fragment or parsed.username or parsed.password):
        raise ValueError("outbound fixture requires exact loopback /deliver endpoint")
    return value


@dataclass(frozen=True)
class DestinationConfig:
    destination_id: str
    action: str
    endpoint: str
    manifest: PluginManifest
    allowed_grants: tuple[str, ...]


def read_destination_config(path: str | Path) -> DestinationConfig:
    """Read only an explicitly named, bounded fixture destination descriptor."""
    path = Path(path)
    try:
        if path.is_symlink() or not stat.S_ISREG(path.stat().st_mode):
            raise ValueError("destination config must be a regular file")
        with path.open("rb") as stream:
            raw = stream.read(4097)
    except OSError as exc:
        raise ValueError("cannot read destination config") from exc
    if len(raw) > 4096:
        raise ValueError("destination config exceeds byte budget")
    try:
        data = decode_message(raw.decode("utf-8", "strict"))
    except UnicodeError as exc:
        raise ValueError("invalid destination config encoding") from exc
    if (set(data) != {"schema", "destination_id", "action", "endpoint", "manifest", "grants"}
            or type(data["schema"]) is not int or data["schema"] != 1
            or not isinstance(data["destination_id"], str)
            or not _ID.fullmatch(data["destination_id"])
            or data["action"] != ACTION
            or not isinstance(data["manifest"], str)
            or not Path(data["manifest"]).is_absolute()
            or data["grants"] != ["loopback-sink-write"]):
        raise ValueError("invalid destination config")
    endpoint = loopback_endpoint(data["endpoint"])
    manifest = PluginManifest.read(data["manifest"])
    return DestinationConfig(data["destination_id"], ACTION, endpoint, manifest,
                             ("loopback-sink-write",))


class OutboundProcess:
    """One pinned long-lived child for the distinct action-adapter/1 protocol."""

    def __init__(self, manifest: PluginManifest, *, destination_id: str, endpoint: str,
                 allowed_grants: tuple[str, ...], timeout: float = 2.0):
        if not isinstance(manifest, PluginManifest):
            raise TypeError("plugin manifest required")
        plan_activation({manifest.id: manifest}, (manifest.id,),
                        host_features=("delivery-attempt/1",),
                        allowed_grants=allowed_grants)
        if "action-adapter/1" not in manifest.provides:
            raise ProtocolError("manifest lacks action-adapter/1")
        if "loopback-sink-write" not in manifest.grants:
            raise ProtocolError("fixture action grant absent")
        if not isinstance(destination_id, str) or not _ID.fullmatch(destination_id):
            raise ValueError("invalid destination ID")
        self.endpoint = loopback_endpoint(endpoint)
        if type(timeout) not in (int, float) or not 0.05 <= timeout <= 30:
            raise ValueError("invalid outbound timeout")
        self.manifest = manifest
        self.destination_id = destination_id
        self.timeout = float(timeout)
        self.process: subprocess.Popen | None = None
        self._directory: tempfile.TemporaryDirectory | None = None
        self._frames: queue.Queue = queue.Queue(maxsize=2)
        self._stop = threading.Event()
        self._exchange_lock = threading.Lock()

    def _artifact(self) -> Path:
        if len(self.manifest.entrypoint) != 1:
            raise ProtocolError("action adapter requires one executable artifact")
        path = Path(self.manifest.entrypoint[0])
        if not path.is_absolute() or path.is_symlink():
            raise ProtocolError("action artifact must be an absolute regular file")
        try:
            info = path.stat()
            if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_ARTIFACT:
                raise ProtocolError("invalid action artifact")
            raw = path.read_bytes()
        except OSError as exc:
            raise ProtocolError("cannot read action artifact") from exc
        if len(raw) > MAX_ARTIFACT or hashlib.sha256(raw).hexdigest() != self.manifest.artifact_sha256:
            raise ProtocolError("action artifact hash mismatch")
        self._directory = tempfile.TemporaryDirectory(prefix="agenturm-action-")
        copied = Path(self._directory.name) / "plugin"
        copied.write_bytes(raw)
        copied.chmod(0o700)
        return copied

    def _reader(self):
        try:
            while not self._stop.is_set():
                raw = read_frame(self.process.stdout, max_bytes=MAX_FRAME)
                if raw is None:
                    raise ProtocolError("action child closed data pipe")
                while not self._stop.is_set():
                    try:
                        self._frames.put(raw, timeout=0.1)
                        break
                    except queue.Full:
                        continue
        except (OSError, FrameError, ProtocolError) as exc:
            if not self._stop.is_set():
                try:
                    self._frames.put_nowait(exc)
                except queue.Full:
                    pass

    def _send(self, value: dict, *, timeout: float | None = None):
        raw = encode_message(value)
        if len(raw.encode()) > MAX_FRAME:
            raise ProtocolError("action control frame exceeds budget")
        errors = []

        def write():
            try:
                write_frame(self.process.stdin, raw, max_bytes=MAX_FRAME)
            except (OSError, FrameError) as exc:
                errors.append(exc)

        thread = threading.Thread(target=write, daemon=True)
        thread.start()
        thread.join(self.timeout if timeout is None else timeout)
        if thread.is_alive() or errors:
            self.close()
            raise ProtocolError("action control pipe failed or timed out")

    def _next(self, *, timeout: float | None = None) -> dict:
        try:
            item = self._frames.get(timeout=self.timeout if timeout is None else timeout)
        except queue.Empty as exc:
            self.close()
            raise ProtocolError("action reply timed out") from exc
        if isinstance(item, BaseException):
            self.close()
            raise ProtocolError("action data pipe failed") from item
        try:
            return decode_message(item)
        except ProtocolError:
            self.close()
            raise

    def start(self):
        if self.process is not None or self._stop.is_set():
            raise ProtocolError("action child already started")
        copied = self._artifact()
        try:
            self.process = subprocess.Popen(
                [str(copied)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL, cwd=self._directory.name,
                env={"PATH": os.defpath, "LANG": "C.UTF-8", "AGENTURM_ACTION_PROTOCOL": "1"},
                bufsize=0, close_fds=True)
            threading.Thread(target=self._reader, daemon=True).start()
            startup_deadline = max(1.0, self.timeout)
            self._send({"type": "hello", "protocol": 1, "plugin_id": self.manifest.id,
                        "destination_id": self.destination_id, "action": ACTION,
                        "endpoint": self.endpoint}, timeout=startup_deadline)
            reply = self._next(timeout=startup_deadline)
            if reply != {"type": "hello_ack", "protocol": 1, "plugin_id": self.manifest.id,
                         "features": ["action-adapter/1"]}:
                raise ProtocolError("action handshake mismatch")
            return self
        except BaseException:
            self.close()
            raise

    def exchange(self, value: dict) -> dict:
        if self.process is None or self.process.poll() is not None:
            raise ProtocolError("action child is not running")
        if not self._exchange_lock.acquire(blocking=False):
            raise ProtocolError("action child already has an outstanding request")
        try:
            self._send(value)
            return self._next()
        finally:
            self._exchange_lock.release()

    def dispatch(self, delivery_id: str, idempotency_key: str, value: int) -> dict:
        reply = self.exchange({"type": "dispatch", "protocol": 1,
                               "delivery_id": delivery_id, "idempotency_key": idempotency_key,
                               "action": ACTION, "value": value})
        if (set(reply) != {"type", "protocol", "delivery_id", "status", "code", "receipt"}
                or reply["type"] != "dispatch_result" or type(reply["protocol"]) is not int
                or reply["protocol"] != 1 or reply["delivery_id"] != delivery_id
                or reply["status"] not in ("accepted", "retryable", "terminal")
                or not isinstance(reply["code"], str) or not _CODE.fullmatch(reply["code"])
                or not isinstance(reply["receipt"], str)
                or reply["receipt"] and not _RECEIPT.fullmatch(reply["receipt"])):
            raise ProtocolError("invalid action result")
        if reply["status"] == "accepted" and not reply["receipt"]:
            raise ProtocolError("accepted action lacks receipt")
        if reply["status"] != "accepted" and reply["receipt"]:
            raise ProtocolError("failed action included receipt")
        return reply

    def lookup(self, idempotency_key: str) -> dict:
        reply = self.exchange({"type": "lookup", "protocol": 1,
                               "idempotency_key": idempotency_key})
        if (set(reply) != {"type", "protocol", "idempotency_key", "status", "receipt"}
                or reply["type"] != "lookup_result" or type(reply["protocol"]) is not int
                or reply["protocol"] != 1 or reply["idempotency_key"] != idempotency_key
                or reply["status"] not in ("accepted", "missing")
                or not isinstance(reply["receipt"], str)
                or reply["receipt"] and not _RECEIPT.fullmatch(reply["receipt"])):
            raise ProtocolError("invalid action lookup result")
        if (reply["status"] == "accepted") != bool(reply["receipt"]):
            raise ProtocolError("lookup receipt mismatch")
        return reply

    def close(self):
        self._stop.set()
        process = self.process
        if process is not None:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=1)
            for pipe in (process.stdin, process.stdout):
                if pipe is not None:
                    pipe.close()
            self.process = None
        if self._directory is not None:
            self._directory.cleanup()
            self._directory = None

    def __enter__(self):
        return self.start()

    def __exit__(self, *_args):
        self.close()


def _handoff_path_proof(database: Path) -> tuple[tuple[int, int], tuple[int, int]]:
    """Bind a direct private database and parent before handoff lock creation."""
    try:
        if not database.is_absolute() or database.resolve(strict=True) != database:
            raise ProtocolError("outbound handoff requires a direct database path")
        db = database.lstat()
        parent = database.parent.lstat()
        if (not stat.S_ISREG(db.st_mode) or db.st_nlink != 1
                or db.st_uid != os.geteuid() or db.st_mode & 0o7777 != 0o600
                or not stat.S_ISDIR(parent.st_mode)
                or parent.st_uid != os.geteuid() or parent.st_mode & 0o077):
            raise ProtocolError("outbound handoff requires a private database and parent")
        for ancestor in database.parent.parents:
            info = ancestor.lstat()
            if (not stat.S_ISDIR(info.st_mode) or
                    info.st_mode & 0o022 and not
                    (info.st_uid == 0 and info.st_mode & 0o1000)):
                raise ProtocolError("outbound handoff ancestor permits replacement")
    except OSError as exc:
        raise ProtocolError("outbound handoff database path is unavailable") from exc
    return (parent.st_dev, parent.st_ino), (db.st_dev, db.st_ino)


class _DispatchLock:
    """Opened lock identity that must still name the held inode."""

    def __init__(self, path: Path, descriptor: int, database: Path | None,
                 parent_identity: tuple[int, int] | None,
                 database_identity: tuple[int, int] | None):
        self.path = path
        self.descriptor = descriptor
        self.identity = self._identity(os.fstat(descriptor))
        self.database = database
        self.database_identity = database_identity
        self.parent_identity = parent_identity

    @staticmethod
    def _identity(info):
        return info.st_dev, info.st_ino

    def verify(self) -> None:
        """Refuse a replaced, linked, or permission-changed lock pathname."""
        try:
            opened = os.fstat(self.descriptor)
            named = self.path.lstat()
            valid = (stat.S_ISREG(opened.st_mode) and stat.S_ISREG(named.st_mode)
                     and self._identity(opened) == self.identity == self._identity(named)
                     and opened.st_nlink == named.st_nlink == 1
                     and opened.st_uid == named.st_uid == os.geteuid()
                     and opened.st_mode & 0o7777 == named.st_mode & 0o7777 == 0o600)
            if self.database is not None:
                parent, current = _handoff_path_proof(self.database)
                valid = (valid and parent == self.parent_identity
                         and current == self.database_identity)
        except OSError as exc:
            raise ProtocolError("outbound dispatch lock or database path changed") from exc
        if not valid:
            raise ProtocolError("outbound dispatch lock or database path changed")


class OutboundQueue:
    """A host-owned fixture outbox. No method dispatches on enqueue."""

    def __init__(self, store: Store):
        self.store = store
        with self.store._source_write(), self.store._connect() as db:
            db.executescript("""
                CREATE TABLE IF NOT EXISTS outbound_policy_v1 (
                    destination_id TEXT PRIMARY KEY, action TEXT NOT NULL,
                    endpoint TEXT NOT NULL, plugin_id TEXT NOT NULL,
                    artifact_sha256 TEXT NOT NULL, policy_epoch INTEGER NOT NULL,
                    enabled INTEGER NOT NULL);
                CREATE TABLE IF NOT EXISTS outbound_delivery_v1 (
                    delivery_id TEXT PRIMARY KEY, destination_id TEXT NOT NULL,
                    action TEXT NOT NULL, value INTEGER NOT NULL,
                    idempotency_key TEXT NOT NULL UNIQUE, state TEXT NOT NULL,
                    attempts INTEGER NOT NULL, next_attempt_ns INTEGER NOT NULL,
                    policy_epoch INTEGER NOT NULL, code TEXT NOT NULL,
                    receipt TEXT NOT NULL, updated_ns INTEGER NOT NULL,
                    FOREIGN KEY(destination_id) REFERENCES outbound_policy_v1(destination_id));
                CREATE INDEX IF NOT EXISTS outbound_due_v1 ON
                    outbound_delivery_v1(destination_id,state,next_attempt_ns,updated_ns);
            """)

    def grant(self, destination_id: str, action: str, endpoint: str,
              manifest: PluginManifest, *, allowed_grants: tuple[str, ...]):
        if not isinstance(destination_id, str) or not _ID.fullmatch(destination_id):
            raise ValueError("invalid destination ID")
        if action != ACTION:
            raise ValueError("unsupported fixture action")
        endpoint = loopback_endpoint(endpoint)
        OutboundProcess(manifest, destination_id=destination_id, endpoint=endpoint,
                        allowed_grants=allowed_grants)
        with self.store._source_write(), self.store._connect() as db:
            db.execute("BEGIN IMMEDIATE")
            row = db.execute("SELECT * FROM outbound_policy_v1 WHERE destination_id=?",
                             (destination_id,)).fetchone()
            if row is not None and (row["action"], row["endpoint"], row["plugin_id"],
                                    row["artifact_sha256"]) != (
                    action, endpoint, manifest.id, manifest.artifact_sha256):
                raise ProtocolError("destination identity or action changed")
            epoch = 1 if row is None else row["policy_epoch"] + int(not row["enabled"])
            db.execute("INSERT INTO outbound_policy_v1 VALUES(?,?,?,?,?,?,1) "
                       "ON CONFLICT(destination_id) DO UPDATE SET "
                       "enabled=1,policy_epoch=excluded.policy_epoch",
                       (destination_id, action, endpoint, manifest.id,
                        manifest.artifact_sha256, epoch))
            return epoch

    def revoke(self, destination_id: str):
        with self.store._source_write(), self.store._connect() as db:
            db.execute("BEGIN IMMEDIATE")
            row = db.execute("SELECT policy_epoch,enabled FROM outbound_policy_v1 WHERE destination_id=?",
                             (destination_id,)).fetchone()
            if row is None:
                raise ProtocolError("unknown destination")
            if not row["enabled"]:
                return row["policy_epoch"]
            epoch = row["policy_epoch"] + 1
            db.execute("UPDATE outbound_policy_v1 SET enabled=0,policy_epoch=? WHERE destination_id=?",
                       (epoch, destination_id))
            db.execute("UPDATE outbound_delivery_v1 SET state='blocked',code='grant_revoked',"
                       "updated_ns=? WHERE destination_id=? AND state IN ('queued','retryable')",
                       (time.time_ns(), destination_id))
            return epoch

    def enqueue(self, delivery_id: str, destination_id: str, action: str, value: int,
                *, now_ns: int | None = None) -> dict:
        if not isinstance(delivery_id, str) or not _ID.fullmatch(delivery_id):
            raise ValueError("invalid delivery ID")
        if action != ACTION or type(value) is not int or not 0 <= value <= 1_000_000:
            raise ValueError("invalid fixture action")
        now = _timestamp(now_ns)
        with self.store._source_write(), self.store._connect() as db:
            db.execute("BEGIN IMMEDIATE")
            existing = db.execute("SELECT * FROM outbound_delivery_v1 WHERE delivery_id=?",
                                  (delivery_id,)).fetchone()
            if existing is not None:
                if (existing["destination_id"], existing["action"], existing["value"]) != (
                        destination_id, action, value):
                    raise ProtocolError("delivery identity reused with different action")
                return dict(existing)
            policy = db.execute("SELECT * FROM outbound_policy_v1 WHERE destination_id=?",
                                (destination_id,)).fetchone()
            if policy is None or not policy["enabled"] or policy["action"] != action:
                raise ProtocolError("destination action is not granted")
            key = secrets.token_hex(24)
            db.execute("INSERT INTO outbound_delivery_v1 VALUES(?,?,?,?,?,'queued',0,?,?,'','',?)",
                       (delivery_id, destination_id, action, value, key, now,
                        policy["policy_epoch"], now))
        return self.status(delivery_id)

    def status(self, delivery_id: str) -> dict:
        with self.store._connect() as db:
            row = db.execute("SELECT * FROM outbound_delivery_v1 WHERE delivery_id=?",
                             (delivery_id,)).fetchone()
        if row is None:
            raise ProtocolError("unknown delivery")
        return dict(row)

    @contextmanager
    def _dispatch_lease(self, *, exclusive: bool, timeout: float = 0,
                        handoff: bool = False):
        """Fence dispatch and reconciliation before taking a Store writer.

        Only the explicit handoff path waits for another owner. Ordinary
        dispatch/recovery retain their nonblocking behavior.
        """
        try:
            import fcntl
        except ImportError as exc:
            raise RuntimeError("outbound fixture requires POSIX file locks") from exc
        if type(timeout) not in (int, float) or not 0 <= timeout <= 60:
            raise ValueError("outbound dispatch lock requires a 0–60 second timeout")
        database = Path(self.store.path)
        parent_identity, database_identity = (
            _handoff_path_proof(database) if handoff else (None, None))
        path = Path(self.store.path + ".outbound-dispatch.lock")
        try:
            descriptor = os.open(path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC,
                                 0o600)
        except OSError as exc:
            raise ProtocolError("outbound dispatch lock cannot be opened safely") from exc
        try:
            guard = _DispatchLock(path, descriptor,
                                  database if handoff else None,
                                  parent_identity, database_identity)
            guard.verify()
            deadline = time.monotonic() + timeout
            while True:
                guard.verify()
                try:
                    fcntl.flock(descriptor, (fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH)
                                | fcntl.LOCK_NB)
                    break
                except BlockingIOError as exc:
                    if time.monotonic() >= deadline:
                        raise ProtocolError("outbound dispatch or recovery is active") from exc
                    time.sleep(min(0.01, max(0, deadline - time.monotonic())))
            guard.verify()
            if handoff and time.monotonic() > deadline:
                raise ProtocolError("outbound handoff lock deadline expired")
            try:
                yield guard
            finally:
                guard.verify()
        finally:
            os.close(descriptor)

    @contextmanager
    def dispatch_handoff_guard(self, timeout: float):
        """Hold cross-process dispatch exclusion for a future writer handoff.

        Acquire this guard before asking Store to quiesce. It does not fence
        grant/revoke/enqueue or certify that uncertain deliveries are settled.
        """
        if type(timeout) not in (int, float) or not 0 < timeout <= 60:
            raise ValueError("outbound handoff requires a positive timeout up to 60 seconds")
        with self._dispatch_lease(exclusive=True, timeout=timeout, handoff=True) as guard:
            yield guard

    def recover_in_flight(self):
        """Mark stranded claims uncertain only when no dispatch holds the lease."""
        with self._dispatch_lease(exclusive=True):
            with self.store._source_write(), self.store._connect() as db:
                db.execute("BEGIN IMMEDIATE")
                return db.execute("UPDATE outbound_delivery_v1 SET state='uncertain',"
                                  "code='response_unknown',updated_ns=? WHERE state='in_flight'",
                                  (time.time_ns(),)).rowcount

    def _compatible(self, policy, process: OutboundProcess):
        if (policy["destination_id"] != process.destination_id
                or policy["endpoint"] != process.endpoint
                or policy["plugin_id"] != process.manifest.id
                or policy["artifact_sha256"] != process.manifest.artifact_sha256):
            raise ProtocolError("action process differs from approved destination")

    def dispatch_one(self, destination_id: str, process: OutboundProcess,
                     *, now_ns: int | None = None) -> dict | None:
        now = _timestamp(now_ns)
        with self._dispatch_lease(exclusive=False):
            return self._dispatch_one_locked(destination_id, process, now)

    def _dispatch_one_locked(self, destination_id: str, process: OutboundProcess,
                             now: int) -> dict | None:
        blocked_id = None
        with self.store._source_write(), self.store._connect() as db:
            db.execute("BEGIN IMMEDIATE")
            policy = db.execute("SELECT * FROM outbound_policy_v1 WHERE destination_id=?",
                                (destination_id,)).fetchone()
            if policy is None:
                raise ProtocolError("unknown destination")
            self._compatible(policy, process)
            if not policy["enabled"]:
                return None
            row = db.execute("SELECT * FROM outbound_delivery_v1 WHERE destination_id=? "
                             "AND state IN ('queued','retryable') AND next_attempt_ns<=? "
                             "ORDER BY next_attempt_ns,updated_ns,delivery_id LIMIT 1",
                             (destination_id, now)).fetchone()
            if row is None:
                return None
            if row["policy_epoch"] != policy["policy_epoch"]:
                db.execute("UPDATE outbound_delivery_v1 SET state='blocked',code='policy_changed',"
                           "updated_ns=? WHERE delivery_id=?", (now, row["delivery_id"]))
                blocked_id = row["delivery_id"]
            else:
                db.execute("UPDATE outbound_delivery_v1 SET state='in_flight',attempts=attempts+1,"
                           "updated_ns=?,code='' WHERE delivery_id=?", (now, row["delivery_id"]))
        if blocked_id is not None:
            return self.status(blocked_id)
        delivery_id = row["delivery_id"]
        try:
            reply = process.dispatch(delivery_id, row["idempotency_key"], row["value"])
            status = reply["status"]
            code = reply["code"]
            receipt = reply["receipt"]
        except Exception:
            # After a durable claim, even an unexpected child failure cannot
            # prove the remote request did not run. Require lookup to resolve.
            status, code, receipt = "uncertain", "response_unknown", ""
        with self.store._source_write(), self.store._connect() as db:
            db.execute("BEGIN IMMEDIATE")
            current = db.execute("SELECT d.*,p.enabled FROM outbound_delivery_v1 d "
                                 "JOIN outbound_policy_v1 p USING(destination_id) WHERE delivery_id=?",
                                 (delivery_id,)).fetchone()
            if current["state"] != "in_flight":
                # Recovery may have conservatively marked a concurrent attempt
                # uncertain; never overwrite an unrelated terminal decision.
                return self.status(delivery_id)
            if status == "accepted":
                state, due = "delivered", now
            elif status == "uncertain":
                state, due = "uncertain", now
            elif not current["enabled"]:
                state, code, due = "blocked", "grant_revoked", now
            elif status == "terminal" or current["attempts"] >= MAX_ATTEMPTS:
                state, due = "dead_letter", now
            else:
                state, due = "retryable", now + min(60, 2 ** current["attempts"]) * 1_000_000_000
            db.execute("UPDATE outbound_delivery_v1 SET state=?,code=?,receipt=?,"
                       "next_attempt_ns=?,updated_ns=? WHERE delivery_id=?",
                       (state, code, receipt, due, time.time_ns(), delivery_id))
        return self.status(delivery_id)

    def reconcile(self, delivery_id: str, process: OutboundProcess,
                  *, now_ns: int | None = None) -> dict:
        now = _timestamp(now_ns)
        # Lookup may make an external request after the first Store read and
        # before the final Store write. Keep it visible to a handoff guard.
        with self._dispatch_lease(exclusive=False):
            return self._reconcile_locked(delivery_id, process, now)

    def _reconcile_locked(self, delivery_id: str, process: OutboundProcess,
                          now: int) -> dict:
        with self.store._connect() as db:
            row = db.execute("SELECT d.*,p.endpoint,p.plugin_id,p.artifact_sha256,"
                             "p.enabled,p.policy_epoch AS current_epoch FROM outbound_delivery_v1 d "
                             "JOIN outbound_policy_v1 p USING(destination_id) WHERE delivery_id=?",
                             (delivery_id,)).fetchone()
        if row is None or row["state"] != "uncertain":
            raise ProtocolError("delivery is not uncertain")
        self._compatible(row, process)
        try:
            reply = process.lookup(row["idempotency_key"])
        except Exception:
            return self.status(delivery_id)
        with self.store._source_write(), self.store._connect() as db:
            db.execute("BEGIN IMMEDIATE")
            current = db.execute("SELECT d.state,d.attempts,p.enabled,p.policy_epoch FROM outbound_delivery_v1 d "
                                 "JOIN outbound_policy_v1 p USING(destination_id) WHERE delivery_id=?",
                                 (delivery_id,)).fetchone()
            if current["state"] != "uncertain":
                return self.status(delivery_id)
            if reply["status"] == "accepted":
                state, code, receipt = "delivered", "reconciled", reply["receipt"]
            elif not current["enabled"] or current["policy_epoch"] != row["policy_epoch"]:
                state, code, receipt = "blocked", "grant_revoked", ""
            elif current["attempts"] >= MAX_ATTEMPTS:
                state, code, receipt = "dead_letter", "confirmed_missing", ""
            else:
                state, code, receipt = "retryable", "confirmed_missing", ""
            db.execute("UPDATE outbound_delivery_v1 SET state=?,code=?,receipt=?,"
                       "next_attempt_ns=?,updated_ns=? WHERE delivery_id=?",
                       (state, code, receipt, now, time.time_ns(), delivery_id))
        return self.status(delivery_id)
