"""Startup must not perform receiver work before the cockpit can render."""
import asyncio
import os
import signal
import subprocess
import sys
import tempfile
import textwrap
import threading
import unittest
import urllib.error
from pathlib import Path
from unittest.mock import MagicMock, patch

from textual.widgets import Static, TabbedContent
from observatory.cockpit_startup import CockpitStartup
from observatory.terminal import Api, Feed, run_cockpit
from observatory.terminal_ui import ObservatoryApp
from tests.test_terminal_search import ExampleApi


class CockpitStartupTests(unittest.TestCase):
    def test_interactive_shell_opens_before_any_receiver_request(self):
        with patch('sys.stdin.isatty', return_value=True), \
             patch('sys.stdout.isatty', return_value=True), \
             patch.object(Api, 'get', return_value={'ok': True}) as get, \
             patch('observatory.terminal_ui.ObservatoryApp') as app, \
             patch('observatory.terminal.LocalReceiver') as receiver:
            def run(shell):
                self.assertIs(shell, app.return_value)
                get.assert_not_called()
                receiver.assert_not_called()
                return 0
            with patch('observatory.terminal._run_terminal_app', side_effect=run):
                self.assertEqual(run_cockpit(db_path='/generated.db',
                    endpoint='http://localhost:14319', local_agents=False), 0)

    def test_live_receiver_has_no_redundant_full_health_preflight(self):
        api = ExampleApi()
        original = api.get
        def get(path):
            return {**original(path), 'ok': True}
        with patch.object(api, 'get', side_effect=get):
            feed = Feed(api)
            feed.prepare = CockpitStartup(api, '/generated.db', 14319)
            feed.refresh()
        self.assertFalse(feed.snapshot.error)
        self.assertGreater(feed.snapshot.updated, 0)
        self.assertEqual(api.requests.count('/api/health?probe=1'), 1)
        # A ready receiver without native inventory needs only its initial health.
        self.assertEqual(api.requests.count('/api/health'), 1)

    def test_slow_or_unauthorized_receiver_never_starts_another_receiver(self):
        for error in (TimeoutError('slow'), urllib.error.URLError(TimeoutError('slow')),
                      urllib.error.HTTPError('http://localhost', 401, 'unauthorized', {}, None)):
            with self.subTest(error=type(error).__name__):
                api = ExampleApi()
                feed = Feed(api)
                startup = CockpitStartup(api, '/generated.db', 14319)
                feed.prepare = startup
                with patch.object(api, 'get', side_effect=error), \
                     patch('observatory.terminal.LocalReceiver') as receiver:
                    feed.refresh()
                receiver.assert_not_called()
                self.assertEqual(feed.snapshot.updated, 0)
                self.assertTrue(feed.snapshot.error)
                if isinstance(error, urllib.error.HTTPError):
                    self.assertEqual(feed.connection.phase, 'auth')
                startup.close()

    def test_metadata_only_rejects_capture_before_reading_data(self):
        api = ExampleApi()
        feed = Feed(api)
        startup = CockpitStartup(api, '/generated.db', 14319,
            endpoint=api.endpoint, metadata_only=True)
        feed.prepare = startup
        with patch.object(api, 'get', side_effect=[{'ok': True},
                {'local_agents': {'capture_content': True}}]) as get:
            feed.refresh()
            feed.refresh()
        self.assertEqual([call.args[0] for call in get.call_args_list],
            ['/api/health?probe=1', '/api/health'])
        self.assertEqual(feed.snapshot.updated, 0)
        self.assertIn('captures session content', feed.snapshot.error)
        startup.close()

    def test_quit_during_constructor_closes_late_receiver_exactly_once(self):
        api = ExampleApi(); api.token = 'generated-token'
        feed = Feed(api)
        startup = CockpitStartup(api, '/generated.db', 14319, local_agents=False)
        feed.prepare = startup
        entered, release = threading.Event(), threading.Event()
        receiver = MagicMock()
        def construct(*args, **kwargs):
            entered.set()
            if not release.wait(5):
                raise AssertionError('Constructor barrier was not released')
            return receiver
        with patch.object(api, 'get', side_effect=ConnectionRefusedError()), \
             patch('observatory.terminal.LocalReceiver', side_effect=construct) as create:
            feed.thread.start()
            try:
                self.assertTrue(entered.wait(3))
                self.assertEqual(feed.connection.stage, 'Starting local receiver')
                feed.request_stop()
                startup.close()
            finally:
                release.set()
                feed.close()
                startup.close()
        self.assertFalse(feed.thread.is_alive())
        create.assert_called_once()
        receiver.close.assert_called_once()
        self.assertEqual(feed.snapshot.updated, 0)


