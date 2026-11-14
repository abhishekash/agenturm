"""Bounded operation facts with separate input, terminal and output provenance.

Only an explicit normalized evidence envelope or semantic attributes supply
structured facts. Arbitrary printed JSON remains output. Nothing executes here.
"""
from __future__ import annotations

import hashlib
import json
import math
import re
import shlex
import unicodedata

VERSION=1
MAX_TEXT=8000
MAX_NODES=128
MAX_DEPTH=6
FAILURES={'error','failed','failure','denied','blocked','policy_denied','incomplete'}
SUCCESSES={'ok','success','passed','completed'}
GENERIC={'shell command','commandexecution','command','tool call','shell','bash','exec_command'}
INPUT_KEYS=('input.value','gen_ai.tool.call.arguments','gcp.vertex.agent.tool_call_args','tool.arguments')
OUTPUT_KEYS=('output.value','gen_ai.tool.call.result','gcp.vertex.agent.tool_response','tool.result')
COMMAND_KEYS=('process.command_line','tool.command','command')
EXIT_KEYS=('process.exit.code','exit_code')
QUOTED_FIELDS=(re.compile(r'"((?:\\.|[^"\\\r\n])*)"\s*:'),
               re.compile(r"'((?:\\.|[^'\\\r\n])*)'\s*:"))


def credential_key(key):
    from .findings import _SENSITIVE_FIELD,_TOKEN_FIELD
    if len(key)>MAX_TEXT:return True
    if '\\' in key:
        try:key=json.loads('"'+key+'"')
        except (ValueError,TypeError):return True
        if '\\' in key:return True
    key=''.join(char for char in key if unicodedata.category(char) not in {'Cc','Cf'})
    return bool(_SENSITIVE_FIELD.search(key) or _TOKEN_FIELD.search(key))


def command_present(value):
    return isinstance(value,str) and bool(value[:MAX_TEXT].strip())


def safe_value(value):
    """Return bounded redacted JSON and whether exact matching remains safe."""
    from .findings import _safe_text
    nodes=MAX_NODES;bytes_left=MAX_TEXT;complete=True
    def visit(item,depth=0):
        nonlocal nodes,bytes_left,complete
        nodes-=1
        if nodes<0 or depth>MAX_DEPTH:
            complete=False;return '[payload omitted]'
        if isinstance(item,str):
            if len(item)>bytes_left:
                complete=False;return '[text omitted: projection limit]'
            try:size=len(item.encode('utf-8'))
            except UnicodeEncodeError:
                complete=False;return '[invalid Unicode omitted]'
            if size>bytes_left:
                complete=False;return '[text omitted: projection byte limit]'
            bytes_left-=size
            if any(credential_key(match.group(1)) for pattern in QUOTED_FIELDS for match in pattern.finditer(item)):
                complete=False;return '[REDACTED credential-bearing text]'
            text=_safe_text(item)
            text=re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]','',text)
            text=''.join(char for char in text if char in '\n\t' or unicodedata.category(char) not in {'Cc','Cf'})
            if text!=item or re.search(r'(?i)\[(?:redact|[^\]]*(?:omitted|truncated|clipped))',text):complete=False
            return text
        if item is None or type(item) is bool:return item
        if type(item) is int and item.bit_length()<=4096:return item
        if type(item) is float and math.isfinite(item):return item
        if isinstance(item,dict):
            result={}
            for key,child in item.items():
                if nodes<=0:complete=False;break
                if not isinstance(key,str):complete=False;continue
                clean_key=visit(key,depth+1)
                if credential_key(key) or credential_key(clean_key):
                    complete=False;result[clean_key]='[REDACTED]'
                else:result[clean_key]=visit(child,depth+1)
            return result
        if isinstance(item,(list,tuple)):
            result=[]
            for child in item:
                if nodes<=0:complete=False;break
                result.append(visit(child,depth+1))
            return result
        complete=False;return '[unsupported value]'
    return visit(value),complete


