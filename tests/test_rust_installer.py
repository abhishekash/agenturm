"""Exercise the opt-in Rust installer with fully local release fixtures."""

import hashlib
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest


INSTALLER = Path(__file__).resolve().parents[1] / "scripts" / "install-rust.sh"
VERSION = "0.3.6"
BINARIES = (
    "agenturm",
    "agenturm-plugin-host",
    "agenturm-connector",
    "agenturm-storage-service",
    "agenturm-codex-harness",
    "agenturm-pi-harness",
    "agenturm-claude-harness",
    "agenturm-kiro-harness",
    "agenturm-opencode-harness",
)
RESOURCES = {
    "LICENSE": b"Fixture license\n",
    "README.md": b"# Fixture release\n",
    "README.html": b"<h1>Fixture release</h1>\n",
    "agenturm-plugins.json": (json.dumps({"version": 1, "plugins": []}) + "\n").encode(),
}


@unittest.skipUnless(sys.platform.startswith("linux"), "installer requires GNU/Linux tools")
class RustInstallerTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory(prefix="agenturm installer ")
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        self.home = self.root / "fixture home"
        self.prefix = self.root / "release prefix"
        self.bin_dir = self.root / "launcher directory"
        self.launcher = self.bin_dir / "agenturm-rust"
        self.config = self.root / "fixture config"
        self.state = self.root / "fixture state"
        self.payloads = self.root / "bundle fixtures"
        self.temp = self.root / "installer staging"
        shims = self.root / "platform shims"
        for directory in (self.home, self.payloads, self.temp, shims):
            directory.mkdir()
        (shims / "uname").write_text(
            '#!/bin/sh\ncase "$1" in\n'
            '  -s) printf "%s\\n" "$INSTALLER_TEST_OS";;\n'
            '  -m) printf "%s\\n" "$INSTALLER_TEST_ARCH";;\n'
            '  *) exit 2;;\nesac\n', encoding="utf-8",
        )
        (shims / "getconf").write_text(
            '#!/bin/sh\n[ "$1" = GNU_LIBC_VERSION ] || exit 2\n'
            '[ -n "$INSTALLER_TEST_LIBC" ] || exit 1\n'
            'printf "%s\\n" "$INSTALLER_TEST_LIBC"\n', encoding="utf-8",
        )
        for shim in shims.iterdir():
            shim.chmod(0o755)
        self.env = {
            "PATH": os.pathsep.join((str(shims), "/usr/bin", "/bin")),
            "LC_ALL": "C",
            "HOME": str(self.home),
            "TMPDIR": str(self.temp),
            "XDG_CONFIG_HOME": str(self.config),
            "XDG_DATA_HOME": str(self.root / "fixture data"),
            "XDG_STATE_HOME": str(self.state),
            "INSTALLER_TEST_OS": "Linux",
            "INSTALLER_TEST_ARCH": "x86_64",
            "INSTALLER_TEST_LIBC": "glibc 2.39",
        }

    def make_release(self, *, version=VERSION, variant="valid", hazard=None, changed=False):
        name = f"agenturm-{version}-linux-x86_64"
        directory = self.payloads / variant
        directory.mkdir()
        archive = directory / f"{name}.tar.gz"
        shim = f"#!/bin/sh\nprintf 'agenturm {version}\\n'\n".encode()
        if changed:
            shim += b"# Same version, distinct release bytes.\n"
        with tarfile.open(archive, "w:gz", format=tarfile.USTAR_FORMAT) as packed:
            for path in (name, f"{name}/bin"):
                member = tarfile.TarInfo(path)
                member.type = tarfile.DIRTYPE
                member.mode = 0o755
                packed.addfile(member)
            for filename, contents in RESOURCES.items():
                self.add_file(packed, f"{name}/{filename}", contents, 0o644)
            for filename in BINARIES:
                path = f"{name}/bin/{filename}"
                if filename == "agenturm" and hazard == "symlink":
                    member = tarfile.TarInfo(path)
                    member.type = tarfile.SYMTYPE
                    member.linkname = "../../../../escape.txt"
                    packed.addfile(member)
                    continue
                self.add_file(packed, path, shim, 0o755)
            if hazard == "traversal":
                self.add_file(packed, f"{name}/../../escape.txt", b"from archive\n", 0o644)
            elif hazard == "duplicate":
                self.add_file(packed, f"{name}/bin/agenturm", shim, 0o755)
        checksum = archive.with_name(archive.name + ".sha256")
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        checksum.write_text(f"{digest}  {archive.name}\n", encoding="ascii")
        return archive, checksum, digest

    @staticmethod
    def add_file(archive, pathname, contents, mode):
        member = tarfile.TarInfo(pathname)
        member.mode = mode
        member.size = len(contents)
        archive.addfile(member, io.BytesIO(contents))

    def invoke(self, action, *flags, env=None):
        return subprocess.run(
            ("bash", str(INSTALLER), action, "--prefix", str(self.prefix),
             "--bin-dir", str(self.bin_dir), *map(str, flags)),
            cwd=self.root, env=self.env if env is None else env,
            capture_output=True, text=True, timeout=15, check=False,
        )

    def install(self, action, release, *flags, version=VERSION, env=None):
        archive, checksum, _ = release
        return self.invoke(action, "--version", version, "--archive", archive,
                           "--checksum", checksum, *flags, env=env)

    def success(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def failure(self, result, diagnostic=None):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        if diagnostic:
            self.assertIn(diagnostic, result.stderr)
        self.assertEqual(result.stdout, "")

    def launcher_version(self):
        result = subprocess.run((str(self.launcher), "--version"), cwd=self.root,
                                env=self.env, capture_output=True, text=True,
                                timeout=5, check=False)
        return self.success(result)

    def test_install_update_and_same_release_replay_preserve_default_command(self):
        self.bin_dir.mkdir(mode=0o700)
        default = self.bin_dir / "agenturm"
        default_command = '#!/bin/sh\nprintf "original Agenturm\\n"\n'
        default.write_text(default_command, encoding="utf-8")
        default.chmod(0o755)
        old_version = "0.3.5"
        original = self.make_release(version=old_version, variant="previous")
        self.assertEqual(self.success(self.install("install", original, version=old_version)),
                         str(self.launcher))
        previous = self.launcher.resolve(strict=True)
        self.assertEqual(self.launcher_version(), f"agenturm {old_version}")

        current = self.make_release()
        self.assertEqual(self.success(self.install("update", current, "--sha256", current[2])),
                         str(self.launcher))
        selected = self.launcher.resolve(strict=True)
        self.assertNotEqual(selected, previous)
        self.assertTrue(previous.is_file())
        self.assertEqual(self.launcher_version(), f"agenturm {VERSION}")
        for binary in BINARIES:
            self.assertTrue(os.access(selected.with_name(binary), os.X_OK), binary)
        for filename, contents in RESOURCES.items():
            self.assertEqual((selected.parent.parent / filename).read_bytes(), contents)

        self.assertEqual(self.success(self.install("update", current)), str(self.launcher))
        self.assertEqual(self.launcher.resolve(strict=True), selected)
        self.assertEqual(default.read_text(encoding="utf-8"), default_command)
        incompatible = self.make_release(variant="modified-bytes", changed=True)
        self.failure(self.install("update", incompatible), "different archive bytes")
        self.assertEqual(self.launcher.resolve(strict=True), selected)
        self.assertEqual(self.launcher_version(), f"agenturm {VERSION}")

        outside_program = self.root / "identical external executable"
        previous.rename(outside_program)
        previous.symlink_to(outside_program)
        self.failure(self.install("update", original, version=old_version), "modified")
        self.assertEqual(self.launcher.resolve(strict=True), selected)
        self.assertEqual(self.launcher_version(), f"agenturm {VERSION}")
        self.assertEqual(previous.resolve(strict=True), outside_program)

    def test_rejects_foreign_rust_launcher_without_touching_its_target(self):
        release = self.make_release()
        self.bin_dir.mkdir(mode=0o700)
        self.launcher.write_bytes(b"foreign executable\n")
        self.failure(self.install("install", release), "not managed")
        self.assertEqual(self.launcher.read_bytes(), b"foreign executable\n")
        self.launcher.unlink()
        foreign = self.root / "foreign target"
        foreign.write_bytes(b"other app\n")
        self.launcher.symlink_to(foreign)
        self.failure(self.install("install", release), "outside this installation")
        self.assertEqual(os.readlink(self.launcher), str(foreign))
        self.assertEqual(foreign.read_bytes(), b"other app\n")
        self.assertFalse(self.prefix.exists())

    def test_unreceipted_or_nested_release_target_is_never_owned(self):
        release = self.make_release()
        self.success(self.install("install", release))
        retained = self.launcher.resolve(strict=True).parent.parent
        foreign_targets = (
            retained.parent / "agenturm-9.9.9-linux-x86_64" / "bin" / "agenturm",
            retained / "nested" / "bin" / "agenturm",
        )
        for foreign in foreign_targets:
            with self.subTest(target=foreign):
                foreign.parent.mkdir(parents=True)
                foreign.write_bytes(b"unmanaged program")
                self.launcher.unlink()
                self.launcher.symlink_to(foreign)
                self.failure(self.install("update", release))
                self.failure(self.invoke("uninstall"))
                self.assertEqual(self.launcher.resolve(strict=True), foreign)
                self.assertEqual(foreign.read_bytes(), b"unmanaged program")
                self.assertTrue((retained / ".install-receipt").is_file())

    def test_same_version_replay_requires_all_installed_executables(self):
        current = self.make_release()
        previous = self.make_release(version="0.3.5", variant="previous")
        self.success(self.install("install", current))
        installed_bin = self.launcher.resolve(strict=True).parent
        self.success(self.install("update", previous, version="0.3.5"))
        selected = self.launcher.resolve(strict=True)
        for name in ("agenturm", "agenturm-plugin-host"):
            binary = installed_bin / name
            before = binary.read_bytes()
            with self.subTest(binary=name):
                binary.chmod(0o600)
                self.failure(self.install("update", current), "not executable")
                self.assertEqual(self.launcher.resolve(strict=True), selected)
                self.assertEqual(self.launcher_version(), "agenturm 0.3.5")
                self.assertEqual(binary.read_bytes(), before)
                self.assertEqual(stat.S_IMODE(binary.stat().st_mode), 0o600)
                binary.chmod(0o755)

    def test_refuses_unsafe_modes_across_the_release_path_before_deletion(self):
        release = self.make_release()
        self.success(self.install("install", release))
        selected = self.launcher.resolve(strict=True)
        release_directory = selected.parent.parent
        self.prefix.chmod(0o755)
        for path in (release_directory.parent, release_directory, selected.parent,
                     selected.with_name("agenturm-plugin-host")):
            with self.subTest(path=path):
                original = stat.S_IMODE(path.stat().st_mode)
                path.chmod(original | 0o022)
                self.failure(self.install("update", release))
                self.failure(self.invoke("uninstall"))
                self.assertEqual(self.launcher.resolve(strict=True), selected)
                self.assertTrue((release_directory / ".install-receipt").is_file())
                self.assertTrue(selected.is_file())
                path.chmod(original)

    def test_prior_predictable_temporary_path_is_never_removed(self):
        archive, checksum, _ = self.make_release()
        self.bin_dir.mkdir(mode=0o700)
        shell = (
            'printf "other program\\n" > "$1/.agenturm-rust.$$.new"; '
            'shift; exec bash "$@"'
        )
        result = subprocess.run(
            ("bash", "-c", shell, "fixture", str(self.bin_dir), str(INSTALLER),
             "install", "--prefix", str(self.prefix), "--bin-dir", str(self.bin_dir),
             "--version", VERSION, "--archive", str(archive), "--checksum", str(checksum)),
            cwd=self.root, env=self.env, capture_output=True, text=True, timeout=15, check=False,
        )
        self.assertEqual(self.success(result), str(self.launcher))
        retained, = self.bin_dir.glob(".agenturm-rust.*.new")
        self.assertEqual(retained.read_bytes(), b"other program\n")
        self.assertEqual(set(self.bin_dir.iterdir()), {retained, self.launcher})

    def test_launchers_share_a_lock_across_release_prefixes(self):
        archive, checksum, _ = self.make_release()
        shims = self.root / "locking shims"
        shims.mkdir()
        ready = self.root / "staged launcher ready"
        resume = self.root / "publish staged launcher"
        ln = shims / "ln"
        ln.write_text(
            '#!/usr/bin/bash\n'
            '/usr/bin/touch -- "$INSTALLER_GATE_READY"\n'
            'for ((step=0; step<500; step++)); do\n'
            '    [[ ! -e "$INSTALLER_GATE_RESUME" ]] || exec /usr/bin/ln "$@"\n'
            '    /usr/bin/sleep .01\n'
            'done\nexit 9\n', encoding="utf-8",
        )
        ln.chmod(0o755)
        env = {**self.env, "PATH": str(shims) + os.pathsep + self.env["PATH"],
               "INSTALLER_GATE_READY": str(ready), "INSTALLER_GATE_RESUME": str(resume)}

        def arguments(prefix, action):
            flags = ["bash", str(INSTALLER), action, "--prefix", str(prefix),
                     "--bin-dir", str(self.bin_dir)]
            if action != "uninstall":
                flags.extend(("--version", VERSION, "--archive", str(archive),
                              "--checksum", str(checksum)))
            return flags

        with subprocess.Popen(arguments(self.prefix, "install"), cwd=self.root, env=env,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as first:
            try:
                deadline = time.monotonic() + 5
                while not ready.exists() and first.poll() is None and time.monotonic() < deadline:
                    time.sleep(.01)
                self.assertTrue(ready.exists(), "first installer did not stage the launcher")
                second_prefix = self.root / "different release prefix"
                for action in ("install", "uninstall"):
                    other = subprocess.run(arguments(second_prefix, action), cwd=self.root,
                                           env=self.env, capture_output=True, text=True, timeout=5)
                    self.failure(other, "installer is using the launcher directory")
                self.assertFalse(self.launcher.exists())
            finally:
                resume.touch()
            stdout, stderr = first.communicate(timeout=10)
        self.assertEqual(first.returncode, 0, stderr)
        self.assertEqual(stdout.strip(), str(self.launcher))
        self.assertTrue(self.launcher.resolve(strict=True).is_relative_to(self.prefix))
        self.assertEqual(self.launcher_version(), f"agenturm {VERSION}")

    def test_download_uses_https_and_disables_implicit_curl_configuration(self):
        archive, checksum, digest = self.make_release()
        shims = self.root / "download shims"
        shims.mkdir()
        curl = shims / "curl"
        curl.write_text(
            '#!/usr/bin/bash\n'
            'set -eu\n'
            '[[ "$1" == -q && "$2" == --proto && "$3" == =https && '
            '"$4" == --proto-redir && "$5" == =https ]]\n'
            'while [[ "$1" != --output ]]; do shift; done\n'
            'release_url="https://github.com/abhishekash/agenturm/releases/download/rust-v0.3.6"\n'
            'case "$3" in\n'
            '  "$release_url/agenturm-0.3.6-linux-x86_64.tar.gz") input=$FIXTURE_ARCHIVE;;\n'
            '  "$release_url/agenturm-0.3.6-linux-x86_64.tar.gz.sha256") input=$FIXTURE_CHECKSUM;;\n'
            '  *) exit 2;;\n'
            'esac\n'
            'exec /usr/bin/cp -- "$input" "$2"\n', encoding="utf-8",
        )
        curl.chmod(0o755)
        env = {**self.env, "PATH": str(shims) + os.pathsep + self.env["PATH"],
               "FIXTURE_ARCHIVE": str(archive), "FIXTURE_CHECKSUM": str(checksum)}
        result = self.invoke("install", "--version", VERSION, "--sha256", digest, env=env)
        self.assertEqual(self.success(result), str(self.launcher))
        self.assertEqual(self.launcher_version(), f"agenturm {VERSION}")

    def test_checksum_and_independent_pin_fail_before_replacing_launcher(self):
        original = self.make_release(version="0.3.5", variant="previous")
        self.success(self.install("install", original, version="0.3.5"))
        previous = self.launcher.resolve(strict=True)
        current = self.make_release()
        corrupt_digest = "0" * 64
        self.failure(self.install("update", current, "--sha256", corrupt_digest), "--sha256")
        archive, checksum, _ = current
        checksum.write_text(f"{corrupt_digest}  {archive.name}\n", encoding="ascii")
        self.failure(self.install("update", current), "SHA-256 mismatch")
        self.assertEqual(self.launcher.resolve(strict=True), previous)
        self.assertEqual(self.launcher_version(), "agenturm 0.3.5")
        self.assertEqual(list(previous.parent.parent.parent.iterdir()), [previous.parent.parent])

    def test_rejects_traversal_duplicate_member_and_symbolic_link_before_update(self):
        original = self.make_release(version="0.3.5", variant="previous")
        self.success(self.install("install", original, version="0.3.5"))
        previous = self.launcher.resolve(strict=True)
        escaped = self.root / "escape.txt"
        sentinel = b"#!/bin/sh\nprintf 'agenturm 0.3.6\\n'\n"
        escaped.write_bytes(sentinel)
        escaped.chmod(0o640)
        for hazard in ("traversal", "duplicate", "symlink"):
            with self.subTest(hazard=hazard):
                malformed = self.make_release(variant=hazard, hazard=hazard)
                self.failure(self.install("update", malformed))
                self.assertEqual(self.launcher.resolve(strict=True), previous)
                self.assertEqual(escaped.read_bytes(), sentinel)
                self.assertEqual(stat.S_IMODE(escaped.stat().st_mode), 0o640)
                self.assertFalse((self.prefix / "escape.txt").exists())
                self.assertEqual(list(previous.parent.parent.parent.iterdir()), [previous.parent.parent])
        self.assertEqual(self.launcher_version(), "agenturm 0.3.5")

    def test_uninstall_refuses_active_unit_and_leaves_user_files(self):
        self.success(self.install("install", self.make_release()))
        selected = self.launcher.resolve(strict=True)
        release = selected.parent.parent
        extra = release / "keep this file.txt"
        extra.write_text("operator file\n", encoding="utf-8")
        extra_binary = selected.with_name("operator command")
        extra_binary.write_bytes(b"keep custom command")
        history = self.state / "agenturm" / "history.sqlite"
        history.parent.mkdir(parents=True)
        history.write_bytes(b"keep telemetry")
        settings = self.config / "agenturm" / "sources.json"
        settings.parent.mkdir(parents=True)
        settings.write_bytes(b"keep configuration")
        service = self.config / "systemd" / "user" / "agenturm-collector.service"
        service.parent.mkdir(parents=True)
        service.write_text(f"[Service]\nExecStart={selected} collect\n", encoding="utf-8")
        self.failure(self.invoke("uninstall"), "service uninstall")
        self.assertEqual(self.launcher.resolve(strict=True), selected)
        self.assertTrue(service.is_file())
        service.unlink()
        service.symlink_to(self.config / "missing.service")
        self.failure(self.invoke("uninstall"), "service uninstall")
        self.assertTrue(service.is_symlink())
        self.assertEqual(self.launcher.resolve(strict=True), selected)
        service.unlink()
        self.success(self.invoke("uninstall", "--dry-run"))
        self.assertEqual(self.launcher.resolve(strict=True), selected)

        self.success(self.invoke("uninstall"))
        self.assertFalse(self.launcher.is_symlink())
        for binary in BINARIES:
            self.assertFalse(selected.with_name(binary).exists(), binary)
        for filename in RESOURCES:
            self.assertFalse((release / filename).exists(), filename)
        self.assertEqual(extra.read_text(encoding="utf-8"), "operator file\n")
        self.assertEqual(extra_binary.read_bytes(), b"keep custom command")
        self.assertEqual(history.read_bytes(), b"keep telemetry")
        self.assertEqual(settings.read_bytes(), b"keep configuration")
        self.success(self.invoke("uninstall"))
        self.assertTrue(extra.exists())

    def test_uninstall_preflights_retained_releases_before_removing_programs(self):
        original = self.make_release(version="0.3.5", variant="previous")
        self.success(self.install("install", original, version="0.3.5"))
        previous_bin = self.launcher.resolve(strict=True).parent
        self.success(self.install("update", self.make_release()))
        selected = self.launcher.resolve(strict=True)
        program = selected.read_bytes()

        def refused(diagnostic):
            self.failure(self.invoke("uninstall"), diagnostic)
            self.assertEqual(self.launcher.resolve(strict=True), selected)
            self.assertEqual(self.launcher_version(), f"agenturm {VERSION}")
            self.assertEqual(selected.read_bytes(), program)

        saved_bin = self.root / "saved old binaries"
        previous_bin.rename(saved_bin)
        foreign_bin = self.root / "foreign binary directory"
        foreign_bin.mkdir()
        foreign_program = foreign_bin / "agenturm"
        foreign_program.write_bytes(b"external program")
        previous_bin.symlink_to(foreign_bin, target_is_directory=True)

        refused("altered managed release")
        self.assertTrue(previous_bin.is_symlink())
        self.assertEqual(foreign_program.read_bytes(), b"external program")
        self.assertTrue((saved_bin / "agenturm").is_file())
        previous_bin.unlink()
        saved_bin.rename(previous_bin)

        receipt = previous_bin.parent / ".install-receipt"
        saved_receipt = self.root / "saved install receipt"
        receipt.rename(saved_receipt)
        os.mkfifo(receipt)
        refused("altered managed release")
        self.assertTrue(stat.S_ISFIFO(receipt.stat().st_mode))
        receipt.unlink()
        saved_receipt.rename(receipt)

        program_directory = previous_bin / "agenturm-plugin-host"
        program_directory.rename(self.root / "saved plugin host")
        program_directory.mkdir()
        extra = program_directory / "extra file"
        extra.write_bytes(b"operator file")
        refused("altered managed file")
        self.assertEqual(extra.read_bytes(), b"operator file")
        self.assertTrue((previous_bin / "agenturm").is_file())

    def test_unsupported_platforms_fail_before_installation(self):
        release = self.make_release()
        self.failure(self.install("install", release, version="latest"), "X.Y.Z")
        for environment, diagnostic in (
            ({"INSTALLER_TEST_OS": "Darwin"}, "Linux"),
            ({"INSTALLER_TEST_ARCH": "riscv64"}, "architecture"),
            ({"INSTALLER_TEST_LIBC": ""}, "glibc"),
            ({"INSTALLER_TEST_LIBC": "glibc 2.38"}, "2.39"),
        ):
            with self.subTest(environment=environment):
                self.failure(self.install("install", release, env={**self.env, **environment}),
                             diagnostic)
                self.assertFalse(self.prefix.exists())
                self.assertFalse(self.bin_dir.exists())

    def test_dry_run_neither_reads_archive_nor_creates_install_paths(self):
        result = self.invoke("install", "--version", VERSION, "--archive", self.root / "missing.tar.gz",
                             "--checksum", self.root / "missing.sha256", "--dry-run")
        self.assertEqual(self.success(result), str(self.launcher))
        self.assertIn(f"agenturm-{VERSION}-linux-x86_64.tar.gz", result.stderr)
        self.assertFalse(self.prefix.exists())
        self.assertFalse(self.bin_dir.exists())
        self.assertEqual(list(self.temp.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
