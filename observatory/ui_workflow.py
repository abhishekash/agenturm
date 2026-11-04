"""Local workflow evidence and bounded, opt-in desktop attention delivery."""
from __future__ import annotations
import math
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import time
from textual import on
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, Checkbox, DataTable, Input, Label, RichLog
from rich.text import Text
from .ui_monitoring import task_run_usage, task_usage_summary
from .session_status import today_bucket


def events_for(item, events):
    agent=item.get('agent_id');session=item.get('session_id')
    return sorted((e for e in events if
        (agent and e.get('agent_id')==agent) or
        (session and e.get('session_id')==session and not e.get('agent_id'))),
        key=lambda e:e.get('time_unix_nano',0))


def workflow_reasons(item, usage, snapshot, limits, now=None):
    now=now or time.time(); reasons=[]
    waiting=str(item.get('waiting_for') or '').lower()
    if not item.get('_workflow_reviewed') and item.get('state')=='Waiting' and any(x in waiting for x in ('approval','permission','user input','input request')):
        reasons.append('Approval requested' if 'approval' in waiting or 'permission' in waiting else 'Reply requested')
    if today_bucket(item,int(now*1e9))=='attention' and item.get('state') in {'Error','Failed','Interrupted','Incomplete'}:reasons.append('Task interrupted' if item.get('state')=='Interrupted' else 'Reported task failure')
    if not item.get('_workflow_reviewed') and item.get('state')=='Possibly stalled':reasons.append('Possible stall · inspect activity and collector health')
    # Limits apply to this recorded run, never session lifetime or inferred prices.
    total=sum(usage.get(k,0) or 0 for k in ('input_tokens','output_tokens') if usage.get(k+'_reported') is not False)
    if limits.get('tokens',0)>0 and total>=limits['tokens']:reasons.append('Task token threshold reached')
    cost=usage.get('known_cost_usd')
    if limits.get('cost',0)>0 and cost is not None and cost>=limits['cost']:reasons.append('Reported task cost threshold reached')
    trace=next((t for t in snapshot.traces if t.get('trace_id')==usage.get('trace_id')),{})
    start=trace.get('start_unix_nano')
    if item.get('active') and limits.get('minutes',0)>0 and start and now-start/1e9>=limits['minutes']*60:
        reasons.append('Task elapsed-time threshold reached')
    return reasons


class AttentionLedger:
    def __init__(self):
        self.previous=None;self.since={};self.rows=[]

    def update(self, items, snapshot, limits, now=None):
        now=now or time.time(); current={}; rows=[]; notifications=[]
        for item in items:
            usage=task_run_usage(snapshot.sessions,item,snapshot.traces)
            reasons=workflow_reasons(item,usage,snapshot,limits,now)
            if not reasons:continue
            key=(item['id'],usage.get('trace_id'),tuple(reasons))
            current[key]=True;self.since.setdefault(key,now)
            rows.append((item,reasons,self.since[key]))
            if self.previous is not None and key not in self.previous:notifications.append((item,reasons))
        self.previous=current;self.since={k:v for k,v in self.since.items() if k in current}
        self.rows=rows
        return notifications


def desktop_notify(title, body):
    """No shell, bounded calls; return delivery status to the UI."""
    if sys.platform.startswith('linux') and shutil.which('notify-send'):
        command=['notify-send','--app-name=Agenturm','--',title,body]
    elif sys.platform=='darwin' and shutil.which('osascript'):
        command=['osascript','-e','on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run',title,body]
    else:return 'Desktop notifications unavailable on this system'
    try:
        subprocess.run(command,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=3,check=True)
        return 'Desktop notification delivered'
    except (OSError,subprocess.SubprocessError):return 'Desktop notification delivery failed; attention remains in Workflow'


def resume_command(item):
    session=str(item.get('session_id') or '')
    if not session or len(session)>200 or any(ord(c)<32 for c in session):return ''
    runtime=str(item.get('runtime') or '').lower()
    command=['codex','resume',session] if runtime=='codex' else ['claude','--resume',session] if runtime=='claude' or runtime=='claude code' else []
    if not command:return ''
    cwd=item.get('working_directory')
    return ('cd -- '+shlex.quote(str(cwd))+' && ' if cwd else '')+shlex.join(command)


def terminal_handoff(item):
    """Linux exact process identity -> ancestor tmux pane; never guess by cwd."""
    if not shutil.which('tmux'):return 'tmux is unavailable; copy the resume command for a stopped session.'
    identities=(item.get('process') or {}).get('identity') or []
    ancestors=set()
    try:
        for pid,started in identities:
            fields=(Path('/proc')/str(int(pid))/'stat').read_text().rsplit(')',1)[1].split()
            if int(fields[19])!=int(started):continue
            seen=set()
            while int(pid)>1 and pid not in seen:
                seen.add(pid);ancestors.add(int(pid))
                fields=(Path('/proc')/str(int(pid))/'stat').read_text().rsplit(')',1)[1].split()
                pid=int(fields[1])
        result=subprocess.run(['tmux','list-panes','-a','-F','#{pane_id} #{pane_pid}'],capture_output=True,text=True,timeout=2,check=True)
        panes=[line.split()[0] for line in result.stdout.splitlines() if len(line.split())==2 and int(line.split()[1]) in ancestors]
        if len(set(panes))!=1:return 'No unique live tmux pane was verified for this session.'
        pane=panes[0]
        if not re.fullmatch(r'%\d+',pane):return 'Invalid tmux pane identity'
        subprocess.run(['tmux','select-window','-t',pane],capture_output=True,timeout=2,check=True)
        subprocess.run(['tmux','select-pane','-t',pane],capture_output=True,timeout=2,check=True)
        return 'Selected tmux pane '+pane+' · attach to its tmux session if this terminal is outside tmux.'
    except (OSError,ValueError,subprocess.SubprocessError):return 'Could not verify a live tmux pane; no terminal was selected.'


