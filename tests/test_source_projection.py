"""Headless and terminal consumers share one pure source/health projection."""
import json
import subprocess
import sys
from types import SimpleNamespace
import unittest

from observatory.source_projection import attention_items,sources
from observatory.agent_hierarchy import canonical_agent_id


class SourceProjectionTests(unittest.TestCase):
    def test_headless_import_does_not_load_textual(self):
        code="from observatory.source_projection import attention_items; import sys; assert not any(n=='textual' or n.startswith('textual.') for n in sys.modules)"
        result=subprocess.run([sys.executable,'-c',code],capture_output=True,text=True,timeout=10)
        self.assertEqual(result.returncode,0,result.stderr)

    def test_attention_and_roster_share_exact_identity_health_and_capture_policy(self):
        agent=canonical_agent_id('Codex','fixture','session')
        item=dict(source_id='source',session_id='session',agent_id=agent,runtime='Codex',service='codex',
            run_state='waiting',waiting_for='Approval request',initial_task='Fix retries',
            task='Later incidental request',last_record_ns=100,capture_content=True)
        snapshot=SimpleNamespace(receiver_id='receiver',agents=[],sessions=[],activity=[],local_status={'sessions':[item]})
        rows=attention_items(snapshot)
        self.assertEqual(rows,sources(snapshot));self.assertEqual(rows[0]['task'],'Fix retries')
        self.assertTrue(rows[0]['attention']);self.assertEqual(rows[0]['agent_id'],agent)
        item['capture_content']=False
        row=attention_items(snapshot)[0]
        self.assertFalse(row['capture_content']);self.assertEqual(row['task'],'')
        self.assertTrue(row['attention'])

    def test_feed_receiver_clock_and_readiness_are_explicit_and_typed(self):
        from tests.test_terminal_search import ManualFeed
        feed=ManualFeed();original=feed.api.get
        for clock in (123456789,True,'123456789',None,-1,2**63):
            feed.api.get=lambda path:dict(ok=True,receiver_id='receiver',observed_time_unix_nano=clock,
                indexing={'ready':True}) if path.startswith('/api/health') else original(path)
            feed.refresh()
            self.assertEqual(feed.snapshot.receiver_time_ns,clock if type(clock) is int and 0<clock<2**63 else 0)
            self.assertTrue(feed.snapshot.indexing_ready)
        feed.api.get=lambda path:dict(ok=True,receiver_id='receiver',indexing={'ready':'true'}) if path.startswith('/api/health') else original(path)
        feed.refresh();self.assertFalse(feed.snapshot.indexing_ready)


if __name__=='__main__':unittest.main()
