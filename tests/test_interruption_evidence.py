"""Interrupted turns retain their cause, ownership and inspection path."""
import json
import tempfile
import time
import unittest
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace

from textual.widgets import Button, TabbedContent
from observatory.core import Store
from observatory.local_agents import Session, identity
from observatory.session_outcome import current_turn_events, interruption_event, interruption_reason, linked_trace
from observatory.terminal_ui import ObservatoryApp, attention_guidance
from tests.test_agent_hierarchy_ui import HierarchyFeed, agent_node


class NativeInterruptionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name)/'session.jsonl'; self.path.touch()
        self.store = Store(Path(self.tmp.name)/'events.db')
        self.session = Session(self.store, self.path, 'Codex')
        self.tick = 0

    def append(self, event, turn=None, **fields):
        self.tick += 1
        payload = {'type':event, **fields}
        if turn: payload['turn_id'] = turn
        row = {'type':'event_msg', 'timestamp':f'2026-11-17T22:17:{self.tick:02d}Z', 'payload':payload}
        with self.path.open('a') as stream: stream.write(json.dumps(row)+'\n')
        self.session.poll()

    def test_abort_resume_completion_and_late_old_abort_preserve_current_turn(self):
        self.append('task_started','first')
        self.append('turn_aborted','first',reason='interrupted')
        self.assertEqual(self.session.run_state,'interrupted')
        self.append('task_started','second')
        self.append('approval_request','second',request_id='approval')
        pending = self.session.request_id
        self.append('turn_aborted','first',reason='late cancellation')
        self.assertEqual(self.session.turn,'second')
        self.assertEqual(self.session.run_state,'waiting')
        self.assertEqual(self.session.request_id,pending)
        self.append('task_complete','first')
        self.append('item_completed','first',item={'type':'AgentMessage','phase':'final_answer','text':'Earlier PR created'})
        self.assertEqual((self.session.turn,self.session.run_state,self.session.request_id),('second','waiting',pending))
        self.append('task_complete','second')
        self.assertEqual(self.session.run_state,'completed')
        self.append('turn_aborted','first')
        self.assertEqual((self.session.turn,self.session.run_state),('second','completed'))
        with self.store._connect() as db:
            rows=db.execute("SELECT trace_id,body FROM logs WHERE body LIKE '%late cancellation%'").fetchall()
        self.assertEqual(len(rows),1)
        self.assertEqual(rows[0][0],identity(self.session.source_id,'first'))
        self.assertIn('turn_aborted',rows[0][1])

    def test_late_tool_output_does_not_reopen_completed_or_interrupted_turn(self):
        for outcome, state in [('task_complete','completed'),('turn_aborted','interrupted')]:
            self.append('task_started',state)
            self.append(outcome,state)
            stamp = self.session.outcome_time_ns
            row={'type':'response_item','timestamp':'2026-11-17T22:18:00Z',
                 'payload':{'type':'function_call_output','call_id':'late','output':'PR created'}}
            with self.path.open('a') as stream:stream.write(json.dumps(row)+'\n')
            self.session.poll()
            self.assertEqual(self.session.run_state,state)
            self.assertEqual(self.session.outcome_time_ns,stamp)

    def test_malformed_lifecycle_record_is_skipped_without_stopping_reader(self):
        self.append('task_started','first')
        self.append(['malformed'],'first')
        self.assertEqual(self.session.schema_errors,1)
        self.append('turn_aborted','first',reason='interrupted')
        self.assertEqual(self.session.run_state,'interrupted')

    def test_reason_is_redacted_bounded_and_metadata_only_omits_it(self):
        self.append('task_started','first')
        self.append('turn_aborted','first',reason='Authorization: Bearer sk-secret-value '+('x'*9000))
        with self.store._connect() as db:
            body=db.execute("SELECT body FROM logs WHERE json_extract(attributes,'$.\"agent.activity.action\"')='interrupted'").fetchone()[0]
        self.assertNotIn('sk-secret-value',body)
        self.assertLess(len(body),2500)
        event=self.store.activity_page(domain='agent',action='interrupted')['activity'][0]
        self.assertTrue(interruption_reason(event)[1])
        self.assertEqual(json.loads(event['detail'])['event'],'turn_aborted')
        self.session.capture_details=False
        self.append('task_started','second')
        self.append('turn_aborted','second',reason='PRIVATE REASON')
        with self.store._connect() as db:
            bodies=[r[0] for r in db.execute('SELECT body FROM logs')]
        self.assertNotIn('PRIVATE REASON',' '.join(bodies))


