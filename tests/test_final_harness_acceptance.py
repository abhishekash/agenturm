"""Registered resource windows and functional read completeness cannot be bypassed."""
from types import SimpleNamespace
import time
import unittest
from unittest.mock import patch

from scripts.benchmark_monitor_idle import idle_publishable
from scripts.validate_combined_monitor import FixtureSession
from tests import test_validation_harness as contracts


class IdlePublicationContracts(unittest.TestCase):
    def options(self,**changes):
        return SimpleNamespace(**{**dict(seconds=300,warmup=60,profile=False,smoke=False,allow_dirty=False),**changes})

    def test_only_complete_registered_windows_can_be_publishable(self):
        options=self.options();source={'dirty':False}
        self.assertTrue(idle_publishable(options,source,[]))
        for seconds,warmup in ((5,5),(299.999,60),(300,59.999)):
            with self.subTest(seconds=seconds,warmup=warmup):
                args=SimpleNamespace(**{**vars(options),'seconds':seconds,'warmup':warmup})
                self.assertFalse(idle_publishable(args,source,[]))
        for field in ('profile','smoke','allow_dirty'):
            with self.subTest(field=field):
                args=SimpleNamespace(**{**vars(options),field:True})
                self.assertFalse(idle_publishable(args,source,[]))
        self.assertFalse(idle_publishable(options,{'dirty':True},[]))
        self.assertFalse(idle_publishable(options,source,['missing sample']))


class FunctionalReadinessContracts(unittest.TestCase):
    def session(self,changes=None):
        now=time.time();row=contracts.ValidationContracts().healthy()
        row.update(at=now,updated=now-1,display_updated=now-1,pid=42,states=[dict(session='generated',state='Working')])
        row.update(changes or {})
        session=FixtureSession.__new__(FixtureSession)
        session.roles={'cockpit':42};session.health={'receiver_id':'rx'}
        session.beats=lambda role:[row]
        return session

    def test_published_useful_frame_must_pass_all_source_storage_and_inventory_masks(self):
        session=self.session()
        self.assertEqual(session.wait_ui('cockpit')['pid'],42)
        for changes in (dict(native_schema_errors=1),dict(source_backlog_bytes=1),dict(pending_storage_records=1),
            dict(opencode_diagnostics={'skipped_budget_records':1}),dict(source_read_error=True),
            dict(indexing_ready=None),dict(native_inventory_warning='partial'),dict(source_omitted=1),
            dict(source_hierarchy_write_errors=1),dict(display_receiver_id='foreign'),dict(display_updated=90)):
            with self.subTest(changes=changes):
                with self.assertRaisesRegex(RuntimeError,'validation failure'):
                    self.session(changes).wait_ui('cockpit')

    def test_old_process_heartbeat_and_empty_shell_do_not_establish_readiness(self):
        # Call exactly one readiness attempt; never wait on a real child process.
        with patch('scripts.validate_combined_monitor.until',side_effect=lambda predicate,*args,**kwargs:predicate()):
            self.assertIsNone(self.session(dict(pid=41,native_schema_errors=1)).wait_ui('cockpit'))
            self.assertIsNone(self.session(dict(states=[])).wait_ui('cockpit'))
            old=time.time()-30
            self.assertIsNone(self.session(dict(at=old,updated=old-1,display_updated=old-1)).wait_ui('cockpit'))


if __name__=='__main__':unittest.main()
