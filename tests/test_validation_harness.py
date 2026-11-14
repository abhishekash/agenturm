"""Safety and evidence contracts for generated final-monitor harnesses."""
from copy import deepcopy
import json
from pathlib import Path
import sqlite3
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from scripts.validation_support import (bind_application,cleanup_clone,clone_seed,create_owned_directory,
    disk_gate,isolated_env,monitor_budgets,nearest_rank,published_heartbeat,raw_manifest,strict_heartbeat_errors)


class ValidationContracts(unittest.TestCase):
    def healthy(self):
        return dict(harness_version=2,receiver_id='rx',display_receiver_id='rx',indexing_ready=True,
            native_inventory_supported=True,native_inventory_warning='',initial_poll_complete=True,
            at=100,updated=99,display_updated=99,error='',read_errors={},sessions_error='',
            native_read_errors=0,native_schema_errors=0,opencode_read_errors=0,
            opencode_diagnostics={},pending_storage_records=0,source_read_error=False,
            source_backlog_bytes=0,source_skipped_records=0,source_omitted=0,
            source_hierarchy_write_errors=0,source_schema_errors=0,published_sources_clipped=False,warning='')

    def test_first_useful_and_budget_reject_unknown_partial_or_failed_reads(self):
        self.assertEqual(strict_heartbeat_errors(self.healthy(),'rx'),[])
        for change in (dict(harness_version=1),dict(receiver_id='other'),dict(display_receiver_id='other'),
            dict(indexing_ready=None),dict(native_inventory_supported=None),dict(native_inventory_warning='partial'),
            dict(initial_poll_complete=None),dict(error='timeout'),dict(read_errors={'activity':'failed'}),
            dict(sessions_error='failed'),dict(native_read_errors=1),dict(native_schema_errors=1),
            dict(opencode_read_errors=1),dict(opencode_diagnostics={'schema_errors':1}),
            dict(opencode_diagnostics={'backlog_records':1}),dict(opencode_diagnostics={'skipped_budget_records':1}),
            dict(pending_storage_records=1),dict(source_read_error=True),dict(source_backlog_bytes=1),
            dict(source_omitted=1),dict(source_skipped_records=1),dict(source_hierarchy_write_errors=1),
            dict(source_schema_errors=1),dict(published_sources_clipped=True),dict(warning='write failed'),
            dict(display_updated=90),dict(updated=None)):
            with self.subTest(change=change):
                row=self.healthy();row.update(change);self.assertTrue(strict_heartbeat_errors(row,'rx'))
        legacy=self.healthy();legacy.update(native_inventory_supported=False,native_inventory_warning=None)
        self.assertEqual(strict_heartbeat_errors(legacy,'rx'),[], 'Unsupported capability stays explicit, not fabricated native evidence')

    def test_heartbeat_observes_published_state_without_work_or_content(self):
        snapshot=SimpleNamespace(local_status=dict(initial_poll_complete=True,sessions=[],read_errors=0,schema_errors=0,
            opencode_read_errors=0,backlog_bytes=0,skipped_records=0,omitted_sources=0,hierarchy={'write_errors':0}),
            error='',receiver_id='rx',updated=99,read_errors={},sessions_error='',indexing_ready=True,
            sessions=[],mcp_summary={},native_lifecycle_warning='')
        def forbidden():raise AssertionError('Heartbeat prepared application sources')
        app=SimpleNamespace(feed=SimpleNamespace(snapshot=snapshot),data=snapshot,
            source_map={'source':dict(session_id='fixture',state='Working',task='PRIVATE BODY')},
            workflow_items=forbidden,workflow_ledger=SimpleNamespace(rows=[]),workflow_controller=SimpleNamespace(warning=''))
        value=published_heartbeat(app,at=100)
        self.assertNotIn('PRIVATE BODY',json.dumps(value));self.assertEqual(value['states'][0]['session'],'fixture')
        self.assertEqual(strict_heartbeat_errors(value,'rx'),[])

    def test_environment_drops_personal_roots_and_provider_secrets(self):
        with patch.dict('os.environ',dict(HOME='/private/home',CODEX_HOME='/private/codex',ANTHROPIC_API_KEY='secret',
            OPENAI_API_KEY='secret',CLAUDE_CONFIG_DIR='/private/config'),clear=False):
            env=isolated_env('/tmp/generated','/tmp/application')
        for name in ('HOME','CODEX_HOME','OPENAI_API_KEY','ANTHROPIC_API_KEY','CLAUDE_CONFIG_DIR'):self.assertNotIn(name,env)
        self.assertEqual(env['OPENCODE_DB'],'/tmp/generated/opencode.db')
        self.assertEqual(env['XDG_STATE_HOME'],'/tmp/generated/state')

    def test_import_root_cannot_relabel_already_imported_application(self):
        loaded=SimpleNamespace(__file__='/tmp/other/observatory/__init__.py')
        with patch.dict(sys.modules,{'observatory':loaded}):
            with self.assertRaisesRegex(RuntimeError,'different application'):bind_application('/tmp/intended')

    def test_disk_gate_fails_before_work(self):
        with patch('scripts.validation_support.shutil.disk_usage',return_value=SimpleNamespace(free=99)):
            with self.assertRaisesRegex(RuntimeError,'headroom'):disk_gate('/tmp/new-output',required_bytes=50,reserve_bytes=50)
        for value in (-1,True,1.5):
            with self.assertRaises(ValueError):disk_gate('/tmp',reserve_bytes=value)

    def test_cpu_identity_and_simultaneous_monitor_rss(self):
        samples=[]
        for moment,values in enumerate(((90,10),(10,90))):
            samples.append({role:dict(pid=i+1,start_ticks=100+i,sampled_at=moment,cpu_seconds=moment*.01,rss_mib=values[i])
                for i,role in enumerate(('receiver','cockpit'))})
        budgets,whole=monitor_budgets(samples)
        self.assertEqual(whole['peak_sampled_rss_mib'],100)
        self.assertAlmostEqual(whole['cpu_one_core_percent'],2)
        for key in ('pid','start_ticks'):
            broken=deepcopy(samples);broken[-1]['receiver'][key]+=1
            with self.assertRaisesRegex(ValueError,'identity'):monitor_budgets(broken)
        for change in (dict(start_ticks=None),dict(cpu_seconds=float('nan')),dict(rss_mib=-1)):
            broken=deepcopy(samples);broken[-1]['receiver'].update(change)
            with self.assertRaises(ValueError):monitor_budgets(broken)
        self.assertEqual(nearest_rank([1,2,3,4],.95),4)
        self.assertIsNone(nearest_rank([],.95))

    def test_clone_preserves_seed_and_cleanup_is_narrow_and_preflighted(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);seed=root/'seed.db'
            with sqlite3.connect(seed) as db:
                db.executescript('CREATE TABLE spans(trace_id TEXT,span_id TEXT,payload TEXT);'
                    'CREATE TABLE logs(digest TEXT,payload TEXT);CREATE TABLE trace_activity(trace_id TEXT,payload TEXT);')
                db.execute('INSERT INTO spans VALUES(?,?,?)',('trace','span','raw'))
                db.execute('INSERT INTO logs VALUES(?,?)',('digest','raw'))
                db.execute('INSERT INTO trace_activity VALUES(?,?)',('trace','raw'))
            before=raw_manifest(seed);base=root/'owned';create_owned_directory(base)
            clone=clone_seed(seed,base/'working.db',reserve_bytes=0)
            self.assertEqual(clone['raw_sha256'],before['raw_sha256']);self.assertEqual(raw_manifest(seed),before)
            with self.assertRaises(ValueError):cleanup_clone(base,'working.db')
            with self.assertRaises(ValueError):cleanup_clone(base,'working.db',children_stopped=True)
            (base/'results.json').write_text('{"raw_results":"preserved"}')
            for name in ('../seed.db','seed.db','results.json'):
                with self.assertRaises(ValueError):cleanup_clone(base,name,children_stopped=True)
            sidecar=base/'working.db-wal';sidecar.symlink_to(seed)
            with self.assertRaisesRegex(ValueError,'symlink'):cleanup_clone(base,'working.db',children_stopped=True)
            self.assertTrue((base/'working.db').exists(),'Preflight refusal must not partially delete the clone')
            sidecar.unlink()
            removed=cleanup_clone(base,'working.db',children_stopped=True)
            self.assertEqual([row['path'] for row in removed],['working.db'])
            self.assertEqual(raw_manifest(seed),before);self.assertTrue((base/'results.json').exists())
            self.assertEqual(json.loads((base/'ownership.json').read_text())['clones']['working.db']['removed'],removed)


if __name__=='__main__':unittest.main()