def fixture():
    now=time.time_ns()
    item=agent_node('selected',None,'check','interrupted',stamp=now)
    item.update(state='Interrupted',trace_id='current',outcome_time_ns=now,initial_task='check')
    owner={key:item[key] for key in ('service','agent_id','session_id')}
    def event(id,domain,action,subject,delta=0,**kwargs):
        return dict(owner,id=id,source_id=id,trace_id='current',time_unix_nano=now+delta,
                    domain=domain,action=action,subject=subject,detail='',result='observed',**kwargs)
    stop=event('stop','agent','interrupted','agenturm')
    stop['detail']=json.dumps({'event':'turn_aborted','reason':'interrupted'})
    request=event('request','message','user','Add usage tracking',-2)
    old=event('old','message','assistant','PR merged in previous turn',-5);old['trace_id']='previous'
    other=dict(stop,id='other',agent_id='sibling',detail='PRIVATE SIBLING REASON')
    return item,[other,stop,request,old]


class OutcomeEvidenceTests(unittest.TestCase):
    def test_exact_turn_and_owner_only(self):
        item,events=fixture()
        self.assertEqual(interruption_event(item,events)['id'],'stop')
        self.assertEqual([e['id'] for e in current_turn_events(item,events)],['stop','request'])
        for key,value in [('service','other'),('agent_id','other'),('session_id','other'),('trace_id','other'),('outcome_time_ns',1)]:
            with self.subTest(key=key):self.assertIsNone(interruption_event(dict(item,**{key:value}),events))
        self.assertIsNone(interruption_event(dict(item,state='Completed'),events))
        self.assertIsNone(interruption_event(dict(item,capture_content=False),events))
        self.assertEqual(current_turn_events({k:v for k,v in item.items() if k!='trace_id'},events),[])

    def test_receiver_identity_is_enforced(self):
        item, events = fixture()
        snapshot = SimpleNamespace(receiver_id='receiver-one')
        own = dict(events[1], receiver_id='receiver-one')
        foreign = dict(own, receiver_id='receiver-two')
        self.assertEqual(interruption_event(item, [foreign, own], snapshot), own)
        self.assertIsNone(interruption_event(item, [foreign], snapshot))

    def test_guidance_never_blames_unrelated_tool_failure_or_invents_cause(self):
        item,events=fixture()
        text=repr(attention_guidance(item,{'subject':'Unrelated pytest failure'},events[1]))
        self.assertIn('turn_aborted',text);self.assertIn('who stopped it or why',text)
        self.assertNotIn('pytest',text)
        self.assertIn('No detailed interruption reason',repr(attention_guidance(item)))
        for detail in ['bad json','[]','{"reason":42}',None]:
            self.assertEqual(interruption_reason({'detail':detail})[1],'')

    def test_trace_ownership_and_current_turn_beat_recent_tool_and_task_names(self):
        item,events=fixture()
        owner={key:item[key] for key in ('service','agent_id','session_id')}
        traces=[dict(owner,trace_id='previous'),dict(owner,trace_id='current')]
        self.assertEqual(linked_trace(item,events,traces),{'trace_id':'current'})
        self.assertEqual(linked_trace(item,events,[]),{'trace_id':'current'})
        for key in ('service','agent_id','session_id'):
            wrong=dict(traces[1],**{key:'another'})
            self.assertIsNone(linked_trace(item,events,[wrong]))
        unanchored={k:v for k,v in item.items() if k!='trace_id'}
        self.assertIsNone(linked_trace(unanchored,[],[dict(trace_id='fake',task='check')]))
        self.assertIsNone(linked_trace(unanchored,[events[0]],[]))
        self.assertIsNone(linked_trace(unanchored,events,traces))
        self.assertEqual(linked_trace(item,events,[{'trace_id':'current','services':[item['service']]}]),
                         {'trace_id':'current'})
        completed=dict(unanchored,state='Completed')
        self.assertEqual(linked_trace(completed,[events[1]],[{'trace_id':'current'}]),
                         {'trace_id':'current','span_id':None})
        self.assertIsNone(linked_trace(completed,[],[{'trace_id':'fake','task':'check'}]))