def decode(value):
    if isinstance(value,str) and len(value)<=MAX_TEXT and value.lstrip().startswith(('{','[')):
        try:return json.loads(value)
        except (ValueError,TypeError,RecursionError):pass
    return value


def exit_code(value):
    if isinstance(value,str) and re.fullmatch(r'-?\d{1,9}',value):value=int(value)
    return value if type(value) is int and -(2**31)<=value<2**31 else None


def digest(value):
    return hashlib.sha256(json.dumps(value,sort_keys=True,ensure_ascii=False,
        separators=(',',':'),allow_nan=False).encode()).hexdigest()


def attribute_evidence(attrs):
    """Known semantic attributes; output keys never supply input or exits."""
    evidence={'version':VERSION,'inputs':[],'commands':[],'exits':[],'outputs':[]}
    for key in INPUT_KEYS:
        if key in attrs:evidence['inputs'].append((key,decode(attrs[key])))
    for key in COMMAND_KEYS:
        if command_present(attrs.get(key)):
            evidence['commands'].append((key,attrs[key]))
    for key in EXIT_KEYS:
        if key in attrs:evidence['exits'].append((key,attrs[key]))
    for key in OUTPUT_KEYS:
        if key in attrs:
            evidence['outputs'].append((key,'output',decode(attrs[key])))
            break
    for key in ('stderr','stdout','tool.error_message','tool.error'):
        if key in attrs:evidence['outputs'].append((key,'stderr' if key=='stderr' else
            'stdout' if key=='stdout' else 'error',attrs[key]))
    evidence['cwd']=attrs.get('process.working_directory',attrs.get('cwd'))
    evidence['tool']=attrs.get('gen_ai.tool.name') or attrs.get('tool.name') or attrs.get('mcp.tool.name')
    evidence['server']=attrs.get('mcp.server.name')
    tool_type=attrs.get('gen_ai.tool.type')
    evidence['is_mcp']=(attrs.get('rpc.system')=='mcp' or
        isinstance(tool_type,str) and tool_type[:64].casefold()=='mcptool' or
        attrs.get('agent.activity.domain')=='mcp' or
        any(key in attrs for key in ('mcp.method.name','mcp.server.name','mcp.tool.name')))
    evidence['is_error']=any(attrs.get(key) is True for key in ('mcp.tool.is_error','mcp.is_error'))
    if evidence['is_mcp']:
        chosen=next((decode(attrs[key]) for key in OUTPUT_KEYS if key in attrs),None)
        if isinstance(chosen,dict) and (chosen.get('isError') is True or chosen.get('is_error') is True):
            evidence['is_error']=True
    evidence['input_clipped']=attrs.get('observatory.operation.input_clipped') is True
    evidence['output_clipped']=attrs.get('observatory.operation.output_clipped') is True
    evidence['command_clipped']=attrs.get('observatory.operation.command_clipped') is True
    return evidence


