"""Feature-off connection drain for one auxiliary SQLite store instance.

This is a cooperating, instance-local handoff fence. It does not isolate an
older process, another instance, or direct module-level SQLite callers.
"""
from __future__ import annotations

import threading
import time


class AuxiliaryStoreHandoff:
    def __init__(self):
        self._condition = threading.Condition()
        self._requested = False
        self._active = 0
        self._cleanup_failed = False

    def admit(self) -> bool:
        with self._condition:
            if self._requested:
                return False
            self._active += 1
            return True

    def cleanup_failed(self) -> None:
        with self._condition:
            self._cleanup_failed = True
            self._condition.notify_all()

    def release(self) -> None:
        with self._condition:
            self._active -= 1
            self._condition.notify_all()

    def quiesce(self, timeout: float) -> bool:
        if type(timeout) not in (int, float) or not 0 < timeout <= 60:
            raise ValueError('Auxiliary store handoff requires a timeout above 0 and at most 60 seconds')
        deadline = time.monotonic() + timeout
        with self._condition:
            self._requested = True
            self._condition.wait_for(
                lambda: self._active == 0 or self._cleanup_failed,
                timeout=max(0.0, deadline - time.monotonic()),
            )
            return (self._active == 0 and not self._cleanup_failed
                    and time.monotonic() <= deadline)
