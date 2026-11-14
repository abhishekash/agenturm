"""Generated-only real HTTP/native startup races; no provider or discovery."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack
from dataclasses import asdict
from datetime import datetime, timezone
import json
from pathlib import Path
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from observatory.agent_hierarchy import canonical_agent_id
from observatory.core import Store
from observatory.local_agents import LocalAgents, Session
from observatory.server import BoundedHTTPServer, handler_for
from observatory.terminal import Api, Feed


PRIVATE = 'GENERATED_PRIVATE_STARTUP_REQUEST'
TOKEN = 'generated-startup-integration-token'


def wait_until(predicate, seconds=8):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.01)
    raise AssertionError('Generated startup condition was not reached')


def seed_history(path):
    """Real raw tables/triggers, >200 Activity jobs and >500 endpoint groups."""
    seed = Store(path, workflow_patterns=False)
    try:
        with seed._connect() as db:
            db.executemany('''INSERT INTO spans
                (trace_id,span_id,name,start_ns,end_ns,status,service,attributes,events,resource_attributes)
                VALUES(?,?,?,?,?,?,?,?,?,?)''', [
                    ('a' * 32, f'{number + 1:016x}', 'generated-history', number + 1,
                     number + 2, 'OK', 'generated-history', '{}', '[]',
                     json.dumps({'device.id': f'generated-device-{number}'}))
                    for number in range(501)])
            db.executemany('''INSERT INTO logs
                (digest,trace_id,span_id,time_ns,service,severity_number,severity_text,
                 event_name,body,attributes,resource_attributes,scope_name)
                VALUES(?,?,?,?,?,?,?,?,?,?,?,?)''', [
                    (f'{number + 1:064x}', None, None, number + 1, 'generated-history',
                     9, 'INFO', 'generated-history', '"Generated history"', '{}',
                     json.dumps({'device.id': 'generated-log-device'}), 'generated')
                    for number in range(401)])
            db.execute("DELETE FROM schema_meta WHERE key='endpoint_inventory_v1'")
    finally:
        seed.close()


def write_native(path, native_id, parent=None):
    stamp = datetime.now(timezone.utc).isoformat()
    meta = {'id': native_id, 'source': 'cli'}
    if parent:
        meta.update(session_id=parent, parent_thread_id=parent,
                    source={'subagent': {'thread_spawn': {'parent_thread_id': parent}}})
    rows = [dict(type='session_meta', timestamp=stamp, payload=meta),
            dict(type='event_msg', timestamp=stamp,
                 payload=dict(type='task_started', turn_id='generated-turn-' + native_id)),
            dict(type='event_msg', timestamp=stamp,
                 payload=dict(type='user_message', message=PRIVATE))]
    path.write_text(''.join(json.dumps(row) + '\n' for row in rows))


class RecordingServer(BoundedHTTPServer):
    def __init__(self, *args, **kwargs):
        self.uncaught = []
        super().__init__(*args, **kwargs)

    def handle_error(self, request, client_address):
        # Keep only an exception class, never source bodies or request headers.
        self.uncaught.append(type(sys.exception()).__name__)


class StartupReceiverHealthTests(unittest.TestCase):
    def test_native_writer_health_and_multibatch_history_publish_only_qualified_data(self):
        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            root = Path(directory)
            database = root / 'generated.db'
            seed_history(database)
            launch = threading.Event()
            entered = threading.Event()
            release_native = threading.Event()
            yielded = threading.Event()
            release_history = threading.Event()
            original_migrate = Store._finish_migration

            def delayed_migration(store):
                if not launch.wait(8):
                    raise AssertionError('Generated migration launch missing')
                return original_migrate(store)

            stack.enter_context(patch.object(Store, '_finish_migration', delayed_migration))
            store = Store(database, async_derived_refresh=True, workflow_patterns=False)
            monitor = server = server_thread = None
            executor = ThreadPoolExecutor(max_workers=2)
            original_policy = store._policy_records
            original_wait = store._migration_stop.wait
            original_update = store._update_endpoints
            endpoint_pages = []
            held_once = False
            health_entered = threading.Event()
            original_nodes = store.upsert_agent_nodes

            def held_source(*args, **kwargs):
                nonlocal held_once
                if threading.current_thread().name == 'observatory-local-agents' and not held_once:
                    held_once = True
                    entered.set()
                    if not release_native.wait(8):
                        raise AssertionError('Generated native writer release missing')
                return original_policy(*args, **kwargs)

            def nodes(*args, **kwargs):
                if threading.current_thread().name.startswith('Thread-'):
                    health_entered.set()
                return original_nodes(*args, **kwargs)

            def history_wait(seconds):
                if threading.current_thread().name == 'agenturm-history-migration' and seconds == .025:
                    yielded.set()
                    if not release_history.wait(8):
                        raise AssertionError('Generated history release missing')
                return original_wait(seconds)

            def endpoints(db, records):
                if threading.current_thread().name == 'agenturm-history-migration':
                    endpoint_pages.append(len(records))
                return original_update(db, records)

            stack.enter_context(patch.object(store, '_policy_records', side_effect=held_source))
            stack.enter_context(patch.object(store, 'upsert_agent_nodes', side_effect=nodes))
            stack.enter_context(patch.object(store._migration_stop, 'wait', side_effect=history_wait))
            stack.enter_context(patch.object(store, '_update_endpoints', side_effect=endpoints))
            stack.enter_context(patch('observatory.opencode.database_path', return_value=root / 'absent-opencode.db'))
            stack.enter_context(patch('observatory.local_runtime.project_directories', return_value=set()))
            stack.enter_context(patch('observatory.local_agents.discover_mcp_configurations', return_value=[]))
            try:
                monitor = LocalAgents(store, capture_details=True)
                for name, parent in [('generated-parent', None), ('generated-child', 'generated-parent')]:
                    source = root / (name + '.jsonl')
                    write_native(source, name, parent)
                    monitor.sessions[source] = Session(store, source, 'Codex', capture_details=True)
                monitor.discover = lambda: None
                monitor.opencode.poll = lambda: 0
                monitor.opencode.set_mcp_servers = lambda *args: None
                monitor.runtime_updated = time.monotonic() + 3600
                store.local_agent_monitor = monitor
                server = RecordingServer(('127.0.0.1', 0), handler_for(store, TOKEN, quiet=True))
                server_thread = threading.Thread(target=server.serve_forever, name='generated-http-server')
                server_thread.start()
                endpoint = f'http://127.0.0.1:{server.server_port}'
                api = Api(endpoint, token=TOKEN)
                # The route is genuinely authenticated, not a direct Store call.
                with self.assertRaises(HTTPError) as denied:
                    urlopen(Request(endpoint + '/api/health'), timeout=3)
                self.assertEqual(denied.exception.code, 401)
                denied.exception.close()
                monitor.start()
                self.assertTrue(entered.wait(3))
                launch.set()
                pending_health = executor.submit(api.get, '/api/health')
                self.assertTrue(health_entered.wait(3))
                self.assertFalse(pending_health.done())
                self.assertFalse(monitor.initial_poll_complete)
                self.assertFalse(store.migration_status['ready'])
                feed = Feed(api)
                self.assertEqual(feed.snapshot.updated, 0)
                self.assertNotIn(PRIVATE, json.dumps(asdict(feed.snapshot)))
                # Let the real native writer commit; the migration must yield
                # outside its transaction before this health can make progress.
                release_native.set()
                self.assertTrue(yielded.wait(5))
                health = pending_health.result(timeout=5)
                self.assertEqual(health['receiver_id'], store.receiver_id)
                self.assertFalse(health['indexing']['ready'])
                wait_until(lambda: monitor.initial_poll_complete)
                health = api.get('/api/health')
                self.assertTrue(health['local_agents']['initial_poll_complete'])
                self.assertFalse(health['indexing']['ready'])
                parent = canonical_agent_id('Codex', 'codex-rollout', 'generated-parent')
                child = canonical_agent_id('Codex', 'codex-rollout', 'generated-child')
                nodes = {row['agent_id']: row for row in store.agent_nodes()}
                self.assertEqual(nodes[child]['parent_agent_id'], parent)
                self.assertEqual(nodes[child]['relationship_state'], 'confirmed')
                native_rows = {row['agent_id']: row for row in health['local_agents']['sessions']}
                self.assertEqual(native_rows[child]['parent_agent_id'], parent)
                revision = store.hierarchy_revision()
                self.assertEqual(health['local_agents']['hierarchy']['revision'], revision)
                api.get('/api/health')
                self.assertEqual(store.hierarchy_revision(), revision)
                feed.refresh()
                self.assertTrue(feed.snapshot.native_bootstrap, feed.snapshot.error)
                self.assertEqual(feed.snapshot.updated, 0)
                self.assertFalse(feed.snapshot.indexing_ready)
                self.assertNotIn(PRIVATE, json.dumps(asdict(feed.snapshot)))
                self.assertEqual(len(feed.snapshot.local_status['sessions']), 2)
                self.assertFalse(feed.snapshot.sessions)
                self.assertFalse(feed.snapshot.activity)
                with self.assertRaises(HTTPError) as unavailable:
                    api.get('/api/sessions?limit=500')
                self.assertEqual(unavailable.exception.code, 503)
                pending_body = json.load(unavailable.exception)
                unavailable.exception.close()
                self.assertFalse(pending_body['indexing']['ready'])
                self.assertNotIn('sessions', pending_body)
                release_history.set()
                wait_until(lambda: store.index_status().get('ready') is True)
                final_health = api.get('/api/health')
                self.assertTrue(final_health['indexing']['ready'])
                self.assertTrue(api.get('/api/sessions?limit=500')['sessions'])
                self.assertEqual(final_health['local_agents']['read_errors'], 0)
                self.assertEqual(final_health['local_agents']['hierarchy']['write_errors'], 0)
                self.assertIsNone(store.last_derived_error)
                self.assertIn(500, endpoint_pages)
                self.assertGreaterEqual(len(endpoint_pages), 2)
                self.assertFalse(server.uncaught)
                with store._read() as db:
                    self.assertEqual(db.execute("SELECT COUNT(*) FROM spans WHERE service='generated-history'").fetchone()[0], 501)
                    self.assertEqual(db.execute("SELECT COUNT(*) FROM logs WHERE service='generated-history'").fetchone()[0], 401)
                    self.assertIsNotNone(db.execute("SELECT 1 FROM schema_meta WHERE key='endpoint_inventory_v1'").fetchone())
            finally:
                release_native.set()
                release_history.set()
                launch.set()
                if monitor:
                    monitor.close()
                if server:
                    server.shutdown()
                    server.server_close()
                if server_thread:
                    server_thread.join(5)
                executor.shutdown(wait=True, cancel_futures=True)
                store.close()
            self.assertFalse(server_thread.is_alive())
            self.assertFalse(monitor.thread.is_alive())
            self.assertFalse(store._migration_thread.is_alive())
            self.assertTrue(store._derived_thread is None or not store._derived_thread.is_alive())
            self.assertFalse(server.uncaught)


    def test_stop_during_real_history_yield_preserves_queue_and_restart_completes(self):
        from contextlib import closing
        import sqlite3
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / 'generated.db'
            seed_history(database)
            launch = threading.Event()
            yielded = threading.Event()
            release = threading.Event()
            original_migrate = Store._finish_migration
            def delayed(store):
                if not launch.wait(5):
                    raise AssertionError('Generated migration launch missing')
                return original_migrate(store)
            with patch.object(Store, '_finish_migration', delayed):
                store = Store(database, async_derived_refresh=True, workflow_patterns=False)
            original_wait = store._migration_stop.wait
            with store._read() as db:
                queued_before = db.execute('SELECT COUNT(*) FROM activity_dirty_v1').fetchone()[0]
            self.assertGreater(queued_before, 200)
            def stopped_wait(seconds):
                if threading.current_thread().name == 'agenturm-history-migration' and seconds == .025:
                    yielded.set()
                    if not release.wait(5):
                        raise AssertionError('Generated history release missing')
                return original_wait(seconds)
            closer = None
            shutdown_errors = []
            def close_store():
                try:
                    store.close()
                except BaseException as exc:
                    shutdown_errors.append(exc)
            try:
                with patch.object(store._migration_stop, 'wait', side_effect=stopped_wait):
                    launch.set()
                    self.assertTrue(yielded.wait(3))
                    closer = threading.Thread(target=close_store, name='generated-store-close')
                    closer.start()
                    self.assertTrue(store._migration_stop.wait(1))
                    self.assertTrue(store._writer_gate.acquire(timeout=1), 'Fairness yield retained local writer admission')
                    store._writer_gate.release()
                    release.set()
                    closer.join(5)
                    self.assertFalse(closer.is_alive())
                    self.assertFalse(shutdown_errors)
                self.assertFalse(store._migration_thread.is_alive())
                self.assertFalse(store.migration_status['ready'])
                with closing(sqlite3.connect(database)) as db:
                    self.assertEqual(db.execute('SELECT COUNT(*) FROM activity_dirty_v1').fetchone()[0], queued_before - 200)
                    self.assertIsNone(db.execute("SELECT 1 FROM schema_meta WHERE key='endpoint_inventory_v1'").fetchone())
                resumed = Store(database, workflow_patterns=False)
                try:
                    self.assertTrue(resumed.index_status()['ready'])
                    self.assertIsNone(resumed.last_derived_error)
                    with resumed._read() as db:
                        self.assertEqual(db.execute("SELECT COUNT(*) FROM spans WHERE service='generated-history'").fetchone()[0], 501)
                        self.assertEqual(db.execute("SELECT COUNT(*) FROM logs WHERE service='generated-history'").fetchone()[0], 401)
                        self.assertEqual(db.execute('SELECT COUNT(*) FROM activity_dirty_v1').fetchone()[0], 0)
                        self.assertIsNotNone(db.execute("SELECT 1 FROM schema_meta WHERE key='endpoint_inventory_v1'").fetchone())
                finally:
                    resumed.close()
            finally:
                launch.set()
                release.set()
                if closer:
                    closer.join(5)
                store.close()


if __name__ == '__main__':
    unittest.main()