def evidence_detail(evidence):
    """Serialize one role-labelled body; do not duplicate input/output at rest.

    Each side is redacted and bounded independently. Terminal metadata survives
    omission of an oversized side. The final body is valid JSON under8KiB.
    """
    input_side,input_complete=safe_value({key:evidence.get(key) for key in
        ('inputs','commands','cwd','tool','server')})
    output_side,output_complete=safe_value(evidence.get('outputs') or [])
    encoded=lambda value:json.dumps(value,ensure_ascii=False,separators=(',',':'))
    if len(encoded(input_side).encode())>3000:
        input_complete=False
        input_side=dict(inputs=[],commands=[],cwd=None,tool=None,server=None)
    if len(encoded(output_side).encode())>3000:
        output_complete=False
        candidates=[row for row in output_side if isinstance(row,list) and len(row)==3]
        preferred=next((row for kind in ('stderr','error','output','stdout') for row in candidates if row[1]==kind),None)
        output_side=([(preferred[0],preferred[1],preferred[2][:1000]+' [clipped]')]
            if preferred and isinstance(preferred[2],str) else [('projection','output','[Output omitted: projection limit]')])
    exits=[]
    raw_exits=evidence.get('exits') or []
    terminal_incomplete=(evidence.get('terminal_incomplete') is True or
                         not isinstance(raw_exits,(list,tuple)) or len(raw_exits)>8)
    for row in raw_exits[:8] if isinstance(raw_exits,(list,tuple)) else []:
        if not isinstance(row,(list,tuple)) or len(row)!=2:
            terminal_incomplete=True;continue
        origin,value=row
        if not isinstance(origin,str) or not re.fullmatch(r'[A-Za-z0-9_.: -]{1,128}',origin):terminal_incomplete=True
        origin=safe_value(origin[:128] if isinstance(origin,str) else 'unknown')[0]
        exits.append((origin,value if value is None or type(value) in {bool,int} and
            (type(value) is bool or value.bit_length()<=32) else str(value)[:32]
            if isinstance(value,str) else '[invalid exit]'))
    payload=dict(version=VERSION,**input_side,exits=exits,outputs=output_side,
        is_error=evidence.get('is_error') is True,
        is_mcp=evidence.get('is_mcp') is True,
        terminal_incomplete=terminal_incomplete,
        command_clipped=evidence.get('command_clipped') is True or not input_complete and not input_side.get('commands') and not input_side.get('inputs'),
        input_clipped=not input_complete or evidence.get('input_clipped') is True,
        output_clipped=not output_complete or evidence.get('output_clipped') is True)
    body=encoded(payload)
    if len(body.encode())>MAX_TEXT:
        payload.update(inputs=[],commands=[],cwd=None,tool=None,server=None,
            outputs=[('projection','output','[Payload omitted: projection limit]')],
            input_clipped=True,output_clipped=True)
        body=encoded(payload)
    return body


def activity_evidence(attrs,detail,domain,subject,*,body=None):
    """Qualify legacy native envelopes without treating general bodies as input."""
    evidence=attribute_evidence(attrs)
    qualified=any(evidence[key] for key in ('inputs','commands','exits','outputs'))
    native=attrs.get('observatory.data_source')=='local-session-log'
    current_capture=type(attrs.get('observatory.operation.capture_version')) is int and attrs['observatory.operation.capture_version']==1
    raw_body=body if body is not None else detail
    value=decode(raw_body)
    direction=attrs.get('observatory.mcp.io.direction')
    if native and attrs.get('observatory.item_type')=='CommandExecution' and isinstance(value,dict):
        qualified=True
        if command_present(value.get('command')):
            evidence['commands'].append(('native.command',value['command']))
            evidence['inputs'].append(('native.input',{key:value[key] for key in ('command','cwd') if key in value}))
        if 'cwd' in value:evidence['cwd']=value['cwd']
        if 'exit_code' in value:evidence['exits'].append(('native.exit_code',value['exit_code']))
        if 'output' in value:evidence['outputs'].append(('native.aggregated_output','output',value['output']))
        evidence['tool']=evidence['tool'] or 'CommandExecution'
        # Older native capture silently shortened individual strings. It still
        # supplies useful check details, but cannot prove exact equivalence.
        evidence.update(input_clipped=True,output_clipped=True)
        evidence['command_clipped']=True
    elif native and domain=='mcp' and isinstance(value,dict):
        qualified=True
        if 'arguments' in value:evidence['inputs'].append(('native.arguments',value['arguments']))
        if 'result' in value and not evidence['outputs']:
            output=decode(value['result'])
            evidence['outputs'].append(('native.result','output',output))
            evidence['is_error']=evidence['is_error'] or isinstance(output,dict) and (
                output.get('isError') is True or output.get('is_error') is True)
        if not current_capture:
            evidence.update(input_clipped=True,output_clipped=True)
    elif native and attrs.get('agent.activity.action')=='call' and attrs.get('agent.activity.result')=='started':
        qualified=True
        evidence['inputs'].append(('native.input',value))
        evidence['input_clipped']=not current_capture
    elif domain=='mcp' and direction in {'arguments','result'}:
        qualified=True
        if direction=='arguments' and not evidence['inputs']:
            evidence['inputs'].append(('mcp.body.arguments',value))
        elif direction=='result' and not evidence['outputs']:
            evidence['outputs'].append(('mcp.body.result','output',value))
            evidence['is_error']=evidence['is_error'] or isinstance(value,dict) and (
                value.get('isError') is True or value.get('is_error') is True)
    elif domain=='mcp' and isinstance(raw_body,dict) and raw_body and set(raw_body)<={'arguments','result'}:
        qualified=True
        if 'arguments' in raw_body and not evidence['inputs']:
            evidence['inputs'].append(('mcp.body.arguments',raw_body['arguments']))
        if 'result' in raw_body and not evidence['outputs']:
            output=decode(raw_body['result'])
            evidence['outputs'].append(('mcp.body.result','output',output))
            evidence['is_error']=evidence['is_error'] or isinstance(output,dict) and (
                output.get('isError') is True or output.get('is_error') is True)
    elif detail not in (None,''):
        evidence['outputs'].append(('activity.detail','output',detail))
    qualified_input=any(isinstance(value,dict) and any(command_present(value.get(key))
        for key in ('cmd','command')) for _,value in evidence['inputs'])
    if not evidence['commands'] and not qualified_input and domain=='shell' and command_present(subject):
        evidence['commands'].append(('subject',subject))
    evidence['qualified_detail']=qualified
    return evidence


