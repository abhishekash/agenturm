"""Bounded concurrent reads and immutable scope for a single feed refresh."""
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass


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
            self.pending[path] = self.executor.submit(self.api.get, path)

    def get(self, path):
        if self.stop.is_set():
            raise InterruptedError('feed stopped')
        if self.progress:self.progress(request_stage(path))
        future = self.pending.get(path)
        try:
            return future.result() if future is not None else self.api.get(path)
        except TimeoutError as exc:
            raise TimeoutError(request_stage(path)+' timed out ('+path.split('?',1)[0]+')') from exc

    def cancel(self):
        for future in tuple(self.pending.values()):
            future.cancel()

    def close(self):
        self.cancel()
        if self.executor is not None:
            self.executor.shutdown(wait=True, cancel_futures=True)
