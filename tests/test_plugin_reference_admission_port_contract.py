"""Reusable provider-specific contract for reference-capable admission ports.

This suite is separate from the metadata-only admission contract. A backend
that accepts trusted reference providers must opt into it and supply a durable
state snapshot plus the receiver's provider-aware policy mutation.
"""
from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from typing import Callable

from observatory.core import Store
from observatory.plugin_admission_port import (PluginAdmissionPort,
                                                PythonStoreAdmissionPort)
from observatory.plugin_reference_admission import (
    DEFAULT_REFERENCE_ADMISSIONS,
    ReferenceAdmissionRegistry,
    ReferenceSubmission,
)
from observatory.plugin_wire import PollBatch
from observatory.plugin_admission import set_source_capture
from tests.test_plugin_reference_admission_spi import AuditAdmission


PLUGIN_ID = "reference-contract.fixture"
SOURCE_ID = "reference-contract-source"
GENERATION = "c" * 64


def reference_batch(cursor: str) -> PollBatch:
    return PollBatch(SOURCE_ID, cursor, ({
        "event_id": "reference-event", "revision": 0,
        "kind": "lifecycle", "time_ns": "9007199254740993",
        "payload": {"state": "completed"},
    },))


class PluginReferenceAdmissionPortContract:
    """Behavioral suite requiring provider-aware backend fixtures.

    ``make_backend`` returns ``(new_port, set_capture_enabled, snapshot,
    set_failure_mode)``. It supplies an audit-compatible test provider that
    accepts the fixture's provider ID/grant and can inject failures after a
    provider commit or erase has written. The policy hook must pass the same
    registry used for admission so revoke can erase provider-owned state
    atomically with evidence and source policy. Snapshot state must include
    source/cursor/evidence, provider-owned rows and durable provider ownership.
    """

    def make_backend(self, directory: Path, registry: ReferenceAdmissionRegistry):
        raise NotImplementedError

    def make_provider(self):
        return AuditAdmission()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.provider = self.make_provider()
        self.registry = DEFAULT_REFERENCE_ADMISSIONS.with_provider(self.provider)
        (self.new_port, self.set_capture_enabled, self.durable_snapshot,
         self.set_failure_mode) = self.make_backend(self.root, self.registry)
        self.port = self.new_port()
        self.addCleanup(self.port.close)
        self.port.install_reference_schema(self.registry)
        self.lease = self.port.activate_source(
            plugin_id=PLUGIN_ID, source_id=SOURCE_ID,
            generation=GENERATION, content_fingerprint="",
            reference_admissions=self.registry)

    def submission(self, ref="audit-a", *, provider_id="audit-fixture",
                   grant="host-owned-audit"):
        return ReferenceSubmission(provider_id, (ref,), grant)

    def commit(self, *, expected="", cursor="cursor-1", ref="audit-a",
               provider_id="audit-fixture", grant="host-owned-audit"):
        return self.port.commit_batch(
            self.lease, expected, reference_batch(cursor), allow_root_gap=False,
            reference_submission=self.submission(ref, provider_id=provider_id,
                                                 grant=grant),
            reference_admissions=self.registry)

    def test_provider_commit_and_exact_replay_are_durable(self):
        self.assertEqual(self.commit(), "cursor-1")
        committed = self.durable_snapshot(SOURCE_ID)
        self.assertEqual(self.commit(), "cursor-1")
        self.assertEqual(self.durable_snapshot(SOURCE_ID), committed)
        with self.assertRaises(Exception):
            self.commit(ref="audit-b")
        self.assertEqual(self.durable_snapshot(SOURCE_ID), committed)

    def test_unregistered_provider_and_invalid_grant_fail_without_changes(self):
        before = self.durable_snapshot(SOURCE_ID)
        with self.assertRaises(Exception):
            self.commit(provider_id="unregistered-provider")
        self.assertEqual(self.durable_snapshot(SOURCE_ID), before)
        with self.assertRaises(Exception):
            self.commit(grant="forged-grant")
        self.assertEqual(self.durable_snapshot(SOURCE_ID), before)

    def test_provider_failure_rolls_back_evidence_cursor_and_provider_state(self):
        before = self.durable_snapshot(SOURCE_ID)
        self.set_failure_mode("provider_commit_after_write", True)
        try:
            with self.assertRaises(Exception):
                self.commit()
        finally:
            self.set_failure_mode("provider_commit_after_write", False)
        self.assertEqual(self.durable_snapshot(SOURCE_ID), before)

    def test_revoke_erases_provider_state_and_old_lease_cannot_commit(self):
        self.commit()
        before_revoke = self.durable_snapshot(SOURCE_ID)
        self.set_failure_mode("provider_erase_after_write", True)
        try:
            with self.assertRaises(Exception):
                self.set_capture_enabled(SOURCE_ID, False)
        finally:
            self.set_failure_mode("provider_erase_after_write", False)
        self.assertEqual(self.durable_snapshot(SOURCE_ID), before_revoke)

        self.set_capture_enabled(SOURCE_ID, False)
        revoked = self.durable_snapshot(SOURCE_ID)
        self.assertTrue(self.port.capture_revoked(SOURCE_ID))
        self.assertEqual(revoked[2], (), "revoke left provider-owned reference rows")
        self.assertEqual(revoked[3], (), "revoke left provider ownership behind")
        with self.assertRaises(Exception) as raised:
            self.port.commit_batch(
                self.lease, "cursor-1", reference_batch("cursor-2"),
                allow_root_gap=False, reference_submission=self.submission("audit-b"),
                reference_admissions=self.registry)
        self.assertEqual(self.port.classify_lease_failure(
            raised.exception, SOURCE_ID), "capture_revoked")
        self.assertEqual(self.durable_snapshot(SOURCE_ID), revoked)