def normalize_operation_result(row):
    """Index the typed terminal observation; retain differing producer labels."""
    if row.get('detail_role')!='operation_v1':return row
    facts=operation_facts(row)
    original=row.get('result')
    if facts['result_conflict']:result='unknown'
    elif facts['expected_nonzero']:result='informational'
    elif facts['result']=='failed':result=original if original in FAILURES else 'error'
    elif facts['result']=='passed':result='success'
    else:result=original
    if result!=original:row['reported_result']=original;row['result']=result
    return row


def supersedes_operation(original,replacement):
    """A source link alone never authorizes borrowing another worker's facts."""
    if not all(original.get(key) and original.get(key)==replacement.get(key)
               for key in ('service','agent_id','session_id')):return False
    first=operation_facts(original);second=operation_facts(replacement)
    if not original.get('detail_role') and not original.get('detail'):
        operation_domains={'shell','tool','mcp','file','browser'}
        return ((replacement.get('domain') in operation_domains or original.get('domain')==replacement.get('domain'))
                and (first['result']=='unknown' or first['result']==second['result']))
    if not first['operation_fingerprint'] or first['operation_fingerprint']!=second['operation_fingerprint']:
        return False
    if first['exit_code'] is not None and first['exit_code']!=second['exit_code']:return False
    if first['result']!='unknown' and first['result']!=second['result']:return False
    return not first['output'] or (not first['output_clipped'] and not second['output_clipped']
        and first['output_kind']==second['output_kind'] and first['output']==second['output'])


