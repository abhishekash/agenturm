"""Offline provenance integrity gates; never invokes a producer or reads user state."""
import hashlib
import json
from pathlib import Path
import re
import unittest

ROOT=Path(__file__).resolve().parents[1]
MANIFEST=ROOT/'tests/fixtures/native_review/provenance.json'


class NativeFixtureProvenanceTests(unittest.TestCase):
    def setUp(self):self.manifest=json.loads(MANIFEST.read_text())

    def test_fixture_and_builder_hashes_bind_the_documented_contract(self):
        for row in self.manifest['fixtures']+self.manifest['legacy_builders']:
            with self.subTest(path=row['path']):
                path=(ROOT/row['path']).resolve()
                self.assertTrue(path.is_relative_to(ROOT/'tests'))
                self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(),row['sha256'])
                self.assertFalse(row['captured_from_live_agent'])
                self.assertIsNone(row['observed_producer_version'])
        for path,sha in self.manifest['reader']['files'].items():
            self.assertEqual(hashlib.sha256((ROOT/path).read_bytes()).hexdigest(),sha)

    def test_only_versioned_primary_serialization_sources_support_current_targets(self):
        sources={row['id']:row for row in self.manifest['primary_sources']}
        for row in sources.values():
            self.assertRegex(row['commit'],r'^[0-9a-f]{40}$')
            self.assertRegex(row['sha256'],r'^[0-9a-f]{64}$')
            self.assertIn(row['repository'],('openai/codex','anomalyco/opencode'))
            self.assertEqual(row['locator'],f"https://raw.githubusercontent.com/{row['repository']}/{row['commit']}/{row['path']}")
        for row in self.manifest['fixtures']:
            target=row['schema_target']
            if target is None:
                self.assertEqual(row['source_ids'],[])
                continue
            self.assertEqual(row['kind'],'synthetic-schema-derived')
            self.assertTrue(row['source_ids'])
            for key in row['source_ids']:self.assertEqual(sources[key]['commit'],target['commit'])
            self.assertFalse(row['upstream_full_decoder_executed'])
        self.assertFalse(self.manifest['codex_rpc_schema']['rollout_proof'])
        self.assertIsNone(self.manifest['local_verification']['ci_run'])

    def test_current_fixture_contains_selected_producer_serialized_shapes(self):
        codex=[json.loads(line)for line in (MANIFEST.parent/'codex-0.167.1.synthetic.jsonl').read_text().splitlines()]
        meta=codex[0]['payload'];self.assertEqual(meta['cli_version'],'0.167.1')
        self.assertTrue(all({'type','timestamp','payload','ordinal'}<=row.keys()for row in codex))
        items=[row['payload']['item']for row in codex if row.get('payload',{}).get('type')=='item_completed']
        reply=next(row for row in items if row['type']=='AgentMessage')
        self.assertEqual(reply['content'][0]['type'],'Text')
        change=next(row for row in items if row['type']=='FileChange')['changes']['src/evidence.py']
        self.assertIn('unified_diff',change);self.assertNotIn('diff',change)
        current=json.loads((MANIFEST.parent/'opencode-2.2.5.synthetic.json').read_text())
        messages=current['rows']['session_message'];parts=current['rows']['session_part']
        self.assertTrue(any(row['type']=='idle'for row in messages))
        self.assertTrue(all('parentID'not in row['data']for row in messages if row['type']=='assistant'))
        self.assertTrue(all(row['data']['content']==[]for row in messages if row['type']=='assistant'))
        self.assertTrue(all(type(row['ordinal'])is int for row in parts))
        self.assertEqual({row['data']['name']for row in parts if row['type']=='tool'},{'shell','edit'})
        for row in parts:
            if row['type']=='tool':self.assertIn('content',row['data']['state'])
