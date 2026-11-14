"""Remote native request inventory survives Activity paging and watcher restart."""
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from observatory.core import Store
from observatory.native_lifecycle import event_log,normalize_event,validate_binding
from tests.test_native_lifecycle import binding,event


class NativeInventoryTests(unittest.TestCase):
    def setUp(self):
        self.tmp=TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.path=Path(self.tmp.name)/'source.db';self.store=Store(self.path);self.addCleanup(self.store.close)
        self.configured=validate_binding({**binding(),'receiver_id':self.store.receiver_id})

    def native(self,sequence=1,kind='approval_requested',request='request'):
        value=normalize_event({**event(sequence,kind,request),'receiver_id':self.store.receiver_id},self.configured)
        value.update(request_kind='approval_requested',received_time_unix_nano=160000000000)
        return event_log(value)

    def noise(self,offset):
        self.store.ingest_logs([dict(trace_id='b'*32,span_id=None,time_unix_nano=200000000000+offset+index,
            service='fixture',scope_name='fixture',severity_number=9,severity_text='INFO',body='',event_name='agent.activity',
            attributes={'agent.activity.domain':'message','agent.activity.action':'assistant','agent.activity.subject':'Newer unrelated record '+str(index),
                'observatory.record_id':str(offset)+'-'+str(index)},resource_attributes={}) for index in range(210)])

    def test_pending_and_exact_resolution_survive_paging_and_store_restart(self):
        self.store.ingest_logs([self.native()]);self.noise(0)
        self.assertFalse(any('native_lifecycle' in row for row in self.store.activity_page(limit=200)['activity']))
        state=self.store.native_lifecycle_inventory()
        self.assertTrue(state['complete']);self.assertEqual(len(state['observations']),1)
        self.assertFalse(state['observations'][0]['resolved'])
        self.store.close();self.store=Store(self.path)
        self.assertEqual(self.store.native_lifecycle_inventory(),state)
        self.store.ingest_logs([self.native(2,'request_resolved')]);self.noise(1000)
        self.assertFalse(any('native_lifecycle' in row for row in self.store.activity_page(limit=200)['activity']))
        resolved=self.store.native_lifecycle_inventory()
        self.assertTrue(resolved['complete']);self.assertEqual(len(resolved['observations']),1)
        self.assertTrue(resolved['observations'][0]['resolved'])
        self.assertEqual(resolved['observations'][0]['request_id'],'request')
        self.assertNotIn('body',str(resolved));self.assertNotIn('Newer unrelated',str(resolved))

    def test_conflicting_actual_scope_or_sender_order_defers_native_delivery(self):
        self.store.ingest_logs([self.native()])
        conflicting=self.native(2,'request_resolved');conflicting['trace_id']='c'*32
        self.store.ingest_logs([conflicting])
        state=self.store.native_lifecycle_inventory()
        self.assertFalse(state['complete']);self.assertEqual(state['observations'],[])
        self.assertIn('conflicts',state['warning'])

    def test_foreign_receiver_metadata_never_enters_current_inventory(self):
        row=self.native();row['attributes']['observatory.native.receiver_id']='foreign'
        self.store.ingest_logs([row])
        state=self.store.native_lifecycle_inventory()
        self.assertTrue(state['complete']);self.assertEqual(state['observations'],[])

    def test_old_activity_writer_invalidates_until_transactional_repair(self):
        self.store.ingest_logs([self.native()])
        with self.store._connect() as db:
            db.execute("UPDATE activity_index_v1 SET payload=json_remove(payload,'$.native_lifecycle')")
        self.assertFalse(self.store.index_status()['ready'])
        self.assertFalse(self.store.native_lifecycle_inventory()['complete'])
        self.store._drain_external_activity()
        self.assertTrue(self.store.native_lifecycle_inventory()['complete'])
        self.assertEqual(len(self.store.native_lifecycle_inventory()['observations']),1)


if __name__=='__main__':unittest.main()