def normalized_evidence(event):
    explicit=event.get('operation_evidence')
    if isinstance(explicit,dict) and type(explicit.get('version')) is int and explicit['version']==VERSION:
        return explicit
    if event.get('detail_role')=='operation_v1':
        detail=decode(event.get('detail'))
        if isinstance(detail,dict) and type(detail.get('version')) is int and detail['version']==VERSION:
            if event.get('detail_clipped'):
                detail.update(input_clipped=True,output_clipped=True)
            return detail
    attrs=event.get('attributes')
    evidence=attribute_evidence(attrs if isinstance(attrs,dict) else {})
    # A shell subject is a recorded command label, not proof of its full inputs.
    qualified_input=any(isinstance(value,dict) and any(command_present(value.get(key))
        for key in ('cmd','command')) for _,value in evidence['inputs'])
    if not evidence['commands'] and not qualified_input and event.get('domain')=='shell' and isinstance(event.get('subject'),str):
        command=event['subject']
        if command_present(command) and command[:MAX_TEXT].casefold() not in GENERIC:
            evidence['commands'].append(('subject',command))
    if 'exit_code' in event:evidence['exits'].append(('event.exit_code',event['exit_code']))
    if event.get('working_directory'):evidence['cwd']=event['working_directory']
    evidence['tool']=event.get('mcp_tool') or evidence['tool']
    evidence['server']=event.get('mcp_server') or evidence['server']
    # An unclassified body, even JSON with command/exit_code keys, is output.
    if event.get('detail') not in (None,''):
        evidence['outputs'].append(('detail','output',event['detail']))
    return evidence


def expected_nonzero(command,code,domain,result,conflict,is_error):
    if code!=1 or domain!='shell' or conflict or is_error or result in {'blocked','denied','policy_denied'}:
        return False
    from .command_evidence import _unwrap
    command=_unwrap(command) if isinstance(command,str) else None
    if not command or any(char in command for char in '&|;`$#\n\r<>'):return False
    try:words=shlex.split(command)
    except ValueError:return False
    if len(words)<2 or '--check' in words:return False
    executable=words[0].rsplit('/',1)[-1]
    return executable in {'grep','rg','diff'} or (
        executable=='git' and words[1:2]==['diff'] and bool({'--quiet','--exit-code'}&set(words)))


def useful_error(value):
    """Flags and empty payloads do not distinguish one observed failure."""
    if isinstance(value,str):
        return bool(value.strip() and value.strip().casefold() not in {'true','false','null','{}','[]'}
                    and not re.fullmatch(r'-?\d+(?:\.\d+)?',value.strip()))
    if isinstance(value,dict):
        return any(useful_error(child) for key,child in value.items()
                   if key not in {'isError','is_error','failed','success','exit_code','exitCode'})
    if isinstance(value,(tuple,list)):return any(useful_error(child) for child in value)
    return False


