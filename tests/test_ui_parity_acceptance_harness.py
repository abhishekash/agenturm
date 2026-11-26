"""Harness-only checks: these do not launch or qualify any Rust build."""
import io
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))
from validate_rust_ui_parity import AcceptanceCockpit, environment, exact_copy
from validation_support import file_hash


class ExactIdentityTests(unittest.TestCase):
    def test_hash_mismatch_does_not_create_a_candidate_copy(self):
        with tempfile.TemporaryDirectory() as directory:
            source, target = Path(directory) / "source", Path(directory) / "copy"
            source.write_bytes(b"synthetic executable bytes, not a Rust build")
            with self.assertRaisesRegex(ValueError, "differs"):
                exact_copy(source, "0" * 64, target)
            self.assertFalse(target.exists())

    def test_copy_is_exact_private_and_never_overwrites(self):
        with tempfile.TemporaryDirectory() as directory:
            source, target = Path(directory) / "source", Path(directory) / "copy"
            source.write_bytes(b"synthetic executable bytes, not a Rust build")
            exact_copy(source, file_hash(source), target)
            self.assertEqual(target.read_bytes(), source.read_bytes())
            self.assertEqual(target.stat().st_mode & 0o777, 0o700)
            with self.assertRaises(FileExistsError):
                exact_copy(source, file_hash(source), target)

    def test_environment_does_not_inherit_home_provider_roots_or_credentials(self):
        with patch.dict(os.environ, {"HOME": "/real-home", "CODEX_HOME": "/real-logs",
                                    "OPENAI_API_KEY": "not-a-real-key", "AGENTURM_TOKEN": "not-a-token"}):
            env = environment(Path("/synthetic/private"))
        self.assertEqual(env["HOME"], "/synthetic/private/home")
        self.assertEqual(env["PATH"], "/synthetic/private/empty-path")
        for name in ("CODEX_HOME", "OPENAI_API_KEY", "AGENTURM_TOKEN"):
            self.assertNotIn(name, env)


@unittest.skipUnless(sys.platform.startswith("linux"), "Linux PTY check")
class HarnessPtyTests(unittest.TestCase):
    def test_python_stub_resize_quit_and_interrupt_restore_termios(self):
        # Explicit Python terminal stub, never presented as Agenturm evidence.
        program = r'''
import fcntl, os, signal, struct, sys, termios, tty
original = termios.tcgetattr(0)
def draw(*_):
    rows, cols, _, _ = struct.unpack("HHHH", fcntl.ioctl(0, termios.TIOCGWINSZ, b"\0" * 8))
    os.write(1, f"\033[2J\033[Hstub {cols}x{rows}".encode())
try:
    tty.setraw(0)
    signal.signal(signal.SIGWINCH, draw)
    os.write(1, b"\033[?1049h")
    draw()
    while os.read(0, 1) not in (b"q", b"\x03"):
        pass
finally:
    os.write(1, b"\033[?1049l")
    termios.tcsetattr(0, termios.TCSANOW, original)
'''
        for exit_key in (b"q", b"\x03"):
            with self.subTest(exit_key=exit_key):
                with tempfile.TemporaryDirectory() as directory:
                    transcript = io.BytesIO()
                    cockpit = AcceptanceCockpit([sys.executable, "-B", "-c", program],
                        environment(Path(directory)), transcript, width=120, height=32)
                    try:
                        cockpit.frame("Python stub startup", lambda text: "stub 120x32" in text)
                        cockpit.resize(60, 18)
                        cockpit.frame("Python stub resize", lambda text: "stub 60x18" in text)
                        cockpit.finish(exit_key)
                    finally:
                        cockpit.close()


if __name__ == "__main__":
    unittest.main()
