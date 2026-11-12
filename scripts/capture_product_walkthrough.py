"""Capture v0.2.0 product screens from synthetic records, without a receiver.

Run from a checkout with CairoSVG installed:
  uv run --with cairosvg python scripts/capture_product_walkthrough.py

Inputs: media/product/scenario.json. Outputs: media/product/{agents,prompts,
review,mcp,handoff,compact}.svg and .png. All persistent state is temporary.
No local transcripts, real workspace capture, provider, or network is used.
"""
from __future__ import annotations

import asyncio
import copy
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit

from render_terminal_demo import FakeFeed, ObservatoryApp, now, snapshot
from observatory.agent_hierarchy import canonical_agent_id
from observatory.saved_tasks import TaskStore
from observatory.ui_saved_tasks import HandoffScreen

ROOT = Path(__file__).resolve().parents[1]


def prepare():
    fixture=json.loads((ROOT/'media/product/scenario.json').read_text())
    data=copy.deepcopy(snapshot)
    rows=[]; events=[]
    for index,agent in enumerate(fixture['agents']):
        namespace={'Codex':'codex-rollout','OpenCode':'opencode-db','Claude Code':'claude-code-projects'}[agent['runtime']]
        aid=canonical_agent_id(agent['runtime'],namespace,agent['native'])
        row=dict(source_id=agent['id'],session_id=agent['native'],agent_id=aid,
                 native_session_id=agent['native'],source_namespace=namespace,
                 runtime=agent['runtime'],service={'Codex':'codex','OpenCode':'opencode','Claude Code':'claude'}[agent['runtime']],
                 project=agent['project'],task=agent['task'],model='example-model',
                 run_state={'Working':'running','Needs input':'waiting','Ready for review':'completed'}[agent['state']],
                 last_record_ns=now-(index+1)*int(1e9),coverage='complete',discovery_state='observed',
                 relationship_kind='delegated' if agent['kind']=='child' else 'unknown',
                 relationship_state='confirmed' if agent['kind']=='child' else 'unresolved',
                 relationship_provenance='Synthetic explicit Codex parent_thread_id' if agent['kind']=='child' else 'Independent example session; no parent claim')
        if agent['kind']=='child':row['parent_agent_id']=rows[0]['agent_id']
        if agent['id']=='api':row['waiting_for']='User input: expired token should return 401 or 403?'
        rows.append(row)

    def event(row,identity,seconds,domain,action,subject,detail=None,**extra):
        value=dict(source_id=identity,id=identity,service=row['service'],session_id=row['session_id'],agent_id=row['agent_id'],
                   trace_id=('b' if row['source_id']=='tests' else 'a')*32,time_unix_nano=now-int(seconds*1e9),
                   domain=domain,action=action,subject=subject,detail=detail or subject,result='success',**extra)
        events.append(value);return value

    for i,prompt in enumerate(fixture['prompts']):
        event(rows[0],f'prompt-{i}',1000-i*200,'message','user',prompt['body'],prompt['raw'])
    rows[0]['last_activity']=event(rows[0],'coordinator-update',3,'message','assistant',fixture['agents'][0]['detail'])
    rows[1]['last_activity']=event(rows[1],'api-question',12,'message','assistant','Should an expired refresh token return 401 or 403?')
    event(rows[2],'regression-edit',360,'file','edit','tests/auth/test_refresh.py',{'operation':'edit','diff':'+ def test_expired_refresh_token_is_rejected():'})
    event(rows[2],'test-check',240,'shell','run','pytest tests/auth -q',{'command':'pytest tests/auth -q','exit_code':0,'output':'12 passed in 1.84s'})
    event(rows[2],'test-done',180,'message','assistant','Added the expiry regression. Tests pass.')
    rows[2]['last_activity']=event(rows[2],'later-edit',120,'file','edit','src/auth/refresh.py',{'operation':'edit','diff':'- retry_expired_token()\n+ reject_expired_token()'})
    rows[3]['last_activity']=event(rows[3],'web-edit',8,'file','edit','src/checkout/keyboard.ts')
    rows[4]['last_activity']=event(rows[4],'docs-done',600,'message','assistant','Updated callback configuration documentation.')
    for row in rows:row['last_record_ns']=row['last_activity']['time_unix_nano']
    call=fixture['tools'][0]
    event(rows[0],'mcp-failure',60,'mcp','call','github / get_pull_request',
          {'arguments':json.loads(call['request']),'response':json.loads(call['response'])},
          mcp_server='github',mcp_tool='get_pull_request',span_id='c'*16,duration_ms=842)
    events[-1]['result']='error'
    data.local_status['sessions']=rows
    data.local_status['sources']=len(rows)
    data.local_status['runtime']={'host':'example-devbox','os':'Linux','architecture':'x86_64','processes':[]}
    data.activity=sorted(events,key=lambda x:x['time_unix_nano'],reverse=True)
    data.review_trace_filter=True;data.receiver_id='synthetic-product-demo'
    data.traces=[dict(trace_id='a'*32,task=rows[0]['task'],status='ERROR',service='codex',session_id=rows[0]['session_id'],agent_id=rows[0]['agent_id'],active=False,span_count=2)]
    data.agents=[]
    data.sessions=[]
    data.totals={}
    data.mcp=[dict(id='synthetic-github-call',time_unix_nano=now-int(60e9),server='github',tool='get_pull_request',
                   method='tools/call',status='ERROR',duration_ms=842,service='codex',session_id=rows[0]['session_id'],agent_id=rows[0]['agent_id'],
                   trace_id='a'*32,span_id='c'*16,source='Synthetic MCP span',arguments=json.loads(call['request']),output=json.loads(call['response']))]
    data.detail={'trace_id':'a'*32,'summary':data.traces[0],'spans':[
        dict(trace_id='a'*32,span_id='a'*16,parent_span_id=None,name='agent.run',status='ERROR',service='codex',start_unix_nano=now-int(65e9),end_unix_nano=now-int(59e9),attributes={},events=[],links=[]),
        dict(trace_id='a'*32,span_id='c'*16,parent_span_id='a'*16,name='github.get_pull_request',status='ERROR',service='codex',start_unix_nano=now-int(60e9),end_unix_nano=now-int(59.158e9),attributes={'mcp.server.name':'github','mcp.tool.name':'get_pull_request'},events=[],links=[])], 'span_page':{'trace_total':2}}
    data.mcp_summary={'servers':[{'server':'github','calls':1,'tools':1,'errors':1}]}
    return data,fixture


