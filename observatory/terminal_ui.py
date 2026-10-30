"""Agent-first terminal UI. Raw evidence lives in scrollable inspectors."""
from __future__ import annotations

import asyncio
import hashlib
import json
import os
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path
from urllib.parse import urlencode, urlsplit

from rich.cells import cell_len, get_character_cell_size
from rich.panel import Panel
from rich.segment import Segment
from rich.syntax import Syntax
from rich.table import Table
from rich.text import Text
from textual import on, work
from textual.app import App, ComposeResult, SystemCommand
from textual.binding import Binding
from textual.containers import Grid, Horizontal, Vertical
from textual.screen import Screen
from textual.strip import Strip
from textual.widgets import Button, Checkbox, DataTable, Input, RichLog, Select, Static, TabbedContent, TabPane, Tabs, Tree

from .terminal import clean, clipped, number, trace_name
from .agent_hierarchy import (AGENT_ID, HierarchyGraph, branch_marker, canonical_agent_id,
                              node_active, node_attention, tree_prefix)
from .session_status import session_health
from .branding import NAME, AMBER, LAVENDER, MINT, ICE, masthead
from .themes import PALETTES, supported_themes, themed_style, themed_text
from .ui_layout import column_layout
from .ui_diagnostics import DiagnosticsScreen, FIRST_RUN
from .ui_preferences import Preferences
from .ui_export import ExportScreen
from .ui_render import update_status_text
from .ui_coverage import analysis_coverage


def cell(value, style=''):
    return Text(clean(value), style=style, overflow='ellipsis', no_wrap=True)


def age(stamp):
    if not stamp:
        return 'unknown'
    seconds=max(0,int(time.time()-float(stamp)/1e9))
    return f'{seconds}s ago' if seconds<60 else f'{seconds//60}m ago' if seconds<3600 else f'{seconds//3600}h ago'


def timing(value):
    try:
        if value is None:
            return 'not reported'
        value=float(value)
        return f'{value:.0f} ms' if value<1000 else f'{value/1000:.2f} s' if value<60000 else f'{value/60000:.1f} min'
    except (ValueError,TypeError):
        return 'not reported'


def quantity(value, singular, plural=None):
    return f"{value} {singular if value==1 else (plural or singular+'s')}"


def reported_number(value):
    return 'Not reported' if value is None else number(value)


def observed_count(record, key):
    if record.get(key + '_reported') is False:
        return 'Not reported'
    return reported_number(record.get(key))


def when(stamp):
    value=time.localtime(stamp/1e9)
    return time.strftime('%H:%M:%S' if value[:3]==time.localtime()[:3] else '%m-%d %H:%M',value)


def table_when(stamp):
    value=time.localtime(stamp/1e9)
    return time.strftime('%H:%M:%S' if value[:3]==time.localtime()[:3] else '%m-%d',value)


def metric_value(point):
    data=(point or {}).get('data') or {}
    try:
        if int(data.get('flags',0)) & 1:
            return 'No recorded value'
    except (TypeError,ValueError,OverflowError):
        pass
    if 'value' in data:
        return str(data['value'])
    if 'count' in data:
        return 'count '+str(data['count'])+(' · sum '+str(data['sum']) if 'sum' in data else '')
    if data.get('quantileValues'):
        return ', '.join(f"{item.get('quantile', '?')}: {item.get('value', '?')}"
                         for item in data['quantileValues'][:3])
    return 'distribution'


def usage_series_text(points, service=None, session_id=None):
    """Render token and run history without implying missing usage is zero."""
    points = sorted(points or [], key=lambda item: item.get('start_unix_nano') or 0)[-7:]
    scope = service or 'all services'
    if session_id:
        scope += ' · session ' + session_id[-8:]
    if not points:
        return f"TOKEN USAGE · LAST 7 DAYS · {scope}\nNo usage history recorded in this scope."

    def total(key):
        if not any(point.get(key + '_reported') for point in points):
            return 'Not reported'
        return number(sum(int(point.get(key) or 0) for point in points))

    lines = [
        f"TOKEN USAGE · LAST 7 DAYS · {scope}",
        f"Input {total('input_tokens')}  ·  Output {total('output_tokens')}  ·  "
        f"Runs {sum(int(point.get('runs') or 0) for point in points)}  ·  "
        f"Errors {sum(int(point.get('errors') or 0) + int(point.get('log_errors') or 0) for point in points)}",
        f"Cache read {total('cache_read_input_tokens')} · write {total('cache_write_input_tokens')} · "
        f"reasoning {total('reasoning_output_tokens')}",
        'Cached input is included in input tokens. Daily values use local dates.',
        'DATE       INPUT       OUTPUT      RUNS  ERRORS  LOGS',
    ]
    for point in points:
        stamp = point.get('start_unix_nano')
        day = time.strftime('%b %d', time.localtime(stamp / 1e9)) if stamp else 'Unknown'
        input_value = number(point.get('input_tokens') or 0) if point.get('input_tokens_reported') else '—'
        output_value = number(point.get('output_tokens') or 0) if point.get('output_tokens_reported') else '—'
        lines.append(f"{day:<10} {input_value:>10}  {output_value:>10}  "
                     f"{int(point.get('runs') or 0):>4}  "
                     f"{int(point.get('errors') or 0) + int(point.get('log_errors') or 0):>6}  "
                     f"{int(point.get('log_events') or 0):>4}")
    return '\n'.join(lines)


def status(value):
    label={'OK':'OK','UNSET':'Observed','RUNNING':'In progress','ERROR':'Error','FAILED':'Failed','INCOMPLETE':'Incomplete',
           'success':'OK','started':'Started','completed':'Completed','observed':'Observed','reported':'Reported','error':'Error'}.get(str(value),str(value).title())
    color='#ff8799' if str(value).lower() in {'error','failed','denied','blocked','incomplete'} else '#86e3b5' if str(value).lower() in {'success','ok','completed'} else '#ffbd69' if str(value).lower() in {'running','started'} else '#b9afc5'
    return cell(label,color)


def decode(value):
    # Local logs often wrap the same event in several JSON-encoded transport
    # envelopes. Unwrap those so newlines and typed text blocks reach the UI.
    for _ in range(24):
        if not isinstance(value,str):
            break
        try:
            parsed=json.loads(value)
        except (ValueError,TypeError):
            break
        if isinstance(parsed,(dict,list)):
            value=parsed
        elif isinstance(parsed,str) and parsed.lstrip().startswith(('{','[')):
            # Some event producers JSON-encode the payload, then JSON-encode
            # that encoded string again inside their text block.
            value=parsed
        else:
            break
    return value


def decode_nested(value,depth=0):
    """Decode JSON strings inside arbitrarily nested telemetry fields."""
    value=decode(value)
    if depth>=64:return value
    if isinstance(value,dict):
        return {key:decode_nested(child,depth+1) for key,child in value.items()}
    if isinstance(value,list):
        return [decode_nested(child,depth+1) for child in value]
    return value


def failure_evidence(event):
    """Build a short, human-readable reason from a recorded failed event."""
    if not event:
        return ''
    details=decode_nested(event.get('detail') or {})
    keys=('exception.message','error.message','tool.error_message','tool.error',
          'exception.type','error.type','message','error','reason','stderr')
    candidates=[]
    def collect(value):
        if isinstance(value,dict):
            for key in keys:
                child=value.get(key)
                if isinstance(child,(str,int,float)) and str(child).strip():
                    candidates.append(str(child).strip())
            for child in value.values():
                collect(child)
        elif isinstance(value,list):
            for child in value:
                collect(child)
    collect(details)
    result=str(event.get('result') or event.get('status') or 'failed').replace('_',' ')
    action=' '.join(str(event.get(key) or '').strip() for key in ('domain','action','subject')).strip()
    prefix=f'{action} · {result}' if action else result
    reason=next((text for text in candidates if text.casefold() not in {result.casefold(), 'error', 'failed'}), '')
    return f'{prefix}: {reason}' if reason else f'{prefix}. Open the failed event for its recorded details.'


def shell_command(value):
    """Remove the login-shell wrapper from commands captured from a PTY."""
    command=str(value).strip()
    try:
        args=shlex.split(command)
    except ValueError:
        return command
    if len(args)>=3 and Path(args[0]).name in {'sh','bash','zsh','fish'} and args[1] in {'-c','-lc','-ilc'}:
        return args[2]
    return command


def heredoc_script(command):
    """Return the language and body for a captured interpreter heredoc."""
    lines=command.splitlines()
    if len(lines)<3:
        return None
    try:
        header=shlex.split(lines[0])
    except ValueError:
        return None
    interpreter=Path(header[0]).name.lower() if header else ''
    delimiter_arg=next((part for part in header if part.startswith('<<')),None)
    if not delimiter_arg:
        return None
    delimiter=delimiter_arg[2:].strip("'\"")
    if not delimiter or lines[-1].strip()!=delimiter:
        return None
    if interpreter.startswith('python'):
        language,title='python','Python script'
    elif interpreter in {'node','nodejs','deno','bun'}:
        language,title='javascript','JavaScript script'
    elif interpreter in {'bash','sh','zsh','fish'}:
        language,title='bash','Shell script'
    else:
        language,title='text',f'{interpreter or "Interpreter"} script'
    return language,title,'\n'.join(lines[1:-1])


def embedded_json(text):
    """Split prose from complete JSON objects embedded in a text record."""
    decoder=json.JSONDecoder()
    segments=[]
    cursor=0
    index=0
    while index<len(text):
        if text[index] not in '{[':
            index+=1
            continue
        try:
            value,end=decoder.raw_decode(text,index)
        except (ValueError,TypeError):
            index+=1
            continue
        if not isinstance(value,(dict,list)) or end<=cursor:
            index+=1
            continue
        if text[cursor:index]:segments.append(('text',text[cursor:index]))
        segments.append(('json',value))
        cursor=end
        index=end
    if not segments:return [('text',text)]
    if text[cursor:]:segments.append(('text',text[cursor:]))
    return segments


def format_json(value):
    """Return indented JSON and multiline string fields for readable display."""
    text_values=[]
    def prepare(item,path=''):
        if isinstance(item,dict):
            return {key:prepare(child,f'{path}.{key}' if path else str(key)) for key,child in item.items()}
        if isinstance(item,list):
            return [prepare(child,f'{path}[{index}]') for index,child in enumerate(item)]
        if isinstance(item,str) and '\n' in item:
            text_values.append((path or 'Text value',item))
            line_count=len(item.splitlines())
            return f'[multiline text · {line_count} lines shown below]'
        return item

    return json.dumps(prepare(value),ensure_ascii=False,indent=2),text_values


def write_json(log,value,theme='Current'):
    """Render indented JSON and move multiline string values into readable blocks."""
    formatted,text_values=format_json(value)
    log.write(evidence_syntax(formatted,'json',theme))
    for path,content in text_values:
        log.write(Text(f'\n{path} · {len(content.splitlines())} lines',style='bold #c5adff'))
        log.write(Text(content))


def copy_event_text(item):
    """Build a readable plain-text export of one selected activity or MCP event."""
    is_mcp=bool(item.get('server') or item.get('tool') or item.get('domain')=='mcp')
    title=f"{item.get('server') or 'MCP'} / {item.get('tool') or 'tool'}" if is_mcp else headline(item)
    lines=[title,f"Agent: {item.get('service') or 'Not reported'}",
           f"Type: {item.get('domain') or ('mcp' if is_mcp else 'Not reported')}",
           f"Result: {item.get('result') or item.get('status') or 'Not reported'}",
           f"When: {when(item['time_unix_nano']) if item.get('time_unix_nano') else 'Not reported'}",
           f"Duration: {timing(item.get('duration_ms'))}"]
    if item.get('trace_id'):lines.append(f"Trace ID: {item['trace_id']}")
    if item.get('span_id'):lines.append(f"Span ID: {item['span_id']}")
    lines.append('\nRecorded content')
    def append_json(value):
        formatted,text_values=format_json(value)
        lines.append(formatted)
        for path,content in text_values:lines.extend((f'\n{path}',content))
    if is_mcp:
        details={key:item.get(key) for key in ('server','tool','method','arguments','output') if item.get(key) is not None}
    else:
        details=decode_nested(item.get('detail') or '')
    if isinstance(details,(dict,list)):
        append_json(details)
    elif isinstance(details,str):
        for kind,content in embedded_json(details):
            if kind=='json':append_json(content)
            elif content.strip():lines.append(content.strip('\n'))
    elif details:
        lines.append(str(details))
    return '\n'.join(lines)


