import unittest

from observatory.ai_summary import summary_evidence


class SummaryScopeTests(unittest.TestCase):
    def test_busy_other_sessions_do_not_hide_selected_evidence(self):
        events = [dict(session_id='selected', time_unix_nano=i, subject=f'work {i}')
                  for i in range(40)]
        events += [dict(session_id='other', time_unix_nano=100+i, subject='unrelated')
                   for i in range(100)]
        result = summary_evidence({'session_id': 'selected'}, iter(events))
        self.assertEqual(len(result['events']), 30)
        self.assertEqual([e['subject'] for e in result['events']],
                         [f'work {i}' for i in range(39, 9, -1)])

    def test_service_without_session_identity_does_not_attribute_events(self):
        events = [dict(service='selected', time_unix_nano=1, subject='relevant')]
        events += [dict(service='other', time_unix_nano=100+i) for i in range(40)]
        result = summary_evidence({'service': 'selected'}, events)
        self.assertEqual(result['events'], [])

    def test_explicit_worker_never_summarizes_siblings_or_untagged_parent(self):
        session=dict(service='codex',session_id='shared',agent_id='worker')
        events=[dict(session,time_unix_nano=1,subject='Own work'),
                dict(session,time_unix_nano=2,agent_id='sibling',subject='Sibling'),
                dict(service='codex',session_id='shared',time_unix_nano=3,subject='Parent')]
        result=summary_evidence(session,events)
        self.assertEqual([e['subject'] for e in result['events']],['Own work'])

    def test_equal_timestamps_keep_input_order_without_comparing_dicts(self):
        events = [dict(session_id='selected', time_unix_nano=1, subject=str(i))
                  for i in range(40)]
        result = summary_evidence({'session_id': 'selected'}, events)
        self.assertEqual([e['subject'] for e in result['events']],
                         [str(i) for i in range(30)])