class DemoApi:
    endpoint='http://synthetic.example.invalid:14318'
    def __init__(self,data):self.data=data
    def get(self,path):
        query=parse_qs(urlsplit(path).query)
        if urlsplit(path).path=='/api/activity':
            events=list(self.data.activity)
            for key in ('service','session_id','agent_id','trace_id','domain','action'):
                if query.get(key):events=[e for e in events if e.get(key)==query[key][0]]
            return {'activity':copy.deepcopy(events),'next_cursor':None,'receiver_id':self.data.receiver_id}
        if urlsplit(path).path.startswith('/api/traces/'):
            return copy.deepcopy(self.data.detail)
        return {}


async def capture(output):
    import cairosvg
    data,fixture=prepare()
    feed=FakeFeed();feed.snapshot=data;feed.api=DemoApi(data);feed.selected_trace='a'*32
    output.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='agenturm-product-demo-') as directory:
        task_path=Path(directory)/'tasks.sqlite3'
        app=ObservatoryApp(feed,persist_preferences=False,task_store_path=task_path)
        app.demo_mode=True
        async with app.run_test(size=(148,42)) as pilot:
            await pilot.pause(.5);app.refresh_timer.stop();app.set_agent_filter('today')
            app.select_agent('coordinator');app.query_one('#agent-table').focus()
            async def save(name):
                await pilot.pause(.4)
                svg=app.export_screenshot(title='Agenturm 0.2.0 · synthetic example')
                (output/f'{name}.svg').write_text(svg)
                svg=svg.replace('Fira Code','DejaVu Sans Mono').replace('FiraCode','DejaVu Sans Mono')
                (output/f'{name}.png').write_bytes(cairosvg.svg2png(bytestring=svg.encode(),output_width=1920))
                print('Captured',name,flush=True)
            await save('agents')
            await pilot.press('enter');await pilot.pause();app.open_session_mode('prompts')
            await app.workers.wait_for_complete();await pilot.pause(.3)
            assert len(app.session_prompts.events)==3,app.session_prompts.events
            app.navigate_prompt('first');await pilot.pause()
            await save('prompts')
            app.navigate_prompt('newer');await pilot.pause();await save('prompt-constraints')
            app.navigate_prompt('latest');await pilot.pause();await save('prompt-review')
            await pilot.press('escape');await pilot.pause()
            app.action_review_work(app.source_map['tests'])
            await app.workers.wait_for_complete();await pilot.pause(.3)
            assert app.screen.projection['checks'],app.screen.projection
            assert app.screen.projection['checks'][0]['freshness']=='later recorded edits'
            await save('review')
            await pilot.press('escape');await pilot.pause()
            app.action_page('mcp');await pilot.pause();app.action_expand();await pilot.pause()
            await save('mcp')
            await pilot.press('escape');await pilot.pause()
            store=TaskStore(task_path)
            task=store.create('Fix refresh token expiry',notes='Synthetic example. Resolve the response-code question, then rerun tests after the latest edit.')
            task=store.update_brief(task['id'],{
                'intent':'Reject expired refresh tokens promptly.',
                'acceptance_criteria':'Add an expiry regression. Preserve the public API.',
                'constraints':'No new dependency.',
                'open_questions':'401 or 403 for an expired token?',
                'next_action':'Resolve the response-code question; rerun pytest tests/auth -q after the latest edit.'})
            task=store.add_decision(task['id'],kind='accepted',text='Keep the public refresh API',rationale='Existing clients depend on its shape.')
            app.push_screen(HandoffScreen(app,task));await app.workers.wait_for_complete();await pilot.pause(.3)
            await save('handoff')
            await pilot.press('escape');await pilot.pause();app.action_page('agents')
            await pilot.resize_terminal(80,24);await pilot.pause();await save('compact')


if __name__=='__main__':
    import argparse
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,default=ROOT/'media/product')
    asyncio.run(capture(parser.parse_args().output))
