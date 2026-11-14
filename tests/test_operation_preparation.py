"""Trace-local sanitized fact reuse never borrows identity or compact authority."""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from observatory import findings,operation_facts,storage_derived
from observatory.error_reminders import trace_error_references
from observatory.operation_preparation import OperationPreparation,MAX_BYTES,MAX_EVENTS
from tests.test_operation_facts import event
from tests.test_derived_preparation import seed,projection,FIXTURE


def activities():
    rows=[]
    for number,changes in enumerate(({}, {'agent_id':'foreign-worker','session_id':'foreign-session'},
        {'operation_evidence':None,'detail':'{"password":"PRIVATE_CONTEXT_SECRET"}'},
        {'operation_evidence':dict(version=1,inputs=[('input',{'password':'PRIVATE_CONTEXT_SECRET'})],
             commands=[],exits=[],outputs=[('output','output','x'*9000)],output_clipped=True)},
        {'operation_reference':True,'operation_version':1,'operation_result':'passed',
         'operation_fingerprint':'a'*64,'error_fingerprint':'b'*64})):
        rows.append(event(id=str(number),source_id=str(number),source='log',**changes))
    return rows


class OperationPreparationTests(unittest.TestCase):
    def test_findings_error_refs_exact_parity_with_fewer_full_calculations(self):
        activity=activities();logs=[dict(trace_id='trace',id=row['source_id'],attributes={},body='',severity_number=0,
            service=row['service']) for row in activity]
        original=operation_facts.operation_facts
        with patch.object(operation_facts,'operation_facts',wraps=original) as calls:
            expected=findings.analyze([],logs,activity=activity,receiver='receiver')
            refs=trace_error_references(activity,'trace',receiver='receiver');uncached=calls.call_count
        with OperationPreparation() as context,patch.object(operation_facts,'operation_facts',wraps=original) as calls:
            context.bind(activity)
            actual=findings.analyze([],logs,activity=activity,receiver='receiver',operation_preparation=context)
            reused=trace_error_references(activity,'trace',receiver='receiver',operation_preparation=context)
            self.assertEqual(actual,expected);self.assertEqual(reused,refs)
            self.assertEqual(calls.call_count,len(activity))
            # Compact error correlation intentionally skips full cached facts.
            self.assertEqual(uncached,2*len(activity)-1)
            self.assertEqual(context.correlation(activity[-1]),operation_facts.correlation_facts(activity[-1]))
            self.assertNotEqual(context.full(activity[-1])['result'],context.correlation(activity[-1])['result'])
            self.assertNotIn(b'PRIVATE_CONTEXT_SECRET',b''.join(context._facts.values()))
        self.assertEqual((context._facts,context._allowed,context._bytes),({},frozenset(),0))

    def test_identity_only_misses_detached_results_and_rebind_clears(self):
        row=event();other=deepcopy(row);original=operation_facts.operation_facts
        with OperationPreparation() as context,patch.object(operation_facts,'operation_facts',wraps=original) as calls:
            context.bind([row]);first=context.full(row);context.full(row)
            self.assertEqual(calls.call_count,1)
            context.full(other);context.full(other);self.assertEqual(calls.call_count,3)
            first['arguments']['flags'].append('mutated consumer')
            self.assertNotIn('mutated consumer',context.full(row)['arguments']['flags'])
            with self.assertRaises(RuntimeError):context.bind([other])
            self.assertEqual(context._facts,{});self.assertEqual(context._allowed,frozenset())
            context.full(row);self.assertEqual(calls.call_count,4)

    def test_event_and_serialized_byte_bounds_fall_back_without_eviction(self):
        rows=[event(id=str(n),source_id=str(n)) for n in range(MAX_EVENTS+1)]
        for row in rows:row['operation_evidence']['inputs'][0][1]['padding']='x'*6000
        with OperationPreparation() as context:
            context.bind(rows);self.assertEqual(len(context._allowed),MAX_EVENTS)
            context.full(rows[-1]);self.assertNotIn(id(rows[-1]),context._facts)
            for row in rows:
                context.full(row)
            self.assertLessEqual(context._bytes,MAX_BYTES)
            self.assertGreater(len(context._facts),0);self.assertLess(len(context._facts),MAX_EVENTS)
            self.assertIn(id(rows[0]),context._facts)
            self.assertEqual(context.full(rows[-1]),operation_facts.operation_facts(rows[-1]))

    def test_full_derived_tuples_and_error_refs_match_frozen_original_and_clear_on_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            store=seed(Path(directory)/'fixture.db')
            try:
                original=storage_derived.derive_trace;contexts=[]
                def prepared(db,trace):
                    with OperationPreparation() as context:
                        contexts.append(context);result=original(db,trace,operation_preparation=context)
                        self.assertEqual(trace_error_references(result[1],trace,receiver='f'*32,operation_preparation=context),
                            trace_error_references(result[1],trace,receiver='f'*32))
                        return result
                with store._connect() as db:
                    db.execute('BEGIN IMMEDIATE');actual=projection(db,SimpleNamespace(derive_trace=prepared))
                self.assertEqual(actual,json.loads(FIXTURE.read_text())['sha256_by_projection'])
                self.assertTrue(all(not item._facts and not item._allowed and not item._bytes for item in contexts))
                captured=[]
                def fail(*args,**kwargs):
                    context=kwargs['operation_preparation'];captured.append(context)
                    context.full(kwargs['activity'][0]);self.assertTrue(context._facts)
                    raise RuntimeError('Synthetic Findings exception')
                with patch.object(findings,'analyze',side_effect=fail),self.assertRaisesRegex(RuntimeError,'Synthetic'):
                    store._refresh_summaries(['5'.zfill(32)])
                self.assertTrue(captured);self.assertTrue(all(not item._facts and not item._allowed for item in captured))
            finally:store.close()


if __name__=='__main__':unittest.main()
