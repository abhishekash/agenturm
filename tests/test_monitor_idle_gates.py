"""Idle-budget evidence must contain a healthy live monitor, not an empty shell."""
from copy import deepcopy
import unittest
from scripts.benchmark_monitor_idle import validate_idle


class IdleMeasurementGates(unittest.TestCase):
    def rows(self):
        return [dict(kind='heartbeat',at=i,pid=7,receiver_id='receiver',updated=i-.2,
            error='',read_errors={},sessions_error='',initial_poll_complete=True,
            native_read_errors=0,native_schema_errors=0,session_count=40,mcp_calls=50,
            native_sessions=['soak-parent','soak-child','soak-stale','soak-opencode']) for i in range(1,10)]

    def test_fresh_data_bearing_interval_is_required(self):
        rows=self.rows()
        self.assertEqual(validate_idle(rows,0,10,7,'receiver',True),[])
        for change in (dict(pid=8),dict(receiver_id='other'),dict(updated=0),dict(error='timeout'),
                       dict(read_errors={'sessions':'timeout'}),dict(sessions_error='failed'),
                       dict(initial_poll_complete=False),dict(native_read_errors=1),
                       dict(native_schema_errors=1),dict(native_sessions=[]),dict(session_count=0),dict(mcp_calls=0)):
            broken=deepcopy(rows);broken[-1].update(change)
            with self.subTest(change=change):self.assertTrue(validate_idle(broken,0,10,7,'receiver',True))
        self.assertTrue(validate_idle(rows[:2],0,10,7,'receiver',True))
        self.assertTrue(validate_idle([],0,10,7,'receiver',True))
