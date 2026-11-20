"""Small exclusive leases for cheap, read-only receiver metadata queries.

This retains connections, never query results or open read transactions. Writers
keep their separate close-on-context connections. Atomic file replacement drops
the previous pool before any subsequent checkout.
"""
from contextlib import contextmanager
from pathlib import Path
import sqlite3
import threading


class ReadLease:
    """Track cursors so a partially consumed SELECT cannot pin a WAL snapshot."""
    def __init__(self, db):
        self._db = db
        self._cursors = []
        self._active = True
        self._finish_failed = False

    def execute(self, sql, parameters=()):
        if not self._active:raise sqlite3.ProgrammingError('Read lease has ended')
        cursor = self._db.execute(sql, parameters)
        self._cursors.append(cursor)
        return cursor

    @property
    def in_transaction(self):
        return self._db.in_transaction

    def set_trace_callback(self, callback):
        if not self._active:raise sqlite3.ProgrammingError('Read lease has ended')
        self._db.set_trace_callback(callback)

    def finish(self):
        self._active = False
        try:
            try:
                for cursor in self._cursors:cursor.close()
            finally:
                self._cursors.clear()
                self._db.set_trace_callback(None)
                self._db.rollback()
        except BaseException:
            self._finish_failed = True
            raise


class ReadConnections:
    MAX_IDLE = 4

    def __init__(self, path):
        # Keep the caller's lexical path: an explicit --db symlink may be
        # retargeted just like an ordinary database can be atomically replaced.
        self.path = Path(path).absolute()
        self.lock = threading.Lock()
        self._drained = threading.Condition(self.lock)
        self._active_leases = 0
        self._cleanup_failed = False
        self.idle = []
        self.identity = None
        self.closed = False

    def _identity(self):
        info = self.path.stat()
        return info.st_dev, info.st_ino

    def _discard_idle(self):
        pending, self.idle = self.idle, []
        first_error = None
        for db in pending:
            try:
                db.close()
            except BaseException as exc:
                self._cleanup_failed = True
                if first_error is None:
                    first_error = exc
        if first_error is not None:
            raise first_error

    @contextmanager
    def read(self):
        with self.lock:
            if self.closed:raise sqlite3.ProgrammingError('Receiver read connections are closed')
            try:identity = self._identity()
            except OSError:
                self._discard_idle()
                self.identity = None
                raise
            if identity != self.identity:
                self._discard_idle()
                self.identity = identity
            if self.idle:
                db = self.idle.pop()
            else:
                db = sqlite3.connect(self.path.as_uri() + '?mode=ro', uri=True,
                                     timeout=10, check_same_thread=False)
                try:
                    db.row_factory = sqlite3.Row
                    db.execute('PRAGMA query_only=ON').close()
                    if self._identity() != identity:
                        raise sqlite3.OperationalError('Receiver database replaced while connecting; retry')
                except BaseException:
                    try:
                        db.close()
                    except BaseException:
                        self._cleanup_failed = True
                        raise
                    raise
            try:
                lease = ReadLease(db)
            except BaseException:
                try:
                    db.close()
                except BaseException:
                    self._cleanup_failed = True
                    raise
                raise
            self._active_leases += 1
        reusable = False
        try:
            yield lease
            # Even an explicit BEGIN must not pin a WAL snapshot between reads.
            lease.finish()
            if self._identity() != identity:
                raise sqlite3.OperationalError('Receiver database replaced during read; retry')
            reusable = True
        finally:
            try:
                if lease._active:lease.finish()
            finally:
                with self.lock:
                    try:
                        if reusable and not self.closed and self.identity == identity and len(self.idle) < self.MAX_IDLE:
                            self.idle.append(db)
                        else:db.close()
                    except BaseException:
                        self._cleanup_failed = True
                        raise
                    finally:
                        if lease._finish_failed:
                            self._cleanup_failed = True
                        self._active_leases -= 1
                        if not self._active_leases:
                            self._drained.notify_all()

    def close(self):
        with self.lock:
            self.closed = True
            try:
                self._discard_idle()
            finally:
                self._drained.notify_all()

    def begin_close(self, timeout: float) -> bool:
        """Feature-off handoff close with a bounded lock-acquisition wait.

        A failed lock acquisition leaves the pool open for a later retry. It
        cannot preempt an SQLite handle already inside ``close()``.
        """
        if type(timeout) not in (int, float) or not 0 <= timeout <= 60:
            raise ValueError('Read pool close requires a timeout from 0 to 60 seconds')
        if not self.lock.acquire(timeout=float(timeout)):
            return False
        try:
            self.closed = True
            try:
                self._discard_idle()
            finally:
                self._drained.notify_all()
            return True
        finally:
            self.lock.release()

    def wait_closed(self, timeout: float) -> bool:
        """Wait at most ``timeout`` seconds for close and all leased reads.

        A caller may start waiting before close. A timeout never cancels an
        active read; the caller must keep its writer fence until this is true.
        """
        if type(timeout) not in (int, float) or not 0 <= timeout <= 60:
            raise ValueError('Read lease drain requires a timeout from 0 to 60 seconds')
        with self._drained:
            return self._drained.wait_for(
                lambda: self.closed and self._active_leases == 0 and not self._cleanup_failed,
                timeout=float(timeout),
            )
