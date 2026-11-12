"""Live work, quiet work, and collection delays must not be conflated."""
import unittest

from observatory.session_status import session_health


class SessionHealthTests(unittest.TestCase):
    now = 2_000_000_000_000_000_000

    def entry(self, seconds=120, **values):
        return {'run_state': 'running', 'last_record_ns': self.now - seconds * 1_000_000_000,
                'diagnostics': {'last_success_ns': self.now, 'backlog_bytes': 0}, **values}

    def process(self, **values):
        return {'pids': [123], 'observed_at_ns': self.now,
                'observed_since_ns': self.now - 900_000_000_000, 'last_progress_ns': 0, **values}

    def test_quiet_processing_does_not_become_no_updates_after_35_seconds(self):
        health = session_health(self.entry(), self.now)
        self.assertEqual(health['state'], 'Working (quiet)')
        self.assertTrue(health['active'])
        self.assertFalse(health['recent'])

    def test_long_tool_call_with_exact_process_work_is_still_processing(self):
        entry = self.entry(900, waiting_for='Tool response: build',
                           process=self.process(last_progress_ns=self.now - 1_000_000_000))
        health = session_health(entry, self.now)
        self.assertEqual(health['state'], 'Tool running')
        self.assertTrue(health['active'])
        self.assertIn('CPU / I/O', health['status_evidence'])

    def test_stall_is_a_warning_only_after_sustained_observation_without_progress(self):
        entry = self.entry(900, process=self.process())
        health = session_health(entry, self.now)
        self.assertEqual(health['state'], 'Possibly stalled')
        self.assertFalse(health['active'])
        self.assertIn('not proof', health['status_evidence'])
        entry['process'] = self.process(observed_since_ns=self.now)
        self.assertEqual(session_health(entry, self.now)['state'], 'Working (quiet)')

    def test_missing_or_stale_process_evidence_cannot_confirm_a_stall(self):
        for process in ({}, self.process(observed_at_ns=self.now - 60_000_000_000)):
            with self.subTest(process=process):
                health = session_health(self.entry(900, process=process), self.now)
                self.assertEqual(health['state'], 'Unconfirmed')
                self.assertFalse(health['active'])

    def test_collector_delay_or_backlog_is_not_an_agent_stall(self):
        entry = self.entry(900, process=self.process())
        entry['diagnostics']['backlog_bytes'] = 100
        self.assertEqual(session_health(entry, self.now)['state'], 'Catching up')
        entry['diagnostics'] = {'last_success_ns': self.now - 60_000_000_000}
        self.assertEqual(session_health(entry, self.now)['state'], 'Monitor delayed')

    def test_explicit_wait_and_completion_survive_silence(self):
        waiting = session_health(self.entry(900, run_state='waiting', waiting_for='Approval'), self.now)
        self.assertEqual(waiting['state'], 'Waiting')
        self.assertFalse(waiting['active'])
        completed = session_health(self.entry(1, run_state='completed', process=self.process()), self.now)
        self.assertEqual(completed['state'], 'Completed')
        self.assertFalse(completed['active'])

    def test_new_header_without_a_started_turn_is_not_counted_as_work(self):
        self.assertFalse(session_health(self.entry(1, run_state='unknown'), self.now)['active'])