async def write_system_clipboard(text):
    """Write to the desktop clipboard when a native clipboard tool is available."""
    candidates=[]
    if sys.platform=='darwin' and shutil.which('pbcopy'):
        candidates.append((['pbcopy'],'macOS'))
    elif os.name=='nt' and shutil.which('clip.exe'):
        candidates.append((['clip.exe'],'Windows'))
    elif sys.platform.startswith('linux'):
        if os.environ.get('WAYLAND_DISPLAY') and shutil.which('wl-copy'):
            candidates.append((['wl-copy'],'Wayland'))
        if os.environ.get('DISPLAY') and shutil.which('xclip'):
            candidates.append((['xclip','-selection','clipboard','-i'],'X11'))
        if os.environ.get('DISPLAY') and shutil.which('xsel'):
            candidates.append((['xsel','--clipboard','--input'],'X11'))
    for command,label in candidates:
        try:
            # Clipboard owners such as xclip fork a background process. Captured
            # pipes can stay open in that child even after the command succeeds.
            process=await asyncio.create_subprocess_exec(*command,stdin=subprocess.PIPE,
                stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        except OSError:
            continue
        try:
            await asyncio.wait_for(process.communicate(text.encode('utf-8')),timeout=3)
        except (asyncio.TimeoutError,asyncio.CancelledError) as exc:
            if process.returncode is None:
                try:process.kill()
                except ProcessLookupError:pass
            await process.wait()
            if isinstance(exc,asyncio.CancelledError):raise
            continue
        if process.returncode==0:
            return label
    return None


def headline(item):
    domain=item.get('domain') or 'tool'
    subject=str(item.get('subject') or item.get('action') or 'Observed activity')
    if domain=='tool' and subject.casefold() in {'exec','execute','shell'}:
        details=decode_nested(item.get('detail') or '')
        def find_command(value):
            if isinstance(value,dict):
                for key,child in value.items():
                    if str(key).casefold() in {'command','cmd'} and isinstance(child,str):
                        return child
                for child in value.values():
                    found=find_command(child)
                    if found:return found
            elif isinstance(value,list):
                for child in value:
                    found=find_command(child)
                    if found:return found
            return None
        command=find_command(details)
        if command:
            command=shell_command(command)
            script=heredoc_script(command)
            if script:return script[1]
            first=command.splitlines()[0] if command.splitlines() else command
            return clean(first)
    if domain=='shell':
        try:
            args=shlex.split(subject)
            if len(args)>=3 and args[1] in {'-lc','-c','-ilc'}:
                subject=args[2]
        except ValueError:
            pass
        first=subject.splitlines()[0] if subject.splitlines() else 'Shell command'
        if ' - <<' in first and first.startswith(('python','python3')):
            return 'Run Python script'
        return clean(first)
    if domain=='file':
        return f"{str(item.get('action') or 'Change').title()} {Path(subject).name}"
    if domain=='mcp':
        return f"{item.get('mcp_server') or subject} / {item.get('mcp_tool') or item.get('action')}"
    if domain=='message':
        return ('User: ' if item.get('action')=='user' else 'Agent: ')+subject
    if domain=='model':
        return 'Model usage · '+subject
    if domain=='agent':
        return {'processing':'Model processing','started':'Turn started','completed':'Turn completed','interrupted':'Turn interrupted','compaction':'Context compacted'}.get(item.get('action'),subject)
    return subject if item.get('action') in {'call','result','execute_tool'} else f"{item.get('action') or domain} · {subject}"


def recorded_message_text(event):
    """Extract text already present in a recorded user-message event."""
    value=decode_nested(event.get('detail') or event.get('subject') or '')
    if isinstance(value,dict):
        value=next((value[key] for key in ('text','content','message','prompt')
                    if isinstance(value.get(key),str)),event.get('subject') or '')
    if isinstance(value,(dict,list)):
        value=json.dumps(value,ensure_ascii=False,indent=2)
    text=str(value or event.get('subject') or 'Recorded request text unavailable')
    # Preserve meaningful line breaks while removing terminal control codes.
    return '\n'.join(clean(line) for line in text.replace('\r\n','\n').split('\n')).strip()


def review_session(entry):
    """Recognize internal reviews in native, connection, and stored summaries."""
    services=[entry.get('service'),*(entry.get('services') or [])]
    models=[entry.get('model'),*(entry.get('models') or [])]
    return (bool(entry.get('review')) or 'codex-review' in services or
            'codex-auto-review' in models)


def sources(snapshot, include_reviews=False):
    connections={item['source_id']:item for item in snapshot.agents}
    result=[]
    for entry in snapshot.local_status.get('sessions',[]):
        connection=connections.pop(entry['source_id'],{})
        review=review_session(entry)
        if review and not include_reviews:
            continue
        if entry.get('discovery_state')=='announced':
            health={'state':'Spawn recorded','active':False,'recent':False,
                    'stamp':entry.get('last_record_ns') or 0,
                    'status_evidence':'Spawn recorded · status unavailable'}
        else:
            health=session_health(entry,time.time_ns())
        result.append({**entry,**health,'id':entry['source_id'],'review':review,
                       'spans':connection.get('span_count',0)})
    for entry in connections.values():
        review=review_session(entry)
        if review and not include_reviews:
            continue
        if 'local-session-log' in entry.get('integrations',[]):
            continue
        result.append({**entry,'id':entry['source_id'],'session_id':None,'project':'SDK / OTLP','model':'',
                       'runtime':entry.get('service'),'task':'Task not reported','recent':entry['state']=='running',
                       'state':'Connected' if entry['state']=='running' else 'Finished' if entry['state']=='stopped' else 'Offline',
                       'stamp':entry.get('last_seen_ns',0),'spans':entry.get('span_count',0),'review':review,
                       'discovery_state':'observed','coverage':'unknown',
                       'relationship_kind':'unknown','relationship_state':'unresolved',
                       'relationship_provenance':'SDK/OTLP producer did not report agent hierarchy'})
    known_sessions={item.get('session_id') for item in result if item.get('session_id')}
    known_sessions.update(item.get('session_id') for item in snapshot.local_status.get('sessions',[]))
    known_agent_ids={item.get('agent_id') for item in result if item.get('agent_id')}
    known_agent_ids.update(item.get('agent_id') for item in snapshot.local_status.get('sessions',[]))
    latest_by_session={}
    first_prompt_by_session={}
    for event in snapshot.activity:
        session_id=event.get('session_id')
        previous=latest_by_session.get(session_id)
        if previous is None or event.get('time_unix_nano',0)>previous.get('time_unix_nano',0):
            latest_by_session[session_id]=event
        if event.get('domain')=='message' and event.get('action')=='user' and session_id:
            prompt=event.get('subject') or event.get('detail') or ''
            if str(prompt).strip():
                previous_prompt=first_prompt_by_session.get(session_id)
                if previous_prompt is None or event.get('time_unix_nano',0)<previous_prompt[0]:
                    first_prompt_by_session[session_id]=(event.get('time_unix_nano',0),clean(str(prompt)))
    # A task description supplied by the runtime is best. If it has none,
    # derive identity only from an explicitly recorded first user message.
    for item in result:
        prompt=first_prompt_by_session.get(item.get('session_id'))
        if prompt and not task_preview(item):
            item['task']=prompt[1]
            item['task_source']='recorded user request'
    for session in getattr(snapshot,'sessions',[]):
        session_id=session.get('session_id')
        if not session_id:
            continue
        stored_agent_id=session.get('agent_id')
        if stored_agent_id and stored_agent_id in known_agent_ids:
            continue
        if session_id in known_sessions and not stored_agent_id:
            continue
        review=review_session(session)
        if review and not include_reviews:
            continue
        known_sessions.add(session_id)
        if stored_agent_id:
            known_agent_ids.add(stored_agent_id)
        services=session.get('services') or []
        service=services[0] if len(services)==1 else ''
        stamp=session.get('last_unix_nano') or 0
        reported=session.get('latest_status','UNKNOWN')
        waiting_for=session.get('waiting_for') or ''
        health=session_health({'run_state':'completed' if reported=='OK' else reported.casefold(),
            'waiting_for':waiting_for,'last_record_ns':stamp},time.time_ns())
        latest_activity=latest_by_session.get(session_id)
        runtime='Gemini CLI' if any('gemini' in name.casefold() for name in services) else 'OpenTelemetry'
        result.append({'id':'otlp-session:'+str(session_id),'source_id':'otlp-session:'+str(session_id),
            'session_id':session_id,'runtime':runtime,'service':service,'project':session.get('project') or '',
            'working_directory':'','branch':'','model':', '.join(session.get('models') or []),
            'task':session.get('latest_task') or first_prompt_by_session.get(session_id,(0,''))[1],
            'task_source':'recorded user request' if not session.get('latest_task') and first_prompt_by_session.get(session_id) else '',
             'run_state':reported.casefold(),
             'waiting_for':waiting_for,'last_activity':latest_activity,'capture_content':False,
             **health,'spans':session.get('span_count',0),
             'trace_count':session.get('trace_count',0),'errors':session.get('errors',0),
             'failed':session.get('failed',0),'incomplete':session.get('incomplete',0),
             'latest_trace_id':session.get('latest_trace_id'),
             'status_complete':session.get('status_complete',True),'review':review,
             'source_namespace':session.get('agent_namespace') or service or runtime,
             'native_session_id':session_id,'native_worker_id':session.get('agent_worker_id'),
             'relationship_kind':session.get('relationship_kind') or 'unknown',
             'relationship_state':session.get('relationship_state') or 'unresolved',
             'relationship_provenance':session.get('relationship_provenance') or
                 'SDK/OTLP session has no explicit parent metadata',
             'discovery_state':'observed','coverage':session.get('coverage') or 'unknown',
             'agent_id':session.get('agent_id'),
             'parent_agent_id':session.get('parent_agent_id'),
             'agent_native_id':session.get('agent_native_id'),
             'parent_agent_native_id':session.get('parent_agent_native_id')})
    for item in result:
        runtime=item.get('runtime') or item.get('service') or 'Agent'
        namespace=item.get('source_namespace') or item.get('source_id') or item.get('service') or runtime
        if not item.get('agent_id') or not AGENT_ID.fullmatch(str(item.get('agent_id'))):
            native=(item.get('agent_native_id') or item.get('native_worker_id') or
                    item.get('session_id') or item.get('id'))
            if native:
                item['agent_id']=(str(native).lower() if AGENT_ID.fullmatch(str(native))
                                  else canonical_agent_id(str(runtime),str(namespace),str(native)))
        explicit_parent=item.get('parent_agent_native_id')
        if explicit_parent and not item.get('parent_agent_id'):
            try:
                item['parent_agent_id']=(str(explicit_parent).lower()
                    if AGENT_ID.fullmatch(str(explicit_parent)) else
                    canonical_agent_id(str(runtime),str(namespace),str(explicit_parent)))
            except ValueError:
                item['parent_agent_id']=None
            if item.get('parent_agent_id') and item.get('relationship_state')=='unresolved':
                item['relationship_kind']='delegated'
                item['relationship_state']='confirmed'
                item['relationship_provenance']='SDK/OTLP explicit agent.parent_id'
        item.setdefault('relationship_kind','unknown')
        item.setdefault('relationship_state','unresolved')
        item.setdefault('relationship_provenance','No parent relationship was reported')
        item.setdefault('discovery_state','observed')
        item.setdefault('coverage','unknown')
        item.setdefault('source_namespace',str(namespace))
        item.setdefault('native_session_id',item.get('session_id'))
        item['attention']=attention_required(item)
        item['owns_attention']=attention_required(item)
    return sorted(result,key=lambda item:(not (item.get('active') or item.get('state')=='Waiting'),not item['recent'],-item['stamp'],item['id']))


def wait_summary(item):
    """Describe a recorded wait without implying this read-only monitor can answer it."""
    reason=str(item.get('waiting_for') or '').strip()
    folded=reason.casefold()
    runtime=item.get('runtime') or 'agent'
    if 'approval' in folded or 'permission' in folded:
        return f'Approval needed · review it in the {runtime} terminal; Goechala is read-only.'
    if 'user input' in folded or 'input request' in folded:
        return f'Your reply is needed · answer in the {runtime} terminal; Goechala is read-only.'
    if reason.lower().startswith('tool response:'):
        return f'{reason} · tool response is pending; no user action was reported.'
    if item.get('state')=='Waiting':
        return 'Waiting · reason not reported; check the agent terminal.'
    if item.get('state') in {'Completed'}:
        return 'The agent reported completion.'
    if item.get('state') in {'Error','Failed','Incomplete'}:
        return 'Review the recorded error in Activity or Traces.'
    if item.get('state') in {'Possibly stalled','Unconfirmed','Monitor delayed','Catching up'}:
        return item.get('status_evidence') or 'Check the agent terminal and collection diagnostics.'
    if item.get('active'):
        return 'The turn is still in progress; no user action was requested.'
    return 'Remaining work is not reported by the agent.'


def attention_evidence(item,summary=None,error_event=None):
    """Explain whether an attention state is backed by workload or collector evidence."""
    if error_event:
        return 'Agent/tool activity failed: '+failure_evidence(error_event)
    if item.get('state') in {'Error','Failed','Incomplete'}:
        aggregate=summary if isinstance(summary,dict) and summary else item
        failures=sum(int(aggregate.get(key) or 0) for key in ('errors','failed','incomplete'))
        traces=int(aggregate.get('trace_count') or item.get('trace_count') or 0)
        trace_id=aggregate.get('latest_trace_id') or item.get('latest_trace_id')
        if failures:
            evidence=(f'Telemetry recorded {failures} failed run(s) across {traces} trace(s); '
                      'this is run-level evidence, not proof of a collector failure or a specific tool failure.')
            if trace_id:
                evidence+=f' Latest trace: {trace_id}.'
            return evidence+' Open Traces to inspect spans; check Diagnostics/Host for collection health.'
        return ('The lifecycle reported '+item['state'].lower()+', but no error message or failed action was captured. '
                'The cause is unknown: inspect Traces and check Diagnostics/Host to distinguish agent/tool failure from missing collection.')
    return item.get('status_evidence') or 'Status comes from recorded lifecycle events.'


def recorded_progress(item, events, totals):
    """Render a fast, zero-setup progress summary from observed records only."""
    recent=sorted(events,key=lambda event:event.get('time_unix_nano',0),reverse=True)
    latest=recent[0] if recent else item.get('last_activity') or {}
    errors=totals.get('errors') if isinstance(totals,dict) else None
    if isinstance(errors,bool) or not isinstance(errors,(int,float)):
        errors=None
    if errors is None:
        errors=sum(str(event.get('result') or '').casefold() in
                   {'error','failed','denied','blocked','policy_denied','incomplete'}
                   for event in recent)
        error_note=f'{errors} recorded in captured recent activity'
    else:
        error_note=f'{int(errors)} recorded across observed runs'
    latest_note=clean(headline(latest))[:120] if latest else 'No activity captured yet.'
    return (f"Current: {item.get('state') or 'Unknown'}\n"
            f"Latest: {latest_note}\n"
            f"Errors: {error_note}\n"
            f"Next: {wait_summary(item)}")


def wait_label(item):
    """Use compact but actionable wording in the session list."""
    reason=str(item.get('waiting_for') or '').strip()
    folded=reason.casefold()
    runtime=item.get('runtime') or 'agent'
    if 'approval' in folded or 'permission' in folded:
        return f'Approval · {runtime} terminal'
    if 'user input' in folded or 'input request' in folded:
        return f'Reply · {runtime} terminal'
    return reason or '—'


def task_preview(item):
    task=clean(' '.join(str(item.get('task') or '').split()))
    if task.startswith(('<send_user_message_question_reply>','[{"questionItemId"','{"questionItemId"')):
        return 'Reply to input request'
    return task if task!='Task not reported' else ''


def brief_activity(item):
    """A bounded action label, never a command's arguments or its output."""
    domain=item.get('domain') or 'agent'
    subject=clean(str(item.get('subject') or ''))
    if domain=='shell':
        try:
            args=shlex.split(subject[:1000])
            if len(args)>=3 and args[1] in {'-c','-lc','-ilc'}:
                args=shlex.split(args[2])
            executable=Path(args[0]).name if args else ''
        except ValueError:
            executable=subject.split(maxsplit=1)[0] if subject else ''
        label='Shell command'+(' · '+executable if executable else '')
    elif domain=='message':
        label='User message' if item.get('action')=='user' else 'Agent message'
    elif domain=='model':
        label='Model call'+(' · '+subject if subject else '')
    elif domain=='file':
        label=str(item.get('action') or 'File').title()+' · '+Path(subject).name
    elif domain=='mcp':
        label='MCP · '+str(item.get('mcp_server') or subject)+' / '+str(item.get('mcp_tool') or item.get('action') or 'call')
    elif domain=='tool':
        label='Tool · '+str(item.get('tool_name') or subject or 'call')
    else:
        label=str(item.get('action') or 'Session update').replace('_',' ').capitalize()
    return clean(' '.join(label.split()))[:76]


def task_text(item):
    """Keep real tasks prominent and missing descriptions quiet."""
    task=task_preview(item)
    if not task:
        return Text('—',style='italic #b9afc5',no_wrap=True)
    return Text(task,style='#f6f0e8' if item.get('active') or input_required(item) else '#e1d9eb',no_wrap=True,overflow='ellipsis')


def agent_identity(item,selected=False):
    """A visible keyboard cursor without turning every row into a bright badge."""
    text=Text('▸ ' if selected else '  ',style='#ffbd69',no_wrap=True,overflow='ellipsis')
    text.append(clean(item['runtime']),style='bold #f6f0e8' if item.get('active') or input_required(item) else '#e1d9eb')
    text.append('\n  '+clean(item.get('project') or '—'),style='#b9afc5')
    return text


def task_metadata(item, summary=None, *, announced=False, unlinked=False, depth=0,
                  parent_unavailable=False, parent_retained=False):
    runtime=clean(item.get('runtime') or item.get('service') or 'Agent')
    project=clean(item.get('project') or 'SDK')
    action=brief_activity(item.get('last_activity') or {}) if item.get('last_activity') else 'No recent action recorded'
    if announced:
        return f"{runtime} · Spawn recorded · status unavailable"
    parts=[runtime,project,action,age(item.get('stamp'))]
    if summary and summary.get('descendants'):
        count=summary.get('descendants',0)
        parts.insert(2,f"{quantity(count,'subagent')}")
        if summary.get('attention'):
            parts.insert(3,f"{summary['attention']} needs attention")
        elif summary.get('working'):
            parts.insert(3,f"{summary['working']} working")
        if summary.get('announced'):
            parts.insert(3,f"{summary['announced']} spawn recorded")
    elif parent_retained:
        parts.insert(2,'parent outside window')
    elif parent_unavailable:
        parts.insert(2,'parent unavailable')
    elif unlinked:
        parts.insert(2,'parent not reported')
    if depth>8:
        parts.append(f"depth {depth}")
    return ' · '.join(parts)


def agent_list_text(item,selected=False,*,prefix='',summary=None,announced=False,
                    unlinked=False,depth=0,ascii_only=False,parent_unavailable=False,
                    parent_retained=False):
    """Two-line task-first row: task first, concise runtime/action provenance below."""
    title=task_preview(item) or (f"Spawn recorded · {item.get('runtime') or 'agent'}" if announced
                                 else f"Untitled session · {item.get('runtime') or 'agent'}")
    title=' '.join(title.split())
    marker=branch_marker(bool(item.get('_expanded')),bool(item.get('_has_children')),ascii_only=ascii_only)
    metadata=task_metadata(item,summary,announced=announced,unlinked=unlinked,depth=depth,
                           parent_unavailable=parent_unavailable,
                           parent_retained=parent_retained)
    text=Text('▸ ' if selected else '  ',style='#dceb5a',no_wrap=True,overflow='ellipsis')
    text.append(prefix+marker,style='#abb4a5')
    text.append(title,style='bold #edf1e7' if item.get('active') or input_required(item) else '#dedce5')
    text.append('\n  '+metadata,style='#abb4a5')
    return text


def agent_status(item):
    """Status uses shape and wording as well as color; history stays subdued."""
    state=item.get('state')
    if input_required(item):
        marker,style='!','bold #ff8799'
    elif state in {'Error','Failed'}:
        marker,style='×','#ff8799'
    elif state in {'Waiting','Possibly stalled','Monitor delayed','Catching up','Unconfirmed'}:
        marker,style='!','#ffbd69'
    elif item.get('active'):
        marker,style=('◌' if state=='Working (quiet)' else '●'),'#86e3b5'
    else:
        marker,style=('✓' if state=='Completed' else '·'),'#b9afc5'
    return cell(marker+' '+input_status(item),style)


def input_status(item):
    """Name only user actions explicitly reported by a session."""
    if not input_required(item):
        state=item.get('state') or 'Unknown'
        return {'Working (quiet)':'Quiet work','Possibly stalled':'May be stalled'}.get(state,state)
    reason=str(item.get('waiting_for') or '').casefold()
    return 'NEEDS APPROVAL' if 'approval' in reason or 'permission' in reason else 'NEEDS REPLY'


def input_required(item):
    reason=str(item.get('waiting_for') or '').casefold()
    return item.get('state')=='Waiting' and any(
        marker in reason for marker in ('approval','permission','user input','input request'))


def attention_required(item):
    """Share the filter/header rule: explicit waits, possible stalls and failures."""
    return input_required(item) or item.get('state') in {
        'Possibly stalled','Error','Failed','Incomplete'
    }


def attention_key(item):
    """Hash the local identity of one failure instance for privacy-safe dismissal."""
    identity='\0'.join(str(item.get(key) or '') for key in
                        ('id','state','latest_trace_id','stamp'))
    return hashlib.sha256(identity.encode('utf-8')).hexdigest()


# Lifecycle outcomes that are finished.  These always sort below live work,
# even when they are newer than a working session.
TERMINATED_STATES = {'Completed','Interrupted','Error','Failed','Incomplete','Finished','Offline'}
WORKING_STATES = {'Working','Processing','Tool running','Working (quiet)'}


def agent_status_group(item):
    """Fixed sort groups: working, needs-action, other, terminated.

    The group order is deliberately independent of the selected updated-time
    direction.  A sort control can change order inside a group; it cannot put a
    terminated session above a working one.
    """
    state=item.get('state')
    if item.get('active') or state in WORKING_STATES:
        return 0
    if state in TERMINATED_STATES:
        return 3
    if attention_required(item):
        return 1
    return 2


def agent_order_key(item, newest_first=True):
    """Sort by updated time inside a fixed status group."""
    stamp=int(item.get('stamp') or 0)
    return (agent_status_group(item), -stamp if newest_first else stamp,
            str(item.get('agent_id') or item.get('id') or ''))


def navigation_hints(width):
    if width >= 120:
        return '↑/↓ Move  Enter Inspect  / Search  e Expand  ? Help  Ctrl+P Palette  d Diagnostics  q Quit'
    if width >= 80:
        return '↑/↓ Move  Enter Inspect  / Search  e Expand  ? Help  q Quit'
    return '↑↓ Move  Enter Inspect  / Search  ? Help'


def coverage_warning(local_status):
    """Report discovery caps so a partial session list is not mistaken for all agents."""
    omitted=[]
    if local_status.get('omitted_sources'):
        omitted.append(f"{local_status['omitted_sources']} older Codex/Claude sessions beyond the {local_status.get('max_sources',50)}-session limit")
    if local_status.get('opencode_omitted_sessions'):
        omitted.append(f"{local_status['opencode_omitted_sessions']} older OpenCode sessions beyond the 50-session limit")
    hierarchy=local_status.get('hierarchy') or {}
    if hierarchy.get('coverage')=='partial':
        omitted.append(f"{hierarchy.get('announced',0)} spawn announcements or partial child evidence; relationship coverage is incomplete")
    return 'Coverage limited: '+'; '.join(omitted) if omitted else ''


class ResultsTable(DataTable):
    """Size columns to the panel while keeping the selected record on resize."""
    BINDINGS=[Binding('right','tree_right','Expand branch',show=False),
              Binding('left','tree_left','Collapse branch',show=False)]

    def action_tree_right(self):
        if self.id=='agent-table' and getattr(self.app,'agent_tree_active',False):
            self.app.action_tree_right()
        else:
            self.action_cursor_right()

    def action_tree_left(self):
        if self.id=='agent-table' and getattr(self.app,'agent_tree_active',False):
            self.app.action_tree_left()
        else:
            self.action_cursor_left()

    async def _on_click(self,event):
        if self.id=='agent-table' and event.button==1:
            meta=event.style.meta
            if 'row' in meta and 'column' in meta and meta.get('row',-1)>=0 and meta.get('column')==0:
                # Click coordinates are relative to this widget, not screen cells.
                relative=event.x
                if 0<=relative<=5:
                    try:
                        source_id=self.ordered_rows[meta['row']].key.value
                    except (IndexError,KeyError):
                        source_id=None
                    if source_id:
                        self.app.toggle_agent_branch(source_id)
                        event.prevent_default()
                        event.stop()
                        return
        await super()._on_click(event)

    def preview_height(self,values):
        if self.id!='agent-table' or not self.column_widths or not values:
            return 1
        if len(values)>=3:
            # Leave one blank terminal row between task-first session records.
            return 3
        task=values[-1]
        return min(2,max(1,len(task.wrap(self.app.console,self.column_widths[-1]))))

    def add_row(self,*cells,**kwargs):
        kwargs.setdefault('height',self.preview_height(cells))
        key=super().add_row(*self.project(cells),**kwargs)
        self.full_rows[key]=cells
        return key

    def project(self,values):
        return [values[index] for index in self.visible_columns]

    def clear(self,columns=False):
        self.full_rows={}
        return super().clear(columns=columns)

    def configure(self, columns):
        self.column_specs=columns
        self.column_widths=()
        self.visible_columns=tuple(range(len(columns)))
        self.full_rows={}
        self.fit_columns()

    def on_resize(self):
        self.fit_columns()

    def fit_columns(self):
        specs=getattr(self,'column_specs',None)
        if not specs:return
        visible,widths=column_layout(self.id,specs,self.scrollable_content_region.width)
        if widths==self.column_widths and visible==self.visible_columns:return
        rows=list(self.full_rows.items())
        selected=self.cursor_row
        scroll=self.scroll_offset
        self.clear(columns=True)
        for index,width in zip(visible,widths):
            self.add_column(specs[index][0],width=width,key=str(index))
        # Row previews must wrap against the new width, not the previous layout.
        self.column_widths=widths
        self.visible_columns=visible
        for key,values in rows:self.add_row(*values,key=key)
        if rows:self.move_cursor(row=min(selected,len(rows)-1),scroll=False)
        self.scroll_to(x=0,y=scroll.y,animate=False)


class SearchInput(Input):
    """Use familiar select-all behavior in cockpit search fields."""
    BINDINGS=[Binding('ctrl+a','select_all','Select all',show=False)]


class CockpitTabs(TabbedContent):
    """Keep secondary-view navigation on the same row as the primary tabs."""
    def compose(self)->ComposeResult:
        for widget in super().compose():
            if isinstance(widget,Tabs):
                widget.border_title='Views · 1–7 to switch'
                yield widget
                yield self.app.more_view_control()
                yield self.app.scope_controls()
            else:
                yield widget


def evidence_syntax(content, lexer, theme='Current'):
    return Syntax(content, lexer, word_wrap=True,
                  theme='friendly' if theme == 'Bright' else 'monokai',
                  background_color=PALETTES[theme]['surface'])


class SelectableRichLog(RichLog):
    """Selectable evidence with visible highlighting and plain-text extraction."""
    def on_show(self):
        # Hidden widgets have no content region, but keep their previous size
        # internally. Revealing one at the same width may not trigger reflow
        # through our width-change guard below.
        self.app.schedule_inspector_reflow()

    def on_resize(self,event):
        # App resize precedes child layout. Rewrap only after the log itself
        # has its new width, rather than caching strips for the old sidebar.
        if event.size.width!=getattr(self,'_reflow_width',None):
            self._reflow_width=event.size.width
            self.app.schedule_inspector_reflow()

    def write(self, content, *args, **kwargs):
        if isinstance(content, Text):
            content = themed_text(self, content)
        return super().write(content, *args, **kwargs)

    def render_line(self,y):
        source_y=self.scroll_offset.y+y
        width=self.scrollable_content_region.width
        if source_y>=len(self.lines):
            # Map empty space to the end, so dragging below the text is safe.
            last=len(self.lines)-1
            return Strip.blank(width,self.rich_style).apply_offsets(
                len(self.lines[last].text) if last>=0 else 0,max(0,last))
        line=self.lines[source_y].apply_style(self.rich_style)
        selection=self.text_selection
        span=selection.get_span(source_y) if selection else None
        if span:
            start,end=span
            text=line.text
            start=cell_len(text[:start])
            end=line.cell_length if end==-1 else cell_len(text[:end])
            if end>start:
                selected=line.crop(start,end)
                highlight=self.screen.get_component_rich_style('screen--selection')
                selected=Strip(Segment.apply_style(selected,post_style=highlight),selected.cell_length)
                line=Strip.join([line.crop(0,start),selected,line.crop(end)])
        cells=characters=0
        for character in line.text:
            if cells>=self.scroll_offset.x:break
            cells+=get_character_cell_size(character)
            characters+=1
        return line.crop_extend(self.scroll_offset.x,self.scroll_offset.x+width,self.rich_style).apply_offsets(characters,source_y)

    @property
    def plain_text(self):
        return '\n'.join(line.text.rstrip() for line in self.lines)

    def get_selection(self,selection):
        if not self.lines:return None
        selected=[]
        for y,line in enumerate(self.lines):
            span=selection.get_span(y)
            if span is not None:
                start,end=span
                selected.append(line.text[start:None if end==-1 else end].rstrip())
        return '\n'.join(selected),'\n'

    def clear(self):
        # A selection from a previous event must never copy the next event.
        if self.is_mounted and self in self.screen.selections:
            self.screen.clear_selection()
        return super().clear()


class ObservatoryScreen(Screen):
    """Copy pointer-selected text through the desktop clipboard when possible."""
    def action_copy_text(self):
        selected=self.get_selected_text()
        if selected is None:
            self.app.notify('Drag across text to select it, then press Ctrl+C.')
            return
        self.app.copy_to_clipboard(selected)


class ObservatoryApp(App):
    TITLE=NAME
    AUTO_FOCUS='#agent-table'
    CSS='''
    Screen { background: $agent-canvas; color: $agent-text; }
    Screen > .screen--selection { background: $agent-selection; color: $agent-on-selection; }
    * { scrollbar-color: $agent-scrollbar; scrollbar-color-hover: $agent-secondary; scrollbar-color-active: $agent-primary; scrollbar-background: $agent-surface; scrollbar-background-hover: $agent-surface; scrollbar-background-active: $agent-surface; }
    #masthead { height: 5; padding: 0 1; margin: 0 2; border: round $agent-border; background: $agent-surface; }
    #identity { height: 1; }
    #brand-lockup { width: auto; height: 1; align-vertical: middle; }
    #brand { width: auto; height: 1; color: $agent-text; text-wrap: nowrap; }
    #connection { width: 1fr; height: 1; color: $agent-success; text-align: right; text-wrap: nowrap; text-overflow: ellipsis; }
    #pause { width: 9; height: 1; padding: 0 1; margin-left: 1; border: none; background: $agent-surface; }
    #summary { width: 100%; height: 1fr; grid-size: 2; grid-columns: 1fr 1fr; grid-rows: 1fr; grid-gutter: 0 2; }
    .stat { height: 1fr; min-width: 0; border: none; background: $agent-surface; padding: 0 1; content-align: left middle; text-wrap: nowrap; text-overflow: ellipsis; }
    #stat-sessions { border-left: solid $agent-success; }
    #stat-attention { border-left: solid $agent-border-subtle; }
    #stat-attention.needs-attention { border-left: solid $agent-primary; }
    .stat.-quiet { display: none; }
    #controls { height: 1; margin: 0 2 0 2; align-vertical: middle; }
    #controls .control-label { width: auto; min-width: 5; height: 1; color: $agent-muted; }
    #agent-filter { display: none; width: 16; height: 1; margin: 0 2 0 1; }
    #controls.agents-active #agent-filter { display: block; }
    #agent-filter SelectCurrent { height: 1; border: none; background: $agent-canvas; padding: 0 1; }
    #scope-label { display: none; width: 8; }
    #controls.agents-active #scope-label { display: block; }
    #scope { display: none; width: 22; height: 1; margin-right: 1; }
    #controls.agents-active #scope { display: block; }
    #scope SelectCurrent { height: 1; border: none; background: $agent-surface; padding: 0 1; }
    #scope:focus > SelectCurrent { border: none; color: $agent-primary; }
    #evidence-scope-label, #evidence-scope { display: none; }
    #evidence-scope-label { width: auto; height: 1; color: $agent-muted; }
    #scope-label { width: auto; }
    #controls.evidence-active #evidence-scope-label { display: block; }
    #controls.evidence-active #evidence-scope { display: block; width: 12; height: 1; margin-right: 1; }
    #evidence-scope SelectCurrent { height: 1; border: none; background: $agent-surface; padding: 0 1; }
    #evidence-scope:focus > SelectCurrent { border: none; color: $agent-primary; }
    #shortcuts { height: 1; max-height: 1; margin: 0 2; color: $agent-muted; text-wrap: nowrap; overflow: hidden; }
    SelectCurrent { background: $agent-panel; border: tall $agent-border; }
    SelectCurrent #label { height: 1; text-wrap: nowrap; text-overflow: ellipsis; }
    Select:focus > SelectCurrent { border: tall $agent-primary; background-tint: transparent; }
    SelectOverlay { background: $agent-panel; border: tall $agent-primary; }
    #controls-spacer { display: none; }
    #refresh-order { height: 1; min-width: 0; width: 9; padding: 0; margin-right: 2; border: none; background: $agent-focus; color: $agent-primary; }
    #agent-sort { display: none; height: 1; min-width: 0; width: 12; padding: 0; margin-right: 2; border: none; background: $agent-surface; color: $agent-secondary; }
    #controls.agents-active #agent-sort { display: block; }
    #agent-sort:hover { background: $agent-hover; color: $agent-primary; }
    #agent-sort:focus { background: $agent-focus; color: $agent-primary; text-style: bold; }
    #reviews { width: 14; height: 1; border: none; background: $agent-surface; margin-top: 0; color: $agent-muted; }
    Checkbox > .toggle--button { color: $agent-disabled; background: $agent-panel; }
    Checkbox.-on > .toggle--button { color: $agent-primary; }
    Button { height: 3; min-width: 9; padding: 1 0 0 0; background: $agent-panel; color: $agent-label; border: none; border-bottom: solid $agent-border; text-style: none; }
    Button:hover { background: $agent-hover; border-bottom: solid $agent-secondary; }
    Button:focus { border-bottom: heavy $agent-primary; background: $agent-focus; color: $agent-primary; text-style: bold; }
    Button:disabled { color: $agent-disabled; border-bottom: solid $agent-border-disabled; background: $agent-surface; }
    #pause { width: 12; height: 1; padding: 0; border: none; background: $agent-surface; }
    #pages { height: 1fr; margin: 0 2; }
    #more-views { display: none; dock: right; layer: above; width: 21; height: 1; margin: 1 2 0 1; }
    Tabs { height: 1; padding-top: 0; margin-top: 0; }
    #pages > Tabs { height: 3; border: round $agent-border; border-title-color: $agent-label; border-title-style: bold; padding: 0 1; background: $agent-surface; }
    Tab { color: $agent-muted; padding: 0 1; background: $agent-canvas; }
    #more-views SelectCurrent { height: 1; border: none; background: $agent-panel; color: $agent-primary; text-style: bold; padding: 0 1; }
    #more-views:hover SelectCurrent { background: $agent-hover; }
    #more-views:focus SelectCurrent { background: $agent-focus; text-style: bold underline; }
    #more-views.secondary-active SelectCurrent { background: $agent-focus; }
    .narrow #more-views { display: block; }
    .narrow #pages > Tabs #--content-tab-host,
    .narrow #pages > Tabs #--content-tab-metrics,
    .narrow #pages > Tabs #--content-tab-findings { display: none; }
    .menu-navigation #pages > Tabs Tab { visibility: hidden; }
    .menu-navigation #more-views { width: 100%; margin: 1 2 0 2; }
    Tab.-active { color: $agent-primary; background: $agent-focus; text-style: bold; }
    Tabs:focus .-active { background: $agent-focus; color: $agent-primary; text-style: underline; }
    Underline > .underline--bar { color: $agent-primary; background: $agent-border-subtle; }
    TabPane { padding: 0; }
    ContentSwitcher { height: 1fr; }
    .hint { height: 2; color: $agent-muted; }
    .box { border: none; background: $agent-surface; padding: 0 1; }
    .results { height: 1fr; border: solid $agent-border-subtle; border-title-color: $agent-label; border-title-style: bold; border-subtitle-color: $agent-muted; }
    .searchbar { height: 1; align-vertical: middle; }
    .searchbar Button { height: 1; padding: 0; border: none; background: $agent-surface; }
    .view-search { width: 1fr; height: 1; margin-right: 1; padding: 0 1; background: $agent-surface; border: none; }
    .view-search:focus { border: none; background: $agent-panel; background-tint: transparent; }
    Input > .input--placeholder { color: $agent-muted; }
    Input > .input--cursor { background: $agent-primary; color: $agent-canvas; }
    Input > .input--selection { background: $agent-selection; color: $agent-on-selection; }
    .clear-search { width: 9; }
    .clear-search:disabled { display: none; }
    .error-filter { width: 12; margin-right: 1; }
    .error-filter.-on { color: $agent-error; border: none; background: $agent-error-surface; text-style: bold; }
    .prompt-filter { width: 12; margin-right: 1; }
    .prompt-filter.-on { color: $agent-primary; border: none; background: $agent-focus; text-style: bold; }
    .search-status { height: 1; color: $agent-muted; padding: 0 1; }
    .history-controls { height: 1; margin-top: 1; }
    .history-controls Static { width: 1fr; color: $agent-muted; }
    .history-controls Button { min-width: 8; width: auto; height: 1; padding: 0 1; margin-left: 1; border: none; }
    .history-controls Button:focus { background: $agent-cursor; color: $agent-on-selection; }
    #history-back { width: 10; height: 1; padding: 0; border: none; margin-right: 1; }
    #history-back:disabled { display: none; }
    DataTable { background: $agent-surface; height: 1fr; scrollbar-size: 1 1; }
    DataTable:focus { background-tint: transparent; }
    DataTable > .datatable--header { background: $agent-header; color: $agent-label; text-style: bold; }
    DataTable > .datatable--cursor { background: $agent-cursor; color: $agent-on-cursor; text-style: none; }
    DataTable:blur > .datatable--cursor { background: $agent-cursor; color: $agent-on-cursor; text-style: none; }
    DataTable > .datatable--hover { background: $agent-row-hover; }
    DataTable > .datatable--odd-row { background: $agent-row-alt; }
    DataTable > .datatable--even-row { background: $agent-surface; }
    RichLog { background: $agent-surface; color: $agent-text; scrollbar-size: 1 1; }
    RichLog:focus { background-tint: transparent; }
    #mcp-servers { height: 7; margin-bottom: 1; }
    .expanded #mcp-servers { display: none; }
    #agent-table { height: 1fr; min-height: 3; }
    #agent-bottom { height: 1fr; width: 38%; min-width: 40; }
    #agent-info { width: 100%; height: 1fr; }
    #agent-detail { width: 100%; height: 1fr; }
    #agent-open { height: 1; width: 1fr; }
    #agent-actions { display: none; }
    .inspector-heading { height: 2; color: $agent-label; border-bottom: solid $agent-border-subtle; margin-bottom: 1; text-style: bold; }
    .inspector-heading Static { width: 1fr; }
    .inspector-heading Button { width: 12; min-width: 12; height: 1; padding: 0; border: none; background: $agent-surface; color: $agent-secondary; }
    .split { height: 1fr; }
    .list { width: 1fr; margin-right: 1; }
    #results-agents { width: 1fr; }
    .detail { width: 1fr; min-width: 36; border: solid $agent-border-subtle; padding: 0 2; }
    .detail-log { height: 1fr; }
    #usage-series { height: 12; min-height: 6; padding: 0 1; color: $agent-secondary; }
    .detail-button { height: 3; width: 100%; }
    .inspector-actions { height: 1; margin: 1 0; }
    .inspector-actions Button { width: 1fr; height: 1; padding: 0; border: none; color: $agent-secondary; background: $agent-surface; }
    .inspector-actions Button:focus { color: $agent-primary; background: $agent-focus; }
    .expand-button { min-width: 12; }
    #trace-detail { width: 1fr; border: solid $agent-border-subtle; }
    #span-tree { height: 1fr; min-height: 4; background: $agent-surface; scrollbar-size: 1 1; }
    #span-info { height: 10; border-top: solid $agent-border-subtle; }
    #health { height: 1; padding: 0 2; color: $agent-muted; text-wrap: nowrap; text-overflow: ellipsis; }
    .narrow #masthead { padding: 0 1; }
    .narrow #summary { grid-gutter: 0 1; }
    .narrow .stat { padding: 0 1; }
    .narrow #controls { margin: 0 1; }
    .narrow #scope { width: 16; }
    .narrow #scope-label { display: none; }
    .narrow #evidence-scope { width: 10; }
    .narrow #evidence-scope-label { display: none !important; }
    .small #evidence-scope { width: 8; }
    .narrow #reviews { display: none; }
    .narrow #pause { width: 8; }
    .narrow #agent-sort { width: 11; margin-right: 1; }
    .small #agent-sort { width: 10; }
    .minimal #agent-sort { display: none; }
    .narrow #shortcuts { height: 1; max-height: 1; margin: 0 1; padding: 0; }
    .narrow #pages { margin: 0 1; }
    .narrow Tab { padding: 0 1; }
    .narrow #agent-bottom { height: 12; width: 100%; }
    .narrow #agent-bottom .inspector-heading { height: 1; border: none; margin-bottom: 0; }
    .narrow #agent-bottom .inspector-actions { margin: 0; }
    .narrow #agent-actions { display: none; }
    .narrow .split { layout: vertical; }
    .narrow .list { width: 100%; height: 1fr; margin-right: 0; margin-bottom: 1; }
    .narrow .detail, .narrow #trace-detail { width: 100%; min-width: 0; height: 1fr; }
    .small Tab { padding: 0 1 0 0; }
    .small #agent-bottom { height: 9; }
    .small #pause { width: 9; }
    .compact #mcp-servers { display: none; }
    .compact #agent-bottom, .compact .detail, .compact #trace-detail, .compact #host-detail { display: none; }
    .compact .list { width: 100%; height: 1fr; margin: 0; }
    .compact .inspector-heading { height: 1; border: none; margin-bottom: 0; }
    .compact .inspector-actions { margin: 0; }
    .expanded .results { display: none; }
    .expanded #agent-bottom { display: block; width: 100%; height: 1fr; margin: 0; }
    .expanded .detail, .expanded #trace-detail, .expanded #host-detail { display: block; width: 100%; height: 1fr; }
    .expanded #agent-info { width: 100%; margin: 0; }
    .expanded #agent-actions { display: none; }
    .expanded #span-info { height: 2fr; }
    .single-pane #agent-bottom { display: none; }
    .single-pane #agents.expanded #agent-bottom { display: block; width: 100%; height: 1fr; }
    .single-pane #scope { width: 12; }
    .single-pane #agent-filter { width: 14; }
    .minimal #masthead { padding: 0 1; }
    .minimal #pages { margin: 0; }
    '''
    BINDINGS=[Binding('/','search','Search'),Binding('e','expand','Expand'),Binding('?','help','Help'),Binding('t','open_trace','Trace'),
              Binding('c','copy','Copy'), Binding('alt+left','history_back','Back',show=False),
              Binding('[','history("newer")','Newer',show=False),Binding(']','history("older")','Older',show=False),
              Binding('l','history("latest")','Latest',show=False),
              Binding('p','pause','Pause'),Binding('a','all','Reset',show=False),Binding('v','reviews','Reviews',show=False),
              Binding('d','diagnostics','Diagnostics'),Binding('x','export','Export'),
              Binding('q','quit','Quit'),Binding('escape','back','Back',show=False),
              Binding('s','focus_agent','Focus agent',show=False),Binding('r','review_attention','Mark reviewed',show=False),
              Binding('f','toggle_tree','Tree or flat agents',show=False),
              Binding('o','toggle_sort','Sort by updated',show=False),
              Binding('alt+up','inspect_parent','Inspect parent',show=False),
              Binding('alt+down','inspect_first_child','Inspect child',show=False),
              Binding('alt+right','inspect_next_attention','Next descendant needing attention',show=False),
              Binding('x','dismiss_attention','Dismiss attention',show=False),
              Binding('1','page("agents")','Agents',show=False),
              Binding('2','page("activity")','Activity',show=False),Binding('3','page("mcp")','MCP',show=False),
              Binding('4','page("traces")','Traces',show=False),Binding('5','page("host")','Host',show=False),
              Binding('6','page("metrics")','Metrics',show=False),Binding('7','page("findings")','Findings',show=False)]

    def __init__(self,feed,*,preferences_path=None,persist_preferences=None):
        super().__init__()
        for name in list(self.available_themes):
            self.unregister_theme(name)
        for theme in supported_themes():
            self.register_theme(theme)
        self.theme = 'Observation'
        self.feed=feed
        self.agent=None
        self.data=feed.snapshot
        self.last_update=0
        self.table_data={}
        self.table_signatures={}
        self.inspector_signatures={}
        self.inspector_reflow_pending=False
        self.selected_source=None
        self.selected_activity=None
        self.selected_mcp=None
        self.selected_metric=None
        self.selected_finding=None
        self.metric_history_requested_key=None
        self.metric_history_loaded_key=None
        self.metric_history_points=[]
        self.scope_options=None
        self.scope_value='all'
        self.agent_filter='recent'
        self.agent_sort_desc=True
        self.agent_order=[]
        self.agent_order_ids=[]
        self.agent_order_key=None
        self.agent_order_pending=False
        self.agent_filter_options=None
        self.tree_mode=True
        self.agent_tree_active=False
        self.ascii_tree=os.environ.get('AGENTURM_ASCII','').lower() in {'1','true','yes','on'}
        self.expanded_agents=set()
        self.collapsed_agents=set()
        self.hierarchy_seen=set()
        self.hierarchy_graph=None
        self.hierarchy_structure_key=None
        self.hierarchy_entries={}
        self.agent_visual_parents={}
        self.hierarchy_reparent_pending=False
        self.visible_agent_rows=[]
        self.agent_row_meta={}
        self.evidence_scope_subtree=False
        self.summary_values={}
        self.span_signature=None
        self.search_timers={}
        self.refresh_timer=None
        self.search_queries={page:'' for page in ('agents','activity','mcp','traces','host','metrics','findings')}
        self.errors_only=set()
        self.prompts_only=False
        self.navigation_history=[]
        self.ai_summaries={}
        self.reviewed_attention=set()
        self.dismissed_attention=set()
        self.summary_busy=False
        self.preferences=Preferences(preferences_path,enabled=persist_preferences is not False)
        self.persist_preferences=persist_preferences
        self.preferences_path=preferences_path
        self.preference_view='agents'
        try:
            from .ai_summary import SummaryConfig, SummaryError
            SummaryConfig.from_env()
            self.ai_summary_available=True
        except SummaryError:
            self.ai_summary_available=False

    def results(self,page,table,placeholder,classes=''):
        """Keep a view's filter, result count, and rows in one panel."""
        controls=[SearchInput(placeholder=placeholder,id='search-'+page,classes='view-search')]
        if page=='activity':
            controls.append(Button('Prompts',id='prompts-activity',classes='prompt-filter',
                                   tooltip='Show only messages you sent to the selected agent'))
        if page in {'activity','mcp'}:
            controls.append(Button('Errors',id='errors-'+page,classes='error-filter',tooltip='Show only failed calls in the current results'))
        controls.append(Button('Clear',id='clear-'+page,classes='clear-search',disabled=True))
        footer=[]
        if page in {'activity','mcp','traces','findings'}:
            footer=[Horizontal(Static('Latest',id='history-label-'+page),
                Button('Newer',id='newer-'+page,compact=True,disabled=True),
                Button('Older',id='older-'+page,compact=True,disabled=True),
                Button('Latest',id='latest-'+page,compact=True,disabled=True),classes='history-controls')]
        return Vertical(
            Horizontal(*controls,classes='searchbar'),
            Static('Loading…',id='mcp-hint' if page=='mcp' else 'search-status-'+page,classes='search-status',markup=False),
            ResultsTable(id=table,cursor_type='row',zebra_stripes=False,cursor_foreground_priority='renderable'),
            *footer,
            id='results-'+page,classes='box results '+classes)

    def scope_controls(self):
        return Horizontal(
            Button('← Back',id='history-back',disabled=True,tooltip='Return to the event you opened · Alt+Left'),
            Static('Show:',classes='control-label'),
            Select([('Recent','recent'),('Active','active'),('All','all')],
                   value='recent',allow_blank=False,id='agent-filter',tooltip='Recent shows working sessions and the last finished session'),
            Button('Reorder',id='refresh-order',tooltip='List changed; apply queued ordering without losing your selected session'),
            Button('Updated ↓',id='agent-sort',tooltip='Sort by updated time. Working stays on top; terminated stays at the bottom.'),
            Static('Session',id='scope-label'),
            Select([('All agent sessions','all')],value='all',allow_blank=False,id='scope'),
            Static('Evidence',id='evidence-scope-label'),
            Select([('This agent','exact'),('This agent + subagents','subtree')],
                   value='exact',allow_blank=False,id='evidence-scope'),
            Static('',id='controls-spacer'), Checkbox('Reviews',id='reviews'),
            id='controls')

    def more_view_control(self):
        return Select([('Other views',''),('5 Host','host'),('6 Metrics','metrics'),('7 Findings','findings')],
                      value='',allow_blank=False,id='more-views',
                      tooltip='Other views: 5 Host · 6 Metrics · 7 Findings · Enter to choose')

    def compose(self)->ComposeResult:
        with Vertical(id='masthead'):
            with Horizontal(id='identity'):
                with Horizontal(id='brand-lockup'):
                    yield Static(masthead(compact=True), id='brand')
                yield Static('○ Connecting',id='connection',markup=False)
                yield Button('Pause',id='pause')
            with Grid(id='summary'):
                for name in ('sessions','attention','mcp-count','usage'):
                    yield Static('',id='stat-'+name,classes='stat'+(' -quiet' if name in {'mcp-count','usage'} else ''),markup=False)
        with CockpitTabs(initial='agents',id='pages'):
            with TabPane('1  Agents',id='agents'):
                with Horizontal(id='agent-split',classes='split'):
                    yield self.results('agents','agent-table','Search sessions…','list')
                    with Vertical(id='agent-bottom',classes='detail box'):
                        with Horizontal(classes='inspector-heading'):
                            yield Static('Session inspector')
                            yield Button('Expand · e',id='expand-agents',classes='expand-button',compact=True)
                        with Vertical(id='agent-info'):
                            yield SelectableRichLog(id='agent-detail',min_width=1,wrap=True,markup=False,auto_scroll=False)
                            with Horizontal(classes='inspector-actions'):
                                yield Button(Text('Activity · s'),id='agent-open',tooltip='Read all recorded activity for this session')
                                yield Button('Trace · t',id='agent-trace',tooltip='Inspect the latest linked trace for this session')
                                yield Button('Mark reviewed · r',id='agent-reviewed',tooltip='Acknowledge this attention item for this run; does not change the recorded session status')
                                yield Button('Dismiss · x',id='agent-dismissed',tooltip='Hide this failure from Recent and Needs attention; it remains in All history and Traces')
                                yield Button('AI summary',id='agent-summary',tooltip='Uses OPENAI_API_KEY when available. Runs only when requested; the recorded progress summary needs no setup.')
                        yield ResultsTable(id='agent-actions',cursor_type='row',zebra_stripes=False)
            with TabPane('2  Activity',id='activity'):
                with Horizontal(classes='split'):
                    yield self.results('activity','activity-table','Filter activity by command, message, tool, or path…','list')
                    with Vertical(id='activity-inspector',classes='detail box'):
                        yield Static('EVENT INSPECTOR',classes='inspector-heading')
                        yield SelectableRichLog(id='activity-detail',classes='detail-log',min_width=1,wrap=True,markup=False,auto_scroll=False)
                        with Horizontal(classes='inspector-actions'):
                            yield Button(Text('Trace · t'),id='activity-trace',tooltip='Follow the selected event into its trace')
                            yield Button(Text('Copy · c'),id='activity-copy')
                            yield Button(Text('Expand · e'),id='expand-activity',classes='expand-button')
            with TabPane('3  MCP Calls',id='mcp'):
                yield ResultsTable(id='mcp-servers',classes='box',cursor_type='row',zebra_stripes=True)
                with Horizontal(classes='split'):
                    yield self.results('mcp','mcp-table','Filter MCP operations by server, method, tool, arguments, or result…','list')
                    with Vertical(id='mcp-inspector',classes='detail box'):
                        yield Static('TOOL INSPECTOR',classes='inspector-heading')
                        yield SelectableRichLog(id='mcp-detail',classes='detail-log',min_width=1,wrap=True,markup=False,auto_scroll=False)
                        with Horizontal(classes='inspector-actions'):
                            yield Button(Text('Trace · t'),id='mcp-trace',tooltip='Follow the selected call into its trace')
                            yield Button(Text('Copy · c'),id='mcp-copy')
                            yield Button(Text('Expand · e'),id='expand-mcp',classes='expand-button')
            with TabPane('4  Traces',id='traces'):
                with Horizontal(classes='split'):
                    yield self.results('traces','trace-table','Filter traces by task, model, service, or ID…','list')
                    with Vertical(id='trace-detail',classes='box'):
                        yield Tree('Select a trace',id='span-tree')
                        yield SelectableRichLog(id='span-info',min_width=1,wrap=True,markup=False,auto_scroll=False)
            with TabPane('5  Host',id='host'):
                yield self.results('host','host-table','Filter processes by name, PID, project, or state…')
                yield SelectableRichLog(id='host-detail',classes='box',min_width=1,wrap=True,markup=False,auto_scroll=False)
            with TabPane('6  Metrics',id='metrics'):
                with Horizontal(classes='split'):
                    yield self.results('metrics','metrics-table','Filter metric, service, kind, or unit…','list')
                    with Vertical(id='metrics-inspector',classes='detail box'):
                        yield Static('METRIC INSPECTOR',classes='inspector-heading')
                        yield Static(id='usage-series')
                        yield SelectableRichLog(id='metrics-detail',classes='detail-log',min_width=1,wrap=True,markup=False,auto_scroll=False)
                        with Horizontal(classes='inspector-actions'):
                            yield Button(Text('Exemplar · t'),id='metrics-trace',disabled=True)
                            yield Button(Text('Copy · c'),id='metrics-copy')
                            yield Button(Text('Expand · e'),id='expand-metrics',classes='expand-button')
            with TabPane('7  Findings',id='findings'):
                with Horizontal(classes='split'):
                    yield self.results('findings','finding-table','Filter findings by severity, type, task, or evidence…','list')
                    with Vertical(id='finding-inspector',classes='detail box'):
                        yield Static('EVIDENCE INSPECTOR',classes='inspector-heading')
                        yield SelectableRichLog(id='finding-detail',classes='detail-log',min_width=1,wrap=True,markup=False,auto_scroll=False)
                        with Horizontal(classes='inspector-actions'):
                            yield Button(Text('Trace · t'),id='finding-trace',tooltip='Open the finding’s source trace')
                            yield Button(Text('Copy · c'),id='finding-copy')
                            yield Button(Text('Expand · e'),id='expand-findings',classes='expand-button')
        yield Static(navigation_hints(self.size.width or 160),id='shortcuts',markup=False)
        yield Static('Connecting to receiver…',id='health',markup=False)

    def get_default_screen(self):
        return ObservatoryScreen()

    def get_system_commands(self,screen):
        """Keep important cockpit actions discoverable in Textual's fuzzy palette."""
        yield from super().get_system_commands(screen)
        commands=(
            ('Go to Agents','Show the task-first session list',lambda:self.action_page('agents')),
            ('Go to Activity','Inspect recorded session events',lambda:self.action_page('activity')),
            ('Go to MCP Calls','Inspect recorded tool calls',lambda:self.action_page('mcp')),
            ('Go to Traces','Inspect execution traces',lambda:self.action_page('traces')),
            ('Go to Host','Inspect known local processes',lambda:self.action_page('host')),
            ('Go to Metrics','Inspect recorded telemetry metrics',lambda:self.action_page('metrics')),
            ('Go to Findings','Review recorded findings',lambda:self.action_page('findings')),
            ('Show recent sessions','Show working sessions and the most recently finished session',lambda:self.set_agent_filter('recent')),
            ('Show active sessions','Show only working sessions',lambda:self.set_agent_filter('active')),
            ('Show all sessions','Show the full session history',lambda:self.set_agent_filter('all')),
            ('Sort sessions by newest update','Newest updated first inside fixed status groups',lambda:self.set_agent_sort(True)),
            ('Sort sessions by oldest update','Oldest updated first inside fixed status groups',lambda:self.set_agent_sort(False)),
            ('Switch sort direction','Toggle newest/oldest update ordering',self.action_toggle_sort),
            ('Refresh session order','Apply queued ordering while preserving session selection',self.refresh_agent_order),
            ('Switch agent tree or flat list','Toggle hierarchy indentation for troubleshooting',self.action_toggle_tree),
            ('Inspect parent agent',"Open the selected agent's confirmed parent",self.action_inspect_parent),
            ('Inspect first child agent','Open the first observed delegated child',self.action_inspect_first_child),
            ('Inspect next descendant needing attention','Navigate to the next descendant evidence',self.action_inspect_next_attention),
            ('Scope evidence to this agent or subtree','Switch related Activity, MCP, Traces and Findings scope',lambda:self.query_one('#evidence-scope',Select).toggle()),
            ('Pause or resume display','Freeze or resume the cockpit display; collection continues',self.action_pause),
            ('Open diagnostics','Inspect local collection health',self.action_diagnostics),
            ('Copy selected evidence','Copy selected evidence or the current record',self.action_copy),
            ('Open linked trace','Follow an exact recorded trace link',self.action_open_trace),
        )
        for name,help_text,callback in commands:
            yield SystemCommand(name,help_text,callback)

    def set_agent_filter(self,value):
        self.agent_filter=value
        self.selected_source=None
        self.agent_order_key=None
        self.agent_order_pending=False
        self.last_update=-1
        self.refresh_data()

    def refresh_agent_order(self):
        self.agent_order_key=None
        self.agent_order_ids=[]
        self.agent_order_pending=False
        self.agent_visual_parents={}
        self.hierarchy_reparent_pending=False
        self.hierarchy_structure_key=None
        self.last_update=-1
        self.refresh_data()

    def set_agent_sort(self,newest_first):
        """Change updated-time direction inside fixed status groups."""
        self.agent_sort_desc=bool(newest_first)
        try:
            button=self.query_one('#agent-sort',Button)
        except Exception:
            button=None
        if button is not None:
            button.label='Updated ↓' if self.agent_sort_desc else 'Updated ↑'
            button.tooltip=(
                'Sort by updated time, newest first. Working stays on top; terminated stays at the bottom.'
                if self.agent_sort_desc else
                'Sort by updated time, oldest first. Working stays on top; terminated stays at the bottom.')
        self.refresh_agent_order()

    def action_toggle_sort(self):
        self.set_agent_sort(not self.agent_sort_desc)
        self.notify('Sort: Updated ↓' if self.agent_sort_desc else 'Sort: Updated ↑',timeout=2)

    def on_mount(self):
        # A normal headless test must never read/write the developer's config.
        if self.is_headless and self.persist_preferences is None and self.preferences_path is None:
            self.preferences.enabled=False
        preferences=self.preferences.load()
        self.theme=preferences.get('theme','Observation')
        self.preference_view=preferences.get('view','agents')
        self.query_one('#pages',TabbedContent).active=self.preference_view
        self.errors_only=set(preferences.get('errors',[]))
        self.prompts_only=preferences.get('prompts',False)
        self.dismissed_attention=set(preferences.get('dismissed_attention',[]))
        self.feed.activity_prompts=self.prompts_only
        for page in self.errors_only:
            button=self.query_one('#errors-'+page,Button)
            button.add_class('-on');button.label='Errors ●'
        if self.prompts_only:
            button=self.query_one('#prompts-activity',Button)
            button.add_class('-on');button.label='Prompts ●'
        self.query_one('#reviews',Checkbox).value=preferences.get('reviews',False)
        self.feed.include_reviews=preferences.get('reviews',False)
        self.query_one('#controls').set_class(self.preference_view=='agents','agents-active')
        self.theme_changed_signal.subscribe(self, self.on_theme_changed)
        columns={
            'agent-table':[('Task / source',34,2),('State',17,0),('Updated',8,0)],
            'agent-actions':[('Time',8,0),('Type',7,0),('Recent actions',15,1)],
            'activity-table':[('Time',8,0),('Type',7,0),('Result',9,0),('Action',20,1)],
            'mcp-servers':[('Server · Enter filters operations',24,3),('Inventory',22,2),('Calls',5,0),('Metric calls',12,0),('Tools',8,0),('Errors',8,0)],
            'mcp-table':[('Time',8,0),('Agent',11,1),('Server',9,1),('Tool / method',14,3),('Result',7,0),('Latency',7,0)],
            'trace-table':[('Result',11,0),('Task / run',20,1),('Spans',5,0),('Tokens in / out',15,0)],
            'host-table':[('Process',10,1),('PID',7,0),('Project',12,2),('State',8,1),('RAM',9,0),('CPU total',9,0)],
            'metrics-table':[('Instrument',20,2),('Service',12,1),('Kind',12,0),('Latest value',17,1),('Unit',8,0),('Points',7,0)],
            'finding-table':[('Severity',9,0),('Kind',18,1),('Finding',32,2),('Occurrences',12,0),('Last seen',10,0)]}
        for name,cols in columns.items():
            self.query_one('#'+name,ResultsTable).configure(cols)
        for name,title in [('results-agents','Agent sessions'),('agent-detail','Session overview'),('agent-actions','Recent work'),('results-activity','Activity stream'),('activity-inspector','Event detail'),('mcp-servers','Configured servers and servers with observed calls'),('results-mcp','MCP operations'),('mcp-inspector','Operation detail'),('results-traces','Traces'),('trace-detail','Execution tree'),('results-host','Agent processes'),('host-detail','Collection status'),('results-metrics','OTLP metric instruments'),('metrics-inspector','Metric detail'),('results-findings','Finding review'),('finding-inspector','Evidence and source')]:
            widget=self.query_one('#'+name)
            if not widget.has_class('detail'):widget.border_title=title
        for page in self.search_queries:
            self.query_one('#results-'+page).border_subtitle='↑↓ select  ·  Enter inspect'
            self.query_one('#search-'+page).tooltip='Search this view · / to focus · Esc to return'
        self.query_one('#reviews').tooltip='Include internal approval review sessions'
        self.query_one('#pause').tooltip='Pause display updates · collection continues'
        for log in self.query(SelectableRichLog):
            log.tooltip='Drag to select text · Ctrl+C copies selection · c copies details'
        self.query_one('#host-detail').styles.height=8
        self.update_layout()
        if not self.feed.thread.is_alive():self.feed.thread.start()
        self.refresh_timer=self.set_interval(.5,self.refresh_data)
        self.action_page(self.preference_view)

    def on_unmount(self):
        self.save_preferences()
        if self.refresh_timer:self.refresh_timer.stop()
        for timer in self.search_timers.values():timer.stop()

    def save_preferences(self):
        self.preferences.save({'theme':self.theme,'view':self.preference_view,
            'reviews':self.feed.include_reviews,'prompts':self.prompts_only,
            'errors':sorted(self.errors_only),
            'dismissed_attention':sorted(self.dismissed_attention)})

    def on_theme_changed(self, theme):
        # RichLog stores rendered strips; cached rows/inspectors must be rebuilt too.
        self.table_signatures.clear()
        self.span_signature=None
        self.update_layout()
        self.reflow_inspectors()

    def on_resize(self,event):
        self.update_layout(event.size)
        self.schedule_inspector_reflow()
        self.call_after_refresh(self.refresh_summary)
        self.call_after_refresh(self.refresh_data)

    def schedule_inspector_reflow(self):
        if self.inspector_reflow_pending:return
        self.inspector_reflow_pending=True
        self.call_after_refresh(self.finish_inspector_reflow)

    def finish_inspector_reflow(self):
        self.inspector_reflow_pending=False
        self.reflow_inspectors()

    def reflow_inspectors(self):
        if not isinstance(self.screen,ObservatoryScreen):return
        self.screen.clear_selection()
        positions={log.id:log.scroll_offset for log in self.query(SelectableRichLog)}
        self.inspector_signatures.clear()
        self.last_update=-1
        self.refresh_data()
        trees=self.query('#span-tree')
        if self.query_one('#pages',TabbedContent).active=='traces' and trees and trees.first().cursor_node:
            self.show_span(trees.first().cursor_node.data)
        def restore():
            for name,position in positions.items():
                self.query_one('#'+name).scroll_to(x=position.x,y=position.y,animate=False)
        self.call_after_refresh(restore)

    def update_layout(self,size=None):
        size=size or self.size
        self.layout_size=size
        self.screen.set_class(size.width<120,'narrow')
        self.screen.set_class(size.width<96,'single-pane')
        self.screen.set_class(size.width<60 or size.height<18,'minimal')
        self.screen.set_class(size.width<80,'small')
        self.screen.set_class(size.height<30,'compact')
        self.screen.set_class(size.width<76,'menu-navigation')
        # Halve the old quarter-screen overview; navigation has its own frame.
        # Keep enough room for identity and counts on short terminals.
        self.overview_height=max(4,(size.height//4-2)//2)
        for header in self.query('#masthead'):
            header.styles.height=self.overview_height
        for brand in self.query('#brand'):
            brand.update(themed_text(brand,masthead(demo=getattr(self,'demo_mode',False),compact=True)))
        for hints in self.query('#shortcuts'):
            hints.update(navigation_hints(size.width))
        if self.query('#more-views') and self.query('#pages'):
            self.sync_more_view(self.query_one('#pages',TabbedContent).active or 'agents')
        if getattr(self,'summary_values',None):
            self.refresh_summary()
        self.last_update=-1

    def stat(self,name,value,label,detail):
        self.summary_values[name]=(value,label,detail)
        accent='#ffbd69' if name=='attention' and value else '#edf1e7'
        text=Text(style='#abb4a5',no_wrap=True,overflow='ellipsis')
        text.append(str(value),style='bold '+accent)
        visible_label=label.lower()
        if name=='sessions' and value==1:
            visible_label='working agent'
        text.append(' '+visible_label,style='#abb4a5')
        widget=self.query_one('#stat-'+name,Static)
        widget.tooltip=detail or None
        widget.border_title=''
        widget.set_class(name=='attention' and bool(value),'needs-attention')
        terminal_width=getattr(self,'layout_size',self.size).width
        width=max(1,min(widget.content_size.width,(terminal_width-4)//2-3))
        text.truncate(width,overflow='ellipsis')
        if name in {'sessions','attention'} and getattr(self,'overview_height',4)>=4:
            secondary=Text(clean(detail or ''),style='#abb4a5',no_wrap=True,overflow='ellipsis')
            secondary.truncate(width,overflow='ellipsis')
            text.append('\n')
            text.append_text(secondary)
        widget.update(themed_text(widget,text))

    def refresh_summary(self):
        for name,values in self.summary_values.items():
            self.stat(name,*values)

    def fill(self,name,rows,preserve=None):
        """Preserve row identity and scroll position while live values change."""
        signature=tuple((key,tuple(str(value) for value in values)) for key,values,record in rows)
        self.table_data[name]={key:record for key,values,record in rows}
        if self.table_signatures.get(name)==signature:return
        self.table_signatures[name]=signature
        table=self.query_one('#'+name,DataTable)
        rows=[(key,[themed_text(table,value) if isinstance(value,Text) else value for value in values],record)
              for key,values,record in rows]
        selected=preserve
        if table.row_count:
            try:
                if not selected:selected=table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value
            except Exception:pass
        scroll=table.scroll_offset
        old_index=table.cursor_row
        with table.prevent(DataTable.RowHighlighted):
            if ([key.value for key in table.rows]==[key for key,_,_ in rows] and
                all(table.rows[key].height==table.preview_height(values) for key,values,_ in rows)):
                for key,values,_ in rows:
                    table.full_rows[key]=values
                    for column,value in zip(table.columns,table.project(values)):
                        previous=table.get_cell(key,column)
                        # Rich Text equality excludes its base style; a theme
                        # change must still recolor otherwise identical cells.
                        if previous!=value or getattr(previous,'style',None)!=getattr(value,'style',None):
                            table.update_cell(key,column,value)
            else:
                table.clear()
                for key,values,record in rows:table.add_row(*values,key=key)
            if rows:
                index=next((index for index,(key,_,__) in enumerate(rows) if key==selected),0)
                table.move_cursor(row=index,scroll=False)
                table.scroll_to(x=scroll.x,y=max(0,scroll.y+index-old_index),animate=False)

    def search_status(self,page,count,noun,plural=None):
        query=self.search_queries[page]
        waiting=page in {'activity','mcp','traces'} and self.data.queries.get(page,'')!=query
        if page=='activity' and self.data.activity_prompts!=self.prompts_only:waiting=True
        text='Filtering…' if waiting else quantity(count,('matching '+noun) if query else noun,
                                                    ('matching '+plural) if query and plural else plural)
        if not waiting and query and not count:text+=' · Clear to remove this filter'
        if page=='agents' and not query and not count and self.scope_value=='all':
            text+=' · Start an agent in its terminal · d Setup / diagnostics'
        if page=='activity' and self.prompts_only:text+=' · user prompts only'
        if page in self.errors_only:text+=' · errors only'
        self.query_one('#search-status-'+page,Static).update(text)

    def visible_results(self,page,items):
        if self.data.queries.get(page,'')!=self.search_queries[page]:return []
        if page=='activity' and self.data.activity_prompts!=self.prompts_only:return []
        if page in self.errors_only:
            return [item for item in items if str(item.get('status') or item.get('result') or '').lower() in {'error','failed','denied','blocked','policy_denied','incomplete'}]
        return items

    def refresh_data(self):
        # Timer messages can already be queued when child widgets unmount.
        if not self.query('#health'):return
        if not isinstance(self.screen,ObservatoryScreen):return
        data=self.feed.snapshot
        state='Reconnecting' if data.error else 'Connected'
        state_color='#ff8799' if data.error else '#86e3b5'
        endpoint=self.feed.api.endpoint.removeprefix('http://').removeprefix('https://')
        connection_widget=self.query_one('#connection',Static)
        connection_widget.tooltip=state+' · '+endpoint
        pause_button=self.query_one('#pause',Button)
        pause_button.label='Resume' if self.feed.paused else 'Pause'
        pause_button.tooltip='Paused display; collection continues' if self.feed.paused else 'Pause display updates; collection continues'
        connection=Text()
        connection.append('● ', style='bold '+state_color)
        local=urlsplit(self.feed.api.endpoint).hostname in {'localhost','127.0.0.1','::1'}
        connection.append('Reconnecting' if data.error else 'Live', style=state_color)
        update_status_text(connection_widget,themed_text(connection_widget,connection))
        health_text=(data.error if data.error else
            f"{'Display paused' if self.feed.paused else 'Updated '+age(int(data.updated*1e9))}  ·  {self.scope_label()}  ·  {'Reviews included' if self.feed.include_reviews else 'Reviews hidden'}")
        source_diagnostics=[entry.get('diagnostics') or {} for entry in data.local_status.get('sessions',[])]
        backlog_sources=sum(bool(item.get('backlog_bytes',0)) for item in source_diagnostics)
        def collector_stale(item):
            try:return bool(item.get('last_success_ns') and time.time_ns()-int(item['last_success_ns'])>15_000_000_000)
            except (TypeError,ValueError,OverflowError):return False
        stale_sources=sum(collector_stale(item) for item in source_diagnostics)
        collection_note=('Collection catching up · '+quantity(backlog_sources,'source') if backlog_sources else
                         'Collection delayed · '+quantity(stale_sources,'source') if stale_sources else '')
        if collection_note:
            health_text+='  ·  '+collection_note
        coverage=coverage_warning(data.local_status)
        if coverage:health_text+='  ·  '+coverage
        health_text+='  ·  d Diagnostics / retention'
        health=Text(health_text,style='#c5adff' if collection_note else '#ffbd69' if coverage and not data.error else '#ff8799' if data.error else '#abb4a5')
        health_widget=self.query_one('#health',Static)
        if self.screen.selections:
            health=Text('Text selected · Ctrl+C to copy · click to clear selection and resume display updates',style='#b9afc5')
        update_status_text(health_widget,themed_text(health_widget,health))
        if self.screen.selections:
            return
        if data.updated==self.last_update:return
        self.last_update=data.updated;self.data=data
        agents=sources(data,self.feed.include_reviews)
        work=[item for item in agents if not item['review']]
        observed_work=[item for item in work if item.get('discovery_state')!='announced']
        recent=sum(item['recent'] for item in observed_work)
        active=[item for item in observed_work if item.get('active') or item.get('state')=='Connected']
        reviews=sum(entry.get('service')=='codex-review' for entry in data.local_status.get('sessions',[]))
        processes=(data.local_status.get('runtime') or {}).get('processes',[])
        live=sum(not str(item.get('state','')).startswith(('Z','X')) for item in processes)
        waiting=sum(item['state']=='Waiting' for item in observed_work)
        replies=sum(input_required(item) for item in observed_work)
        groups=sum(1 for item in observed_work
                   if item.get('relationship_state')=='confirmed' and
                   not item.get('parent_agent_id') and item.get('relationship_kind')=='unknown')
        workers=sum(1 for item in observed_work
                    if item.get('relationship_state')=='confirmed' and
                    item.get('relationship_kind')=='delegated' and item.get('parent_agent_id'))
        known_agents={item.get('agent_id') for item in observed_work}
        unlinked=sum(1 for item in observed_work
                     if item.get('relationship_state') in {'unresolved','conflicted','invalid'} or
                     (item.get('parent_agent_id') and item.get('parent_agent_id') not in known_agents))
        runtimes={name:sum(item.get('runtime')==name for item in active) for name in sorted({item.get('runtime') or 'Agent' for item in active})}
        session_detail=' · '.join(f'{name} {count}' for name,count in runtimes.items()) or f'{len(observed_work)} sessions · {replies} action'
        if groups or workers:
            session_detail=f"{quantity(groups,'task group')} · {quantity(workers,'worker')}"
            if unlinked:
                session_detail+=f" · {unlinked} unlinked"
            elif replies:
                session_detail+=f" · {replies} action"
        elif replies and active:
            session_detail+=f' · {replies} action'
        stat_width=(self.size.width-(4 if self.size.width>=140 else 2))//(2 if self.size.width<100 else 4)
        if cell_len(session_detail)>stat_width-(5 if self.size.width>=140 else 3):
            session_detail=f'{len(work)} sessions · {replies} action'
        self.stat('sessions',len(active),'Working agents',session_detail)
        stalled=sum(item.get('state')=='Possibly stalled' for item in work)
        failures=sum(item.get('state') in {'Error','Failed','Incomplete'} for item in work)
        needs_attention=sum(attention_required(item) and item['id'] not in self.reviewed_attention
                            and attention_key(item) not in self.dismissed_attention for item in work)
        self.stat('attention',needs_attention,'Needs attention',
                  f'{replies} need input · {stalled} may be stalled · {failures} failures' if needs_attention else 'No action needed')
        self.query_one('#stat-attention').tooltip=(f'{replies} explicit approval/reply requests · {stalled} possible stalls · {failures} recorded failures. '
            f'Open a session and press r to acknowledge it for this run or x to dismiss it from Recent and Needs attention. Dismissed sessions remain in All history and Traces. Ordinary tool calls do not need action. {live} live host processes are listed in Host.')
        servers={item.get('server') for item in data.mcp if item.get('server') and item.get('server')!='Not reported'}
        operations={(item.get('server'),item.get('tool')) for item in data.mcp if item.get('tool') and item.get('tool')!='Not reported'}
        mcp_errors=sum(str(item.get('status','')).upper() in {'ERROR','FAILED'} for item in data.mcp)
        inventory=data.mcp_summary
        call_count=inventory.get('calls',len(data.mcp))
        server_count=len(inventory['servers']) if inventory else len(servers)
        tool_count=inventory.get('tools',len(operations))
        mcp_errors=inventory.get('errors',mcp_errors)
        self.stat('mcp-count',number(call_count),'MCP calls',
                  f'{quantity(server_count,"server")} · {quantity(mcp_errors,"error")}')
        self.query_one('#stat-mcp-count').tooltip=f'{quantity(tool_count,"tool")} · All stored calls in the selected scope. Configured servers appear only after an observed call.\n'+', '.join(row['server'] for row in inventory.get('servers',[]))
        if inventory.get('metric_calls'):
            self.query_one('#stat-mcp-count').tooltip += (
                f"\nCursor aggregate MCP metric: {number(inventory['metric_calls'])} calls across all conversations; "
                "shown separately because metrics have no session IDs or call details.")
        self.query_one('#mcp-servers').tooltip=(
            'Calls are individual calls with operation details. Metric calls are Cursor aggregate counts; '
            'they cannot be opened or attributed to a session. Calls without a reported server are listed separately.')
        self.stat('usage',observed_count(data.totals,'input_tokens')+' / '+observed_count(data.totals,'output_tokens'),'Tokens in / out','Includes cached input')
        self.query_one('#stat-sessions').tooltip=(
            f'{len(active)} working · {len(observed_work)} observed agents · {recent} recently updated\n'
            f'{groups} task groups · {workers} delegated workers · {unlinked} unlinked/unresolved\n'
            f'{waiting} waiting · {replies} need action · {stalled} possibly stalled · {reviews} approval reviews\n'
            +' · '.join(f'{name}: {count} working' for name,count in runtimes.items())
            +'\nWorking includes quiet unfinished turns and exact process CPU / I/O progress, not every open terminal. '
             'Spawn announcements are not counted as working.')
        self.query_one('#stat-usage').tooltip=(
            'Recorded token totals; repeated and cached input is included. '
            f"Cache read {observed_count(data.totals,'cache_read_input_tokens')} · "
            f"cache write {observed_count(data.totals,'cache_write_input_tokens')} · "
            f"reasoning output {observed_count(data.totals,'reasoning_output_tokens')}. "
            'Breakdowns are already included in headline totals. This is not a billable cost estimate.')
        options=[('All agent sessions','all')]+[(f"{item['runtime']} · {item.get('project') or 'SDK'} · {(item.get('session_id') or item['id'])[-6:]}",item['id']) for item in agents]
        if options!=self.scope_options:
            picker=self.query_one('#scope',Select);old=self.scope_value
            available=any(value==old for _,value in options)
            if not available:
                self.scope_value='all'
                self.feed.session_id='';self.feed.service='';self.feed.scope_agent_id=''
            picker.set_options(options);picker.value=old if available else 'all';self.scope_options=options
        self.source_map={item['id']:item for item in agents}
        self.sync_evidence_scope()
        # Build view models and evidence only for the visible tab. Hidden views
        # retain their selection and are refreshed from the latest snapshot on
        # activation, rather than repeatedly decoding invisible evidence.
        page=self.query_one('#pages',TabbedContent).active
        render=getattr(self,'render_'+page,None)
        if render:render()
        if page in {'activity','mcp','traces','findings'}:self.render_paging(page)

    def hierarchy_for(self,agents):
        """Reuse one adjacency index until structural relationships change."""
        key=tuple(sorted((item.get('agent_id'),item.get('parent_agent_id'),
                          item.get('relationship_kind'),item.get('relationship_state'),
                          item.get('relationship_provenance'),item.get('discovery_state'),
                          item.get('coverage'))
                         for item in agents))
        graph=self.hierarchy_graph
        if graph is None or key!=self.hierarchy_structure_key:
            graph=HierarchyGraph(agents)
            self.hierarchy_graph=graph
            self.hierarchy_structure_key=key
        else:
            dynamic=('state','active','attention','stamp','last_activity','waiting_for',
                     'task','project','run_state','last_record_ns','source_id','id','review')
            for item in agents:
                node=graph.nodes.get(item.get('agent_id'))
                if node is None:continue
                for field in dynamic:
                    if field in item:node[field]=item[field]
            graph._summary_cache.clear()
        return graph

    def render_agents(self):
        agents=list(self.source_map.values())
        # Duplicate observations of one canonical agent are one node.  A later
        # transcript always supersedes its earlier spawn announcement.
        by_agent={}
        for item in agents:
            agent_id=item.get('agent_id')
            if not agent_id:continue
            previous=by_agent.get(agent_id)
            if previous is None or (previous.get('discovery_state')=='announced' and
                                    item.get('discovery_state')=='observed'):
                by_agent[agent_id]=item
        agents=list(by_agent.values())
        self.hierarchy_entries={item['agent_id']:item for item in agents}
        graph=self.hierarchy_for(agents)
        tree_available=any(node.get('parent_agent_id') and node.get('relationship_state')=='confirmed'
                           and node.get('relationship_kind')=='delegated'
                           for node in graph.nodes.values())
        tree_mode=bool(self.tree_mode and tree_available)
        self.agent_tree_active=tree_mode
        now=time.time_ns()
        for agent_id,node in graph.nodes.items():
            if agent_id in self.hierarchy_seen:continue
            self.hierarchy_seen.add(agent_id)
            summary=graph.summary(agent_id)
            if ((node_active(node) or node_attention(node) or summary['working'] or summary['attention'])
                    and agent_id not in self.collapsed_agents):
                self.expanded_agents.add(agent_id)
        agent_query=self.search_queries['agents'].casefold()
        scoped_agents=[item for item in agents if self.scope_value=='all' or item['id']==self.scope_value]

        def searchable(item):
            return (' '.join(str(item.get(key) or '') for key in
                ('runtime','service','project','working_directory','branch','task','model',
                 'session_id','id','state','waiting_for'))+' '+task_preview(item)).casefold()
        searched=[item for item in scoped_agents if not agent_query or agent_query in searchable(item)]
        finished_states={'Completed','Interrupted','Error','Failed','Incomplete'}
        finished=sorted((item for item in scoped_agents if item.get('state') in finished_states),
                        key=lambda item:(-item['stamp'],item['id']))
        recent_ids={item['id'] for item in scoped_agents
                    if item.get('active') or attention_required(item)
                    if attention_key(item) not in self.dismissed_attention}
        latest_finished=next((item for item in finished
                              if attention_key(item) not in self.dismissed_attention),None)
        if latest_finished:
            recent_ids.add(latest_finished['id'])
        counts={'recent':len(recent_ids),'active':sum(bool(item.get('active')) for item in scoped_agents),
                'all':len(scoped_agents)}
        filter_options=[(f'Recent {counts["recent"]}','recent'),
                        (f'Active {counts["active"]}','active'),
                        (f'All {counts["all"]}','all')]
        picker=self.query_one('#agent-filter',Select)
        with picker.prevent(Select.Changed):
            if filter_options!=self.agent_filter_options:
                picker.set_options(filter_options)
                self.agent_filter_options=filter_options
            if str(picker.value)!=self.agent_filter:
                picker.value=self.agent_filter

        def filter_match(item):
            if self.agent_filter=='all':return True
            if self.agent_filter=='active':return bool(item.get('active'))
            return item['id'] in recent_ids
        matching=[item for item in searched if filter_match(item)]
        match_ids={item['agent_id'] for item in matching}
        if tree_mode:
            actual_parents={}
            for agent_id,node in graph.nodes.items():
                parent=node.get('parent_agent_id')
                actual_parents[agent_id]=(parent if node.get('relationship_state')=='confirmed'
                    and node.get('relationship_kind')=='delegated' and parent in graph.nodes
                    else None)
            visual=dict(self.agent_visual_parents)
            reparent_pending=False
            for agent_id,parent in actual_parents.items():
                previous=visual.get(agent_id)
                if previous is None:
                    visual[agent_id]=parent
                elif parent!=previous:
                    old_exists=previous in graph.nodes
                    if parent is not None and old_exists and \
                            graph.nodes[agent_id].get('relationship_state')=='confirmed' and \
                            graph.nodes[agent_id].get('relationship_kind')=='delegated':
                        # Keep the last applied placement until explicit Reorder.
                        reparent_pending=True
                    else:
                        visual[agent_id]=parent
            visual={agent_id:parent for agent_id,parent in visual.items()
                    if agent_id in graph.nodes}
            overrides={agent_id:parent for agent_id,parent in visual.items()
                       if parent!=actual_parents.get(agent_id)}
            self.hierarchy_reparent_pending=reparent_pending
            allowed=(set(graph.nodes) if not agent_query and self.agent_filter=='all'
                     else graph.context_for_matches(match_ids))
            expanded=set(self.expanded_agents)
            if agent_query or self.agent_filter!='all':
                # Search and Recent/Active filters temporarily reveal every
                # required path without destroying the saved expansion choice.
                expanded |= graph.context_for_matches(match_ids)
            if overrides:
                # A frozen visual placement also needs its old ancestor path.
                for agent_id in list(allowed):
                    parent=overrides.get(agent_id,actual_parents.get(agent_id))
                    visited=set()
                    while parent and parent in graph.nodes and parent not in visited:
                        visited.add(parent)
                        allowed.add(parent)
                        if agent_query or self.agent_filter!='all':
                            expanded.add(parent)
                        parent=overrides.get(parent,actual_parents.get(parent))
            hint=None
            if self.agent_order_ids:
                hint={agent_id:index for index,agent_id in enumerate(self.agent_order_ids)}
            def tree_sort_key(agent_id):
                return agent_order_key(graph.nodes.get(agent_id) or {},
                                       newest_first=self.agent_sort_desc)
            natural=graph.flatten(expanded,allowed,sort_key=tree_sort_key)
            frozen=graph.flatten(expanded,allowed,order_hint=hint,
                                 parent_overrides=overrides,
                                 sort_key=tree_sort_key) if (hint or overrides) else natural
            if overrides:
                self.agent_visual_parents=visual
            else:
                self.agent_visual_parents=visual
            prior_known=set(hint or ())
            natural_known=[row['agent_id'] for row in natural if row['agent_id'] in prior_known]
            frozen_known=[row['agent_id'] for row in frozen if row['agent_id'] in prior_known]
            if prior_known and natural_known!=frozen_known:
                self.agent_order_pending=True
            if reparent_pending:
                self.agent_order_pending=True
            rows=frozen if (hint or overrides) else natural
            visible_agents=[]
            self.agent_row_meta={}
            for position,row in enumerate(rows):
                item=self.hierarchy_entries.get(row['agent_id'])
                if item is None:continue
                summary=graph.summary(row['agent_id'])
                node_meta=graph.nodes.get(row['agent_id'],{})
                parent_unavailable=bool(node_meta.get('parent_agent_id') and
                                        not node_meta.get('parent_available'))
                parent_retained=bool(parent_unavailable and item.get('parent_context'))
                unlinked=(node_meta.get('relationship_state') in {'unresolved','conflicted','invalid'} or
                          parent_unavailable)
                shown=dict(item)
                shown.update(_expanded=row['expanded'],_has_children=row['has_children'],
                             _tree_depth=row['depth'],_subtree=summary,_unlinked=unlinked,
                             _parent_unavailable=parent_unavailable,
                             _parent_retained=parent_retained)
                shown['_prefix']=tree_prefix(row['depth'],last=row['last'],
                    ancestor_last=row['ancestor_last'],ascii_only=self.ascii_tree)
                self.agent_row_meta[item['id']]=shown
                visible_agents.append((shown,item,row,summary,unlinked))
            desired=[shown['agent_id'] for shown,_,_,_,_ in visible_agents]
        else:
            self.agent_visual_parents={
                agent_id:(node.get('parent_agent_id') if node.get('relationship_state')=='confirmed'
                          and node.get('relationship_kind')=='delegated' and
                          node.get('parent_agent_id') in graph.nodes else None)
                for agent_id,node in graph.nodes.items()}
            self.hierarchy_reparent_pending=False
            matching.sort(key=lambda item: agent_order_key(item, self.agent_sort_desc))
            desired=[item['agent_id'] for item in matching]
            visible_agents=[(item,item,None,None,False) for item in matching]
            self.agent_row_meta={item['id']:item for item in matching}
        order_key=(self.scope_value,agent_query,self.agent_filter,tree_mode,
                   self.agent_sort_desc)
        if order_key!=self.agent_order_key:
            self.agent_order_key=order_key
            self.agent_order=[shown['id'] for shown,_,_,_,_ in visible_agents]
            self.agent_order_ids=list(desired)
            self.agent_order_pending=False
        elif tree_mode:
            # The tree projection already froze root/sibling order; applying a
            # second global row freeze would place a new child after unrelated
            # roots instead of under its parent.
            self.agent_order=[shown['id'] for shown,_,_,_,_ in visible_agents]
            self.agent_order_ids=[shown['agent_id'] for shown,_,_,_,_ in visible_agents]
        else:
            by_id={shown['id']:(shown,item,row,summary,unlinked)
                   for shown,item,row,summary,unlinked in visible_agents}
            stable=[source_id for source_id in self.agent_order if source_id in by_id]
            stable.extend(shown['id'] for shown,_,_,_,_ in visible_agents
                          if shown['id'] not in stable)
            if stable!=[shown['id'] for shown,_,_,_,_ in visible_agents]:
                self.agent_order_pending=True
            self.agent_order=stable
            visible_agents=[by_id[source_id] for source_id in stable]
            self.agent_order_ids=[shown['agent_id'] for shown,_,_,_,_ in visible_agents]
        self.query_one('#refresh-order',Button).display=self.agent_order_pending
        self.visible_agent_rows=[shown['id'] for shown,_,_,_,_ in visible_agents]
        if self.selected_source not in self.visible_agent_rows:
            self.selected_source=self.visible_agent_rows[0] if self.visible_agent_rows else None
        rows=[]
        for shown,item,row,summary,unlinked in visible_agents:
            announced=shown.get('discovery_state')=='announced'
            rows.append((shown['id'],[
                agent_list_text(shown,shown['id']==self.selected_source,
                    prefix=shown.get('_prefix',''),summary=summary,announced=announced,
                    unlinked=unlinked,depth=shown.get('_tree_depth',0),
                    ascii_only=self.ascii_tree,
                    parent_unavailable=shown.get('_parent_unavailable',False),
                    parent_retained=shown.get('_parent_retained',False)),
                agent_status(shown),
                cell(age(shown['stamp']),'#abb4a5')],shown))
        self.fill('agent-table',rows,preserve=self.selected_source)
        if agent_query:
            context=max(0,len(visible_agents)-len(match_ids))
            status=f"{quantity(len(match_ids),'matching session')}"
            if context:status+=f" · {quantity(context,'ancestor context row')}"
            if not match_ids:status='No matching sessions · Clear to remove this filter'
            self.query_one('#search-status-agents',Static).update(status)
            title=f"Sessions · {len(match_ids)} matches · {len(visible_agents)} rows"
        else:
            self.search_status('agents',len(visible_agents),'session')
            title=f"Sessions · {len(visible_agents)}"
        self.query_one('#results-agents').border_title=title
        self.show_agent()

    def select_agent(self,source_id):
        previous=self.selected_source
        self.selected_source=source_id
        table=self.query_one('#agent-table',ResultsTable)
        for key in {previous,source_id}:
            if key not in table.full_rows:continue
            values=list(table.full_rows[key])
            item=getattr(self,'agent_row_meta',{}).get(key,self.source_map[key])
            summary=item.get('_subtree')
            values[0]=themed_text(table,agent_list_text(item,key==source_id,
                prefix=item.get('_prefix',''),summary=summary,
                announced=item.get('discovery_state')=='announced',
                unlinked=item.get('_unlinked',False),depth=item.get('_tree_depth',0),
                ascii_only=self.ascii_tree,
                parent_unavailable=item.get('_parent_unavailable',False),
                parent_retained=item.get('_parent_retained',False)))
            table.full_rows[key]=values
            if 0 in table.visible_columns:table.update_cell(key,'0',values[0])
        self.show_agent()

    def render_activity(self):
        data=self.data
        activity_items=self.visible_results('activity',data.activity_results if data.activity_results is not None else data.activity)
        activity_items=self.hold_selected('activity',activity_items,self.selected_activity,'source_id')
        activity=[(item.get('id') or item['source_id'],[cell(table_when(item['time_unix_nano'])),cell(item['domain'].upper(),'#c5adff'),status(item.get('result')),cell(headline(item))],item) for item in activity_items]
        self.query_one('#results-activity').border_title='User prompts · newest first' if self.prompts_only else 'Activity stream'
        self.fill('activity-table',activity)
        self.search_status('activity',len(activity),'prompt' if self.prompts_only else 'event')
        if self.selected_activity not in self.table_data.get('activity-table',{}):self.selected_activity=activity[0][0] if activity else None
        self.show_activity()

    def render_mcp(self):
        data=self.data
        observed={row['server']:row for row in data.mcp_summary.get('servers',[])}
        metric_observed={row['server']:row for row in data.mcp_summary.get('metric_servers',[])}
        configured={}
        for item in data.local_status.get('mcp_configurations',[]):
            configured.setdefault(item['server'],[]).append(item)
        inventory=[]
        for server in sorted(set(observed)|set(metric_observed)|set(configured),key=str.casefold):
            calls=observed.get(server,{'calls':0,'tools':0,'errors':0})
            metric_calls=metric_observed.get(server,{}).get('calls',0)
            configs=configured.get(server,[])
            origins=', '.join(sorted({item['client']+' ('+item['scope']+')' for item in configs}))
            label=('Configured · '+origins if configs else
                   'Aggregate metric · Cursor' if metric_calls and server not in observed else 'Observed')
            if configs and server in observed:
                label='Observed · configured in '+origins
            if metric_calls and (configs or server in observed):
                label+=' · metric totals available'
            row={'server':server,'calls':calls.get('calls',0),'metric_calls':metric_calls,
                 'tools':calls.get('tools',0),'errors':calls.get('errors',0)}
            inventory.append((server,[cell(server),cell(label,'#ffbd69' if configs else '#b9afc5'),
                cell(number(row['calls'])),cell(number(row['metric_calls'])),
                cell(number(row['tools'])),cell(number(row['errors']))],row))
        unknown_calls=data.mcp_summary.get('unknown_server_calls',0)
        if unknown_calls:
            unknown={'server':'Not reported','calls':unknown_calls,
                     'metric_calls':0,'tools':data.mcp_summary.get('unknown_server_tools',0),
                     'errors':data.mcp_summary.get('unknown_server_errors',0)}
            inventory.append(('Not reported',[cell('Not reported'),
                cell('Server identity missing · attribution unavailable','#ffbd69'),
                cell(number(unknown['calls'])),cell('0'),cell(number(unknown['tools'])),
                cell(number(unknown['errors']))],unknown))
        self.fill('mcp-servers',inventory)
        mcp_items=self.visible_results('mcp',data.mcp_results if data.mcp_results is not None else data.mcp)
        mcp_items=self.hold_selected('mcp',mcp_items,self.selected_mcp,'id')
        self.fill('mcp-table',[(item['id'],[cell(table_when(item['time_unix_nano']),'#b9afc5'),cell((item.get('service') or 'Unknown')+' · '+str(item.get('session_id') or '—')[-6:]),cell(item['server'],'#c5adff'),cell(item['tool'] if item.get('tool') not in ('', 'Not reported') else item.get('method') or 'Not reported'),status(item['status']),cell('—' if item['duration_ms'] is None else timing(item['duration_ms']))],item) for item in mcp_items])
        if self.selected_mcp not in self.table_data.get('mcp-table',{}):self.selected_mcp=mcp_items[0]['id'] if mcp_items else None
        self.show_mcp()
        if not data.mcp_available:
            mcp_hint='This receiver version does not expose MCP operations. Update and restart the receiver.'
        elif data.queries.get('mcp','')!=self.search_queries['mcp']:
            mcp_hint='Filtering MCP operations…'
        elif mcp_items:
            server_count=len({item.get('server') for item in mcp_items if item.get('server') and item['server']!='Not reported'})
            error_count=sum(str(item.get('status','')).upper() in {'ERROR','FAILED'} for item in mcp_items)
            mcp_hint=f"{quantity(len(mcp_items),'matching operation' if self.search_queries['mcp'] else 'operation')} · {quantity(server_count,'server')} · {quantity(error_count,'error')}"
        elif self.search_queries['mcp']:
            mcp_hint='No matching MCP operations · Clear to show all calls in this scope'
        elif data.mcp_summary.get('metric_calls'):
            mcp_hint=(f"No per-call events · {quantity(data.mcp_summary['metric_calls'],'aggregate MCP metric call')} "
                      "from Cursor; select Metrics for the underlying series")
        else:
            mcp_hint='No MCP operations in this scope · General tool activity is available in Activity'
        if 'mcp' in self.errors_only:mcp_hint=f"{quantity(len(mcp_items),'failed call')} · errors only · current results"
        config_errors=data.local_status.get('mcp_config_errors',0)
        if config_errors:
            mcp_hint+=f" · {quantity(config_errors,'MCP config could not be read')}"
        self.query_one('#mcp-hint',Static).update(mcp_hint)

    def render_traces(self):
        data=self.data
        traces=self.visible_results('traces',data.traces[:])
        detail=data.detail
        if self.feed.pinned_trace and detail.get('summary') and not any(item['trace_id']==detail['trace_id'] for item in traces):traces.insert(0,detail['summary'])
        self.fill('trace-table',[(item['trace_id'],[cell('Active · errors','yellow') if item.get('active') and item['status']=='ERROR' else cell('Error','red') if item['status']=='ERROR' else status(item['status']),cell(trace_name(item)),cell(item['span_count']),cell(observed_count(item,'input_tokens')+' / '+observed_count(item,'output_tokens'))],item) for item in traces],preserve=self.feed.selected_trace)
        self.search_status('traces',len(traces),'trace')
        self.show_spans()

    def render_findings(self):
        data=self.data
        finding_query=self.search_queries['findings'].casefold()
        finding_items=[item for item in data.finding_results if not finding_query or finding_query in ' '.join(
            str(item.get(key) or '') for key in ('severity','kind','title','detail','task','session_id','services','run_status')).casefold()]
        self.fill('finding-table',[(item['id'],[
            cell(str(item.get('severity') or 'Unknown').upper(),'#ff8799' if item.get('severity')=='high' else '#ffbd69' if item.get('severity')=='medium' else '#c5adff'),
            cell(item.get('kind') or 'Finding'),cell(item.get('title') or 'Review finding'),
            cell(number(item.get('occurrence_count',1))),cell(age(item.get('time_unix_nano',0)) if item.get('time_unix_nano') else '—')],item)
            for item in finding_items])
        self.search_status('findings',len(finding_items),'finding')
        if self.selected_finding not in self.table_data.get('finding-table',{}):
            self.selected_finding=finding_items[0]['id'] if finding_items else None
        self.show_finding()

    def render_paging(self,page):
        paging=self.data.pagination.get(page,{})
        page_number=paging.get('page',1)
        self.query_one('#history-label-'+page,Static).update('Latest' if page_number==1 else f'History · page {page_number}')
        self.query_one('#older-'+page,Button).disabled=not paging.get('next_cursor')
        self.query_one('#newer-'+page,Button).disabled=page_number==1
        self.query_one('#latest-'+page,Button).disabled=page_number==1
        self.query_one('#older-'+page).tooltip='Load older saved records · ]' if paging.get('supported') else 'Restart the updated receiver to enable history paging'

    def render_host(self):
        data=self.data
        processes=(data.local_status.get('runtime') or {}).get('processes',[])
        host_query=self.search_queries['host'].casefold()
        visible_processes=[item for item in processes if not host_query or host_query in ' '.join(str(item.get(key) or '') for key in ('name','pid','parent_pid','project','state')).casefold()]
        self.fill('host-table',[(str(item['pid']),[cell(item['name']),cell(item['pid']),cell(item.get('project') or '—'),cell(item['state']),cell(f"{item['memory_rss_kib']/1024:.0f} MiB"),cell(f"{item['cpu_seconds']:.1f} s")],item) for item in visible_processes])
        self.search_status('host',len(visible_processes),'process','processes')
        runtime=data.local_status.get('runtime') or {}
        opencode_sessions=sum(entry.get('runtime')=='OpenCode' for entry in data.local_status.get('sessions',[]))
        self.inspect('host-detail','Host & collection status',[
            ('Host',runtime.get('host','Not reported')),('Platform',str(runtime.get('os',''))+' / '+str(runtime.get('architecture',''))),
            ('Load average',', '.join(str(value) for value in runtime.get('load_average',[])) or 'Unavailable'),
            ('Sources',f"{data.local_status.get('sources',0)} Codex / Claude files · {opencode_sessions} OpenCode sessions · lookback {data.local_status.get('lookback_hours',24)}h · read errors {data.local_status.get('read_errors',0)+data.local_status.get('opencode_read_errors',0)}"),
             ('Coverage','Source logs can be partial. Costs and model-call latency are unavailable unless reported. CPU is cumulative process time.')],[])

    def render_metrics(self):
        data=self.data
        metric_query=self.search_queries['metrics'].casefold()
        selected_source=getattr(self,'source_map',{}).get(self.scope_value)
        scoped_service=selected_source.get('service') if selected_source else None
        visible_catalog=[]
        for item in data.metric_catalog:
            if scoped_service and item.get('service')!=scoped_service:
                continue
            searchable=' '.join(str(item.get(key) or '') for key in
                ('name','service','kind','unit','description','scope_name')).casefold()
            if metric_query and metric_query not in searchable:
                continue
            visible_catalog.append(item)
        def metric_key(item):
            return json.dumps([item.get(key) for key in
                ('service','name','kind','temporality','scope_name','unit','description')],ensure_ascii=False)
        latest_by_key={}
        for point in data.metric_points:
            latest_by_key.setdefault(metric_key(point),point)
        metric_rows=[]
        for item in visible_catalog:
            latest=latest_by_key.get(metric_key(item))
            metric_rows.append((metric_key(item),[
                cell(item.get('name') or 'unnamed'),cell(item.get('service') or 'unknown'),
                cell(item.get('kind') or '—'),cell(metric_value(latest) if latest else 'No point'),
                cell(item.get('unit') or '—'),cell(number(item.get('point_count') or 0))],item))
        self.fill('metrics-table',metric_rows,preserve=self.selected_metric)
        if self.selected_metric not in self.table_data.get('metrics-table',{}):
            self.selected_metric=metric_rows[0][0] if metric_rows else None
        metric=self.table_data.get('metrics-table',{}).get(self.selected_metric)
        if metric:
            selected_metric_key=metric_key(metric)
        else:
            selected_metric_key=None
            self.metric_history_requested_key=None
            self.metric_history_loaded_key=None
            self.metric_history_points=[]
        if not data.metrics_available:
            metric_hint='This receiver version does not expose OTLP metrics. Update and restart the receiver.'
        elif metric_query and not metric_rows:
            metric_hint='No matching metric instruments · Clear to show the catalog.'
        elif metric_rows:
            metric_hint=quantity(len(metric_rows),'instrument')+' · latest readings from stored OTLP points'
        else:
            metric_hint='No OTLP metrics recorded in this scope.'
        self.query_one('#search-status-metrics',Static).update(metric_hint)
        self.query_one('#usage-series',Static).update(usage_series_text(
            data.usage_series,data.usage_service,data.usage_session_id))
        if metric:
            history=self.metric_history_points if self.metric_history_loaded_key==selected_metric_key else []
            latest=history[0] if history else latest_by_key.get(metric_key(metric))
            exemplar=self.metric_exemplar()
            self.query_one('#metrics-trace',Button).disabled=exemplar is None
            point_fields=[]
            if latest:
                temporality=('Cumulative (implicit for Summary)' if latest.get('kind')=='summary' else
                    {0:'Unspecified',1:'Delta',2:'Cumulative'}.get(latest.get('temporality'),str(latest.get('temporality'))))
                point_fields=[('Latest reading',metric_value(latest)),('Observed',age(latest.get('time_unix_nano'))),
                    ('Temporality',temporality),
                    ('Point start',when(latest.get('start_unix_nano')) if latest.get('start_unix_nano') else 'Not reported')]
                monotonic=(latest.get('data') or {}).get('isMonotonic')
                if latest.get('kind')=='sum' and isinstance(monotonic,bool):
                    point_fields.append(('Semantics','Monotonic counter' if monotonic else 'Up/down sum'))
            history_lines=[]
            for point in history[:12]:
                stamp=point.get('time_unix_nano')
                label=when(stamp) if stamp else 'Unknown time'
                attrs=point.get('attributes') or {}
                suffix=' · '+json.dumps(attrs,ensure_ascii=False,sort_keys=True) if attrs else ''
                history_lines.append(f"{label}  {metric_value(point)} {metric.get('unit') or ''}{suffix}".rstrip())
            self.inspect('metrics-detail','OTLP metric instrument',[
                ('Name',metric.get('name')),('Service',metric.get('service')),('Kind',metric.get('kind')),
                ('Unit',metric.get('unit') or 'Unitless'),('Description',metric.get('description') or 'Not reported'),
                ('Instrumentation scope',metric.get('scope_name') or 'Not reported'),
                ('Stored points',metric.get('point_count',0)),*point_fields],
                [('Latest point attributes',latest.get('attributes') if latest else None),
                 ('Resource attributes',latest.get('resource_attributes') if latest else None),
                 ('Recent points · newest first','\n'.join(history_lines) if history_lines else None),
                 ('Point data',latest.get('data') if latest else None)])
        else:
            self.query_one('#metrics-trace',Button).disabled=True
            self.inspect('metrics-detail','OTLP metric instruments',[('Status',metric_hint)],[])

    def scope_label(self):
        item=getattr(self,'source_map',{}).get(self.scope_value)
        if not item:return 'all agent sessions'
        label=f"{item['runtime']} / {item.get('project') or 'SDK'}"
        page=self.query_one('#pages',TabbedContent).active
        if item.get('agent_id') and page in {'activity','mcp','traces','findings'}:
            label+=' · subtree' if self.evidence_scope_subtree else ' · this agent'
        return label

    def hold_selected(self,page,items,selected,key):
        """Keep the event being read when new arrivals push it off the live page."""
        current=self.table_data.get(page+'-table',{}).get(selected)
        if current and selected not in {item.get('id') or item[key] for item in items}:
            return [*items,current]
        return items

    def inspect(self,name,title,fields,sections,*,overview=False):
        log=self.query_one('#'+name,RichLog)
        width=log.scrollable_content_region.width
        # Live/selection callbacks can run after a pane has been hidden. RichLog
        # would render at min_width=1 and retain those strips after it is shown.
        # Don't cache an unrendered update; on_show rebuilds the latest evidence.
        if not width:return
        signature=(title,str(fields),str(sections),overview,width)
        if self.inspector_signatures.get(name)==signature:return
        previous=self.inspector_signatures.get(name)
        self.inspector_signatures[name]=signature
        scroll=log.scroll_offset
        log.clear();log.write(Text(title,style='bold #f6f0e8'))
        if overview:log.write('')
        table=Table.grid(padding=(0,2));table.add_column(style=themed_style(log,'#b9afc5'),no_wrap=True)
        table.add_column(no_wrap=overview,overflow='ellipsis')
        for label,value in fields:
            value=themed_text(log,value) if isinstance(value,Text) else Text(str('Not reported' if value is None or value=='' else value))
            table.add_row(Text(label),value)
        log.write(table)
        for label,value in sections:
            if overview:
                if not value:continue
                log.write(Text('\n\n'+label.upper(),style='#b9afc5'))
                log.write(Text(str(value),style='#f6f0e8'))
            elif label=='Latest recorded event' and value:
                log.write(Panel(Text(value,style=themed_style(log,'#ffbd69')),
                    title=Text('LATEST RECORDED EVENT',style=themed_style(log,'#b9afc5')),
                    title_align='left',border_style=themed_style(log,PALETTES['Current']['border-subtle']),
                    style='on '+PALETTES[self.theme]['canvas'],padding=(0,1)))
            else:
                self.write_evidence(log,label,value)
        if previous and previous[0]==title and previous[2]==signature[2]:
            log.scroll_to(y=scroll.y,animate=False)

    def write_evidence(self,log,label,value,depth=0):
        if value is None or value=='':return
        value=decode_nested(value)
        if isinstance(value,str) and label.casefold() in {'recorded content','text','output','response'}:
            segments=embedded_json(value)
            if any(kind=='json' for kind,_ in segments):
                log.write(Text('\n'+label.title(),style='bold #c5adff'))
                for kind,content in segments:
                    if kind=='text':
                        if content.strip():log.write(Text(content.strip('\n')))
                    else:
                        write_json(log,content,self.theme)
                return
        # Native log adapters may wrap each piece of the same record in a
        # one-key `recorded_content` envelope. Unwrap that without consuming
        # the JSON nesting budget or repeating the label at every level.
        if isinstance(value,dict) and len(value)==1:
            key,child=next(iter(value.items()))
            if key.lower() in {'recorded_content','content','payload'}:
                self.write_evidence(log,label,child,depth);return
            if key.lower()=='wrapper':
                self.write_evidence(log,label,child,depth+1);return
        if depth==0 and isinstance(value,(dict,list)) and value:
            log.write(Text('\n'+label.upper(),style='bold #ffbd69'))
            if label.lower()=='arguments':
                write_json(log,value,self.theme)
                return
        if isinstance(value,(dict,list)) and not value:
            log.write(Text('\n'+label.title(),style='bold #c5adff'))
            log.write(Text('No entries' if isinstance(value,list) else 'Empty object'))
            return
        if isinstance(value,dict) and value.get('type') in {'text','input_text','output_text'} and 'text' in value:
            self.write_evidence(log,label,value['text'],depth+1);return
        # Any structured value nested in captured evidence is easier to scan
        # as JSON than as a sequence of flattened label/value rows. This also
        # handles JSON strings stored under fields such as `output` or `text`.
        if depth and isinstance(value,(dict,list)):
            write_json(log,value,self.theme)
            return
        if depth<64 and isinstance(value,dict):
            for key,child in value.items():
                self.write_evidence(log,str(key).replace('_',' '),child,depth+1)
            return
        if depth<64 and isinstance(value,list):
            for child in value:self.write_evidence(log,label,child,depth+1)
            return
        if isinstance(value,(dict,list)):
            log.write(Text('\n'+label.title(),style='bold #c5adff'))
            write_json(log,value,self.theme)
            return
        log.write(Text('\n'+label.title(),style='bold #c5adff'))
        if label.lower()=='command':
            command=shell_command(value)
            script=heredoc_script(command)
            if script:
                language,title,body=script
                log.write(Text(title,style='bold #c5adff'))
                log.write(evidence_syntax(body or '# Empty script',language,self.theme))
                return
            log.write(evidence_syntax(command,'bash',self.theme))
        elif label.lower() in {'diff','patch','unified diff','code'}:
            lexer='diff' if 'diff' in label.lower() or label.lower()=='patch' else 'python' if label.lower()=='code' else 'bash'
            log.write(evidence_syntax(str(value),lexer,self.theme))
        else:log.write(Text(str(value)))

    def hierarchy_sections(self,item):
        graph=self.hierarchy_graph
        if not graph or not item or not item.get('agent_id'):return [], []
        node=graph.nodes.get(item['agent_id'])
        if not node:return [], []
        if not any(related.get('parent_agent_id') or related.get('discovery_state')=='announced'
                   or related.get('relationship_state')=='conflicted'
                   for related in graph.nodes.values()):
            # A flat inventory with no relationship evidence stays as concise as
            # before; absence of parent metadata is already visible in list rows.
            return [], []
        fields=[]
        sections=[]
        path=graph.path_to(item['agent_id'])
        if len(path)>1:
            labels=[]
            for agent_id in path:
                related=graph.nodes[agent_id]
                labels.append(task_preview(related) or 'Untitled session')
            fields.append(('Breadcrumb',' → '.join(labels)))
        parent_id=node.get('parent_agent_id')
        if parent_id and node.get('parent_available'):
            parent=graph.nodes.get(parent_id,{})
            fields.append(('Parent agent',task_preview(parent) or 'Parent agent'))
        if node.get('relationship_kind')=='forked':
            sections.append(('Relationship','Fork/continuation recorded. A fork is not counted as a delegated worker.'))
        elif parent_id and not node.get('parent_available'):
            context=item.get('parent_context') or {}
            if context.get('retained'):
                sections.append(('Parent relationship',
                    'Parent identity is retained from hierarchy history but is outside the current source window. '
                    'It is not shown or counted as a monitored row; the child remains independently inspectable.'))
            else:
                sections.append(('Parent relationship',
                    'Parent relationship was reported, but parent evidence is not in the current retained or visible set. This child remains a monitored observation, not a working descendant of a placeholder.'))
        elif node.get('relationship_state')=='unresolved':
            sections.append(('Parent relationship','Parent relationship not reported. This session is shown as unlinked rather than assumed to be a root.'))
        elif node.get('relationship_state')=='conflicted':
            sections.append(('Parent relationship',
                'Conflicting parent claims were recorded. The child is shown as unlinked until the relationship is resolved.'))
        elif node.get('relationship_state')=='invalid':
            sections.append(('Parent relationship','Invalid or cyclic relationship metadata was rejected; this agent is shown unlinked.'))
        if node.get('relationship_provenance'):
            sections.append(('Delegation provenance',clean(node['relationship_provenance'])))
        if node.get('discovery_state')=='announced':
            sections.append(('Child telemetry','Child telemetry not yet captured. The spawn was recorded; status is unavailable and this node is not counted as working.'))
        children=graph.children_of(item['agent_id'])
        summary=graph.summary(item['agent_id'])
        if children:
            lines=[]
            for child_id in children:
                child=graph.nodes[child_id]
                lines.append(f"{task_preview(child) or 'Untitled session'} · {child.get('state') or 'Unknown'}")
            sections.append(('Direct children', '\n'.join(lines[:20]) + ('\n…' if len(lines)>20 else '')))
        if summary['observed'] or summary['announced']:
            sections.append(('Subtree summary',
                f"{summary['working']} working · {summary['waiting']} waiting · {summary['failed']} failed · "
                f"{summary['attention']} need attention · coverage {summary['coverage']}"))
        sections.append(('Hierarchy actions',
            'Alt+Up inspect parent · Alt+Down first child · Alt+Right next descendant needing attention · '
            'f switches tree/flat'))
        return fields, sections

    def show_agent(self):
        item=getattr(self,'source_map',{}).get(self.selected_source)
        button=self.query_one('#agent-summary',Button)
        button.display=self.ai_summary_available and bool(item)
        button.disabled=not bool(item) or self.summary_busy or not self.ai_summary_available
        if not self.ai_summary_available:
            button.tooltip='No compatible API key or provider is configured. The recorded progress summary below works without setup.'
        if not item:
            self.query_one('#agent-open',Button).disabled=True
            self.query_one('#agent-trace',Button).disabled=True
            self.query_one('#agent-reviewed',Button).display=False
            self.query_one('#agent-dismissed',Button).display=False
            self.fill('agent-actions',[])
            if self.search_queries['agents']:
                self.inspect('agent-detail','No matching agent sessions',[],[('Search','Clear the filter in Agent sessions to show all sessions in this scope.')])
            elif self.agent_filter=='recent':
                self.inspect('agent-detail','No recent sessions',[],[
                    ('Show all history','Choose All in the session filter to include older sessions.'),
                    ('Waiting for work','Working sessions appear here automatically.'),
                ])
            elif self.agent_filter=='active':
                self.inspect('agent-detail','No active sessions',[],[
                    ('Show recent','Choose Recent to include the latest finished session.'),
                    ('Show all history','Choose All to include older sessions.'),
                ])
            else:
                self.inspect('agent-detail','No agent sessions found',[],[('Next step',FIRST_RUN),('Diagnostics / retention','Press d for source paths, read health, and retention instructions.')])
            return
        self.query_one('#agent-open',Button).disabled=False
        summary=next((entry for entry in self.data.sessions
                      if item.get('agent_id') and entry.get('agent_id')==item.get('agent_id')),None)
        if summary is None:
            summary=next((entry for entry in self.data.sessions
                if item.get('session_id') and entry.get('session_id')==item.get('session_id')
                and (not entry.get('agent_id') or not item.get('agent_id'))),{})
        trace_button=self.query_one('#agent-trace',Button)
        trace_button.disabled=not bool(self.linked_agent_trace())
        reviewed_button=self.query_one('#agent-reviewed',Button)
        reviewed= item['id'] in self.reviewed_attention
        reviewed_button.display=attention_required(item)
        reviewed_button.label='Reviewed ✓ · r' if reviewed else 'Mark reviewed · r'
        reviewed_button.tooltip=('Acknowledged for this run. Press r to restore it to Needs attention.' if reviewed else
                                 'Acknowledge this attention item for this run; does not change the recorded session status.')
        dismissed=attention_key(item) in self.dismissed_attention
        dismissed_button=self.query_one('#agent-dismissed',Button)
        dismissed_button.display=attention_required(item)
        dismissed_button.label='Restore · x' if dismissed else 'Dismiss · x'
        dismissed_button.tooltip=('Restore this failure to Recent and Needs attention.' if dismissed else
                                  'Hide this failure from Recent and Needs attention; keep it in All history and Traces.')
        latest=item.get('last_activity') or {}
        generated=self.ai_summaries.get(item['id'])
        ai_sections=[]
        if generated:
            stamp,text=generated
            label='AI summary · earlier snapshot' if stamp!=item.get('stamp') else 'AI summary · provider generated'
            ai_sections=[(label,text)]
        if item.get('agent_id'):
            actions=[event for event in self.data.activity
                     if event.get('agent_id')==item.get('agent_id')]
            if not actions:
                actions=[event for event in self.data.activity
                         if item.get('session_id') and event.get('session_id')==item.get('session_id')
                         and not event.get('agent_id')]
        elif item.get('session_id'):
            actions=[event for event in self.data.activity
                     if event.get('session_id')==item.get('session_id')]
        else:
            actions=[event for event in self.data.activity
                     if event.get('service')==item.get('service')]
        meaningful=[event for event in actions if event.get('domain') in {'shell','file','mcp','browser','model','message'}]
        actions=sorted(meaningful or actions,key=lambda event:event.get('time_unix_nano',0),reverse=True)
        recorded=next((event for event in actions if event.get('domain')!='message'),actions[0] if actions else latest)
        error_event=next((event for event in actions if str(event.get('result') or event.get('status') or '').casefold()
                          in {'error','failed','denied','blocked','policy_denied','incomplete'}),None)
        failure_detail=failure_evidence(error_event)
        request_event=next((event for event in actions if event.get('domain')=='message'
                            and event.get('action')=='user'),None)
        if request_event:
            request_text=recorded_message_text(request_event).replace('\n',' ')
            request_limit=360
            if cell_len(request_text)>request_limit:
                request_text=clipped(request_text,request_limit)+'… Open Activity for the full request.'
            request_text+='  ·  '+age(request_event.get('time_unix_nano'))
        else:
            request_text=''
        self.agent_primary_event_id=((error_event or request_event or recorded or {}).get('id')
                                     or (error_event or request_event or recorded or {}).get('source_id'))
        primary_label=('Inspect failed call · s' if error_event else
                       'View recorded request · s' if input_required(item) and request_event else
                       'Activity · s')
        primary_tip=('Open the exact recorded failed event' if error_event else
                     'Open the recorded user request; reply in the agent terminal' if request_event and input_required(item) else
                     'Read all recorded activity for this session')
        self.query_one('#agent-open',Button).label=Text(primary_label)
        self.query_one('#agent-open',Button).tooltip=primary_tip
        # Retain trace links without rendering a duplicate activity table.
        self.table_data['agent-actions']={event.get('id') or event['source_id']:event for event in actions[:30]}
        if not self.query_one('#agents').has_class('expanded'):
            title=task_preview(item) or 'Session overview'
            title=title[:71]+'…' if len(title)>72 else title
            agent=item['runtime']+(' / '+str(item['project']) if item.get('project') else '')
            fields=[('Agent',agent),('Status',agent_status(item))]
            if not request_event:
                fields.append(('Request','Not captured'))
            if item['stamp']:
                fields.append(('Updated',age(item['stamp'])))
            sections=[('Latest recorded action',brief_activity(recorded) if recorded else 'No activity captured yet.')]
            if failure_detail:
                sections.insert(0,('Failure details',failure_detail))
            if request_text:
                sections.insert(0,('Latest recorded request',request_text))
            sections.append(('What happened',attention_evidence(item,summary,error_event)))
            if reviewed:
                sections.append(('Review state','Marked reviewed for this run. The session status and recorded evidence are unchanged.'))
            if dismissed:
                sections.append(('Attention state','Dismissed from Recent and Needs attention. Still available in All history and Traces.'))
            if input_required(item):
                sections.append(('Next step',wait_summary(item)))
            elif item['state'] in {'Possibly stalled','Unconfirmed','Monitor delayed','Catching up','Error','Failed'}:
                sections[-2 if reviewed else -1]=('What happened',attention_evidence(item,summary,error_event)[:400])
            hierarchy_fields,hierarchy_sections=self.hierarchy_sections(item)
            fields.extend(hierarchy_fields)
            self.inspect('agent-detail',title,fields,
                hierarchy_sections+sections+ai_sections,overview=True)
            return
        progress=recorded_progress(item,actions,summary)
        preview=''
        if recorded:
            content=decode_nested(recorded.get('detail') or '')
            if isinstance(content,dict):
                content=next((content[key] for key in ('output','response','result') if content.get(key)),content)
            if isinstance(content,(dict,list)):
                content=json.dumps(content,ensure_ascii=False,indent=2)
            preview=clean(headline(recorded))[:500]
            if content:
                preview+='\n'+clean(str(content))[:8000]
        work_detail=(f"{reported_number(summary.get('trace_count'))} traces · {reported_number(item.get('spans'))} spans · "
                     f"{observed_count(summary,'input_tokens')} in / {observed_count(summary,'output_tokens')} out")
        token_detail=(f"Cache read {observed_count(summary,'cache_read_input_tokens')} · "
                      f"cache write {observed_count(summary,'cache_write_input_tokens')} · "
                      f"reasoning output {observed_count(summary,'reasoning_output_tokens')}")
        cost_detail=('Not reported' if not summary or summary.get('unknown_cost_runs') or summary.get('known_cost_usd') is None
                     else f"${summary.get('known_cost_usd',0):.4f} known")
        detail_fields=[('Session ID',item.get('session_id') or item['id']),
            ('Repo / project',item.get('project')),('Branch',item.get('branch')),('Model',item.get('model'))]
        if summary:
            detail_fields.extend([('Work',work_detail),('Token breakdown',token_detail),('Cost',cost_detail)])
        session_details='\n'.join(f'{label}: {value}' for label,value in detail_fields if value)
        fields=[('Agent',item['runtime']),('Status',item['state'])]
        if item['state']=='Waiting' or item.get('waiting_for'):
            fields.append(('Waiting on',wait_label(item)))
        if item['stamp']:
            fields.append(('Updated',when(item['stamp'])+' · '+age(item['stamp'])))
        prompt_events=sorted((event for event in actions
                              if event.get('domain')=='message' and event.get('action')=='user'),
                             key=lambda event:event.get('time_unix_nano') or 0)
        prompt_limit=10
        if prompt_events:
            shown_prompts=prompt_events[-prompt_limit:]
            prompt_lines=[]
            if len(prompt_events)>prompt_limit:
                prompt_lines.append(f'Showing the latest {prompt_limit} of {len(prompt_events)} recorded prompts; see Activity for the full history.\n')
            for event in shown_prompts:
                stamp=event.get('time_unix_nano')
                prompt_lines.extend([age(stamp) if stamp else 'Time not reported',recorded_message_text(event),''])
            prompt_history='\n'.join(prompt_lines).strip()
        else:
            prompt_history='No user prompts were captured for this session.'
        activity_events=[event for event in actions if event.get('domain')!='message']
        activity_limit=15
        shown_activity=activity_events[-activity_limit:]
        activity_lines=[]
        if len(activity_events)>activity_limit:
            activity_lines.append(f'Showing the latest {activity_limit} of {len(activity_events)} recorded actions; see Activity for the full history.\n')
        for event in shown_activity:
            stamp=event.get('time_unix_nano')
            description=headline(event)
            if cell_len(description)>200:
                description=clipped(description,197)+'…'
            result=event.get('result') or event.get('status')
            recorded_result=f' · {clean(str(result))}' if result else ''
            detail='\n'.join(clean(line) for line in str(event.get('detail') or '').splitlines()).strip()
            if detail:
                detail=detail[:2000]+('… [detail clipped]' if len(detail)>2000 else '')
            activity_lines.append(f"{age(stamp) if stamp else 'Time not reported'} · {brief_activity(event)}{recorded_result}\n  {description}"+
                                  (f'\n  {detail}' if detail else ''))
        activity_history='\n'.join(activity_lines) if activity_lines else 'No non-message actions were captured for this session.'
        hierarchy_fields,hierarchy_sections=self.hierarchy_sections(item)
        fields.extend(hierarchy_fields)
        self.inspect('agent-detail',clean(task_preview(item) or f"Untitled session · {item.get('runtime') or 'agent'}"),fields,
            hierarchy_sections+[('Latest recorded event',preview),('Progress summary · recorded activity',progress),
             ('Recorded user prompts',prompt_history),('Recent recorded activity',activity_history)]+ai_sections+[
              ('Session details',session_details),
              ('Analysis coverage',analysis_coverage(summary)),
              ('Working directory',item.get('working_directory')),
              ('Status evidence',attention_evidence(item,summary,error_event)),
              ('Review state','Marked reviewed for this run; status and evidence are unchanged.' if reviewed else None),
              ('Attention state','Dismissed from Recent and Needs attention. Still available in All history and Traces.' if dismissed else None)])

    def show_activity(self):
        item=self.table_data.get('activity-table',{}).get(self.selected_activity)
        self.query_one('#activity-trace',Button).disabled=not bool(item and item.get('trace_id'))
        if not item:
            if self.data.activity_prompts!=self.prompts_only or self.data.queries.get('activity','')!=self.search_queries['activity']:
                self.inspect('activity-detail','Filtering activity',[],[('Status','Waiting for matching events from the receiver.')]);return
            if 'activity' in self.errors_only:
                self.inspect('activity-detail','No failed events',[],[('Errors filter','No failures in the current results. Turn off Errors to see all activity.')]);return
            if self.prompts_only:
                self.inspect('activity-detail','No user prompts in this scope',[],[('Prompts','No user prompts were recorded for this agent in the selected history. Choose another session or turn off Prompts to see all activity.')]);return
            self.inspect('activity-detail','No activity in this scope',[],[('Scope','Choose another agent session or clear the search to view recent activity.')]);return
        details=decode(item.get('detail') or '')
        sections=list(details.items()) if isinstance(details,dict) else [('Recorded content',details)]
        if item.get('metadata'):
            sections.append(('Runtime metadata',item['metadata']))
        self.inspect('activity-detail',headline(item),[('Agent',item.get('service')),('Type',item.get('domain')),('Result',str(status(item.get('result')))),
            ('When',when(item['time_unix_nano'])),('Duration',timing(item.get('duration_ms')))],
            sections+[('Source',item.get('source')),('Trace ID',item.get('trace_id')),('Span ID',item.get('span_id'))])

    def show_mcp(self):
        item=self.table_data.get('mcp-table',{}).get(self.selected_mcp)
        self.query_one('#mcp-trace',Button).disabled=not bool(item and item.get('trace_id'))
        if not item:
            if not self.data.mcp_available:
                title='MCP inventory unavailable'
                note='The connected receiver is too old to provide this endpoint. Update and restart the receiver.'
            elif self.data.queries.get('mcp','')!=self.search_queries['mcp']:
                title='Filtering MCP operations'
                note='Waiting for matching calls from the receiver.'
            elif 'mcp' in self.errors_only:
                title='No failed MCP calls'
                note='No failures in the current results. Turn off Errors to see all calls.'
            elif self.search_queries['mcp']:
                title='No matching MCP operations'
                note='Clear Search or choose another agent session. Search also matches server, tool, arguments, and result.'
            else:
                title='No MCP operations in this scope'
                note='Calls identified as Model Context Protocol appear here. Other tool activity remains in Activity.'
            self.inspect('mcp-detail',title,[('Scope',self.scope_label())],[('How to proceed',note)])
            return
        operation=item.get('tool') if item.get('tool') not in ('', 'Not reported') else item.get('method') or 'MCP operation'
        self.inspect('mcp-detail',item['server']+' / '+operation,[
            ('Result',str(status(item.get('status')))+' · '+timing(item.get('duration_ms'))),
            ('Agent',(item.get('service') or 'Not reported')+' · '+str(item.get('session_id') or 'Unknown session')[-12:]),
            ('Method',item.get('method') or 'Not reported'),('Observed',when(item.get('time_unix_nano') or 0))],
            [('Arguments',item.get('arguments')),('Response',item.get('output')),
             ('Runtime metadata',item.get('metadata')),
             ('Evidence',item.get('source')),('Session ID',item.get('session_id')),
             ('Trace ID',item.get('trace_id') or 'Not linked'),('Span ID',item.get('span_id') or 'Not linked')])

    def show_finding(self):
        item=self.table_data.get('finding-table',{}).get(self.selected_finding)
        self.query_one('#finding-trace',Button).disabled=not bool(item and item.get('trace_id'))
        if not item:
            paging=self.data.pagination.get('findings',{})
            note=('No findings in this page. Use Older to inspect earlier findings.'
                  if paging.get('page',1)>1 else
                  'Recorded security, policy, tool, and run findings appear here with links to their evidence. Analysis uses bounded evidence windows; no matching findings does not establish that no issues occurred.')
            self.inspect('finding-detail','No findings match this view',[],[('Coverage',note)])
            return
        evidence='\n'.join(f"{entry.get('type','evidence')} · {entry.get('id','not reported')}"
                           for entry in item.get('evidence',[])) or 'No linked evidence was recorded.'
        self.inspect('finding-detail',item.get('title') or 'Review finding',[
            ('Severity',str(item.get('severity') or 'Not reported').upper()),
            ('Type',item.get('kind') or 'Not reported'),
            ('Last observed',when(item.get('time_unix_nano') or 0)),
            ('Occurrences',f"{number(item.get('occurrence_count',1))} across {number(item.get('trace_count',1))} traces · {number(item.get('session_count',0))} sessions"),
            ('Agent / session',(item.get('services') or ['Not reported'])[0]+' · '+str(item.get('session_id') or 'Not reported')),
            ('Task',item.get('task') or 'Not reported')],
            [('What was observed',item.get('detail') or 'No additional detail reported.'),
             ('Evidence',evidence),('Trace ID',item.get('trace_id') or 'Not linked')])

    def show_spans(self):
        detail=self.data.detail
        if not detail.get('trace_id'):
            tree=self.query_one('#span-tree',Tree);tree.clear();tree.root.set_label('No trace selected')
            self.query_one('#span-info',RichLog).clear();self.span_signature=None
            return
        if detail.get('trace_id')!=self.feed.selected_trace:return
        spans=detail.get('spans',[])
        coverage=analysis_coverage(detail.get('summary') or {})
        self.query_one('#trace-detail').border_subtitle=f"{len(spans)} / {detail.get('span_page',{}).get('trace_total',len(spans))} spans loaded"+(' · partial analysis' if coverage else '')
        signature=(detail.get('trace_id'),coverage,tuple((item['span_id'],item['status'],item['end_unix_nano'],str(item['attributes'])) for item in spans))
        if signature==self.span_signature:return
        self.span_signature=signature
        tree=self.query_one('#span-tree',Tree)
        selected=tree.cursor_node.data.get('span_id') if tree.cursor_node and tree.cursor_node.data else None
        tree.clear()
        tree.root.set_label(Text('Trace '+str(detail.get('trace_id',''))[:12],style='bold'))
        nodes={};pending=list(spans)
        for _ in range(len(spans)+1):
            remaining=[]
            for item in pending:
                parent=item.get('parent_span_id')
                if parent and parent not in nodes and any(span['span_id']==parent for span in spans):remaining.append(item);continue
                attrs=item['attributes'];kind='MCP' if attrs.get('mcp.server.name') or attrs.get('mcp.tool.name') else 'MODEL' if attrs.get('gen_ai.operation.name')=='chat' or 'gen_ai.usage.input_tokens' in attrs else 'TOOL' if attrs.get('tool.name') else 'RUN'
                duration=None if attrs.get('observatory.duration_unknown') else (item['end_unix_nano']-item['start_unix_nano'])/1e6
                label=Text(f"{kind:5} ",style='#c5adff');label.append(clean(item['name']));label.append(' · '+timing(duration),style='#b9afc5')
                if item['status']=='ERROR':label.append(' · ERROR',style='bold red')
                nodes[item['span_id']]=(nodes.get(parent) or tree.root).add(themed_text(tree,label),data=item,expand=True)
            if not remaining:break
            if len(remaining)==len(pending):
                for item in remaining:nodes[item['span_id']]=tree.root.add(Text(item['name']),data=item)
                break
            pending=remaining
        tree.root.expand()
        target=None
        if getattr(self,'jump_span',None) in nodes:
            target=nodes[self.jump_span];self.jump_span=None
        elif nodes:
            target=nodes.get(selected) or next(iter(nodes.values()))
        # A just-activated pane may not have painted yet. Reading last_line
        # builds Textual's line mapping before move_cursor reads node.line;
        # deferring the move alone can leave the cursor on the empty root.
        if target is not None and tree.last_line>=0:
            tree.move_cursor(target)

    def valid_row(self,event):
        table=event.data_table
        if not table.row_count:return None
        try:
            if table.coordinate_to_cell_key(table.cursor_coordinate).row_key!=event.row_key:return None
        except Exception:return None
        return self.table_data.get(table.id,{}).get(event.row_key.value)

    @on(DataTable.RowHighlighted)
    def row_highlighted(self,event):
        page={'agent-table':'agents','agent-actions':'agents','activity-table':'activity',
            'mcp-table':'mcp','mcp-servers':'mcp','trace-table':'traces',
            'finding-table':'findings','metrics-table':'metrics','host-table':'host'}.get(event.data_table.id)
        if page!=self.query_one('#pages',TabbedContent).active:return
        item=self.valid_row(event)
        if not item:return
        name=event.data_table.id
        if name=='agent-table':self.select_agent(item['id'])
        elif name=='activity-table':self.selected_activity=event.row_key.value;self.show_activity()
        elif name=='mcp-table':self.selected_mcp=item['id'];self.show_mcp()
        elif name=='finding-table':self.selected_finding=item['id'];self.show_finding()
        elif name=='metrics-table':self.selected_metric=event.row_key.value;self.last_update=-1;self.refresh_data()
        elif name=='trace-table':
            if self.feed.pinned_trace and self.data.detail.get('trace_id')!=self.feed.selected_trace:return
            if self.feed.selected_trace!=item['trace_id']:
                self.feed.selected_trace=item['trace_id'];self.feed.pinned_trace=True;self.feed.request_refresh()
                self.feed.selected_span_id=None
                self.query_one('#span-tree',Tree).root.set_label(Text('Loading selected trace…'))

    @on(DataTable.RowSelected)
    def row_selected(self,event):
        item=self.valid_row(event)
        if not item:return
        if event.data_table.id=='mcp-servers':
            self.query_one('#search-mcp',Input).value=item['server']
            self.query_one('#mcp-table',DataTable).focus()
            return
        if event.data_table.id=='agent-table':
            self.select_agent(item['id'])
            self.action_expand()
            return
        if event.data_table.id!='agent-actions' and self.screen.has_class('compact'):
            self.action_expand()
            return
        if event.data_table.id=='agent-actions':
            self.selected_activity=item.get('id') or item['source_id'];self.action_page('activity');self.show_activity()
        elif event.data_table.id=='activity-table':self.query_one('#activity-detail',RichLog).focus()
        elif event.data_table.id=='mcp-table':self.query_one('#mcp-detail',RichLog).focus()
        elif event.data_table.id=='finding-table':self.query_one('#finding-detail',RichLog).focus()
        elif event.data_table.id=='trace-table':self.query_one('#span-tree',Tree).focus()
        elif event.data_table.id=='host-table':self.query_one('#host-detail',RichLog).focus()
        elif event.data_table.id=='metrics-table':
            key=event.row_key.value
            metric=self.table_data.get('metrics-table',{}).get(key)
            if metric and self.metric_history_loaded_key!=key:self.request_metric_history(metric,key)
            self.query_one('#metrics-detail',RichLog).focus()

    @on(Tree.NodeHighlighted,'#span-tree')
    def span_highlighted(self,event):
        self.show_span(event.node.data)

    def show_span(self,span):
        if not span:return
        attrs=span['attributes']
        self.inspect('span-info',span['name'],[('Span',span['span_id']),('Parent',span.get('parent_span_id') or 'Root'),
            ('Status',span['status']),('Service',span['service'])],
            [('Analysis coverage',analysis_coverage(self.data.detail.get('summary') or {}))]+
            [(key,value) for key,value in attrs.items()])

    def sync_evidence_scope(self):
        page=self.query_one('#pages',TabbedContent).active
        active=page in {'activity','mcp','traces','findings'} and self.scope_value!='all'
        self.query_one('#controls').set_class(active,'evidence-active')
        self.query_one('#controls').set_class(page=='agents','agents-active')
        item=getattr(self,'source_map',{}).get(self.scope_value)
        scoped=bool(item) and self.scope_value!='all'
        self.feed.scope_agent_id=(item.get('agent_id') or '') if scoped else ''
        self.feed.scope_subtree=bool(self.evidence_scope_subtree) if scoped else False
        if scoped and self.evidence_scope_subtree:
            # Canonical subtree membership replaces exact-session filtering;
            # keeping both would intersect child evidence with the root session.
            self.feed.session_id=''
            self.feed.service=''
        elif scoped:
            self.feed.session_id=item.get('session_id') or ''
            self.feed.service=item.get('service') or ''
        else:
            self.feed.session_id=''
            self.feed.service=''

    @on(Select.Changed,'#scope')
    def scope_changed(self,event):
        if event.value==Select.BLANK or event.value==self.scope_value:return
        self.scope_value=str(event.value);item=getattr(self,'source_map',{}).get(self.scope_value)
        self.selected_activity=self.selected_mcp=self.selected_finding=None
        self.feed.session_id=item.get('session_id') or '' if item else ''
        self.feed.service=item.get('service') or '' if item else ''
        self.feed.scope_agent_id=(item.get('agent_id') or '') if item else ''
        self.sync_evidence_scope()
        self.feed.selected_trace=None;self.feed.pinned_trace=False
        self.query_one('#controls').set_class(self.scope_value!='all','evidence-active')
        self.feed.request_refresh()
        self.feed.selected_span_id=None
        self.last_update=-1;self.refresh_data()

    @on(Select.Changed,'#evidence-scope')
    def evidence_scope_changed(self,event):
        if event.value==Select.BLANK:return
        subtree=str(event.value)=='subtree'
        if subtree==self.evidence_scope_subtree:return
        self.evidence_scope_subtree=subtree
        self.sync_evidence_scope()
        self.selected_activity=self.selected_mcp=self.selected_finding=None
        self.feed.selected_trace=None;self.feed.pinned_trace=False
        self.feed.request_refresh();self.last_update=-1;self.refresh_data()

    @on(Select.Changed,'#agent-filter')
    def agent_filter_changed(self,event):
        if event.value and str(event.value)!=self.agent_filter:
            self.set_agent_filter(str(event.value))

    @on(Select.Changed,'#more-views')
    def more_view_changed(self,event):
        if event.value and event.value!=Select.BLANK and event.value!=self.query_one('#pages',TabbedContent).active:
            self.action_page(str(event.value))

    @on(Input.Changed,'.view-search')
    def search_changed(self,event):
        page=event.input.id.removeprefix('search-')
        value=event.value.strip()
        self.query_one('#clear-'+page,Button).disabled=not event.value
        if self.search_queries[page]==value:return
        self.search_queries[page]=value
        if page=='activity':self.selected_activity=None
        if page=='mcp':self.selected_mcp=None
        if page=='findings':self.selected_finding=None
        if page in self.search_timers:self.search_timers.pop(page).stop()
        def apply():
            if not self.query('#health'):return
            attr={'activity':'activity_query','mcp':'mcp_query','traces':'query'}.get(page)
            if attr:
                setattr(self.feed,attr,value)
                if page=='traces':
                    self.feed.selected_trace=None;self.feed.selected_span_id=None;self.feed.pinned_trace=False
                self.feed.request_refresh()
            self.last_update=-1;self.refresh_data()
        if page in {'activity','mcp','traces'}:self.search_timers[page]=self.set_timer(.3,apply)
        else:apply()
        self.last_update=-1;self.refresh_data()

    @on(Checkbox.Changed,'#reviews')
    def reviews_changed(self,event):
        self.selected_activity=self.selected_mcp=None
        self.feed.include_reviews=event.value;self.feed.selected_trace=None;self.feed.pinned_trace=False;self.feed.request_refresh()
        self.feed.selected_span_id=None

    @on(TabbedContent.TabActivated,'#pages')
    def page_activated(self,event):
        self.preference_view=event.pane.id
        self.screen.set_class(event.pane.has_class('expanded'),'inspecting')
        self.query_one('#controls').set_class(event.pane.id=='agents','agents-active')
        self.query_one('#controls').set_class(
            event.pane.id in {'activity','mcp','traces','findings'} and self.scope_value!='all',
            'evidence-active')
        self.sync_more_view(event.pane.id)
        self.sync_evidence_scope()
        if event.pane.id=='agents':
            self.agent_order_key=None
            self.agent_order_pending=False
        self.last_update=-1
        self.refresh_data()

    def sync_more_view(self,page):
        picker=self.query_one('#more-views',Select)
        menu_navigation=getattr(self,'layout_size',self.size).width<76
        if getattr(self,'view_menu_mode',None)!=menu_navigation:
            options=[('Other views',''),('5 Host','host'),('6 Metrics','metrics'),('7 Findings','findings')]
            if menu_navigation:
                options=[('Views: 1 Agents','agents'),('Views: 2 Activity','activity'),
                         ('Views: 3 MCP Calls','mcp'),('Views: 4 Traces','traces')]+options[1:]
            with picker.prevent(Select.Changed):
                picker.set_options(options)
                picker.value=page if menu_navigation or page in {'host','metrics','findings'} else ''
            self.view_menu_mode=menu_navigation
        else:
            picker.value=page if menu_navigation or page in {'host','metrics','findings'} else ''
        picker.set_class(page in {'host','metrics','findings'},'secondary-active')

    @on(Button.Pressed)
    def button_pressed(self,event):
        if event.button.id=='pause':self.action_pause()
        elif event.button.id=='refresh-order':
            self.refresh_agent_order()
        elif event.button.id=='agent-sort':
            self.action_toggle_sort()
        elif event.button.id and event.button.id.startswith('filter-'):
            self.set_agent_filter(event.button.id.removeprefix('filter-'))
        elif event.button.id=='history-back':self.action_history_back()
        elif event.button.id.startswith(('older-','newer-','latest-')):
            direction,page=event.button.id.split('-',1)
            self.action_history(direction,page)
        elif event.button.id.startswith('expand-'):self.action_expand()
        elif event.button.id=='prompts-activity':
            self.prompts_only=not self.prompts_only
            self.feed.activity_prompts=self.prompts_only
            self.selected_activity=None
            button=event.button
            button.set_class(self.prompts_only,'-on')
            button.label='Prompts ●' if self.prompts_only else 'Prompts'
            if self.prompts_only and 'activity' in self.errors_only:
                self.errors_only.remove('activity')
                errors=self.query_one('#errors-activity',Button)
                errors.remove_class('-on');errors.label='Errors'
            self.last_update=-1;self.feed.request_refresh();self.refresh_data()
        elif event.button.id.startswith('errors-'):
            page=event.button.id.removeprefix('errors-')
            if page=='activity' and self.prompts_only:
                self.prompts_only=False;self.feed.activity_prompts=False
                self.feed.request_refresh()
                prompts=self.query_one('#prompts-activity',Button)
                prompts.remove_class('-on');prompts.label='Prompts'
            if page=='activity':self.selected_activity=None
            if page=='mcp':self.selected_mcp=None
            if page in self.errors_only:self.errors_only.remove(page)
            else:self.errors_only.add(page)
            event.button.set_class(page in self.errors_only,'-on')
            event.button.label='Errors ●' if page in self.errors_only else 'Errors'
            self.last_update=-1;self.refresh_data()
        elif event.button.id.startswith('clear-'):
            field=self.query_one('#search-'+event.button.id.removeprefix('clear-'),Input)
            field.value='';field.focus()
        elif event.button.id=='agent-open':self.action_focus_agent()
        elif event.button.id=='agent-trace':self.action_open_trace()
        elif event.button.id=='agent-reviewed':self.action_review_attention()
        elif event.button.id=='agent-dismissed':self.action_dismiss_attention()
        elif event.button.id=='agent-summary':self.request_ai_summary()
        elif event.button.id in {'activity-trace','mcp-trace','metrics-trace','finding-trace'}:self.action_open_trace()
        elif event.button.id in {'activity-copy','mcp-copy','metrics-copy','finding-copy'}:self.action_copy()

    def request_ai_summary(self):
        from .ai_summary import SummaryConfig, SummaryError
        item=getattr(self,'source_map',{}).get(self.selected_source)
        if not item or self.summary_busy:return
        if item.get('review'):
            self.notify('AI summaries exclude internal approval reviews.',severity='warning');return
        try:
            config=SummaryConfig.from_env()
        except SummaryError as exc:
            self.notify(str(exc),severity='warning',timeout=12);return
        cached=self.ai_summaries.get(item['id'])
        if cached and cached[0]==item.get('stamp'):
            self.notify('Showing the existing summary for this snapshot.');return
        self.summary_busy=True
        self.query_one('#agent-summary',Button).label='Summarizing…'
        self.show_agent()
        aggregate=next((dict(row) for row in self.data.sessions
                        if item.get('agent_id') and row.get('agent_id')==item.get('agent_id')),None)
        if aggregate is None:
            aggregate=next((dict(row) for row in self.data.sessions
                if item.get('session_id') and row.get('session_id')==item.get('session_id')
                and (not row.get('agent_id') or not item.get('agent_id'))),{})
        summary_item=dict(item)
        summary_item['telemetry_totals']=aggregate
        self.summarize_agent(summary_item,config)

    @work(group='ai-summary')
    async def summarize_agent(self,item,config):
        from .ai_summary import generate_summary, summary_evidence, SummaryError
        try:
            params={'limit':30,'service':item.get('service') or '',
                    'session_id':item.get('session_id') or '', 'include_reviews':0}
            page=await asyncio.to_thread(self.feed.api.get,'/api/activity?'+urlencode(params))
            evidence=summary_evidence(item,page.get('activity',[]))
            result=await asyncio.to_thread(generate_summary,config,evidence)
            self.ai_summaries[item['id']]=(item.get('stamp'),result)
            if len(self.ai_summaries)>50:self.ai_summaries.pop(next(iter(self.ai_summaries)))
        except SummaryError as exc:
            self.notify(str(exc),severity='error',timeout=10)
        except Exception:
            self.notify('Could not load session evidence for the summary. Retry when the receiver is connected.',severity='error')
        finally:
            self.summary_busy=False
            self.query_one('#agent-summary',Button).label='AI summary'
            self.show_agent()

    def action_pause(self):
        self.feed.paused=not self.feed.paused
        self.query_one('#pause',Button).label='Resume' if self.feed.paused else 'Pause'
        self.feed.request_refresh();self.refresh_data()

    def action_help(self):
        self.notify('↑/↓ move · Enter inspect · / search · e expand · Tab controls · Ctrl+P palette · d diagnostics · x export · p pause · q quit',timeout=8)

    def action_diagnostics(self):
        self.push_screen(DiagnosticsScreen(self.feed.snapshot.local_status),self.resume_cockpit)

    def resume_cockpit(self,result=None):
        # The terminal may have resized and collection may have advanced while
        # a privacy/diagnostics modal intentionally suspended cockpit rendering.
        self.update_layout()
        self.call_after_refresh(self.reflow_inspectors)

    def action_export(self):
        page=self.query_one('#pages',TabbedContent).active
        if page=='agents':
            record=getattr(self,'source_map',{}).get(self.selected_source)
        elif page=='traces':
            record=self.data.detail if self.data.detail.get('trace_id')==self.feed.selected_trace else None
        else:
            selected={'activity':self.selected_activity,'mcp':self.selected_mcp,
                'metrics':self.selected_metric,'findings':self.selected_finding}.get(page)
            table={'findings':'finding-table'}.get(page,page+'-table')
            record=self.table_data.get(table,{}).get(selected)
        if not record:
            self.notify('Select session, event, call, trace, metric, or finding evidence to export.');return
        scope=None
        if page in {'activity','mcp','traces','findings'} and self.feed.scope_agent_id:
            scope={'mode':'subtree' if self.feed.scope_subtree else 'exact',
                   'root_agent_id':self.feed.scope_agent_id}
        self.push_screen(ExportScreen(page,record,scope),self.resume_cockpit)

    def action_search(self):
        page=self.query_one('#pages',TabbedContent).active
        if page in self.search_queries:
            self.collapse(page)
            self.query_one('#search-'+page,Input).focus()
    def action_back(self):
        if self.screen.selections:
            self.screen.clear_selection();return
        page=self.query_one('#pages',TabbedContent).active
        if isinstance(self.focused,Input) or self.query_one('#'+page).has_class('expanded'):
            self.action_page(page)
        elif self.navigation_history:
            self.action_history_back()
        else:self.action_page(page)

    def remember_location(self):
        page=self.query_one('#pages',TabbedContent).active
        self.navigation_history.append({'page':page,'expanded':self.query_one('#'+page).has_class('expanded'),
            'focus':self.focused.id if self.focused else None,
            'tree_mode':self.tree_mode,'expanded_agents':set(self.expanded_agents),
            'collapsed_agents':set(self.collapsed_agents),'agent_filter':self.agent_filter,
            'scope_value':self.scope_value,'selected_source':self.selected_source,
            'evidence_scope_subtree':self.evidence_scope_subtree,
            'scroll':{widget.id:widget.scroll_offset for widget in self.query('#'+page+' RichLog, #'+page+' DataTable')}})
        self.query_one('#history-back',Button).disabled=False

    def action_history_back(self):
        if not self.navigation_history:return
        location=self.navigation_history.pop()
        self.tree_mode=location.get('tree_mode',self.tree_mode)
        self.expanded_agents=set(location.get('expanded_agents',self.expanded_agents))
        self.collapsed_agents=set(location.get('collapsed_agents',self.collapsed_agents))
        self.agent_filter=location.get('agent_filter',self.agent_filter)
        self.evidence_scope_subtree=location.get('evidence_scope_subtree',self.evidence_scope_subtree)
        self.scope_value=location.get('scope_value',self.scope_value)
        self.selected_source=location.get('selected_source',self.selected_source)
        self.action_page(location['page'])
        if location['expanded']:self.action_expand()
        def restore():
            picker=self.query_one('#scope',Select)
            if any(value==self.scope_value for _,value in self.scope_options or ()):
                with picker.prevent(Select.Changed):
                    picker.value=self.scope_value
            evidence=self.query_one('#evidence-scope',Select)
            with evidence.prevent(Select.Changed):
                evidence.value='subtree' if self.evidence_scope_subtree else 'exact'
            self.sync_evidence_scope()
            self.last_update=-1
            self.refresh_data()
            if location['focus']:self.query_one('#'+location['focus']).focus()
            for name,position in location['scroll'].items():
                self.query_one('#'+name).scroll_to(x=position.x,y=position.y,animate=False)
        self.call_after_refresh(restore)
        self.query_one('#history-back',Button).disabled=not self.navigation_history

    def action_history(self,direction,page=None):
        page=page or self.query_one('#pages',TabbedContent).active
        if page not in self.feed.history:return
        self.screen.clear_selection()
        old=self.selected_activity,self.selected_mcp,self.selected_finding
        if page=='activity':self.selected_activity=None
        if page=='mcp':self.selected_mcp=None
        if page=='findings':self.selected_finding=None
        if not self.feed.navigate_history(page,direction):
            self.selected_activity,self.selected_mcp,self.selected_finding=old
            return
        self.last_update=-1;self.refresh_data()

    def copy_to_clipboard(self,text):
        """Route all copy actions, including search inputs, to the OS clipboard."""
        self._clipboard=text  # Keep Textual's in-app paste buffer in sync.
        if text:self._copy_in_background(text)

    @work(group='clipboard',exclusive=True)
    async def _copy_in_background(self,text):
        method=await write_system_clipboard(text)
        if method:
            self.notify(f'Copied to {method} clipboard.')
        else:
            super().copy_to_clipboard(text)
            self.notify('Copy sent to terminal; clipboard support depends on your terminal.')

    def action_copy(self):
        selected=self.screen.get_selected_text()
        if selected:
            self.copy_to_clipboard(selected);return
        page=self.query_one('#pages',TabbedContent).active
        if page=='mcp':
            item=self.table_data.get('mcp-table',{}).get(self.selected_mcp)
        elif page=='activity':
            item=self.table_data.get('activity-table',{}).get(self.selected_activity)
        elif page=='findings':
            item=self.table_data.get('finding-table',{}).get(self.selected_finding)
            if item:
                self.copy_to_clipboard(json.dumps(item,ensure_ascii=False,indent=2));return
        elif page=='metrics':
            item=self.table_data.get('metrics-table',{}).get(self.selected_metric)
            if item:
                self.copy_to_clipboard(json.dumps(item,ensure_ascii=False,indent=2));return
            self.notify('Select a metric instrument to copy.');return
        else:
            name={'agents':'agent-detail','traces':'span-info','host':'host-detail'}[page]
            self.copy_to_clipboard(self.query_one('#'+name,SelectableRichLog).plain_text)
            return
        if not item:
            self.notify('Select an activity, MCP operation, or finding to copy.')
            return
        self.copy_to_clipboard(copy_event_text(item))
    def collapse(self,page):
        self.query_one('#'+page,TabPane).remove_class('expanded')
        self.screen.remove_class('inspecting')
        for button in self.query('#'+page+' .expand-button'):button.label=Text('Expand · e')
        self.call_after_refresh(self.reflow_inspectors)
    def action_expand(self):
        page=self.query_one('#pages',TabbedContent).active
        if page not in self.search_queries:return
        pane=self.query_one('#'+page,TabPane)
        if pane.has_class('expanded'):
            self.action_back();return
        pane.add_class('expanded')
        self.screen.add_class('inspecting')
        self.call_after_refresh(self.reflow_inspectors)
        for button in self.query('#'+page+' .expand-button'):button.label=Text('Back · e')
        target={'agents':'agent-detail','activity':'activity-detail','mcp':'mcp-detail','traces':'span-tree','host':'host-detail','metrics':'metrics-detail','findings':'finding-detail'}[page]
        self.query_one('#'+target).focus()
        self.notify('Expanded detail · Esc to return',timeout=2)
    def action_focus_agent(self):
        if self.selected_source:
            self.remember_location()
            self.selected_activity=getattr(self,'agent_primary_event_id',None)
            self.query_one('#scope',Select).value=self.selected_source
            self.action_page('activity')
    def action_reviews(self):self.query_one('#reviews',Checkbox).toggle()
    def action_all(self):
        self.selected_activity=self.selected_mcp=self.selected_finding=None
        self.query_one('#scope',Select).value='all'
        self.errors_only.clear()
        self.prompts_only=False;self.feed.activity_prompts=False
        for button in self.query('.error-filter'):
            button.remove_class('-on');button.label='Errors'
        prompt_button=self.query_one('#prompts-activity',Button)
        prompt_button.remove_class('-on');prompt_button.label='Prompts'
        for page in self.search_queries:self.query_one('#search-'+page,Input).value=''
        self.last_update=-1;self.refresh_data()
    def action_page(self,page):
        self.screen.clear_selection()
        self.collapse(page)
        self.query_one('#pages',TabbedContent).active=page
        target={'agents':'agent-table','activity':'activity-table','mcp':'mcp-table','traces':'trace-table','host':'host-table','metrics':'metrics-table','findings':'finding-table'}[page]
        self.query_one('#'+target).focus()

    def _selected_agent_node(self):
        item=getattr(self,'agent_row_meta',{}).get(self.selected_source)
        if not item:return None
        graph=self.hierarchy_graph
        agent_id=item.get('agent_id')
        return graph.nodes.get(agent_id) if graph and agent_id else None

    def _focus_source_row(self,source_id):
        table=self.query_one('#agent-table',ResultsTable)
        try:
            index=[key.value for key in table.rows].index(source_id)
        except ValueError:
            return
        with table.prevent(DataTable.RowHighlighted):
            table.move_cursor(row=index,scroll=True)
        self.select_agent(source_id)

    def _select_agent_id(self,agent_id):
        item=getattr(self,'hierarchy_entries',{}).get(agent_id)
        if item:
            self._focus_source_row(item['id'])
            return True
        return False

    def action_toggle_tree(self):
        self.tree_mode=not self.tree_mode
        self.agent_order_key=None
        self.agent_order_ids=[]
        self.agent_order=[]
        self.last_update=-1
        self.refresh_data()
        self.notify('Agent tree' if self.tree_mode else 'Flat agent list')

    def action_tree_right(self):
        if self.query_one('#pages',TabbedContent).active!='agents':
            return
        node=self._selected_agent_node()
        if not node:
            return
        agent_id=node['agent_id']
        graph=self.hierarchy_graph
        if not graph.children_of(agent_id):
            return
        if agent_id not in self.expanded_agents:
            self.collapsed_agents.discard(agent_id)
            self.expanded_agents.add(agent_id)
            self.last_update=-1
            self.refresh_data()
            return
        children=graph.children_of(agent_id)
        first=children[0] if children else None
        if first:
            self._select_agent_id(first)

    def action_tree_left(self):
        if self.query_one('#pages',TabbedContent).active!='agents':
            return
        node=self._selected_agent_node()
        if not node:
            return
        agent_id=node['agent_id']
        graph=self.hierarchy_graph
        if graph.children_of(agent_id) and agent_id in self.expanded_agents:
            self.expanded_agents.discard(agent_id)
            self.collapsed_agents.add(agent_id)
            # The collapsing ancestor remains selected and visible; detail and
            # row title are rebuilt together by refresh_data.
            self.last_update=-1
            self.refresh_data()
            return
        parent=graph.parent_of(agent_id)
        if parent:
            self._select_agent_id(parent)

    def action_inspect_parent(self):
        node=self._selected_agent_node()
        graph=self.hierarchy_graph
        if not node or not graph:return
        parent=graph.parent_of(node['agent_id'])
        if parent:
            self._select_agent_id(parent)
        else:
            item=getattr(self,'agent_row_meta',{}).get(self.selected_source) or {}
            if (item.get('parent_context') or {}).get('retained'):
                self.notify('Parent identity is retained outside the current source window; it has no live row to inspect.')
            else:
                self.notify('No retained parent relationship for this agent.')

    def action_inspect_first_child(self):
        node=self._selected_agent_node()
        graph=self.hierarchy_graph
        if not node or not graph:return
        children=graph.children_of(node['agent_id'])
        if children:
            self._select_agent_id(children[0])
        else:
            self.notify('No observed direct children for this agent.')

    def action_inspect_next_attention(self):
        node=self._selected_agent_node()
        graph=self.hierarchy_graph
        if not node or not graph:return
        candidates=[item for item in graph.descendants(node['agent_id'])
                    if node_attention(graph.nodes[item]) or graph.summary(item)['attention']]
        if not candidates:
            self.notify('No descendant needs attention in the retained tree.')
            return
        target=next((item for item in candidates if item!=node['agent_id']),candidates[0])
        # Prefer a directly attention-requiring node; ancestors are navigation.
        target=next((item for item in candidates if node_attention(graph.nodes[item])),target)
        self._select_agent_id(target)

    def toggle_agent_branch(self,source_id):
        item=getattr(self,'agent_row_meta',{}).get(source_id) or self.source_map.get(source_id)
        graph=self.hierarchy_graph
        if not item or not graph:
            return
        agent_id=item.get('agent_id')
        if not agent_id or not graph.children_of(agent_id):
            return
        if agent_id in self.expanded_agents:
            self.expanded_agents.discard(agent_id)
            self.collapsed_agents.add(agent_id)
            selected=graph.nodes.get((self.agent_row_meta.get(self.selected_source) or {}).get('agent_id'))
            if selected and (agent_id==selected['agent_id'] or agent_id in graph.ancestors(selected['agent_id'])):
                self.last_update=-1
                self.refresh_data()
                self._select_agent_id(agent_id)
                return
        else:
            self.collapsed_agents.discard(agent_id)
            self.expanded_agents.add(agent_id)
        self.last_update=-1
        self.refresh_data()
    def linked_agent_trace(self):
        """The trace recorded for the selected session, when one is linked.

        Recent work first, then the session's own last event, then a recorded
        trace for the same session, then one whose task names the same work.
        """
        session=getattr(self,'source_map',{}).get(self.selected_source)
        if not session:return None
        traces=list(self.data.traces) if self.data else []
        known={item.get('trace_id') for item in traces}
        events=[*self.table_data.get('agent-actions',{}).values(),session.get('last_activity') or {}]
        for event in events:
            trace_id=event.get('trace_id')
            if trace_id and trace_id in known:
                return {'trace_id':trace_id,'span_id':event.get('span_id')}
        trace_id=session.get('latest_trace_id')
        agent_id=session.get('agent_id')
        if agent_id:
            match=next((item for item in traces if item.get('agent_id')==agent_id),None)
            if match:return {'trace_id':match['trace_id']}
        if trace_id:
            return {'trace_id':trace_id}
        session_id=session.get('session_id')
        if session_id:
            match=next((item for item in traces if item.get('session_id')==session_id
                        and (not agent_id or not item.get('agent_id'))),None)
            if match:return {'trace_id':match['trace_id']}
        task=str(session.get('task') or '').strip().casefold()
        if task:
            match=next((item for item in traces if task and task in str(item.get('task') or '').casefold()),None)
            if match:return {'trace_id':match['trace_id']}
        return None

    def metric_exemplar(self):
        """Resolve exemplars only within the selected instrument and its history."""
        metric=self.table_data.get('metrics-table',{}).get(self.selected_metric)
        if not metric:return None
        if self.metric_history_loaded_key==self.selected_metric and self.metric_history_points:
            points=self.metric_history_points
        else:
            keys=('service','name','kind','temporality','scope_name','unit','description')
            points=[point for point in self.data.metric_points
                    if all(point.get(key)==metric.get(key) for key in keys)]
        return next((exemplar for point in points
                     for exemplar in ((point.get('data') or {}).get('exemplars') or [])
                     if exemplar.get('trace_id')),None)

    def action_open_trace(self):
        page=self.query_one('#pages',TabbedContent).active
        if page=='metrics':
            exemplar=self.metric_exemplar()
            item={'trace_id':exemplar.get('trace_id'),'span_id':exemplar.get('span_id')} if exemplar else None
        elif page=='agents':
            item=self.linked_agent_trace()
        elif page in {'activity','mcp','findings'}:
            item=(self.table_data.get('mcp-table',{}).get(self.selected_mcp) if page=='mcp' else
                  self.table_data.get('activity-table',{}).get(self.selected_activity) if page=='activity' else
                  self.table_data.get('finding-table',{}).get(self.selected_finding))
            if page=='findings' and item:
                span_id=next((entry.get('id') for entry in item.get('evidence',[])
                              if entry.get('type')=='span'),None)
                item={**item,'span_id':span_id}
        else:return
        if not item or not item.get('trace_id'):
            self.notify('No linked trace for this selection.');return
        self.remember_location()
        self.jump_span=item.get('span_id')
        self.feed.selected_span_id=self.jump_span
        self.feed.selected_trace=item['trace_id'];self.feed.pinned_trace=True;self.feed.request_refresh();self.action_page('traces')

    def action_review_attention(self):
        item=getattr(self,'source_map',{}).get(self.selected_source)
        if not item or not attention_required(item):
            return
        if item['id'] in self.reviewed_attention:
            self.reviewed_attention.remove(item['id'])
            self.notify('Returned to Needs attention.')
        else:
            self.reviewed_attention.add(item['id'])
            self.notify('Marked reviewed for this run; recorded status is unchanged.')
        self.last_update=-1
        self.refresh_data()

    def action_dismiss_attention(self):
        item=getattr(self,'source_map',{}).get(self.selected_source)
        if not item or not attention_required(item):
            return
        key=attention_key(item)
        if key in self.dismissed_attention:
            self.dismissed_attention.remove(key)
            self.notify('Restored to Recent and Needs attention.')
        else:
            self.dismissed_attention.add(key)
            self.reviewed_attention.discard(item['id'])
            self.notify('Dismissed from attention. The session remains in All history and Traces.')
        self.save_preferences()
        self.last_update=-1
        self.refresh_data()

    @work(group='metric-history',exclusive=True,thread=True)
    def load_metric_history(self,metric,key):
        try:
            points=self.feed.metric_history(metric,100)
        except Exception as exc:
            try:self.call_from_thread(self.finish_metric_history,key,None,clean(exc))
            except RuntimeError:pass
            return
        try:self.call_from_thread(self.finish_metric_history,key,points,'')
        except RuntimeError:pass

    def request_metric_history(self,metric,key):
        self.metric_history_requested_key=key
        self.metric_history_points=[]
        self.load_metric_history(metric,key)

    def finish_metric_history(self,key,points,error):
        if self.metric_history_requested_key!=key:return
        if error:
            self.metric_history_loaded_key=key
            self.metric_history_points=[]
            self.notify('Could not load metric history: '+error,severity='warning')
            return
        self.metric_history_points=points or []
        self.metric_history_loaded_key=key
        self.last_update=-1
        self.refresh_data()