def operation_facts(event):
    evidence=normalized_evidence(event)
    domain=event.get('domain') if isinstance(event.get('domain'),str) else ''
    def entries(key,size):
        values=evidence.get(key) or []
        if not isinstance(values,(list,tuple)):return [],False
        rows=[tuple(value) for value in values[:8] if isinstance(value,(list,tuple))
              and len(value)==size and isinstance(value[0],str)
              and re.fullmatch(r'[A-Za-z0-9_.: -]{1,128}',value[0])]
        return rows,len(rows)==len(values) and len(values)<=8
    inputs,inputs_valid=entries('inputs',2)
    raw_commands,commands_valid=entries('commands',2)
    command_clipped=any(isinstance(value,str) and len(value)>MAX_TEXT for _,value in raw_commands)
    commands=[(origin,value if len(value)<=MAX_TEXT else '[command omitted: projection limit]')
              for origin,value in raw_commands if command_present(value)]
    commands_valid=commands_valid and len(commands)==len(raw_commands)
    if domain=='shell':
        for origin,value in inputs:
            if isinstance(value,dict):
                for key in ('cmd','command'):
                    if command_present(value.get(key)):
                        if len(value[key])>MAX_TEXT:
                            command_clipped=True;commands.append((origin+'.'+key,'[command omitted: projection limit]'))
                        else:commands.append((origin+'.'+key,value[key]))
    command_values={value for _,value in commands if isinstance(value,str)}
    command=commands[0][1] if commands else None
    input_conflict=len(command_values)>1 or not inputs_valid or not commands_valid
    if len(inputs)>1:
        sanitized=[safe_value(value) for _,value in inputs]
        input_conflict=input_conflict or not all(complete for _,complete in sanitized) or len({digest(value) for value,_ in sanitized})>1
    cwd=evidence.get('cwd')
    directories={value.get(key) for _,value in inputs if isinstance(value,dict)
        for key in ('cwd','workdir','working_directory') if isinstance(value.get(key),str) and value[key]}
    if isinstance(cwd,str) and cwd:directories.add(cwd)
    if len(directories)==1:cwd=next(iter(directories))
    elif len(directories)>1:input_conflict=True
    raw_input={'command':command,'arguments':inputs[0][1] if inputs else None,
        'cwd':cwd,'tool':evidence.get('tool'),'server':evidence.get('server'),
        'domain':domain,'action':event.get('action')}
    safe_input,input_complete=safe_value(raw_input)
    input_complete=bool(input_complete and not input_conflict and not command_clipped and not evidence.get('input_clipped')
        and (inputs or any(origin!='subject' for origin,_ in commands))
        and isinstance(evidence.get('tool'),str) and evidence['tool'].strip()
        and (domain!='shell' or isinstance(cwd,str) and cwd.strip())
        and (not (domain[:64].casefold() in {'mcp','mcptool'} or evidence.get('is_mcp') is True)
             or isinstance(evidence.get('server'),str) and evidence['server'].strip()))
    raw_exits,exits_valid=entries('exits',2)
    exits=[(origin,exit_code(value)) for origin,value in raw_exits]
    malformed_exit=any(value is not None and parsed is None for (_,value),(_,parsed) in zip(raw_exits,exits))
    codes={value for _,value in exits if value is not None}
    code=next(iter(codes)) if len(codes)==1 else None
    reported=event['reported_result'] if 'reported_result' in event else event.get('result') or event.get('status') or ''
    result=reported[:64].casefold() if isinstance(reported,str) else 'unknown'
    if result not in FAILURES|SUCCESSES:result='unknown'
    is_error=evidence.get('is_error') is True
    conflict=(not exits_valid or evidence.get('terminal_incomplete') is True or malformed_exit or len(codes)>1
        or (code==0 and (result in FAILURES or is_error)) or (code not in (None,0) and result in SUCCESSES))
    terminal=('unknown' if conflict else 'failed' if is_error else 'passed' if code==0 else
        'failed' if code is not None else 'passed' if result in SUCCESSES else 'failed' if result in FAILURES else 'unknown')
    outputs=[]
    raw_outputs,outputs_valid=entries('outputs',3)
    decoded_outputs=[(origin,kind,decode(value)) for origin,kind,value in raw_outputs]
    bounded_outputs,output_complete=safe_value(decoded_outputs)
    for row in bounded_outputs:
        if not isinstance(row,(list,tuple)) or len(row)!=3:
            outputs_valid=False;continue
        origin,kind,sanitized=row
        text=sanitized if isinstance(sanitized,str) else json.dumps(sanitized,ensure_ascii=False,sort_keys=True)
        if text:outputs.append((str(origin),str(kind),text,output_complete,useful_error(sanitized)))
    output=next((entry for kind in ('stderr','error','output','stdout') for entry in outputs if entry[1]==kind and entry[4]),None)
    origin,kind,text,complete,_=output or ('','','',output_complete,False)
    complete=complete and outputs_valid and all(entry[3] for entry in outputs) and not evidence.get('output_clipped')
    command_complete=bool(command and commands[0][0]!='subject' and not command_clipped
        and not evidence.get('command_clipped') and safe_value(command)[1] and not input_conflict)
    informational=expected_nonzero(safe_input.get('command'),code,domain,result,conflict,is_error) and command_complete
    display_subject=safe_value(event.get('subject') or '')[0]
    return dict(version=VERSION,command=safe_input.get('command'),display_subject=display_subject,
        command_origin=commands[0][0] if commands else None,
        command_complete=command_complete,
        arguments=safe_input.get('arguments'),cwd=safe_input.get('cwd'),tool=safe_input.get('tool'),server=safe_input.get('server'),
        input_complete=input_complete,input_conflict=input_conflict,
        input_origins=[origin for origin,_ in inputs],exit_code=code,
        exit_sources=[origin for origin,_ in exits],result=terminal,result_conflict=conflict,
        result_basis='invalid recorded exit code' if malformed_exit else 'conflicting recorded results' if conflict else 'recorded exit code' if code is not None else
            'producer result label' if terminal!='unknown' else 'no terminal result',
        output=text[:1500],output_kind=kind or None,output_origin=origin or None,output_reported=bool(raw_outputs),
        output_clipped=bool(outputs and (len(text)>1500 or not complete)) or evidence.get('output_clipped') is True,
        expected_nonzero=bool(informational),
        operation_fingerprint=digest({'version':VERSION,'input':safe_input}) if input_complete else None,
        error_fingerprint=digest({'version':VERSION,'exit':code,
            'result':result if result in {'blocked','denied','policy_denied'} else terminal,'is_error':is_error,
            'outputs':[(kind,value) for _,kind,value,_,_ in outputs]})
            if text and any(entry[4] for entry in outputs) and complete and not conflict else None)


