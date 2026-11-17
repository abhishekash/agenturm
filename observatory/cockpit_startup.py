"""Receiver setup owned by the feed worker, never the terminal event loop."""
import threading
import urllib.error


class CockpitStartup:
    def __init__(self, api, db_path, port, *, endpoint=None, **receiver_options):
        self.api = api
        self.db_path = db_path
        self.port = port
        self.endpoint = endpoint
        self.options = receiver_options
        self._lock = threading.Lock()
        self._closed = False
        self._receiver = None
        self._failure = None

    def __call__(self, feed):
        from .terminal import CockpitError, LocalReceiver
        with self._lock:
            if self._closed or feed.stop.is_set():
                raise InterruptedError('Receiver startup cancelled')
        if self._failure is not None:
            raise CockpitError(self._failure)
        # An explicit endpoint needs no local setup. Feed owns its liveness,
        # authentication and full-health reads, including reconnect attempts.
        if self.endpoint and not self.options.get('metadata_only'):
            return False
        feed.connection_stage('Checking receiver')
        try:
            health = self.api.get('/api/health?probe=1')
        except (urllib.error.URLError, OSError) as exc:
            reason = exc.reason if isinstance(exc, urllib.error.URLError) else exc
            # A slow or unauthorized live receiver is not an absent receiver.
            if self.endpoint or not isinstance(reason, ConnectionRefusedError):
                raise
            feed.connection_stage('Starting local receiver')
            from .receiver_state import local_token
            self.api.token = self.api.token or local_token(self.port, create=True)
            if feed.stop.is_set():
                raise InterruptedError('Receiver startup cancelled')
            try:
                receiver = LocalReceiver(self.db_path, self.port, self.api.token, **self.options)
            except Exception as error:
                self._failure = 'Cannot start local receiver: ' + str(error)
                raise CockpitError(self._failure) from error
            with self._lock:
                cancelled = self._closed or feed.stop.is_set()
                if not cancelled:
                    self._receiver = receiver
            if cancelled:
                receiver.close()
                raise InterruptedError('Receiver startup cancelled')
            return True
        if not health.get('ok'):
            raise CockpitError('The endpoint is not a ready Agenturm receiver.')
        if self.options.get('metadata_only'):
            # Preserve the opt-in capture check before permitting data reads.
            feed.connection_stage('Checking receiver capture policy')
            health = self.api.get('/api/health')
            if health.get('local_agents', {}).get('capture_content'):
                self._failure = 'The existing receiver captures session content. Restart it with serve --metadata-only, then reconnect.'
                raise CockpitError(self._failure)
        return True

    def close(self):
        # A constructor finishing after quit owns closing its late receiver.
        # Never hold this lock across database initialization or shutdown.
        with self._lock:
            self._closed = True
            receiver, self._receiver = self._receiver, None
        if receiver is not None:
            receiver.close()
