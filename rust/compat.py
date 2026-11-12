"""Verify Rust writes remain readable by the current Python store.

Build first: cargo build --release --manifest-path rust/Cargo.toml
Run from the repository root: python rust/compat.py
"""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from observatory.core import Store, normalize_native  # noqa: E402

BINARY = ROOT / "rust/target/release/agent-observatory-rs"
TRACE = "a" * 32
SPAN = "1" * 16


def write(path, task, *, agent_id=None):
    span = {"trace_id": TRACE, "span_id": SPAN, "name": "agent.run",
            "start_unix_nano": 100, "end_unix_nano": 200,
            "attributes": {"agent.task": task}, "events": [],
            "resource_attributes": {"host.name": "test-host", "os.type": "linux"}}
    if agent_id:
        span["attributes"]["agent.id"] = agent_id
    path.write_text(json.dumps(span) + "\n", encoding="utf-8")
    return span


def import_with_rust(db, source):
    subprocess.run([str(BINARY), "--db", str(db), "import-jsonl", str(source)],
                   check=True, capture_output=True)


def run():
    if not BINARY.is_file():
        raise SystemExit(f"Build the release binary first: {BINARY}")
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        db = root / "runs.db"
        source = root / "spans.jsonl"

        write(source, "First task")
        import_with_rust(db, source)
        store = Store(db)
        assert store.traces()[0]["task"] == "First task"
        endpoints = store.endpoints()
        assert len(endpoints) == 1 and endpoints[0]["identity_value"] == "test-host"

        store.close()
        updated = write(source, "Updated task")
        import_with_rust(db, source)
        store = Store(db)
        assert store.traces()[0]["task"] == "Updated task"
        assert len(store.trace(TRACE)["spans"]) == 1
        assert normalize_native(updated)["resource_attributes"]["host.name"] == "test-host"
        owner = "ag1_" + "a" * 32
        store.ingest([normalize_native(write(source, "Python owner", agent_id=owner))])
        store.close()
        replacement = "ag1_" + "b" * 32
        write(source, "Rust replacement owner", agent_id=replacement)
        import_with_rust(db, source)
        store = Store(db)
        with store._connect() as connection:
            assert connection.execute("SELECT agent_id FROM spans").fetchone()[0] == replacement
        store.close()
        write(source, "Unowned replacement")
        import_with_rust(db, source)
        store = Store(db)
        with store._connect() as connection:
            assert connection.execute("SELECT agent_id FROM spans").fetchone()[0] is None
        store.close()
        print("Rust import, endpoint inventory, Python queries and current-schema ownership: passed")


if __name__ == "__main__":
    run()
