"""Overview summarizes exact loaded evidence, not assistant claims or other agents."""
from types import SimpleNamespace
import unittest
from observatory.overview import overview_facts,overview_summary,EVENT_LIMIT


def event(identity,**changes):
    row=dict(source_id=identity,receiver_id='receiver',trace_id='a'*32,service='sdk',agent_id='worker',
        session_id='session',time_unix_nano=10,domain='file',action='update',subject='retry.py',result='success')
    row.update(changes);return row


class OverviewFactsTests(unittest.TestCase):
    def setUp(self):
        self.item=dict(service='sdk',agent_id='worker',session_id='session',trace_id='a'*32,capture_content=True)
        self.snapshot=SimpleNamespace(receiver_id='receiver')
    def read(self,rows,**values):return overview_facts(self.item,self.snapshot,rows,**values)

    def test_counts_and_paths_are_source_backed_with_explicit_unknown_check_results(self):
        rows=[event('file'),event('check',domain='shell',action='execute',subject='pytest tests',
            attributes={'process.command_line':'pytest tests','process.exit.code':0}),
            event('label',domain='shell',action='execute',subject='pytest label-only'),
            event('assistant',domain='message',action='assistant',subject='Changed 100 files and all tests passed')]
        facts=self.read(rows,loaded=True)
        self.assertEqual(facts['edit_count'],1);self.assertEqual(facts['paths'],['retry.py'])
        self.assertEqual(facts['check_results'],dict(passed=1,failed=0,unknown=1))
        self.assertIn('1 file operation',overview_summary(facts))
        self.assertNotIn('100',overview_summary(facts))
        self.assertIn('whole-run',facts['coverage'])

    def test_foreign_owner_namespace_and_execution_never_enter_counts(self):
        rows=[event('own')]+[event(key,**{key:'foreign'}) for key in ('receiver_id','service','agent_id','session_id','trace_id')]
        self.assertEqual(self.read(rows)['edit_count'],1)
        self.item['capture_content']=False
        self.assertEqual(self.read(rows)['source_count'],0)

    def test_duplicate_latest_source_and_bounded_window_are_explicit(self):
        old=event('same',subject='old.py',time_unix_nano=1)
        fresh=event('same',subject='fresh.py',time_unix_nano=2)
        self.assertEqual(self.read([fresh,old])['paths'],['fresh.py'])
        self.assertEqual(self.read([old,fresh])['paths'],['fresh.py'])
        facts=self.read([event(str(i),subject=str(i)+'.py',time_unix_nano=i+1) for i in range(EVENT_LIMIT+5)],loaded=True)
        self.assertEqual(facts['source_count'],EVENT_LIMIT);self.assertTrue(facts['partial'])

    def test_recorded_turn_joins_steps_only_under_proven_owner(self):
        usage=dict(turn_id='b'*32,scope_owner=dict(producer='sdk',agent_id='worker',session_id='session'))
        rows=[event('one',turn_id='b'*32),event('two',trace_id='c'*32,turn_id='b'*32,subject='other.py'),
            event('other-turn',turn_id='d'*32)]
        facts=self.read(rows,usage=usage)
        self.assertEqual(facts['scope'],'turn');self.assertEqual(facts['edit_count'],2)
        usage['scope_owner']['agent_id']='foreign'
        self.assertEqual(self.read(rows,usage=usage)['edit_count'],2) # Exact run only: both run-a records remain.

    def test_partial_owner_and_unbounded_candidates_never_create_turn_or_unbounded_read(self):
        from itertools import repeat
        usage=dict(turn_id='b'*32,scope_owner={})
        item=dict(service='sdk',session_id='session')
        facts=overview_facts(item,self.snapshot,[event('other',trace_id='c'*32,turn_id='b'*32)],usage=usage)
        self.assertNotEqual(facts['scope'],'turn')
        facts=self.read(repeat(event('same')),loaded=True)
        self.assertEqual(facts['source_count'],1);self.assertTrue(facts['partial'])


if __name__=='__main__':unittest.main()
