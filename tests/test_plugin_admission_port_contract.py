"""Reusable behavioral contract for trusted process-plugin admission ports.

A backend conformance test can subclass ``PluginAdmissionPortContract`` and
supply a fixture with a durable receiver, a port factory and a receiver-owned
capture-policy mutation. The default Python Store backend exercises the same
suite below; structural Protocol checks alone are not conformance evidence.
"""
from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from typing import Callable

from observatory.core import Store
from observatory.plugin_admission import set_source_capture
from observatory.plugin_admission_port import (PluginAdmissionPort,
                                                PythonStoreAdmissionPort)
from observatory.plugin_reference_admission import DEFAULT_REFERENCE_ADMISSIONS
from observatory.plugin_wire import PollBatch, ProtocolError


PLUGIN_ID = "contract.fixture"
SOURCE_ID = "contract-source"
GENERATION = "a" * 64


def lifecycle_record(event_id: str, state: str = "completed") -> dict:
    return {"event_id": event_id, "revision": 0, "kind": "lifecycle",
            "time_ns": "9007199254740993", "payload": {"state": state}}


def batch(cursor: str, *records: dict) -> PollBatch:
    return PollBatch(SOURCE_ID, cursor, tuple(records))


class PluginAdmissionPortContract:
    """Behavioral suite for an isolated backend fixture.

    ``make_backend`` returns ``(new_port, set_capture_enabled)``. The fixture
    must preserve receiver state between ports made with the same directory.
    ``set_capture_enabled`` models the receiver's policy-control plane, which
    is deliberately outside the admission port itself.
    """

    def make_backend(self, directory: Path) -> tuple[
            Callable[[], PluginAdmissionPort], Callable[[str, bool], object]]:
        raise NotImplementedError

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.new_port, self.set_capture_enabled = self.make_backend(self.root)
        self.port = self.new_port()
        self._port_closed = False
        self.reference_admissions = DEFAULT_REFERENCE_ADMISSIONS
        self.addCleanup(self._close_original_port)

    def _close_original_port(self):
        if not self._port_closed:
            self._port_closed = True
            self.port.close()

    def activate(self, port=None):
        target = self.port if port is None else port
        return target.activate_source(
            plugin_id=PLUGIN_ID, source_id=SOURCE_ID, generation=GENERATION,
            content_fingerprint="",
            reference_admissions=self.reference_admissions)

    def test_activation_and_initial_reads_have_empty_baseline(self):
        lease = self.activate()
        self.assertEqual(lease.cursor, "")
        self.assertEqual(self.port.member_cursors(lease), {})
        self.assertEqual(self.port.last_source_diagnostic(lease), {})

    def test_diagnostic_and_member_cursors_round_trip_through_port(self):
        lease = self.activate()
        diagnostic = {"event_id": "warning-1", "revision": 0,
                      "kind": "diagnostic", "time_ns": "123456789",
                      "payload": {"code": "source_timeout", "severity": "warning"}}
        diagnostic_batch = batch("cursor-diagnostic", diagnostic)
        self.port.commit_batch(
            lease, "", diagnostic_batch, allow_root_gap=False,
            reference_submission=None,
            reference_admissions=self.reference_admissions)
        self.assertEqual(self.port.last_source_diagnostic(lease), {
            "code": "source_timeout", "severity": "warning",
            "time_ns": "123456789"})

        member_id = "b" * 64
        member_batch = PollBatch(SOURCE_ID, "cursor-member", (),
                                 member_id=member_id,
                                 member_cursor="member-cursor-1")
        self.port.commit_batch(
            lease, "cursor-diagnostic", member_batch, allow_root_gap=False,
            reference_submission=None,
            reference_admissions=self.reference_admissions)
        self.assertEqual(self.port.member_cursors(lease), {
            member_id: "member-cursor-1"})

    def test_durable_cursor_survives_reopen_and_exact_replay(self):
        lease = self.activate()
        first = batch("cursor-1", lifecycle_record("event-1"))
        self.assertEqual(self.port.commit_batch(
            lease, "", first, allow_root_gap=False,
            reference_submission=None,
            reference_admissions=self.reference_admissions), "cursor-1")
        self._close_original_port()
        reopened = self.new_port()
        self.addCleanup(reopened.close)
        resumed = self.activate(reopened)
        self.assertEqual(resumed.cursor, "cursor-1")
        self.assertEqual(reopened.commit_batch(
            resumed, "", first, allow_root_gap=False,
            reference_submission=None,
            reference_admissions=self.reference_admissions), "cursor-1")
        with self.assertRaises(ProtocolError):
            reopened.commit_batch(
                resumed, "", batch("cursor-other", lifecycle_record("event-2")),
                allow_root_gap=False, reference_submission=None,
                reference_admissions=self.reference_admissions)

    def test_new_activation_fences_old_lease_and_classifies_it(self):
        stale = self.activate()
        self.activate()
        with self.assertRaises(Exception) as raised:
            self.port.commit_batch(
                stale, "", batch("cursor-1", lifecycle_record("event-1")),
                allow_root_gap=False, reference_submission=None,
                reference_admissions=self.reference_admissions)
        self.assertEqual(self.port.classify_lease_failure(
            raised.exception, SOURCE_ID), "source_fenced")

    def test_revocation_is_visible_and_classified_as_capture_revoked(self):
        self.activate()
        self.set_capture_enabled(SOURCE_ID, False)
        self.assertTrue(self.port.capture_revoked(SOURCE_ID))
        self.assertEqual(self.port.classify_lease_failure(
            ProtocolError("plugin source capture revoked"), SOURCE_ID),
            "capture_revoked")

    def test_conflicting_batch_rolls_back_cursor_and_can_resume(self):
        lease = self.activate()
        first = batch("cursor-1", lifecycle_record("event-1"))
        self.port.commit_batch(lease, "", first, allow_root_gap=False,
                               reference_submission=None,
                               reference_admissions=self.reference_admissions)
        conflict = batch("cursor-2", lifecycle_record("event-2"),
                         lifecycle_record("event-1", "failed"))
        with self.assertRaises(ProtocolError):
            self.port.commit_batch(lease, "cursor-1", conflict,
                                   allow_root_gap=False,
                                   reference_submission=None,
                                   reference_admissions=self.reference_admissions)
        reopened = self.new_port()
        self.addCleanup(reopened.close)
        self.assertEqual(self.activate(reopened).cursor, "cursor-1")


class PythonStoreAdmissionPortContractTests(PluginAdmissionPortContract,
                                            unittest.TestCase):
    def make_backend(self, directory: Path):
        store = Store(str(directory / "receiver.db"))
        self.addCleanup(store.close)
        return (lambda: PythonStoreAdmissionPort(store),
                lambda source_id, enabled: set_source_capture(
                    store, source_id=source_id, enabled=enabled))
