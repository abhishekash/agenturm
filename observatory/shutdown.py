"""Finish owned-resource cleanup without a second Ctrl-C aborting it."""
from __future__ import annotations

import signal
import threading
from contextlib import ExitStack, contextmanager


@contextmanager
def _ignore_cleanup_interrupts():
    # Only the main thread can manage signal handlers. SIGTERM and normal
    # application-time SIGINT behavior are intentionally left alone.
    main = threading.current_thread() is threading.main_thread()
    previous = signal.signal(signal.SIGINT, signal.SIG_IGN) if main else None
    try:
        yield
    finally:
        if main:
            signal.signal(signal.SIGINT, previous)


def complete_cleanup(*callbacks):
    """Run callbacks in order, restoring SIGINT even if a closer fails.

    ExitStack ensures a real cleanup error does not skip later resources;
    errors still propagate. Nested calls restore the enclosing handler.
    """
    with _ignore_cleanup_interrupts(), ExitStack() as cleanup:
        for callback in reversed(callbacks):
            cleanup.callback(callback)