def workflow_report(item, items, snapshot):
    usage=task_run_usage(snapshot.sessions,item,snapshot.traces)
    lines=[str(item.get('task') or 'Selected session'),'','OBSERVATION HEALTH',
           'Receiver snapshot: '+(f'{max(0,int(time.time()-snapshot.updated))}s old' if snapshot.updated else 'not received'),
           'Receiver: '+(snapshot.error or 'last read succeeded'),
           'Agent state: '+str(item.get('state') or 'Unknown')]
    diagnostics=item.get('diagnostics') or {};process=item.get('process') or {}
    lines+=['Last recorded activity: '+(f"{max(0,int(time.time()-item['stamp']/1e9))}s ago" if item.get('stamp') else 'not reported'),
            'Exact observed process IDs: '+(', '.join(map(str,process.get('pids',[]))) or 'not reported'),
            'Collector backlog bytes: '+str(diagnostics.get('backlog_bytes','not reported')),
            'Process CPU seconds / I/O chars: '+str(process.get('cpu_seconds','not reported'))+' / '+str(process.get('io_chars','not reported')),
            'State evidence: '+str(item.get('status_evidence') or 'not reported'),
            'A quiet process is evidence, not proof of a stall.','', 'WORKSPACE']
    cwd=item.get('working_directory');lines+=['Directory: '+str(cwd or 'not reported'),'Branch: '+str(item.get('branch') or 'not reported')]
    overlap=[x for x in items if x['id']!=item['id'] and cwd and x.get('working_directory')==cwd and x.get('active')]
    lines+=['Other active sessions in this directory: '+(', '.join(str(x.get('task') or x['id'])[:80] for x in overlap) or 'none observed')]
    lines+=['Shared directory is an overlap warning; file ownership is not inferred.','','TASK USAGE',task_usage_summary(usage,scope='trace')[0]]
    children=[x for x in items if item.get('agent_id') and x.get('parent_agent_id')==item['agent_id'] and x.get('relationship_state')=='confirmed' and x.get('relationship_kind')=='delegated']
    if children:
        lines+=['','CONFIRMED DIRECT CHILDREN · separate run usage, not an added subtotal']
        for child in children:
            u=task_run_usage(snapshot.sessions,child,snapshot.traces)
            lines+=[str(child.get('task') or child['id'])[:120],task_usage_summary(u,scope='trace')[0]]
    events=events_for(item,snapshot.activity)
    lines+=['','COMPLETION REVIEW · loaded activity only','Agent reported state: '+str(item.get('state') or 'Unknown')]
    files=[e for e in events if e.get('domain')=='file'];shell=[e for e in events if e.get('domain')=='shell']
    lines+=['Recorded file operations: '+str(len(files))]
    for e in files[-8:]:lines.append('  '+str(e.get('subject') or e.get('action') or 'file operation')[:180])
    last_edit=max((e.get('time_unix_nano',0) for e in files),default=0)
    checks=[]
    for e in shell:
        subject=str(e.get('subject') or '')
        if not re.search(r'\b(pytest|unittest|test|check|lint|tsc)\b',subject):continue
        after='after latest recorded file operation' if last_edit and e.get('time_unix_nano',0)>last_edit else 'freshness against edits unknown' if not last_edit else 'before latest recorded file operation'
        checks.append('  '+subject[:140]+' · '+str(e.get('result') or e.get('status') or 'result not reported')+' · '+after)
    lines+=['Recorded check commands:']+(checks[-8:] or ['  None in loaded activity.'])
    lines+=['Recorded command results do not establish complete verification or a clean worktree.','','CONTEXT DIAGNOSTICS']
    context=[e for e in events if re.search(r'compact|truncat|context',str(e.get('action',''))+' '+str(e.get('subject','')),re.I)]
    lines += [str(e.get('subject') or e.get('action'))[:180] for e in context[-5:]] or ['No context events in loaded activity.']
    lines+=['Context occupancy is not inferred from cumulative token usage.','','HANDOFF',resume_command(item) or 'Native resume command unavailable for this runtime.',
            'Resume only after checking that the session is not already active.']
    return '\n'.join(lines)


