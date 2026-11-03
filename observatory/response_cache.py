"""Bounded serialized API responses, invalidated by committed SQLite writes."""
from collections import OrderedDict
from pathlib import Path
import sqlite3
import threading


class ResponseCache:
    MAX_ENTRIES = 32
    MAX_BYTES = 4 * 1024 * 1024
    MAX_ENTRY_BYTES = 1024 * 1024
    PATHS = frozenset(('/api/activity', '/api/metrics', '/api/mcp',
                       '/api/measurements', '/api/metric-catalog', '/api/findings',
                       '/api/sessions', '/api/traces'))

    def __init__(self, path):
        self.path = path
        self.lock = threading.Lock()
        self.db = None
        self.identity = None
        self.version = None
        self.entries = OrderedDict()
        self.size = 0
        self.closed = False

    def eligible(self, path):
        # Health, connections and rolling usage windows depend on time or
        # external processes. They must always be computed afresh.
        return path in self.PATHS or path.startswith('/api/traces/')

    def _version(self):
        if self.closed:
            return None
        stat = Path(self.path).stat()
        identity = (stat.st_dev, stat.st_ino)
        if self.identity != identity:
            if self.db is not None:
                self.db.close()
            self.db = None
            self.version = None
            self.identity = identity
        if self.db is None:
            self.db = sqlite3.connect(Path(self.path).resolve().as_uri() + '?mode=ro',
                                      uri=True, check_same_thread=False, timeout=.1)
        # data_version is comparable only on the SAME connection. This one
        # never writes; commits from Store, derived workers, and other processes
        # all invalidate the cache. No read transaction is kept open.
        version = (identity, self.db.execute('PRAGMA data_version').fetchone()[0])
        if version != self.version:
            self.entries.clear()
            self.size = 0
            self.version = version
        return version

    def get(self, key):
        with self.lock:
            try:
                version = self._version()
            except (sqlite3.Error, OSError):
                return None, None
            data = self.entries.get(key)
            if data is not None:
                self.entries.move_to_end(key)
            return version, data

    def put(self, key, version, data):
        if version is None or len(data) > self.MAX_ENTRY_BYTES:
            return
        with self.lock:
            try:
                if self._version() != version:
                    return  # A write raced the query; do not retain its result.
            except (sqlite3.Error, OSError):
                return
            previous = self.entries.pop(key, None)
            if previous is not None:
                self.size -= len(previous)
            self.entries[key] = data
            self.size += len(data)
            while len(self.entries) > self.MAX_ENTRIES or self.size > self.MAX_BYTES:
                _, removed = self.entries.popitem(last=False)
                self.size -= len(removed)

    def close(self):
        with self.lock:
            self.closed = True
            self.entries.clear()
            self.size = 0
            if self.db is not None:
                self.db.close()
                self.db = None