class PythonStoreReferenceAdmissionPortContractTests(
        PluginReferenceAdmissionPortContract, unittest.TestCase):
    def make_backend(self, directory: Path, registry: ReferenceAdmissionRegistry):
        store = Store(str(directory / "receiver.db"))
        self.addCleanup(store.close)

        def set_capture_enabled(source_id, enabled):
            return set_source_capture(store, source_id=source_id, enabled=enabled,
                                      reference_admissions=registry)

        def durable_snapshot(source_id):
            with store._connect() as db:
                source = db.execute(
                    "SELECT receiver_id,plugin_id,generation,cursor,dirty_seq,lease_epoch,"
                    "policy_epoch,capture_enabled FROM plugin_source_cursor_v1 WHERE source_id=?",
                    (source_id,)).fetchone()
                evidence = db.execute(
                    "SELECT event_id_hash,revision,record_digest FROM plugin_evidence_v1 "
                    "WHERE source_id=? ORDER BY event_id_hash,revision", (source_id,)).fetchall()
                provider_rows = db.execute(
                    "SELECT source_id,cursor,digest FROM audit_fixture WHERE source_id=?",
                    (source_id,)).fetchall()
                provider_owners = db.execute(
                    "SELECT provider_id FROM plugin_reference_owner_v1 WHERE source_id=? "
                    "ORDER BY provider_id", (source_id,)).fetchall()
            return (tuple(source) if source is not None else None,
                    tuple(tuple(row) for row in evidence),
                    tuple(tuple(row) for row in provider_rows),
                    tuple(tuple(row) for row in provider_owners))

        def set_failure_mode(mode, enabled):
            if mode == "provider_commit_after_write":
                self.provider.fail_after_write = enabled
            elif mode == "provider_erase_after_write":
                self.provider.fail_erase = enabled
            else:
                raise ValueError("unknown provider failure mode")

        return (lambda: PythonStoreAdmissionPort(store), set_capture_enabled,
                durable_snapshot, set_failure_mode)
