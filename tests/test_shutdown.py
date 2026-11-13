"""Real interrupts during exit, not just exceptions from mocked join methods."""
import os
import signal
import subprocess
import sys
import tempfile
import textwrap
import threading
import unittest
from pathlib import Path
from types import SimpleNamespace

from observatory.shutdown import complete_cleanup
from observatory.terminal import LocalReceiver


class CleanupTests(unittest.TestCase):
    def test_callbacks_run_in_order_and_signal_handler_is_restored(self):
        previous = signal.getsignal(signal.SIGINT)
        seen = []

        def nested():
            seen.append(signal.getsignal(signal.SIGINT))
            complete_cleanup(lambda: seen.append(signal.getsignal(signal.SIGINT)))
            seen.append(signal.getsignal(signal.SIGINT))

        complete_cleanup(nested)
        self.assertEqual(seen, [signal.SIG_IGN] * 3)
        self.assertEqual(signal.getsignal(signal.SIGINT), previous)

    def test_real_errors_propagate_but_do_not_skip_later_cleanup(self):
        previous = signal.getsignal(signal.SIGINT)
        order = []

        def broken():
            order.append('broken')
            raise RuntimeError('cleanup error')

        with self.assertRaisesRegex(RuntimeError, 'cleanup error'):
            complete_cleanup(broken, lambda: order.append('later'))
        self.assertEqual(order, ['broken', 'later'])
        self.assertEqual(signal.getsignal(signal.SIGINT), previous)

    def test_worker_thread_cleanup_does_not_try_to_install_signal_handlers(self):
        seen = []
        worker = threading.Thread(target=lambda: complete_cleanup(lambda: seen.append('closed')))
        worker.start()
        worker.join(2)
        self.assertFalse(worker.is_alive())
        self.assertEqual(seen, ['closed'])

    def test_receiver_still_closes_store_when_an_earlier_closer_fails(self):
        receiver = LocalReceiver.__new__(LocalReceiver)
        order = []

        def stop_local():
            order.append('local')
            raise RuntimeError('local failure')

        receiver.local = SimpleNamespace(close=stop_local)
        receiver.server = SimpleNamespace(shutdown=lambda: order.append('shutdown'),
                                          server_close=lambda: order.append('socket'))
        receiver.thread = SimpleNamespace(join=lambda: order.append('join'))
        receiver.store = SimpleNamespace(close=lambda: order.append('store'))
        with self.assertRaisesRegex(RuntimeError, 'local failure'):
            receiver.close()
        self.assertEqual(order, ['local', 'shutdown', 'socket', 'join', 'store'])

    def run_child(self, code):
        with tempfile.TemporaryDirectory() as directory:
            return subprocess.run([sys.executable, '-c', textwrap.dedent(code),
                                   str(Path(directory) / 'test.db')],
                                  capture_output=True, text=True, timeout=15, env={**os.environ,"XDG_STATE_HOME":directory,"OBSERVATORY_TOKEN":""})

    @unittest.skipUnless(os.name == 'posix', 'requires Unix SIGINT delivery')
    def test_repeated_sigint_during_actual_http_shutdown_wait(self):
        result = self.run_child('''
            import os, signal, sys, threading
            from observatory.terminal import LocalReceiver
            receiver = LocalReceiver(sys.argv[1], 0, None, local_agents=False)
            derived = receiver.store._derived_thread
            entered, release = threading.Event(), threading.Event()
            def pause_accept_loop():
                entered.set()
                release.wait(3)
            receiver.server.service_actions = pause_accept_loop
            assert entered.wait(3)
            original = receiver.server.shutdown
            def interrupt_shutdown():
                timers = [threading.Timer(delay, lambda: os.kill(os.getpid(), signal.SIGINT))
                          for delay in (.04, .08, .12)]
                timers.append(threading.Timer(.25, release.set))
                for timer in timers:
                    timer.start()
                original()
                for timer in timers:
                    timer.join(1)
            receiver.server.shutdown = interrupt_shutdown
            receiver.close()
            assert not receiver.thread.is_alive()
            assert not derived.is_alive()
            assert receiver.server.socket.fileno() == -1
            assert signal.getsignal(signal.SIGINT) == signal.default_int_handler
            receiver.server.shutdown = original
            receiver.close()
            print('clean exit')
        ''')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('clean exit', result.stdout)
        self.assertNotIn('KeyboardInterrupt', result.stderr)

    @unittest.skipUnless(os.name == 'posix', 'requires Unix SIGINT delivery')
    def test_cockpit_finally_protects_feed_and_receiver_cleanup(self):
        result = self.run_child('''
            import os, signal, urllib.error
            from unittest.mock import patch
            from observatory.terminal import run_cockpit, Snapshot
            from types import SimpleNamespace
            order = []
            def interrupted_close(name):
                order.append(name)
                for _ in range(3):
                    os.kill(os.getpid(), signal.SIGINT)
            class Feed:
                def __init__(self, api): self.snapshot=Snapshot()
                def refresh(self): pass
                def close(self): interrupted_close('feed')
            class Receiver:
                def __init__(self, *args, **kwargs): self.store=SimpleNamespace(_derived_pending={})
                def close(self): interrupted_close('receiver')
            with patch('observatory.terminal.Api.get', side_effect=[urllib.error.URLError('offline'), {'indexing': {'ready': True}}]), \\
                 patch('observatory.terminal.Feed', Feed), \\
                 patch('observatory.terminal.LocalReceiver', Receiver), \\
                 patch('observatory.terminal.print_snapshot'):
                run_cockpit(once=True)
            assert order == ['feed', 'receiver'], order
            assert signal.getsignal(signal.SIGINT) == signal.default_int_handler
            print('clean exit')
        ''')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('clean exit', result.stdout)

    @unittest.skipUnless(os.name == 'posix', 'requires Unix signal handlers')
    def test_repeated_terminal_runs_restore_custom_signal_handlers(self):
        result = self.run_child('''
            import asyncio, os, signal
            from observatory.terminal import _run_terminal_app
            def existing(signum, frame):
                raise AssertionError('Previous signal handler ran inside the cockpit')
            signal.signal(signal.SIGINT, existing)
            signal.signal(signal.SIGTERM, existing)
            for signum in (signal.SIGTERM, signal.SIGINT, signal.SIGTERM):
                class App:
                    async def run_async(self):
                        loop = asyncio.get_running_loop()
                        self.finished = loop.create_future()
                        loop.call_soon(os.kill, os.getpid(), signum)
                        await self.finished
                    def exit(self, *, return_code):
                        self.return_code = return_code
                        self.finished.set_result(None)
                assert _run_terminal_app(App()) == 128 + signum
                assert signal.getsignal(signal.SIGINT) is existing
                assert signal.getsignal(signal.SIGTERM) is existing
            print('clean repeated runs')
        ''')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('clean repeated runs', result.stdout)


