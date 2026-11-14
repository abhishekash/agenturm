"""Exercise the real functional run loop without starting a retained workload.

The AST boundary avoids mocking the entire process startup/shutdown stack. The
compiled loop is the production code, including reconstruction checks, API reads,
process observations and phase timing; no scheduling predicate is copied here.
"""
import ast
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

from scripts import soak_monitor


class Clock:
    def __init__(self):self.now=0
    def monotonic(self):return self.now
    def time(self):return 1_700_000_000+self.now
    def sleep(self,seconds):self.now+=seconds


def production_loop():
    path=Path(soak_monitor.__file__)
    module=ast.parse(path.read_text(),filename=str(path))
    run=next(node for node in module.body if isinstance(node,ast.FunctionDef)and node.name=='run')
    loops=[node for node in ast.walk(run)if isinstance(node,ast.While)
        and 'last_phase % 6 != 5' in ast.unparse(node.test)]
    if len(loops)!=1:raise AssertionError('Expected exactly one functional workload loop')
    return compile(ast.fix_missing_locations(ast.Module(body=loops,type_ignores=[])),str(path),'exec')


class SoakTailObservationTests(unittest.TestCase):
    def fixture(self,base,*,last_phase=16,next_phase=0,next_sample=100,duration=1,fail=None,mismatch=False):
        clock=Clock();calls=[];published=[];phases=[]
        (base/'telemetry.db').write_bytes(b'generated-test-placeholder')
        rows=[dict(session_id='soak-'+name,agent_id=name,trace_id='trace-'+name)
            for name in ('parent','child','opencode')]
        health=dict(receiver_id='generated-receiver',indexing=dict(ready=True),
            local_agents=dict(sessions=rows,initial_poll_complete=True))
        def get(path):
            if path.startswith('/api/review-turn?'):return dict(turn=dict(review_owner=dict(agent_id='opencode')))
            return health
        def request(api,path):
            calls.append(path)
            if path==fail:raise RuntimeError('generated sample failure')
            return get(path)
        def projection(api,*,service,agent_id,session_id,trace_id,**kwargs):
            name=session_id.removeprefix('soak-')
            if fail=='projection':raise RuntimeError('generated reconstruction failure')
            return dict(scope=dict(agent_id=agent_id),checks=[{}],
                edits=[dict(path=f'src/{name}_2.py')],
                usage=dict(values=dict(input_tokens=30 if service=='opencode'else 102)),
                request=dict(text=f'Validate OpenCode cycle 2'if service=='opencode'else f'Validate {name} cycle 2'),
                agent_update=dict(text='Validated OpenCode cycle 2'if not mismatch else'wrong'))
        def sample(pid):
            return dict(pid=pid,start_ticks=pid+1000,sampled_at=clock.now,cpu_seconds=0,
                rss_mib=1,peak_rss_mib=1,open_fds=1,threads=1)
        def append(path,row):
            published.append((path.name,json.loads(json.dumps(row))))
        env=dict(time=clock,args=SimpleNamespace(duration=duration,phase_seconds=2,minimum_free_mib=4096),
            started=0,next_phase=next_phase,last_phase=last_phase,next_sample=next_sample,
            sample_interval=2,base=base,workload=SimpleNamespace(phase=lambda cycle,phase:phases.append((cycle,phase))),
            errors=[],pending_cycle=None,keys=lambda *args:None,screen=lambda label:'Shift+R',
            collect_review=projection,api=SimpleNamespace(get=get),ExecutionScope=lambda **kw:SimpleNamespace(**kw),
            classify=lambda row,scope,*args,**kw:[SimpleNamespace(id=scope.agent_id,kind='completion')],
            expected_reviews=set(),verified=[],identity=lambda *args:'generated-trace',disk_gate=lambda *args,**kw:None,
            request=request,index_proof=lambda *args:dict(receiver_matches=True,health_matches=True,
                derived_marker_complete=True,file_unchanged=True),registered_receiver='generated-receiver',
            receiver=SimpleNamespace(pid=220),actor=SimpleNamespace(pid=330),ui_pid=lambda:440,
            os=SimpleNamespace(getpid=lambda:550),process_sample=sample,previous={},samples=[],append=append,
            receiver_restart=True,mid_restart=True,reconnects=[],rendered_approvals=set(),approval_latencies=[])
        return env,calls,published,phases,clock

    def test_completed_final_phase_is_observed_after_work_even_before_sample_due(self):
        with tempfile.TemporaryDirectory()as directory:
            env,calls,published,phases,clock=self.fixture(Path(directory))
            exec(production_loop(),env)
            self.assertEqual(phases,[(2,5)])
            self.assertEqual(env['verified'],['parent:2','child:2','opencode:2'])
            self.assertEqual(env['expected_reviews'],{'parent','child','opencode'})
            self.assertEqual(env['errors'],[])
            self.assertEqual(calls,['/api/health','/api/sessions','/api/activity?limit=50','/api/metrics','/api/mcp?limit=20'])
            self.assertEqual(len(env['samples']),1)
            row=env['samples'][0]
            self.assertEqual((row['cycle'],row['phase']),(2,5))
            self.assertGreater(row['elapsed'],env['args'].duration)
            self.assertEqual(row['elapsed'],1.6)
            self.assertEqual(row['index_observed_elapsed'],row['elapsed'])
            self.assertTrue(row['data_reads_complete']);self.assertTrue(all(row['index_proof'].values()))
            self.assertEqual({role:(p['pid'],p['start_ticks'])for role,p in row['processes'].items()},
                dict(receiver=(220,1220),cockpit=(440,1440),synthetic_daemon=(330,1330),driver=(550,1550)))
            self.assertEqual(published,[('samples.jsonl',row)])
            self.assertEqual(env['next_sample'],3.6)
            self.assertEqual(env['next_phase'],3.6) # phase cadence remains completion +2s

    def test_nonphase_iterations_remain_interval_gated(self):
        for due,expected in ((100,0),(0,1)):
            with self.subTest(due=due),tempfile.TemporaryDirectory()as directory:
                env,calls,published,phases,clock=self.fixture(Path(directory),last_phase=17,
                    next_phase=100,next_sample=due,duration=.4)
                exec(production_loop(),env)
                self.assertEqual(phases,[])
                self.assertEqual(len(env['samples']),expected)
                self.assertEqual(len(calls),expected*5)

    def test_reconstruction_or_tail_api_failure_is_not_a_completed_observation(self):
        for failure in ('projection','/api/mcp?limit=20'):
            with self.subTest(failure=failure),tempfile.TemporaryDirectory()as directory:
                env,calls,published,phases,clock=self.fixture(Path(directory),fail=failure)
                with self.assertRaisesRegex(RuntimeError,'generated .* failure'):
                    exec(production_loop(),env)
                self.assertEqual(env['samples'],[]);self.assertEqual(published,[])
                self.assertEqual(phases,[(2,5)])
                if failure=='projection':self.assertEqual(calls,[])
                else:self.assertEqual(calls[-1],'/api/mcp?limit=20')

    def test_completed_phase_does_not_erase_reconstruction_mismatch(self):
        with tempfile.TemporaryDirectory()as directory:
            env,calls,published,phases,clock=self.fixture(Path(directory),mismatch=True)
            exec(production_loop(),env)
            self.assertEqual(env['errors'],['OpenCode reconstruction mismatch: cycle 2'])
            self.assertEqual(env['verified'],['parent:2','child:2'])
            self.assertEqual(len(env['samples']),1)


if __name__=='__main__':unittest.main()
