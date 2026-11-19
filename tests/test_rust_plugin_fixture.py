"""Cross-language process/Store conformance for a separately compiled Rust plugin."""
import hashlib
import os
import tempfile
import unittest
from contextlib import ExitStack, closing
from pathlib import Path

from observatory.core import Store
from observatory.plugin_manifest import PluginManifest
from observatory.plugin_process import PluginProcess
from observatory.plugin_runtime import PluginSession


class RustFixtureIntegrationTests(unittest.TestCase):
    def test_rust_worker_commits_once_then_idles_across_restart(self):
        executable = os.environ.get("AGENTURM_RUST_FIXTURE_BIN")
        if not executable:
            self.skipTest("Rust fixture binary is built in the Rust CI job")
        binary = Path(executable).resolve()
        self.assertTrue(binary.is_file())
        with tempfile.TemporaryDirectory() as root, ExitStack() as stack:
            base = Path(root)
            manifest_path = base / "rust-fixture.toml"
            manifest_path.write_text(
                'id = "fixture.rust"\n'
                'version = "0.0.1"\n'
                'protocol = "1"\n'
                f'entrypoint = ["{binary}"]\n'
                f'artifact_sha256 = "{hashlib.sha256(binary.read_bytes()).hexdigest()}"\n'
                'provides = ["source-adapter/1"]\n'
                'requires = ["evidence-ingest/1"]\n'
                'depends_on = []\n'
                'grants = ["configured-source-read"]\n')
            manifest = PluginManifest.read(manifest_path)
            store = stack.enter_context(closing(Store(str(base / "agenturm.db"))))
            def session():
                return PluginSession(store, manifest, source_id="rust-source",
                                     allowed_grants=("configured-source-read",), timeout=2.0)

            with session() as worker:
                first = worker.poll_once()
                self.assertEqual(first.cursor, "rust-fixture-next")
                self.assertEqual(first.records[0]["time_ns"], "9007199254740993")
                self.assertEqual(worker.poll_once().records, ())
                self.assertEqual(worker.cursor, "rust-fixture-next")
            with store._connect() as db:
                row = db.execute("SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 "
                                 "WHERE source_id='rust-source'").fetchone()
                evidence = db.execute("SELECT kind,time_ns,payload_json FROM plugin_evidence_v1").fetchall()
                self.assertEqual(tuple(row), ("rust-fixture-next", 1))
                self.assertEqual(len(evidence), 1)
                self.assertEqual(evidence[0]["time_ns"], 9007199254740993)
            with session() as worker:
                self.assertEqual(worker.cursor, "rust-fixture-next")
                self.assertEqual(worker.poll_once().records, ())
            with store._connect() as db:
                self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 1)
                self.assertEqual(db.execute("SELECT dirty_seq FROM plugin_source_cursor_v1 "
                                            "WHERE source_id='rust-source'").fetchone()[0], 1)
            # The process transport accepts character-counted Unicode source
            # IDs, including C1 characters. The Rust fixture must use the same
            # handshake predicate even though the metadata inbox uses a
            # narrower host-owned source naming policy.
            for source_id in ("α" * 129, "source\u0085id"):
                with self.subTest(source_id=source_id), PluginProcess(
                    manifest, source_id=source_id,
                    allowed_grants=("configured-source-read",), timeout=2.0) as host:
                    self.assertEqual(host.poll("").source_id, source_id)


if __name__ == "__main__":
    unittest.main()