@unittest.skipUnless(os.name == 'posix', 'requires a pseudo-terminal and Unix signals')
class TerminalQuitTests(unittest.TestCase):
    def terminal_exit(self, mode, expected_code):
        """Assert the shell's actual tty state, not mocked framework cleanup."""
        import fcntl
        import pty
        import select
        import socket
        import struct
        import termios
        import time

        code = textwrap.dedent('''
            import os, signal, sys, threading
            from observatory.__main__ import main
            from observatory.terminal_ui import ObservatoryApp
            ready_fd, mode, gate_fd = int(sys.argv[2]), sys.argv[4], int(sys.argv[5])
            previous = {sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)}
            original_mount = ObservatoryApp.on_mount
            def fail():
                raise RuntimeError('SYNTHETIC_TERMINAL_FAILURE')
            def mount(self):
                if mode == 'startup-term':
                    os.write(ready_fd, b'READY\\n')
                    os.read(gate_fd, 1)
                original_mount(self)
                if mode != 'startup-term':
                    def ready():
                        os.write(ready_fd, b'READY\\n')
                        if mode == 'error': self.call_later(fail)
                    self.call_after_refresh(ready)
            ObservatoryApp.on_mount = mount
            sys.argv = ['agenturm', '--db', sys.argv[1], 'cockpit', '--port', sys.argv[3], '--no-local-agents']
            status = 0
            try:
                main()
            except SystemExit as exc:
                status = exc.code
            assert all(signal.getsignal(sig) == handler for sig, handler in previous.items()), 'signal handler leaked'
            assert not any(t.is_alive() and t.name in ('observatory-receiver', 'observatory-derived-refresh')
                           for t in threading.enumerate()), 'receiver worker leaked'
            os.write(ready_fd, b'CLEAN_EXIT\\n')
            raise SystemExit(status)
        ''')
        with tempfile.TemporaryDirectory() as directory:
            master, slave = pty.openpty()
            read_fd, write_fd = os.pipe()
            gate_fd, gate_write = os.pipe()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 80, 0, 0))
            before = termios.tcgetattr(slave)
            with socket.socket() as address:
                address.bind(('127.0.0.1', 0))
                port = address.getsockname()[1]
            env = {**os.environ, 'TERM': 'xterm-256color', 'AGENTURM_UI_PREFERENCES': '0',
                   'XDG_CONFIG_HOME': directory, 'XDG_STATE_HOME': directory,
                   'OBSERVATORY_ENDPOINT': '', 'OBSERVATORY_TOKEN': ''}
            process = subprocess.Popen([sys.executable, '-c', code, str(Path(directory)/'test.db'),
                                        str(write_fd), str(port), mode, str(gate_fd)], stdin=slave, stdout=slave,
                                       stderr=slave, pass_fds=(write_fd,gate_fd), env=env, start_new_session=True)
            os.close(write_fd)
            os.close(gate_fd)
            output, notices, sent = bytearray(), bytearray(), False
            try:
                deadline = time.monotonic()+20
                readers = [master, read_fd]
                while time.monotonic()<deadline:
                    ready, _, _ = select.select(readers, [], [], .05)
                    for fd in ready:
                        chunk = os.read(fd, 65536)
                        if not chunk:
                            readers.remove(fd)
                        else:
                            (notices if fd==read_fd else output).extend(chunk)
                    if not sent and b'READY\n' in notices:
                        sent = True
                        if mode=='quit': os.write(master,b'q')
                        elif mode in {'term','startup-term'}:
                            os.kill(process.pid,signal.SIGTERM)
                            if mode=='startup-term': os.write(gate_write,b'G')
                        elif mode=='interrupt': os.kill(process.pid,signal.SIGINT)
                    if process.poll() is not None and not ready:
                        break
                process.wait(timeout=2)
                detail = output.decode(errors='replace')
                self.assertTrue(sent,detail)
                self.assertEqual(termios.tcgetattr(slave),before,'Terminal raw mode leaked: '+mode)
                self.assertIn(b'\x1b[?1049l',output,'Alternate screen was not restored: '+mode)
                self.assertIn(b'\x1b[?25h',output,'Cursor was not restored: '+mode)
                self.assertIn(b'CLEAN_EXIT',notices,detail)
                self.assertEqual(process.returncode,expected_code,detail)
                if mode!='error':self.assertNotIn('Traceback',detail)
                else:self.assertIn('SYNTHETIC_TERMINAL_FAILURE',detail)
            finally:
                if process.poll() is None:
                    process.kill();process.wait(timeout=3)
                os.close(master);os.close(slave);os.close(read_fd);os.close(gate_write)

    def test_normal_quit_restores_terminal(self):
        self.terminal_exit('quit',0)

    def test_sigterm_restores_terminal_and_reports_signal_status(self):
        self.terminal_exit('term',143)

    def test_sigterm_during_mount_restores_terminal(self):
        self.terminal_exit('startup-term',143)

    def test_external_sigint_restores_terminal_and_reports_signal_status(self):
        self.terminal_exit('interrupt',130)

    def test_ui_failure_restores_terminal_and_reports_failure(self):
        self.terminal_exit('error',1)

    def test_actual_q_key_then_ctrl_c_during_receiver_shutdown(self):
        import errno
        import fcntl
        import pty
        import select
        import socket
        import struct
        import termios
        import time

        code = textwrap.dedent('''
            import os, sys, threading
            from observatory.__main__ import main
            from observatory.receiver import BoundedHTTPServer
            from observatory.terminal_ui import ObservatoryApp
            ready_fd = int(sys.argv[2])
            original_mount = ObservatoryApp.on_mount
            def mount(self):
                original_mount(self)
                self.call_after_refresh(lambda: os.write(ready_fd, b'READY\\n'))
            ObservatoryApp.on_mount = mount
            original_shutdown = BoundedHTTPServer.shutdown
            def shutdown(self):
                entered, release = threading.Event(), threading.Event()
                def pause():
                    entered.set()
                    release.wait(3)
                self.service_actions = pause
                assert entered.wait(3)
                os.write(ready_fd, b'SHUTDOWN\\n')
                timer = threading.Timer(.4, release.set)
                timer.start()
                try:
                    original_shutdown(self)
                finally:
                    timer.join(4)
            BoundedHTTPServer.shutdown = shutdown
            sys.argv = ['agenturm', '--db', sys.argv[1], 'cockpit', '--port', sys.argv[3], '--no-local-agents']
            main()
            assert not any(t.is_alive() and t.name in ('observatory-receiver', 'observatory-derived-refresh')
                           for t in threading.enumerate())
            print('CLEAN_EXIT', flush=True)
        ''')
        with tempfile.TemporaryDirectory() as directory:
            master, slave = pty.openpty()
            read_fd, write_fd = os.pipe()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 36, 120, 0, 0))
            with socket.socket() as address:
                address.bind(('127.0.0.1', 0))
                port = address.getsockname()[1]
            env = {**os.environ, 'TERM': 'xterm-256color', 'AGENTURM_UI_PREFERENCES': '0',
                   'OBSERVATORY_ENDPOINT': '', 'OBSERVATORY_TOKEN': ''}
            process = subprocess.Popen([sys.executable, '-c', code, str(Path(directory) / 'test.db'),
                                        str(write_fd), str(port)],
                                       stdin=slave, stdout=slave, stderr=slave,
                                       pass_fds=(write_fd,), env=env, start_new_session=True)
            os.close(slave)
            os.close(write_fd)
            output, notices = bytearray(), bytearray()
            sent_q = sent_interrupt = False
            readers = [master, read_fd]
            try:
                deadline = time.monotonic() + 15
                while readers and time.monotonic() < deadline:
                    ready, _, _ = select.select(readers, [], [], .1)
                    for fd in ready:
                        try:
                            chunk = os.read(fd, 65536)
                        except OSError as exc:
                            if exc.errno != errno.EIO:
                                raise
                            chunk = b''
                        if not chunk:
                            readers.remove(fd)
                            continue
                        (notices if fd == read_fd else output).extend(chunk)
                    if b'READY\n' in notices and not sent_q:
                        os.write(master, b'q')
                        sent_q = True
                    if b'SHUTDOWN\n' in notices and not sent_interrupt:
                        os.kill(process.pid, signal.SIGINT)
                        sent_interrupt = True
                process.wait(timeout=2)
                self.assertTrue(sent_q, output.decode(errors='replace'))
                self.assertTrue(sent_interrupt, output.decode(errors='replace'))
                self.assertEqual(process.returncode, 0, output.decode(errors='replace'))
                self.assertIn(b'CLEAN_EXIT', output)
                self.assertNotIn(b'Traceback', output)
                self.assertNotIn(b'KeyboardInterrupt', output)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=3)
                os.close(master)
                os.close(read_fd)