class StartupJourney(unittest.IsolatedAsyncioTestCase):
    async def test_shell_handles_keys_and_resize_while_receiver_setup_is_blocked(self):
        api = ExampleApi()
        feed = Feed(api)
        startup = CockpitStartup(api, '/generated.db', 14319)
        feed.prepare = startup
        entered, release = threading.Event(), threading.Event()
        original = api.get
        def get(path):
            if not entered.is_set():
                entered.set()
                if not release.wait(10):
                    raise AssertionError('Probe barrier was not released')
            return {**original(path), 'ok': True}
        with patch.object(api, 'get', side_effect=get):
            app = ObservatoryApp(feed, persist_preferences=False)
            try:
                async with app.run_test(size=(120, 40)) as pilot:
                    self.assertTrue(await asyncio.to_thread(entered.wait, 3))
                    await pilot.pause()
                    self.assertIn('Preparing view', app.query_one('#connection', Static).render().plain)
                    self.assertEqual(feed.snapshot.updated, 0)
                    await pilot.press('2')
                    self.assertEqual(app.query_one('#pages', TabbedContent).active, 'activity')
                    await pilot.resize_terminal(60, 18)
                    await pilot.press('1')
                    self.assertEqual(app.query_one('#pages', TabbedContent).active, 'agents')
                    release.set()
                    for _ in range(80):
                        if feed.snapshot.updated:
                            break
                        await pilot.pause(.05)
                    self.assertGreater(feed.snapshot.updated, 0, feed.snapshot.error)
                    app.refresh_data()
                    await pilot.pause()
                    self.assertTrue(app.visible_agent_rows)
                    await pilot.press('q')
                    self.assertTrue(feed.stop.is_set())
            finally:
                release.set()
                feed.close()
                startup.close()


@unittest.skipUnless(os.name == 'posix', 'requires a real Unix terminal')
class StartupTerminalTests(unittest.TestCase):
    def test_quit_during_blocked_receiver_setup_restores_terminal(self):
        import errno, fcntl, pty, select, socket, struct, termios, time
        code = textwrap.dedent('''
            import os, sys, threading
            from observatory.__main__ import main
            from observatory.core import Store
            from observatory.terminal_ui import ObservatoryApp
            ready_fd = int(sys.argv[2])
            original_init = Store.__init__
            def blocked_init(self, *args, **kwargs):
                os.write(ready_fd, b'SETUP\\n')
                os.read(int(sys.argv[4]), 1)
                original_init(self, *args, **kwargs)
            Store.__init__ = blocked_init
            original_mount = ObservatoryApp.on_mount
            def mount(self):
                original_mount(self)
                self.call_after_refresh(lambda: os.write(ready_fd, b'READY\\n'))
            ObservatoryApp.on_mount = mount
            sys.argv = ['agenturm', '--db', sys.argv[1], 'cockpit', '--port', sys.argv[3], '--no-local-agents']
            status = 0
            try: main()
            except SystemExit as exc: status = exc.code
            assert not any(thread.name == 'observatory-terminal-feed' and thread.is_alive()
                           for thread in threading.enumerate()), 'feed worker leaked'
            os.write(ready_fd, b'CLEAN_EXIT\\n')
            raise SystemExit(status)
        ''')
        with tempfile.TemporaryDirectory() as directory:
            master, slave = pty.openpty()
            read_fd, write_fd = os.pipe()
            gate_fd, gate_write = os.pipe()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 18, 60, 0, 0))
            before = termios.tcgetattr(slave)
            with socket.socket() as address:
                address.bind(('127.0.0.1', 0)); port = address.getsockname()[1]
            env = {**os.environ, 'TERM':'xterm-256color', 'AGENTURM_UI_PREFERENCES':'0',
                   'XDG_CONFIG_HOME':directory, 'XDG_STATE_HOME':directory,
                   'OBSERVATORY_ENDPOINT':'', 'OBSERVATORY_TOKEN':''}
            process = subprocess.Popen([sys.executable,'-c',code,str(Path(directory)/'generated.db'),
                str(write_fd),str(port),str(gate_fd)],stdin=slave,stdout=slave,stderr=slave,
                pass_fds=(write_fd,gate_fd),env=env,start_new_session=True)
            os.close(write_fd); os.close(gate_fd)
            output, notices = bytearray(), bytearray()
            sent = False
            try:
                deadline=time.monotonic()+15
                readers=[master,read_fd]
                while readers and time.monotonic()<deadline:
                    ready,_,_=select.select(readers,[],[],.05)
                    for fd in ready:
                        try: chunk=os.read(fd,65536)
                        except OSError as exc:
                            if exc.errno != errno.EIO: raise
                            chunk=b''
                        if not chunk: readers.remove(fd)
                        else: (notices if fd==read_fd else output).extend(chunk)
                    if not sent and b'READY\n' in notices and b'SETUP\n' in notices:
                        sent=True
                        os.write(master,b'q')
                        os.write(gate_write,b'G')
                    if process.poll() is not None and not ready: break
                process.wait(timeout=3)
                detail=output.decode(errors='replace')
                self.assertTrue(sent,detail)
                self.assertEqual(process.returncode,0,detail)
                self.assertEqual(termios.tcgetattr(slave),before)
                self.assertIn(b'\x1b[?1049l',output)
                self.assertIn(b'\x1b[?25h',output)
                self.assertIn(b'CLEAN_EXIT',notices)
                self.assertNotIn('Traceback',detail)
            finally:
                if process.poll() is None: process.kill(); process.wait(timeout=3)
                for fd in (master,slave,read_fd,gate_write): os.close(fd)
