"""Host-selected reference hooks share the source evidence transaction."""
import hashlib
import tempfile
import unittest
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
from observatory.plugin_reference_admission import (
    DEFAULT_REFERENCE_ADMISSIONS, PreparedReference, ReferenceAdmissionProvider,
    ReferenceSubmission,
)
from observatory.plugin_pi_reference_admission import PiContentAdmission
from observatory.plugin_wire import PollBatch, ProtocolError
from tests import test_plugin_pi_content


class AuditAdmission(ReferenceAdmissionProvider):
    """Synthetic trusted hook with no source content or private reference."""
    id = "audit-fixture"

    def __init__(self):
        self.fail_after_write = False
        self.on_verify = None
        self.reason = ""

    def prepare(self, refs, grant, batch, lease):
        if (type(refs) is not tuple or len(refs) != 1
                or refs[0] not in ("audit-a", "audit-b")
                or grant != "host-owned-audit" or batch.source_id != lease.source_id):
            raise ProtocolError("invalid trusted audit submission")
        return PreparedReference(self.id, refs, hashlib.sha256(refs[0].encode()).hexdigest(),
                                 grant)

    def verify(self, prepared, batch, prior_cursor):
        if self.on_verify is not None:
            self.on_verify()
        return prepared.checked, self.reason

    def replay(self, db, source_id, cursor, prepared):
        row = db.execute("SELECT cursor,digest FROM audit_fixture WHERE source_id=?",
                         (source_id,)).fetchone()
        if ((row is None and prepared is not None)
                or (row is not None and (prepared is None or
                    (row["cursor"], row["digest"]) != (cursor, prepared.digest)))):
            raise ProtocolError("audit replay mismatch")

    def commit(self, context):
        if context.prepared is None:
            context.db.execute("DELETE FROM audit_fixture WHERE source_id=?",
                               (context.lease.source_id,))
            return None
        context.db.execute("INSERT INTO audit_fixture VALUES(?,?,?) "
                           "ON CONFLICT(source_id) DO UPDATE SET cursor=excluded.cursor,"
                           "digest=excluded.digest",
                           (context.lease.source_id, context.batch.cursor,
                            context.prepared.digest))
        if self.fail_after_write:
            raise RuntimeError("injected trusted hook failure")
        return None


class ReferenceAdmissionSPITests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.store = Store(Path(temporary.name) / "receiver.db")
        self.addCleanup(self.store.close)
        self.lease = activate_source(self.store, plugin_id="fixture.plugin",
                                     source_id="fixture-source", generation="a" * 64)
        with self.store._connect() as db:
            db.execute("CREATE TABLE audit_fixture(source_id TEXT PRIMARY KEY,"
                       "cursor TEXT NOT NULL,digest TEXT NOT NULL)")
        self.provider = AuditAdmission()
        self.registry = DEFAULT_REFERENCE_ADMISSIONS.with_provider(self.provider)
        self.batch = PollBatch("fixture-source", "cursor-one", ({
            "event_id": "event-one", "revision": 0, "kind": "lifecycle",
            "time_ns": "9007199254740993", "payload": {"state": "completed"},
        },))

    def snapshot(self):
        with self.store._connect() as db:
            cursor = db.execute("SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 "
                                "WHERE source_id=?", (self.lease.source_id,)).fetchone()
            evidence = db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0]
            audit = db.execute("SELECT cursor,digest FROM audit_fixture").fetchall()
        return tuple(cursor), evidence, [tuple(row) for row in audit]

    def commit(self, refs=("audit-a",), expected=""):
        return commit_batch(
            self.store, self.lease, expected, self.batch,
            reference_submission=ReferenceSubmission(
                "audit-fixture", refs, "host-owned-audit"),
            reference_admissions=self.registry)

    def test_trusted_hook_and_evidence_commit_atomically_and_replay_exactly(self):
        before = self.snapshot()
        self.provider.fail_after_write = True
        with self.assertRaisesRegex(RuntimeError, "injected trusted hook failure"):
            self.commit()
        self.assertEqual(self.snapshot(), before)
        self.provider.fail_after_write = False
        self.assertEqual(self.commit(), "cursor-one")
        after = self.snapshot()
        self.assertEqual((after[0], after[1]), (("cursor-one", 1), 1))
        self.assertEqual(len(after[2]), 1)
        self.assertEqual(self.commit(), "cursor-one")
        self.assertEqual(self.snapshot(), after)
        with self.assertRaisesRegex(ProtocolError, "audit replay mismatch"):
            self.commit(refs=("audit-b",))
        self.assertEqual(self.snapshot(), after)

    def test_default_receiver_cannot_select_fixture_or_mix_legacy_arguments(self):
        before = self.snapshot()
        with self.assertRaisesRegex(ProtocolError, "provider is unavailable"):
            commit_batch(self.store, self.lease, "", self.batch,
                         reference_submission=ReferenceSubmission(
                             "audit-fixture", ("audit-a",), "host-owned-audit"))
        with self.assertRaisesRegex(ProtocolError, "cannot mix"):
            commit_batch(self.store, self.lease, "", self.batch,
                         content_refs=("forged",),
                         reference_submission=ReferenceSubmission(
                             "audit-fixture", ("audit-a",), "host-owned-audit"),
                         reference_admissions=self.registry)
        self.assertEqual(self.snapshot(), before)

    def test_malformed_provider_id_rejected_before_dispatch(self):
        with self.assertRaisesRegex(ProtocolError, "invalid trusted reference provider ID"):
            commit_batch(self.store, self.lease, "", self.batch,
                         reference_submission=ReferenceSubmission(
                             ["audit-fixture"], ("audit-a",), "host-owned-audit"),
                         reference_admissions=self.registry)
        self.assertEqual(self.snapshot()[1], 0)

    def test_pi_content_preparation_snapshots_mutable_reference(self):
        fixture = test_plugin_pi_content.PiContentTests("runTest")
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        batch, ref = fixture.bundle()
        original_hash = ref["row_sha256"]
        prepared = PiContentAdmission().prepare((ref,), fixture.grant, batch, fixture.lease)
        ref["row_sha256"] = "0" * 64
        self.assertEqual(prepared.checked[0][0]["row_sha256"], original_hash)
        self.assertNotEqual(prepared.checked[0][0]["row_sha256"], ref["row_sha256"])

    def test_revocation_fences_admission_but_generic_receipt_cleanup_is_not_installed(self):
        self.commit()
        set_source_capture(self.store, source_id=self.lease.source_id, enabled=False)
        with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
            self.commit()
        after = self.snapshot()
        self.assertEqual(after[1], 0)
        # The fixture receipt contains no private data. A future lifecycle SPI
        # must add provider cleanup before private custom providers can ship.
        self.assertEqual(len(after[2]), 1)

    def test_revocation_between_verify_and_writer_transaction_blocks_commit(self):
        self.provider.on_verify = lambda: set_source_capture(
            self.store, source_id=self.lease.source_id, enabled=False)
        with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
            self.commit()
        after = self.snapshot()
        self.assertEqual(after[1:], (0, []))

    def test_unrelated_provider_reason_does_not_change_pi_status(self):
        self.provider.reason = "unanchored_origin"
        self.commit()
        with self.store._connect() as db:
            status = db.execute("SELECT reason FROM plugin_content_status_v1 "
                                "WHERE source_id=?", (self.lease.source_id,)).fetchone()
        self.assertEqual(status[0], "not_requested")


if __name__ == "__main__":
    unittest.main()
