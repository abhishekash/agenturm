"""Actual derived storage wires complete-source usage and repairs older patterns."""
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from observatory.core import Store,normalize_native
from tests.test_reported_usage_progress import operation,model,ATTRS


class ExplanationCacheUpgradeTests(unittest.TestCase):
    def test_complete_store_evidence_supplies_qualified_reported_usage_context(self):
        with TemporaryDirectory() as directory:
            store=Store(Path(directory)/'source.db',workflow_patterns=True);self.addCleanup(store.close)
            spans=[]
            for identity,stamp in ((1,1),(2,5),(3,9)):
                event=operation(identity,stamp=stamp*10**9)
                spans.append(dict(trace_id=event['trace_id'],span_id=event['span_id'],name='build',status='ERROR',
                    start_unix_nano=event['time_unix_nano'],end_unix_nano=event['time_unix_nano'],
                    attributes={**ATTRS,**event['attributes']}))
            spans.extend([model(101,2,3,inputs=10,outputs=2),model(102,6,7,inputs=20,outputs=3)])
            store.ingest([normalize_native(row,'fixture') for row in spans])
            finding=next(row for row in store.findings() if row.get('workflow_pattern') and row['kind']=='repeated_tool_error')
            context=finding['reported_usage_context']
            self.assertTrue(context['available']);self.assertEqual((context['input_tokens'],context['output_tokens']),(30,5))
            self.assertFalse(finding['attention'])
            self.assertTrue({f'{value:016x}' for value in (101,102)}<={row['id'] for row in finding['evidence']})
            self.assertEqual(len(store.finding_source(finding['id'])['sources']),5)

    def test_first_optin_upgrade_replaces_legacy_cached_pattern_before_readiness(self):
        with TemporaryDirectory() as directory:
            path=Path(directory)/'source.db';store=Store(path,workflow_patterns=True)
            spans=[]
            for identity,passed in ((1,False),(2,True)):
                event=operation(identity,success=passed)
                spans.append(normalize_native(dict(trace_id=event['trace_id'],span_id=event['span_id'],name='build',
                    start_unix_nano=event['time_unix_nano'],end_unix_nano=event['time_unix_nano']+1,
                    status='OK' if passed else 'ERROR',attributes={**ATTRS,**event['attributes']}),'fixture'))
            store.ingest(spans)
            with store._connect() as db:
                db.execute("DELETE FROM schema_meta WHERE key='workflow_patterns_v2'")
                rows=db.execute('SELECT finding_id,finding_json FROM finding_cache').fetchall()
                for row in rows:
                    value=json.loads(row['finding_json'])
                    if value.get('workflow_pattern'):
                        value['title']='LEGACY UNQUALIFIED PATTERN'
                        db.execute('UPDATE finding_cache SET finding_json=? WHERE finding_id=?',(json.dumps(value),row['finding_id']))
            store.close();store=Store(path,workflow_patterns=True);self.addCleanup(store.close)
            self.assertTrue(store.index_status()['ready'])
            findings=store.findings()
            self.assertNotIn('LEGACY UNQUALIFIED PATTERN',json.dumps(findings))
            self.assertTrue(any(row['kind']=='later_matching_success' for row in findings))
            with store._read() as db:
                self.assertTrue(db.execute("SELECT 1 FROM schema_meta WHERE key='workflow_patterns_v2'").fetchone())
                self.assertIsNone(db.execute('SELECT 1 FROM finding_dirty_v1 LIMIT 1').fetchone())


if __name__=='__main__':unittest.main()
