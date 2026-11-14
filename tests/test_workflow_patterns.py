import unittest
from observatory.workflow_patterns import analyze_patterns


def span(n,*,tool='build',status='ERROR',agent='worker',args='{"file":"a"}',seconds=None):
    return dict(trace_id='a'*32,receiver_id='fixture-receiver',span_id=str(n),name=tool,service='codex',status=status,
        start_unix_nano=((n if seconds is None else seconds)+1)*1_000_000_000,
        attributes={'agent.id':agent,'session.id':'session','tool.name':tool,'tool.arguments':args,'tool.error':'Connection timed out' if status=='ERROR' else ''})

class WorkflowPatternTests(unittest.TestCase):
    def test_malformed_scope_and_tool_metadata_never_group_or_crash(self):
        for field, invalid in (('agent.id', {}), ('agent.id', ['worker']),
                               ('session.id', {}), ('session.id', ['session'])):
            rows = [span(n) for n in range(3)]
            for row in rows:
                row['attributes'].pop('agent.id', None)
                row['attributes'][field] = invalid
            self.assertEqual(analyze_patterns(rows, []), [], (field, invalid))
        for producer in (None, {}, ['codex'], 42):
            rows = [span(n) for n in range(3)]
            for row in rows: row['service'] = producer
            self.assertEqual(analyze_patterns(rows, []), [], producer)
        for tool in ({'name':'build'}, ['build'], 42):
            rows = [span(n) for n in range(3)]
            for row in rows: row['attributes']['tool.name'] = tool
            self.assertEqual(analyze_patterns(rows, []), [], tool)

    def test_shell_wrappers_do_not_turn_known_polling_into_equivalent_calls(self):
        for tool, arguments in (('exec_command', {'cmd':'sleep 1'}),
                                ('shell', {'command':'tail -f build.log'}),
                                ('run', {'cmd':'watch -n 1 ps -ef'})):
            rows = [span(n, status='OK', tool=tool, args=arguments) for n in range(5)]
            self.assertEqual(analyze_patterns(rows, []), [], (tool, arguments))
        ordinary = [span(n, status='OK', tool='exec_command', args={'cmd':'python generate_artifacts.py'}) for n in range(5)]
        self.assertEqual([finding['kind'] for finding in analyze_patterns(ordinary, [])], ['repeated_equivalent_call'])

    def test_matching_errors_are_windowed_exact_and_safe(self):
        rows=[span(n) for n in range(3)]
        f=analyze_patterns(rows,[])
        self.assertEqual([x['kind'] for x in f],['repeated_tool_error'])
        self.assertNotIn('verysecret',str(f))
        self.assertEqual(len(f[0]['evidence']),3)
        self.assertEqual(analyze_patterns([span(0),span(1,agent='sibling'),span(2)],[]),[])
        self.assertEqual(analyze_patterns([span(0),span(1),span(2,seconds=601)],[]),[])

    def test_equivalent_calls_exclude_polling_and_unknown_arguments(self):
        rows=[span(n,status='OK',args='{"a":1,"b":2}' if n%2 else '{"b":2,"a":1}') for n in range(5)]
        self.assertEqual([x['kind'] for x in analyze_patterns(rows,[])],['repeated_equivalent_call'])
        self.assertEqual(analyze_patterns([span(n,status='OK',tool='poll_status') for n in range(8)],[]),[])
        self.assertEqual(analyze_patterns([span(n,status='OK',args='x'*17000) for n in range(5)],[]),[])
        self.assertEqual(analyze_patterns([span(n,status='OK',args='{"password":"value'+str(n)+'"}') for n in range(5)],[]),[])
        self.assertEqual(analyze_patterns([span(n,status='OK',args='{"password":"[REDACTED]"}') for n in range(5)],[]),[])
        missing=[span(n) for n in range(3)]
        for r in missing:r['attributes'].pop('tool.error')
        self.assertEqual(analyze_patterns(missing,[]),[])

    def test_compaction_and_completion_do_not_claim_correctness(self):
        rows=[span(n,status='OK') for n in range(3)]
        for row in rows:row['name']='context.compaction';row['attributes'].pop('tool.name')
        f=analyze_patterns(rows,[])
        self.assertEqual(f[0]['kind'],'repeated_compaction')
        fail=span(4);done=span(6,status='OK');done['name']='agent.run';done['attributes']={'agent.id':'worker','session.id':'session','agent.stopped_reason':'completed'}
        self.assertIn('completion_unresolved_error',[x['kind'] for x in analyze_patterns([fail,done],[])])
        self.assertNotIn('completion_unresolved_error',[x['kind'] for x in analyze_patterns([fail,span(5,status='OK'),done],[])])
        self.assertIn('completion_unresolved_error',[x['kind'] for x in analyze_patterns([fail,span(5,status='OK',args='different'),done],[])])

    def test_unknown_identity_is_not_grouped_and_work_is_bounded(self):
        rows=[span(n,status='OK',tool='wait') for n in range(600)]
        rows.extend([span(n+700) for n in range(3)])
        f=analyze_patterns(rows,[])
        self.assertTrue(all(x['analysis_partial'] for x in f))
        for r in rows:r['attributes'].pop('agent.id',None)
        self.assertEqual(analyze_patterns(rows,[]),[])

    def test_completion_patterns_require_errors_before_exact_outcome(self):
        done=span(3,status='OK');done['name']='agent.run';done['attributes']={'agent.id':'worker','session.id':'session','agent.run.state':'completed'}
        kind=lambda rows:[f['kind'] for f in analyze_patterns(rows,[])]
        self.assertNotIn('completion_unresolved_error',kind([done,span(4)]))
        self.assertIn('completion_unresolved_error',kind([span(1),done]))
        self.assertNotIn('completion_unresolved_error',kind([span(1),done,span(4,status='OK')]))

    def test_malformed_session_does_not_widen_an_explicit_agent_scope(self):
        rows=[span(n) for n in range(3)]
        for row in rows:row['attributes']['session.id']={}
        self.assertEqual(analyze_patterns(rows,[]),[])
