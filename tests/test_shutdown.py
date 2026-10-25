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
                                  capture_output=True, text=True, timeout=15)

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
            from observatory.terminal import run_cockpit
            order = []
            def interrupted_close(name):
                order.append(name)
                for _ in range(3):
                    os.kill(os.getpid(), signal.SIGINT)
            class Feed:
                def __init__(self, api): pass
                def close(self): interrupted_close('feed')
            class Receiver:
                def __init__(self, *args, **kwargs): pass
                def close(self): interrupted_close('receiver')
            with patch('observatory.terminal.Api.get', side_effect=urllib.error.URLError('offline')), \\
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


@unittest.skipUnless(os.name == 'posix', 'requires a pseudo-terminal and Unix signals')
class TerminalQuitTests(unittest.TestCase):
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
