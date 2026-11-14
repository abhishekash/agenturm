"""Native envelopes survive real source projection without claimed ownership."""
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from observatory.core import Store
from observatory.native_lifecycle import activity_observations,event_log,normalize_event,validate_binding
from tests.test_native_lifecycle import binding,event


class NativeActivityIntegrationTests(unittest.TestCase):
    def test_store_activity_preserves_only_typed_metadata_and_actual_scope(self):
        with TemporaryDirectory() as directory:
            store=Store(Path(directory)/'source.db')
            try:
                configured=validate_binding({**binding(),'receiver_id':store.receiver_id})
                value=normalize_event({**event(),'receiver_id':store.receiver_id},configured)
                value.update(request_kind='approval_requested',received_time_unix_nano=160000000000)
                row=event_log(value);store.ingest_logs([row])
                page=store.activity_page(limit=200)
                observed=activity_observations(page['activity'],store.receiver_id)
                self.assertEqual(len(observed),1)
                self.assertEqual(observed[0]['agent_id'],configured['agent_id'])
                self.assertEqual(observed[0]['trace_id'],configured['trace_id'])
                self.assertNotIn('agent_id',page['activity'][0]['native_lifecycle'])
                self.assertEqual(page['activity'][0]['detail'],'')
                self.assertEqual(activity_observations(page['activity'],'different-receiver'),[])
                row['attributes']['observatory.native.sequence']=True
                row['attributes']['observatory.record_id']='malformed-event'
                store.ingest_logs([row])
                records=store.activity_page(limit=200)['activity']
                malformed=next(item for item in records if 'native_lifecycle' not in item)
                self.assertEqual(malformed['agent_id'],configured['agent_id'])
            finally:store.close()

    def test_attention_entrypoint_does_not_initialize_a_receiver_database(self):
        from unittest.mock import patch
        from observatory.__main__ import main
        with TemporaryDirectory() as directory:
            state=Path(directory)/'workflow.db'
            with patch('sys.argv',['agenturm','attention','status','--state',str(state)]),\
                 patch('observatory.__main__.Store',side_effect=AssertionError('telemetry database')),\
                 patch('builtins.print'):
                main()


if __name__=='__main__':unittest.main()
