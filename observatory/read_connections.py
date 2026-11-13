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
            for cursor in self._cursors:cursor.close()
        finally:
            self._cursors.clear()
            self._db.set_trace_callback(None)
            self._db.rollback()


class ReadConnections:
    MAX_IDLE = 4

    def __init__(self, path):
        # Keep the caller's lexical path: an explicit --db symlink may be
        # retargeted just like an ordinary database can be atomically replaced.
        self.path = Path(path).absolute()
        self.lock = threading.Lock()
        self.idle = []
        self.identity = None
        self.closed = False

    def _identity(self):
        info = self.path.stat()
        return info.st_dev, info.st_ino

    def _discard_idle(self):
        for db in self.idle:db.close()
        self.idle.clear()

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
                    db.close()
                    raise
        lease = ReadLease(db)
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
                    if reusable and not self.closed and self.identity == identity and len(self.idle) < self.MAX_IDLE:
                        self.idle.append(db)
                    else:db.close()

    def close(self):
        with self.lock:
            self.closed = True
            self._discard_idle()
