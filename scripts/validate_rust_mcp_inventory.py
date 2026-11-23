#!/usr/bin/env python3
"""Verify installed MCP inventory and exact call-to-trace keyboard navigation."""

import argparse
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace

from validate_rust_real_session_roster import Cockpit, environment, run

source_key = 'harness.codex.mcp-qualify'
session = 'mcp-qualification'
turn = 'mcp-turn'
timestamp = '2026-11-23T09:00:00Z'


def write_json(path, obj):
    path.write_text(json.dumps(obj) + '\n', encoding='utf-8')
    path.chmod(0o600)


def trace(char, scope):
    attrs = [('agenturm.source.id', source_key), ('session.id', session),
             ('agenturm.turn.id', turn)] + list(scope.items())
    return dict(traceId=char * 32, spanId=char * 16, startTimeUnixNano='1795424400000000000',
                endTimeUnixNano='1795424400000000100', status={'code': 1},
                attributes=[{'key':k,'value':{'stringValue':v}} for k,v in attrs])


def verify(cockpit, cases):
    cockpit.markers('roster', 'Agenturm command center')
    cockpit.key(b'3')
    cockpit.markers('recorded MCP calls', 'MCP', 'chosen-tool', 'other-tool')
    def identify(frame):
        return any(
            row.lstrip().removeprefix("│").lstrip().startswith("› ")
            and "chosen-tool" in row
            for row in frame.splitlines()
        )

    cockpit.until(identify, 'exact selected MCP call')
    cockpit.key(b'i')
    cockpit.markers('idle configuration and observed calls', 'MCP servers', 'idle-provider', 'Disabled config', 'Observed')
    assert 'sensitive-credential' not in cockpit.screen.contents()
    cockpit.key(b'\x1b')
    cockpit.until(identify, 'return to exact selected call from inventory')
    cockpit.key(b't')
    cockpit.until(lambda frame: 'Traces' in frame and 'chosen-tool' not in frame and 'Loading' not in frame, 'related trace route')
    cockpit.key(b'\r')
    cockpit.markers('exact related trace ID', cases[0][:8])
    assert all(case[:8] not in cockpit.screen.contents() for case in cases[1:])
    cockpit.key(b'\x1b')
    cockpit.until(lambda frame: identify(frame) or '↑↓ select · Enter inspect' in frame, 'close the trace inspector')
    if not identify(cockpit.screen.contents()):
        cockpit.key(b'\x1b')
    cockpit.until(identify, 'return to exact selected call from trace')
    cockpit.quit()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, required=True, help="installed Agenturm bundle binary")
    args = parser.parse_args()
    app = args.app.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix='agenturm-mcp-final-qualification-') as folder:
        work = Path(folder)
        env = environment(SimpleNamespace(codex_home=work/'unused-codex', claude_home=work/'unused-claude'), work)
        history = work/'history.sqlite'
        jsonl = work/'rollout.jsonl'
        rows = [{'type':'session_meta','payload':{'id':session,'cwd':str(work),'source':'cli'}},
                {'type':'event_msg','timestamp':timestamp,'payload':{'type':'task_started','turn_id':turn}}]
        for n, (operation, tool) in enumerate([('other','other-tool'),('chosen','chosen-tool')], start=1):
            rows.append({'type':'event_msg','timestamp':f'2026-11-23T09:00:0{n}Z','payload':{
                'type':'item_completed','thread_id':session,'turn_id':turn,
                'item':{'type':'McpToolCall','id':operation,'server':'recorded-provider','tool':tool,'status':'completed',
                        'arguments':{'private':'sensitive-credential'}}}})
        jsonl.write_text(''.join(json.dumps(row)+'\n' for row in rows), encoding='utf-8')
        config = work/'sources.json'
        write_json(config, {
            'version':2, 'plugin_host':str(app.with_name('agenturm-plugin-host')),
            'plugin_catalog':str(app.parent.parent/'agenturm-plugins.json'),
            'sources':[dict(plugin_id='example.rust-codex-harness',source_id=source_key,source_root=str(jsonl))],
        })
        mcp_config = work/'fixture.json'
        write_json(mcp_config, {'mcpServers': {'idle-provider': {'enabled':False, 'env':{'TOKEN':'sensitive-credential'}}}})
        manifest = work/'inventory.json'
        write_json(manifest, {'version':1, 'sources':[dict(client='claude',scope='project',path=str(mcp_config))]})
        env = {**env, 'AGENTURM_MCP_CONFIG': str(manifest)}
        run(app,env,work,'collect','--config',config,'--db',history)
        report=json.loads(run(app,env,work,'query','--db',history))
        ops={row['observation']['tool']:row['identity']['operation_id'] for row in report['observations']
             if row['observation']['kind']=='mcp_call'}
        assert set(ops) == {'chosen-tool','other-tool'}
        attachment=lambda op: {'agenturm.operation.id':ops[op]}
        spans=[trace('1',attachment('chosen-tool')),trace('2',attachment('other-tool')),
               trace('3',{**attachment('chosen-tool'),'agenturm.worker.id':'different-worker'}),trace('4',{})]
        fixture=work/'otlp.json'
        write_json(fixture, {'resourceSpans':[{'scopeSpans':[{'spans':spans}]}]})
        run(app,env,work,'import-traces','--db',history,'--source-id','otlp.mcp-qualify','--file',fixture)
        jsonl_trace=work/'span.jsonl'
        write_json(jsonl_trace, dict(trace_id='5'*32,span_id='5'*16,start_unix_nano='1795424400000000000',
                                     end_unix_nano='1795424400000000100',status='OK',
                                     attributes={'agenturm.source.id':source_key,'session.id':session,
                                                 'agenturm.turn.id':turn,'agenturm.operation.id':ops['other-tool']}))
        run(app,env,work,'import-jsonl','--db',history,'--source-id','otlp.mcp-jsonl','--file',jsonl_trace)
        import sqlite3
        with sqlite3.connect(history.as_uri()+'?mode=ro',uri=True) as retained:
            evidence = retained.execute("SELECT json_extract(kind_json,'$.target_operation_id') FROM agenturm_observations WHERE json_extract(kind_json,'$.kind')='trace_span' ORDER BY event_id").fetchall()
        assert evidence==[(ops['chosen-tool'],),(ops['other-tool'],),(ops['chosen-tool'],),(None,),(ops['other-tool'],)]
        for width,height in [(120,32),(80,24),(60,18)]:
            with Cockpit(app,env,work,width,height,history) as cockpit:
                try:
                    verify(cockpit,[span['traceId'] for span in spans] + ['5' * 32])
                except Exception:
                    print(cockpit.screen.contents(),flush=True)
                    raise
            print(f'Verified MCP configured idle, exact trace link and return at {width}×{height}', flush=True)


if __name__ == "__main__":
    main()
