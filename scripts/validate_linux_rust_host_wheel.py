"""Exercise native and pure Agenturm wheels in isolated uv tool directories."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import signal
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from urllib.request import urlopen
from zipfile import ZipFile


ROOT = Path(__file__).resolve().parents[1]


def wheel_contents(path: Path, *, native: bool) -> None:
    with ZipFile(path) as archive:
        names = archive.namelist()
        wheel = next(name for name in names if name.endswith(".dist-info/WHEEL"))
        tags = archive.read(wheel).decode()
        artifacts = [name for name in names if name.endswith("/_native/agenturm-plugin-host")]
        metadata = [name for name in names if name.endswith("/_native/host.json")]
        if native:
            assert "Tag: py3-none-linux_x86_64" in tags
            assert "Root-Is-Purelib: false" in tags
            assert len(artifacts) == len(metadata) == 1
            expected = json.loads(archive.read(metadata[0]))["sha256"]
            assert hashlib.sha256(archive.read(artifacts[0])).hexdigest() == expected
        else:
            assert "Tag: py3-none-any" in tags
            assert "Root-Is-Purelib: true" in tags
            assert not artifacts and not metadata


def install_tool(wheel: Path, base: Path) -> tuple[Path, Path, dict[str, str]]:
    tools, binaries = base / "tools", base / "bin"
    environment = {**os.environ, "UV_TOOL_DIR": str(tools),
                   "UV_TOOL_BIN_DIR": str(binaries)}
    subprocess.run(["uv", "tool", "install", str(wheel), "--python", sys.executable],
                   cwd=base, env=environment, capture_output=True, text=True, check=True)
    cli = binaries / "agenturm"
    python = tools / "agenturm" / "bin" / "python"
    assert cli.is_file() and python.is_file()
    return cli, python, environment


def native_receiver(cli: Path, adapter: Path, base: Path, environment: dict[str, str]) -> None:
    source = base / "source.jsonl"
    source.write_text(json.dumps({"id": "completed", "session": "wheel-installed",
                                  "at_ns": "100", "kind": "lifecycle",
                                  "state": "completed"}) + "\n")
    install = subprocess.run([
        sys.executable, str(ROOT / "examples/rust_sample_harness_plugin/install.py"),
        "--binary", str(adapter), "--destination", str(base / "plugin"),
        "--source", str(source)], cwd=base, capture_output=True, text=True, check=True)
    config = Path(install.stdout.strip())
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    command = [str(cli), "--db", str(base / "telemetry.db"), "serve",
               "--no-auth", "--no-local-agents", "--port", str(port),
               "--plugin-config", str(config), "--use-bundled-rust-plugin-host"]
    process = subprocess.Popen(command, cwd=base,
                               env={**environment, "UV_OFFLINE": "1"},
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                               text=True)
    try:
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise AssertionError("wheel receiver exited before projected evidence")
            try:
                with urlopen(f"http://127.0.0.1:{port}/api/health", timeout=1) as response:
                    local = json.load(response)["local_agents"]
                sessions = local.get("sessions") or []
                workers = local.get("plugin_workers") or []
                if (sessions and sessions[0]["native_session_id"] == "wheel-installed"
                        and sessions[0]["run_state"] == "completed"
                        and workers and workers[0]["state"] == "running"):
                    return
            except OSError:
                pass
            time.sleep(0.05)
        raise AssertionError("installed wheel did not project a Rust-hosted session")
    finally:
        if process.poll() is None:
            process.send_signal(signal.SIGINT)
        try:
            output, _ = process.communicate(timeout=8)
        except subprocess.TimeoutExpired:
            process.kill()
            output, _ = process.communicate(timeout=3)
        if process.returncode != 0:
            raise AssertionError(f"wheel receiver exited {process.returncode}: {output[-2000:]}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-wheel", required=True, type=Path)
    parser.add_argument("--pure-wheel", required=True, type=Path)
    parser.add_argument("--adapter-bin", required=True, type=Path)
    args = parser.parse_args()
    native, pure, adapter = (path.resolve() for path in
                             (args.native_wheel, args.pure_wheel, args.adapter_bin))
    wheel_contents(native, native=True)
    wheel_contents(pure, native=False)
    with tempfile.TemporaryDirectory(prefix="agenturm-wheel-verify-") as temp:
        base = Path(temp)
        native_base = base / "native"
        pure_base = base / "pure"
        native_base.mkdir()
        pure_base.mkdir()
        cli, python, environment = install_tool(native, native_base)
        result = subprocess.run([str(python), "-c",
            "from observatory.rust_host_bundle import bundled_rust_host; "
            "from pathlib import Path; import hashlib; "
            "p,h=bundled_rust_host(); assert Path(p).is_file(); "
            "assert hashlib.sha256(Path(p).read_bytes()).hexdigest()==h"],
            cwd=native_base, capture_output=True, text=True)
        assert result.returncode == 0, result.stderr
        native_receiver(cli, adapter, native_base, environment)
        pure_cli, pure_python, _ = install_tool(pure, pure_base)
        assert subprocess.run([str(pure_cli), "serve", "--help"], cwd=pure_base,
                              capture_output=True).returncode == 0
        unavailable = subprocess.run([str(pure_python), "-c",
            "from observatory.rust_host_bundle import bundled_rust_host; bundled_rust_host()"],
            cwd=pure_base, capture_output=True, text=True)
        assert unavailable.returncode != 0
        assert "no bundled Rust plugin host" in unavailable.stderr
        refused = subprocess.run([
            str(pure_cli), "--db", str(pure_base / "not-created.db"), "serve",
            "--plugin-config", str(pure_base / "not-created.toml"),
            "--use-bundled-rust-plugin-host"], cwd=pure_base,
            capture_output=True, text=True)
        assert refused.returncode == 2
        assert "no bundled Rust plugin host" in refused.stderr
        assert not (pure_base / "not-created.db").exists()
    print("Linux native wheel and pure compatibility wheel verified in isolated uv tools")


if __name__ == "__main__":
    main()