class InterruptionJourney(unittest.IsolatedAsyncioTestCase):
    async def test_preview_loads_exact_evidence_and_activity_keeps_context(self):
        for size in ((150,40),(80,24),(60,18)):
            with self.subTest(size=size):
                item,events=fixture();feed=HierarchyFeed()
                feed.snapshot=replace(feed.snapshot,local_status={'sessions':[item]},activity=[],traces=[])
                requests=[]
                def get(path):
                    requests.append(path)
                    return {'activity':events,'next_cursor':None}
                feed.api=SimpleNamespace(get=get,endpoint='http://example.invalid')
                app=ObservatoryApp(feed,persist_preferences=False)
                async with app.run_test(size=size) as pilot:
                    await pilot.pause();app.refresh_timer.stop()
                    app.select_agent(item['source_id'])
                    if size[0] <= 80:app.action_expand()
                    await app.workers.wait_for_complete();await pilot.pause()
                    text=' '.join(app.query_one('#agent-detail').plain_text.split())
                    self.assertIn('turn_aborted',text)
                    self.assertIn('Add usage tracking',text)
                    self.assertNotIn('PR merged',text)
                    self.assertNotIn('PRIVATE SIBLING',text)
                    self.assertEqual(app.agent_primary_event_id,'stop')
                    self.assertIn('Interruption',str(app.query_one('#agent-open',Button).label))
                    self.assertTrue(any('session_id=selected' in path and 'subtree=0' in path for path in requests))
                    app.agent_filter='errors';app.agent_has_failures=True
                    await pilot.press('s');await pilot.pause()
                    self.assertEqual(app.query_one('#pages',TabbedContent).active,'activity')
                    self.assertNotIn('activity',app.errors_only)
                    self.assertEqual(app.scope_value,item['source_id'])
                    self.assertEqual(app.selected_activity,'stop')
                    self.assertEqual(app.activity_fallback_trace,{'trace_id':'current'})

    async def test_paused_and_missing_evidence_are_explicit(self):
        item,_=fixture();feed=HierarchyFeed();feed.paused=True
        feed.snapshot=replace(feed.snapshot,local_status={'sessions':[item]},activity=[],traces=[])
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();app.refresh_timer.stop();app.select_agent(item['source_id']);app.action_expand();await pilot.pause()
            text=' '.join(app.query_one('#agent-detail').plain_text.split())
            self.assertIn('Paused view',text);self.assertIn('p Follow live',text)
            self.assertIn('No detailed interruption reason',text)
            self.assertIn('request for this turn is not loaded',text)
            self.assertNotIn('failed action',text)
            for feedback, value, expected in [('busy',True,'Loading the interruption details'),
                    ('error','Receiver unavailable','could not be loaded'),
                    ('partial',True,'outside this window')]:
                app.session_evidence.busy=False;app.session_evidence.error='';app.session_evidence.partial=False
                setattr(app.session_evidence,feedback,value)
                app.show_agent();await pilot.pause()
                self.assertIn(expected,' '.join(app.query_one('#agent-detail').plain_text.split()))
