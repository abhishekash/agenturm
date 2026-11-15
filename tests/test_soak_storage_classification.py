"""Healthy writes are not retries; missing, stalled or final work cannot pass."""
from contextlib import redirect_stdout
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from scripts.soak_monitor import storage_observation
from scripts.validate_combined_monitor import FixtureSession
from scripts import validate_combined_monitor


def source(**changes):
    diagnostics=dict(storage_diagnostics_version=1,storage_state='idle',
        pending_storage_records=0,storage_in_flight_records=0,storage_retry_records=0,
        storage_unknown_records=0,storage_retry_pending=False,storage_attempt_in_flight=False,
        storage_batch_id=None,storage_batch_age_seconds=0,storage_progress_age_seconds=0,
        storage_progress_sequence=0)
    diagnostics.update(changes)
    return dict(runtime='Codex',source_id='generated-source',diagnostics=diagnostics)


def writing(**changes):
    return source(**{**dict(storage_state='writing',pending_storage_records=4,
        storage_in_flight_records=4,storage_attempt_in_flight=True,storage_batch_id='batch-1',
        storage_batch_age_seconds=.1,storage_progress_age_seconds=.1,storage_progress_sequence=1),**changes})


class StorageClassificationTests(unittest.TestCase):
    def observe(self,row,**kwargs):
        return storage_observation(dict(sessions=[row]),elapsed=kwargs.pop('elapsed',0),
            tracking=kwargs.pop('tracking',{}),**kwargs)

    def test_active_batch_is_counted_and_only_quiescent_state_can_finish(self):
        result=self.observe(writing())
        self.assertEqual(result['failures'],[])
        self.assertEqual(result['total_buffered_records'],4)
        self.assertEqual(result['in_flight_records'],4)
        self.assertTrue(self.observe(writing(),quiescent=True)['failures'])
        self.assertEqual(self.observe(source(),quiescent=True)['failures'],[])

    def test_policy_only_retry_is_failure_even_with_zero_records(self):
        result=self.observe(source(storage_state='retry',storage_retry_pending=True,
            storage_batch_id='policy-batch'))
        self.assertIn('Native storage retry pending',result['failures'])
        self.assertEqual(result['total_buffered_records'],0)

    def test_retry_during_a_new_attempt_stays_failed_until_fully_drained(self):
        result=self.observe(writing(storage_state='retry',storage_retry_pending=True,
            storage_in_flight_records=0,storage_retry_records=4))
        self.assertIn('Native storage retry pending',result['failures'])
        self.assertEqual(result['retry_records'],4)

    def test_incomplete_invalid_or_incoherent_metadata_cannot_certify_storage(self):
        cases=[source(),writing(storage_batch_age_seconds=float('nan')),
            writing(storage_in_flight_records=3),writing(storage_retry_pending=True),
            source(storage_diagnostics_version=True),writing(storage_state='unknown'),
            source(storage_attempt_in_flight=True)]
        del cases[0]['diagnostics']['storage_state']
        for row in cases:
            with self.subTest(row=row):self.assertTrue(self.observe(row)['failures'])

    def test_both_reported_age_and_independent_progress_clock_are_bounded(self):
        self.assertTrue(self.observe(writing(storage_batch_age_seconds=5.001))['failures'])
        self.assertTrue(self.observe(writing(storage_progress_age_seconds=5.001))['failures'])
        tracking={}
        self.assertFalse(self.observe(writing(),elapsed=10,tracking=tracking)['failures'])
        self.assertTrue(self.observe(writing(),elapsed=16,tracking=tracking)['failures'])
        self.assertFalse(self.observe(writing(storage_progress_sequence=2),elapsed=16,tracking=tracking)['failures'])
        self.assertFalse(self.observe(source(),elapsed=17,tracking=tracking)['failures'])
        self.assertEqual(tracking,{})

    def test_legacy_missing_native_state_fails_but_explicit_opencode_uses_its_own_contract(self):
        legacy=dict(runtime='Codex',source_id='generated',diagnostics=dict(pending_storage_records=0))
        self.assertTrue(self.observe(legacy)['failures'])
        self.assertEqual(self.observe(dict(runtime='OpenCode',source_id='generated',diagnostics={}))['failures'],[])


class ModalInputReadinessTests(unittest.TestCase):
    def test_modal_return_and_palette_input_are_observed_before_more_keys(self):
        with patch('scripts.validate_combined_monitor.until',side_effect=lambda predicate,*a,**k:predicate()):
            main=dict(pid=42,at=100,screen='ObservatoryScreen',modal_stack=1,focus='agent-table')
            def session(row):
                value=FixtureSession.__new__(FixtureSession);value.roles={'cockpit':42};value.beats=lambda role:[row];return value
            self.assertIsNotNone(session(main).wait_cockpit('cockpit',after=99))
            for changes in (dict(at=98),dict(screen='UsageScreen'),dict(modal_stack=3),dict(focus=None),dict(pid=41)):
                self.assertIsNone(session({**main,**changes}).wait_cockpit('cockpit',after=99))
            palette={**main,'screen':'CommandPalette','focused_control_type':'CommandInput'}
            self.assertIsNotNone(session(palette).wait_palette('cockpit',after=99))
            for changes in (dict(at=98),dict(focused_control_type=None),dict(screen='UsageScreen'),dict(pid=41)):
                self.assertIsNone(session({**palette,**changes}).wait_palette('cockpit',after=99))

    def test_interrupted_modal_validation_cannot_publish_a_success(self):
        class InterruptedSession:
            def __init__(self,args):self.report={'errors':[]};self.closed=False
            def start(self):raise KeyboardInterrupt
            def close(self):
                self.closed=True;self.report['passed']=not self.report['errors']
        with tempfile.TemporaryDirectory()as directory:
            session=InterruptedSession(None)
            argv=['validate_combined_monitor.py','modals','--output',str(Path(directory)/'new-run')]
            with patch('sys.argv',argv),patch.object(validate_combined_monitor,'FixtureSession',return_value=session),\
                    patch.object(validate_combined_monitor,'bind_application'),\
                    patch.object(validate_combined_monitor.shutil,'which',return_value='/bin/tmux'),redirect_stdout(io.StringIO()):
                self.assertEqual(validate_combined_monitor.main(),130)
            self.assertTrue(session.closed);self.assertFalse(session.report['passed'])
            self.assertEqual(session.report['errors'],['Validation interrupted before completion'])


if __name__=='__main__':unittest.main()
