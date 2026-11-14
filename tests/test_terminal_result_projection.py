"""Frozen full-payload parity for terminal-only Activity result projection."""
from copy import deepcopy
import hashlib
import itertools
import json
from pathlib import Path
import unittest

from observatory import operation_facts as implementation

FIXTURE=Path(__file__).parent/'fixtures'/'operation_result_e795.json'


def event(*,domain='shell',result='observed',exits=(),is_error=False,terminal_incomplete=False):
    evidence=dict(version=1,inputs=[('native.input',{'cmd':'pytest tests','cwd':'/work'})],
        commands=[('native.command','pytest tests')],exits=deepcopy(exits),
        outputs=[('native.output','output',{'count':2})],cwd='/work',tool='exec_command',server='generated',
        is_error=is_error,terminal_incomplete=terminal_incomplete)
    return dict(detail_role='operation_v1',operation_evidence=evidence,domain=domain,result=result,
        action='execute',subject='Shell command',detail='',receiver_id='generated-receiver',service='generated',
        agent_id='worker',session_id='session',trace_id='1'*32,source_id='generated-source')


def groups():
    exits=[[],None,False,{},'invalid',[("native.exit",None)],[("native.exit",0)],[("native.exit",1)],
        [("native.exit",2)],[("native.exit",-1)],[("native.exit","0")],[("native.exit","1")],
        [("native.exit",True)],[("native.exit",0.)],[("native.exit",2**40)],[("native.exit"," 1")],
        [("native.exit","１２")],[("native.exit","١")],[("native.exit",[])],[("native.exit",{})],
        [("native.exit",0),("other.exit",1)],[("native.exit",1),("other.exit",2)],
        [("native.exit",0)]*8,[("native.exit",0)]*9,[("bad\u200dorigin",0)],
        [("native.exit",0,"extra")],[None],[(False,0)]]
    rows=[]
    for domain,result,values,is_error,incomplete,reported in itertools.product(
            ('shell','tool'),('observed','success','error','blocked','unknown','FAILED','ok',None),
            exits,(False,True,1),(False,True),(False,True)):
        row=event(domain=domain,result=result,exits=values,is_error=is_error,terminal_incomplete=incomplete)
        if reported:row['reported_result']='observed'
        rows.append(row)
    yield 'terminal_matrix',rows
    rows=[]
    commands=['rg needle .','grep needle file','diff a b','git diff --quiet','git diff --exit-code',
        'diff --check a b','pytest tests','rg needle . && true','rg needle . &','sh -c "rg needle ."',
        'rg --api-key SECRET needle .','rg needle .\u200d','rg needle .\x00','rg needle .\n',
        'rg "token=SECRET" .','rg needle . '+('x'*8001)]
    for command,clipped,input_clipped in itertools.product(commands,(False,True),(False,True)):
        row=event(result='error',exits=[('native.exit',1)])
        evidence=row['operation_evidence'];evidence['commands']=[('native.command',command)]
        evidence['inputs']=[('native.input',{'cmd':command,'cwd':'/work'})]
        evidence.update(command_clipped=clipped,input_clipped=input_clipped)
        rows.append(row)
    for command in commands[:5]:
        row=event(result='error',exits=[('native.exit',1)]);row.pop('operation_evidence')
        row['subject']=command;row['attributes']={'process.exit.code':1};rows.append(row)
    yield 'shell_one_qualification',rows
    rows=[]
    for result,reported,code in itertools.product((True,False,{},[],0,1,'ßSUCCESS','success\x00'),
            (None,False,{},[],'success','error'),(0,1,9,None)):
        row=event(result=result,exits=[('native.exit',code)])
        if reported is not None:row['reported_result']=reported
        rows.append(row)
    for kind in ('mcp','tool','shell',None,{},0):
        row=event(domain=kind,result='observed',exits=[('native.exit',9)]);rows.append(row)
    yield 'malformed_labels_and_domains',rows
    rows=[]
    for label,code,domain in itertools.product(sorted(implementation.FAILURES|implementation.SUCCESSES),
            (None,0,1,9),('tool','shell')):
        row=event(result=label,exits=[('native.exit',code)]);rows.append(row)
        row=deepcopy(row);row['reported_result']=None;rows.append(row)
    for status,code in itertools.product(('SUCCESS','ERROR',{},[],1),(None,0,1,9)):
        row=event(exits=[('native.exit',code)]);row.pop('result');row['status']=status;rows.append(row)
    for code,incomplete in itertools.product((-2**31,2**31-1,2**31,-2**31-1,10**100,float('inf'),float('nan')),
            (None,1,'true',[])):
        rows.append(event(exits=[('native.exit',code)],terminal_incomplete=incomplete))
    yield 'terminal_bounds_and_status_fallback',rows
    rows=[]
    for command in ('pytest tests','rg needle .','rg api_key=SECRET .','rg api\u200d_key=SECRET .',
                    'rg api\\u005fkey=SECRET .','rg token=SECRET .','rg test .\ud800'):
        for detail_mode in ('semantic','typed','badversion','clipped'):
            row=event(result='observed',exits=[('native.exit',0)])
            evidence=row.pop('operation_evidence');evidence['commands']=[('native.command',command)]
            evidence['inputs']=[('native.input',{'cmd':command,'cwd':'/work','api_key':'SECRET'})]
            evidence['outputs']=[('native.output','stderr','Authorization: Bearer SECRET\u200d')]
            if detail_mode=='semantic':
                row['attributes']={'process.command_line':command,'process.exit.code':0,'output.value':False}
            else:
                if detail_mode=='badversion':evidence['version']=True
                row['detail']=json.dumps(evidence,ensure_ascii=True)
                if detail_mode=='clipped':row['detail_clipped']=True
            rows.append(row)
    row=event();row['operation_evidence']['outputs']=[('native.output','output','x'*100000)];rows.append(row)
    row=event();row['detail_role']='unclassified';rows.append(row)
    yield 'provenance_credentials_unicode',rows