class WorkflowScreen(ModalScreen):
    DEFAULT_CSS='''
    WorkflowScreen { align: center middle; background: $background 70%; }
    #workflow-dialog { width: 96%; height: 96%; background: $surface; border: round $primary; padding: 0 1; }
    #workflow-inbox { height: 5; }
    #workflow-log { height: 1fr; }
    #workflow-controls { height: 3; }
    #workflow-controls Button { min-width: 8; width: 1fr; }
    #workflow-limits { height: 3; }
    #workflow-limits Input { width: 1fr; }
    '''
    BINDINGS=[('escape','dismiss','Close')]
    def __init__(self,owner):super().__init__();self.owner=owner
    def compose(self):
        with Vertical(id='workflow-dialog'):
            yield Label('Workflow · Enter selects · Tab review · Esc returns')
            yield DataTable(id='workflow-inbox',cursor_type='row')
            yield RichLog(id='workflow-log',wrap=True,markup=False,min_width=1,auto_scroll=False)
            yield Checkbox('Desktop attention alerts · omit task text',value=self.owner.workflow_notify,id='workflow-notify')
            with Horizontal(id='workflow-limits'):
                for key,label in (('tokens','Task tokens'),('cost','Task USD'),('minutes','Task minutes')):
                    yield Input(str(self.owner.workflow_limits.get(key,0)),placeholder=label,id='workflow-'+key,tooltip=label+' alert threshold; 0 disables')
                yield Button('Save limits',id='workflow-save')
            with Horizontal(id='workflow-controls'):
                yield Button('Refresh',id='workflow-refresh')
                yield Button('Snooze',id='workflow-snooze',tooltip='Snooze desktop alerts for 10 minutes')
                yield Button('Terminal',id='workflow-terminal')
                yield Button('Resume',id='workflow-resume',tooltip='Copy native resume command')
                yield Button('Settings',id='workflow-settings')
    def on_mount(self):
        self.query_one(DataTable).add_columns('Task / project','Needs attention','Observed wait')
        self.query_one('#workflow-notify').display=False
        self.query_one('#workflow-limits').display=False
        for key,title in (('tokens','Tokens'),('cost','USD'),('minutes','Minutes')):
            self.query_one('#workflow-'+key,Input).border_title=title
        self.call_after_refresh(self.render_report)
    def on_resize(self):
        if self.is_mounted:self.call_after_refresh(self.render_report)
    def render_report(self):
        owner=self.owner;table=self.query_one(DataTable);table.clear()
        table.display=bool(owner.workflow_ledger.rows)
        self.query_one(Label).update('Workflow · '+('Attention inbox · Enter selects' if table.display else 'No attention items')+' · Esc returns')
        for item,reasons,since in owner.workflow_ledger.rows:
            table.add_row(str(item.get('task') or item['id'])[:80],'; '.join(reasons),f'{int((time.time()-since)/60)}m',key=item['id'])
        items=owner.workflow_items()
        item=next((x for x in items if x['id']==owner.selected_source),None)
        text=workflow_report(item,items,owner.feed.snapshot) if item else 'Select a session in Agents to inspect its workflow.'
        log=self.query_one(RichLog)
        if not log.content_size.width:return
        log.clear();log.write(Text(text+'\n\nNotifications: '+owner.workflow_delivery),width=max(1,log.content_size.width-1))
        if not table.display and self.focused is table:log.focus()
    @on(DataTable.RowSelected,'#workflow-inbox')
    def select_attention(self,event):
        self.owner.selected_source=event.row_key.value;self.render_report()
    @on(Checkbox.Changed,'#workflow-notify')
    def toggle_notifications(self,event):
        self.owner.workflow_notify=event.value
        self.owner.workflow_delivery='Enabled · waiting for new attention' if event.value else 'Disabled'
        self.owner.save_preferences()
    @on(Button.Pressed)
    def pressed(self,event):
        owner=self.owner;key=event.button.id
        if key=='workflow-settings':
            show=not self.query_one('#workflow-limits').display
            self.query_one('#workflow-limits').display=show
            self.query_one('#workflow-notify').display=show
        elif key=='workflow-refresh':self.render_report()
        elif key=='workflow-snooze':owner.workflow_snooze=time.time()+600;owner.workflow_delivery='Snoozed for 10 minutes';self.render_report()
        elif key=='workflow-save':
            try:
                limits={k:float(self.query_one('#workflow-'+k,Input).value) for k in ('tokens','cost','minutes')}
                if any(not math.isfinite(v) or v<0 for v in limits.values()):raise ValueError()
            except ValueError:owner.notify('Use finite, nonnegative thresholds; 0 disables',severity='error');return
            owner.workflow_limits=limits;owner.workflow_updated=None;owner.save_preferences();owner.notify('Task alert thresholds saved')
        elif key=='workflow-terminal':owner.workflow_terminal(next((x for x in owner.workflow_items() if x['id']==owner.selected_source),{}))
        elif key=='workflow-resume':
            command=resume_command(next((x for x in owner.workflow_items() if x['id']==owner.selected_source),{}))
            if command:owner.copy_to_clipboard(command);owner.notify('Resume command copied; check session ownership before running')
            else:owner.notify('No supported native resume command for this session')