def operation_key(event,*,receiver='',facts=None):
    facts=facts if facts is not None else operation_facts(event)
    fingerprint=facts.get('operation_fingerprint')
    recorded_receiver=event.get('receiver_id') or ''
    if receiver and recorded_receiver and receiver!=recorded_receiver:return None
    receiver=recorded_receiver or receiver
    if not isinstance(receiver,str) or not 0<len(receiver)<=2048:return None
    if not isinstance(fingerprint,str) or not re.fullmatch(r'[0-9a-f]{64}',fingerprint):return None
    if not all(isinstance(event.get(key),str) and 0<len(event[key])<=512
               for key in ('service','agent_id','session_id')):
        return None
    execution=event.get('turn_id') or event.get('trace_id')
    if not isinstance(execution,str) or not 0<len(execution)<=512:return None
    return (receiver,event['service'],event['agent_id'],event['session_id'],
        ('turn',event['turn_id']) if event.get('turn_id') else ('trace',event['trace_id']),VERSION,fingerprint)


def operation_stamp(event):
    stamp=event.get('time_unix_nano')
    return stamp if type(stamp) is int and stamp>0 else None


def operation_metadata(event,*,facts=None):
    facts=correlation_facts(event) if facts is None else facts
    return dict(operation_reference=True,operation_version=VERSION,
        operation_fingerprint=facts['operation_fingerprint'],error_fingerprint=facts['error_fingerprint'],
        operation_result=facts['result'],operation_result_conflict=facts['result_conflict'],
        operation_expected_nonzero=facts['expected_nonzero'])


def correlation_facts(event):
    """Read only server-generated compact metadata, keeping legacy refs unknown."""
    if event.get('operation_reference') is not True:return operation_facts(event)
    valid=type(event.get('operation_version')) is int and event['operation_version']==VERSION
    def fingerprint(key):
        value=event.get(key)
        return value if valid and isinstance(value,str) and re.fullmatch(r'[0-9a-f]{64}',value) else None
    result=event.get('operation_result')
    result=result if valid and isinstance(result,str) and result in {'passed','failed','unknown'} else 'unknown'
    raw=event.get('result')
    conflict=event.get('operation_result_conflict') is True or (result=='passed' and isinstance(raw,str) and raw.casefold() in FAILURES)
    if conflict:result='unknown'
    return dict(version=VERSION,result=result,result_conflict=conflict,
        expected_nonzero=valid and result=='failed' and event.get('operation_expected_nonzero') is True,
        operation_fingerprint=fingerprint('operation_fingerprint'),error_fingerprint=fingerprint('error_fingerprint'))