def captured(function,row):
    try:return dict(value=function(deepcopy(row)))
    except Exception as exc:return dict(exception=type(exc).__name__)


def supersedes_groups():
    rows=[]
    for domain,other_domain,result,other_result,kind in itertools.product(('shell','file','other'),
            ('shell','file','other'),('unknown','success','error'),('unknown','success','error'),('generic','typed','legacy')):
        first=event(domain=domain,result=result);second=event(domain=other_domain,result=other_result)
        if kind=='generic':first.pop('detail_role');first.pop('operation_evidence')
        elif kind=='legacy':first.pop('operation_evidence');first['detail']='Recorded legacy output'
        rows.append((first,second))
    yield 'generic_and_qualified_results',rows
    rows=[]
    for first_code,second_code,tool,inputs,command in itertools.product((None,0,1,9),(None,0,1,9),
            (None,'','exec_command'),(False,True),(False,True)):
        first=event(exits=[('native.exit',first_code)]);second=event(exits=[('native.exit',second_code)])
        first['operation_evidence'].update(tool=tool)
        if not inputs:first['operation_evidence']['inputs']=[]
        if not command:first['operation_evidence']['commands']=[('subject','pytest tests')]
        rows.append((first,second))
    for key in ('service','agent_id','session_id'):
        for value in ('',None,'foreign'):
            first=event();second=event();second[key]=value;rows.append((first,second))
    for key,value in (('inputs',[('native.input',None)]),('inputs',{}),('commands',None),
            ('tool',False),('tool',0),('tool','tool\u200d'),('outputs',[('native.output','output','api_key=SECRET')])):
        first=event();second=event();first['operation_evidence'][key]=value;rows.append((first,second))
    yield 'negative_qualification_and_execution_guards',rows


def supersedes_fingerprints(module):
    result={}
    for name,rows in supersedes_groups():
        hashed=hashlib.sha256()
        for pair in rows:
            hashed.update(json.dumps(captured(lambda pair:module.supersedes_operation(*pair),pair),
                sort_keys=True,separators=(',',':')).encode()+b'\n')
        result[name]=dict(count=len(rows),sha256=hashed.hexdigest())
    return result


def fingerprints(module):
    result={}
    for name,rows in groups():
        hashes={key:hashlib.sha256()for key in ('facts','normalization')}
        for row in rows:
            for kind,function in (('facts',module.operation_facts),('normalization',module.normalize_operation_result)):
                hashes[kind].update(json.dumps(captured(function,row),ensure_ascii=True,sort_keys=True,
                    separators=(',',':'),allow_nan=True).encode()+b'\n')
        result[name]=dict(count=len(rows),**{key:value.hexdigest()for key,value in hashes.items()})
    return result


class TerminalResultProjectionTests(unittest.TestCase):
    def test_full_facts_and_normalized_rows_match_frozen_e795_corpus(self):
        expected=json.loads(FIXTURE.read_text())
        actual=fingerprints(implementation)
        self.assertEqual(actual,expected['groups'])

    def test_supersession_booleans_match_frozen_e795_corpus(self):
        self.assertEqual(supersedes_fingerprints(implementation),json.loads(FIXTURE.read_text())['supersedes_groups'])

    def test_representative_terminal_and_shell_one_meanings(self):
        for code,result,expected in ((0,'observed','success'),(9,'observed','error'),
                (0,'error','unknown'),(9,'success','unknown'),(None,'observed','observed')):
            self.assertEqual(implementation.normalize_operation_result(event(result=result,
                exits=[('native.exit',code)]))['result'],expected)
        row=event(result='error',exits=[('native.exit',1)])
        row['operation_evidence']['commands']=[('native.command','rg needle .')]
        row['operation_evidence']['inputs']=[('native.input',{'cmd':'rg needle .','cwd':'/work'})]
        self.assertEqual(implementation.normalize_operation_result(row)['result'],'informational')
        row=event(result='error',exits=[('native.exit',1)]);row['subject']='rg needle .'
        row.pop('operation_evidence');row['attributes']={'process.exit.code':1}
        self.assertEqual(implementation.normalize_operation_result(row)['result'],'error')
