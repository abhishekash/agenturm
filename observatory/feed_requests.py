"""Bounded concurrent reads and immutable scope for a single feed refresh."""
from concurrent.futures import Future, ThreadPoolExecutor, TimeoutError as FutureTimeout
from dataclasses import dataclass
import threading


_slots_lock = threading.Lock()

def shared_get(api, path, *, review=False, cancelled=None):
    """Share four active reads, and deduplicate overlapping identical reviews."""
    if not review:return _bounded_get(api,path,review=False,cancelled=cancelled)
    with _slots_lock:
        pending=getattr(api,'_agenturm_review_pending',None)
        if pending is None:
            pending={};setattr(api,'_agenturm_review_pending',pending)
        future=pending.get(path)
        leader=future is None
        if leader:
            future=Future();pending[path]=future
    if not leader:
        while True:
            if cancelled and cancelled():raise InterruptedError('Evidence request cancelled')
            try:return future.result(timeout=.1)
            except FutureTimeout:
                if future.done():raise
    try:
        result=_bounded_get(api,path,review=True,cancelled=cancelled)
        future.set_result(result)
        return result
    except BaseException as exc:
        future.set_exception(exc)
        raise
    finally:
        with _slots_lock:
            if pending.get(path) is future:pending.pop(path,None)


def _bounded_get(api,path,*,review,cancelled):
    """The leader alone holds a network slot; waiting readers cannot deadlock it."""
    with _slots_lock:
        slots = getattr(api, '_agenturm_read_slots', None)
        if slots is None:
            slots = threading.BoundedSemaphore(4)
            setattr(api, '_agenturm_read_slots', slots)
    while not slots.acquire(timeout=.1):
        if cancelled and cancelled():raise InterruptedError('Evidence request cancelled')
    try:
        if cancelled and cancelled():raise InterruptedError('Evidence request cancelled')
        return (api.get_review if review and callable(getattr(api, 'get_review', None)) else api.get)(path)
    finally:
        slots.release()


@dataclass(frozen=True)
class ConnectionProgress:
    phase: str = 'connecting'
    stage: str = 'Checking local receiver'
    attempt: int = 1
    started: float = 0
    retry_at: float = 0


def request_stage(path):
    if path=='/api/health?probe=1':return 'Checking receiver'
    path=path.split('?',1)[0]
    return {'/api/health':'Loading local collection status', '/api/metrics':'Loading usage totals',
            '/api/connections':'Loading agents', '/api/activity':'Loading activity',
            '/api/sessions':'Loading sessions', '/api/mcp':'Loading tool calls',
            '/api/traces':'Loading traces', '/api/findings':'Loading findings',
            '/api/metric-catalog':'Loading metrics', '/api/measurements':'Loading measurements',
            '/api/usage-series':'Loading usage history'}.get(path,'Loading trace evidence')


@dataclass(frozen=True)
class FeedScope:
    service: str
    session_id: str
    query: str
    activity_query: str
    mcp_query: str
    errors_only: bool
    include_reviews: bool
    activity_prompts: bool
    activity_errors: bool
    selected_trace: str | None
    selected_span_id: str | None
    detail_enabled: bool
    include_global_totals: bool
    active_page: str | None
    pinned_trace: bool
    scope_agent_id: str
    scope_subtree: bool
    hierarchy_revision: str
    generation: int

    @classmethod
    def capture(cls, feed):
        return cls(**{name: getattr(feed, name) for name in cls.__dataclass_fields__})


class RequestBatch:
    """Prefetch only known independent reads; dependent reads remain explicit.

    Workers never mutate the feed or publish snapshots. All results are consumed
    by the polling thread, and outstanding reads are cancelled on shutdown.
    """
    MAX_WORKERS = 4
    MAX_PREFETCH = 16

    def __init__(self, api, stop, progress=None):
        self.api, self.stop = api, stop
        self.progress = progress
        self.executor = None
        self.pending = {}

    def prefetch(self, paths):
        for path in paths:
            if self.stop.is_set():
                break
            if path in self.pending:
                continue
            if len(self.pending) >= self.MAX_PREFETCH:
                raise ValueError('feed prefetch budget exceeded')
            if self.executor is None:
                self.executor = ThreadPoolExecutor(max_workers=self.MAX_WORKERS,
                                                   thread_name_prefix='agenturm-read')
            self.pending[path] = self.executor.submit(shared_get, self.api, path, cancelled=self.stop.is_set)

    def get(self, path):
        if self.stop.is_set():
            raise InterruptedError('feed stopped')
        if self.progress:self.progress(request_stage(path))
        future = self.pending.get(path)
        try:
            return future.result() if future is not None else shared_get(self.api, path, cancelled=self.stop.is_set)
        except TimeoutError as exc:
            raise TimeoutError(request_stage(path)+' timed out ('+path.split('?',1)[0]+')') from exc

    def cancel(self):
        for future in tuple(self.pending.values()):
            future.cancel()

    def close(self):
        self.cancel()
        if self.executor is not None:
            self.executor.shutdown(wait=True, cancel_futures=True)
