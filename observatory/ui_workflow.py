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
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.screen import ModalScreen
from textual.widgets import Button, Checkbox, DataTable, Input, Label, RichLog
from rich.text import Text
from .ui_monitoring import task_run_usage, task_usage_summary
from .session_status import today_bucket


from .attention import AttentionLedger as PureAttentionLedger, classify, fresh_observation
from .workflow_evidence import scope_for, safe_text, check_observation
from .ui_workflow_controller import WorkflowController, prepared_item


def events_for(item, events):
    scope=scope_for(item)
    return sorted((event for event in events if scope.matches(event)),
                  key=lambda event:event.get('time_unix_nano') or 0)


def workflow_reasons(item, usage, snapshot, limits, now=None):
    """Compatibility description for Today; the durable inbox retains open history."""
    now=time.time() if now is None else now
    selected=dict(item)
    if selected.get('state') in {'Error','Failed','Interrupted','Incomplete','Completed'} and not today_bucket(selected,int(now*1e9)):
        return []
    if usage.get('trace_id') and not selected.get('trace_id'):selected['trace_id']=usage['trace_id']
    return [incident.reason for incident in classify(selected,scope_for(selected,snapshot),usage,limits,now=now,
        stall_anchor='preview' if fresh_observation(item,snapshot,now) else '',events=snapshot.activity)
        if incident.kind!='completion']


class AttentionLedger(PureAttentionLedger):
    """Compatibility adapter; the incident engine itself has no UI imports."""
    def update(self,items,snapshot,limits,now=None,*,endpoint=''):
        return super().update([prepared_item(item,snapshot) for item in items],snapshot,limits,now,endpoint=endpoint)


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
    if item.get('stamp'):lines.append(f"Last recorded activity: {max(0,int(time.time()-item['stamp']/1e9))}s ago")
    if item.get('status_evidence'):lines.append(str(item['status_evidence']))
    if process.get('pids'):lines.append('Exact observed process IDs: '+', '.join(map(str,process['pids'])))
    if diagnostics.get('backlog_bytes'):lines.append('Collector backlog bytes: '+str(diagnostics['backlog_bytes']))
    if item.get('state')=='Possibly stalled':lines.append('A quiet process is evidence, not proof of a stall; check the native terminal.')
    cwd=item.get('working_directory')
    if cwd:lines+=['','WORKSPACE','Directory: '+str(cwd)]
    if item.get('branch'):lines.append('Branch: '+str(item['branch']))
    overlap=[x for x in items if x['id']!=item['id'] and cwd and x.get('working_directory')==cwd and x.get('active')]
    if overlap:lines+=['Other active sessions in this directory: '+', '.join(str(x.get('task') or x['id'])[:80] for x in overlap),
                      'Shared directory is an overlap warning; file ownership is not inferred.']
    if usage:lines+=['','TASK USAGE',task_usage_summary(usage,scope='trace')[0]]
    children=[x for x in items if item.get('agent_id') and x.get('parent_agent_id')==item['agent_id'] and x.get('relationship_state')=='confirmed' and x.get('relationship_kind')=='delegated']
    if children:
        lines+=['','CONFIRMED DIRECT CHILDREN · separate run usage, not an added subtotal']
        for child in children:
            u=task_run_usage(snapshot.sessions,child,snapshot.traces)
            lines+=[str(child.get('task') or child['id'])[:120],task_usage_summary(u,scope='trace')[0]]
    events=events_for(item,snapshot.activity)
    lines+=['','RECORDED WORK · loaded activity only','Open Review to load checks and edits for this exact execution.']
    files=[e for e in events if e.get('domain')=='file'];shell=[e for e in events if e.get('domain')=='shell']
    lines+=['Recorded file operations: '+str(len(files))]
    for e in files[-8:]:lines.append('  '+str(e.get('subject') or e.get('action') or 'file operation')[:180])
    last_edit=max((e.get('time_unix_nano',0) for e in files),default=0)
    checks=[]
    for event in shell:
        check=check_observation(event)
        if not check:continue
        stamp=check.get('completed_ns')
        later=bool(stamp and last_edit and stamp<last_edit)
        freshness='before latest recorded file operation; later edits exist' if later else 'freshness unknown'
        checks.append('  '+check['command'][:140]+' · '+check['result']+' · '+freshness)
    lines+=['Recorded check commands:']+(checks[-8:] or ['  None in loaded activity.'])
    lines+=['Recorded command results do not establish complete verification or a clean worktree.','','CONTEXT DIAGNOSTICS']
    context=[e for e in events if re.search(r'compact|truncat|context',str(e.get('action',''))+' '+str(e.get('subject','')),re.I)]
    lines += [str(e.get('subject') or e.get('action'))[:180] for e in context[-5:]] or ['No context events in loaded activity.']
    lines+=['Context occupancy is not inferred from cumulative token usage.','','HANDOFF',resume_command(item) or 'Native resume command unavailable for this runtime.',
            'Resume only after checking that the session is not already active.']
    return safe_text('\n'.join(lines))


class WorkflowScreen(ModalScreen):
    DEFAULT_CSS="""
    WorkflowScreen { align: center middle; background: $background 70%; }
    #workflow-dialog { width: 100%; height: 100%; background: $surface; border: round $primary; padding: 0 1; }
    #workflow-title { height: 1; }
    #workflow-body { height: 1fr; }
    #workflow-inbox, #workflow-log { height: 1fr; width: 1fr; }
    #workflow-settings-panel { height: 1fr; }
    #workflow-controls, #workflow-limits, #workflow-more { height: 3; }
    #workflow-controls Button, #workflow-more Button { min-width: 7; width: 1fr; }
    #workflow-limits Input { width: 1fr; border-title-color: $text; }
    #workflow-save { min-width: 7; width: 10; }
    #workflow-retention { height: auto; width: 1fr; }
    #workflow-too-small { height: 1fr; width: 1fr; content-align: center middle; }
    """
    BINDINGS=[('escape','back','Back')]

    def __init__(self,owner):
        super().__init__();self.owner=owner;self.selected_incident=None;self.rows_by_id={}
        self._table_signature=None;self._log_signature=None;self._layout_signature=None;self._settings=False;self._detail=False

    def compose(self):
        with Vertical(id='workflow-dialog'):
            yield Label('Workflow · Enter selects · Tab actions · Esc back',id='workflow-title')
            yield Label('Workflow needs 60 × 18. Resize to continue; Esc returns.',id='workflow-too-small')
            with Horizontal(id='workflow-body'):
                yield DataTable(id='workflow-inbox',cursor_type='row',zebra_stripes=True)
                yield RichLog(id='workflow-log',wrap=True,markup=False,min_width=1,auto_scroll=False)
            with VerticalScroll(id='workflow-settings-panel'):
                yield Checkbox('Desktop alerts · project and reason only',value=self.owner.workflow_notify,id='workflow-notify')
                with Horizontal(id='workflow-limits'):
                    for key,label in (('tokens','Task tokens'),('cost','Task USD'),('minutes','Task minutes')):
                        yield Input(str(self.owner.workflow_limits.get(key,0)),placeholder=label,id='workflow-'+key,tooltip=label+' threshold; 0 disables')
                    yield Button('Save',id='workflow-save')
                with Horizontal(id='workflow-more'):
                    yield Button('Snooze',id='workflow-snooze',tooltip='Snooze desktop alerts for 10 minutes')
                    yield Button('Terminal',id='workflow-terminal')
                    yield Button('Resume',id='workflow-resume',tooltip='Copy native resume command')
                    yield Button('Cleanup',id='workflow-cleanup',tooltip='Apply retention; unresolved incidents stay')
                yield Label('Receipts and their minimal execution references expire after 90 days. Other resolved incidents and delivery claims expire after 30 days. '
                    'Unresolved requests remain visible after Seen. Notifications attempt once; Retry explicitly repeats a failed attempt.',id='workflow-retention')
            with Horizontal(id='workflow-controls'):
                yield Button('Review',id='workflow-review')
                yield Button('Seen',id='workflow-seen')
                yield Button('Retry',id='workflow-retry',tooltip='Retry the selected failed desktop notification')
                yield Button('Refresh',id='workflow-refresh')
                yield Button('Settings',id='workflow-settings')

    def on_mount(self):
        self.query_one('#workflow-too-small').display=False
        self.query_one('#workflow-settings-panel').display=False
        for key,title in (('tokens','Tokens'),('cost','USD'),('minutes','Minutes')):
            self.query_one('#workflow-'+key,Input).border_title=title
        self.call_after_refresh(self.render_report)
        self.call_after_refresh(self.focus_initial)

    def focus_initial(self):
        table=self.query_one('#workflow-inbox',DataTable)
        (table if table.display and table.row_count else self.query_one('#workflow-log',RichLog)).focus()

    def on_resize(self):
        if self.is_mounted:self.call_after_refresh(self.render_report)

    def action_back(self):
        if self._settings:
            self._settings=False;self.query_one('#workflow-settings-panel').display=False;self.render_report()
            self.query_one('#workflow-settings').focus()
        elif self._detail and self.size.width<120:
            self._detail=False;self.render_report();self.query_one(DataTable).focus()
        else:self.dismiss()

    def selected_item(self):
        row=self.rows_by_id.get(self.selected_incident)
        if row:return row[0]
        return next((item for item in self.owner.workflow_items() if item['id']==self.owner.selected_source),None)

    def render_report(self):
        owner=self.owner;table=self.query_one(DataTable)
        small=self.size.width<60 or self.size.height<18
        self.query_one('#workflow-too-small').display=small
        self.query_one('#workflow-controls').display=not small
        self.query_one('#workflow-settings-panel').display=self._settings and not small
        self.query_one('#workflow-body').display=not self._settings and not small
        if small:return
        rows=owner.workflow_ledger.rows
        self.rows_by_id={item['_attention_id']:(item,reasons,since) for item,reasons,since in rows}
        if self.selected_incident not in self.rows_by_id:
            self.selected_incident=next((key for key,(item,_,_) in self.rows_by_id.items() if item['id']==owner.selected_source),None)
        wide=self.size.width>=120 and self.size.height>=24
        layout=(wide,self._detail,self._settings,self.size,bool(rows))
        if layout!=self._layout_signature:
            self._layout_signature=layout;self.call_after_refresh(self.render_report)
        self.query_one('#workflow-body').display=not self._settings
        table.display=bool(rows) and (wide or not self._detail)
        self.query_one('#workflow-log').display=wide or self._detail or not rows
        title='Workflow · '+('Settings' if self._settings else str(len(rows))+' items · Enter inspect' if rows else 'No attention items')+' · Esc back'
        warning=owner.workflow_controller.warning
        if 'capacity reached' in warning:title='Workflow · '+str(len(rows))+' items · Storage full · Esc back'
        elif warning and ('unavailable' in warning.lower() or 'disabled' in warning.lower()):title='Workflow · '+str(len(rows))+' items · Storage unavailable'
        self.query_one('#workflow-title',Label).update(title)
        available=max(24,table.scrollable_content_region.width-6)
        wait_width=12;reason_width=min(30,max(18,int(available*.4)));task_width=max(4,available-wait_width-reason_width)
        widths=(task_width,reason_width,wait_width)
        signature=(widths,tuple((item['_attention_id'],str(item.get('task') or item.get('project') or item.get('service') or 'Recorded execution'),tuple(reasons),int((time.time()-since)/60)) for item,reasons,since in rows))
        if signature!=self._table_signature:
            cursor_key=table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value if table.row_count else None
            old_y=table.scroll_y;old_row=table.cursor_row;table.clear(columns=True)
            for label,width in zip(('Task / project','Attention','Wait'),widths):table.add_column(label,width=width)
            for item,reasons,since in rows:
                stamp=item.get('_attention_source_time_ns')
                wait=f'{max(0,int((time.time()-(stamp/1e9 if stamp else since))/60))}m'+(' observed' if not stamp else '')
                reason={'approval':'Approval requested','reply':'Reply requested','failure':'Reported failure',
                    'interruption':'Task interrupted','completion':'Completion review','stall':'Possible stall',
                    'tokens':'Token threshold','cost':'Cost threshold','minutes':'Time threshold'}.get(item.get('_attention_kind'),'; '.join(reasons))
                if item.get('_attention_seen'):reason=reason.replace(' requested','')+' · seen'
                table.add_row(Text(safe_text(item.get('task') or item.get('project') or item.get('service') or 'Recorded execution')[:80]),
                              Text(reason),wait,key=item['_attention_id'])
            cursor_key=cursor_key if cursor_key in self.rows_by_id else self.selected_incident
            new_row=table.get_row_index(cursor_key) if cursor_key in self.rows_by_id else 0
            if cursor_key in self.rows_by_id:table.move_cursor(row=new_row,animate=False)
            table.scroll_to(y=max(0,old_y+new_row-old_row),animate=False)
            self._table_signature=signature
        item=self.selected_item()
        items=owner.workflow_items()
        text=workflow_report(item,items,owner.feed.snapshot) if item else 'Select a session in Agents or an incident above to inspect its workflow.'
        if item and item.get('_attention_id'):
            status='Seen; the recorded request remains pending.' if item.get('_attention_seen') else 'Not yet acknowledged.'
            if item.get('_attention_stale'):status+=' Capture is stale or this source is missing; resolution is unconfirmed.'
            if not item.get('_attention_persistent'):status+=' Durable acknowledgement unavailable: missing exact run/request identity or storage capacity.'
            text=status+'\n\n'+text
        text+='\n\nNotifications: '+owner.workflow_delivery+('\n'+warning if warning else '')
        log=self.query_one(RichLog)
        key=(self.selected_incident,owner.selected_source,text,log.content_size.width)
        if log.content_size.width and self._log_signature!=key:
            old_y=log.scroll_y if self._log_signature and self._log_signature[:2]==key[:2] else 0
            log.clear();log.write(Text(safe_text(text)),width=max(1,log.content_size.width-1))
            self.call_after_refresh(log.scroll_to,y=old_y,animate=False)
            self._log_signature=key
        seen=self.query_one('#workflow-seen',Button)
        seen.label='Undo' if item and item.get('_attention_seen') else 'Reviewed' if item and item.get('_attention_kind')=='completion' else 'Seen'
        seen.disabled=not item or not item.get('_attention_persistent')
        self.query_one('#workflow-retry',Button).disabled=not item or item.get('_attention_delivery') not in {'failed','unavailable'}
        self.query_one('#workflow-review',Button).disabled=item is None
        if not table.display and self.focused is table:log.focus()

    @on(DataTable.RowSelected,'#workflow-inbox')
    def select_attention(self,event):
        self.selected_incident=event.row_key.value
        item=self.rows_by_id[self.selected_incident][0]
        self.owner.selected_source=item['id'];self._detail=True;self.render_report()
        if self.size.width<120:self.query_one(RichLog).focus()

    @on(Checkbox.Changed,'#workflow-notify')
    def toggle_notifications(self,event):
        self.owner.workflow_notify=event.value
        self.owner.workflow_delivery='Enabled · waiting for new attention' if event.value else 'Disabled'
        self.owner.workflow_updated=None;self.owner.save_preferences();self.owner.update_workflow()

    @on(Button.Pressed)
    def pressed(self,event):
        owner=self.owner;key=event.button.id;item=self.selected_item() or {}
        if key=='workflow-settings':
            self._settings=not self._settings
            self.query_one('#workflow-settings-panel').display=self._settings
            self.render_report()
            if self._settings:self.query_one('#workflow-notify').focus()
        elif key=='workflow-refresh':
            owner.workflow_updated=None;owner.update_workflow();self.render_report()
        elif key=='workflow-review' and item:owner.action_review_work(item)
        elif key=='workflow-seen' and item:owner.workflow_controller.acknowledge(item,action='reviewed' if item.get('_attention_kind')=='completion' else 'seen')
        elif key=='workflow-retry' and item:owner.workflow_controller.retry(item)
        elif key=='workflow-cleanup':owner.workflow_controller.cleanup()
        elif key=='workflow-snooze':
            owner.workflow_snooze=0 if time.time()<owner.workflow_snooze else time.time()+600
            owner.workflow_delivery='Snoozed for 10 minutes' if owner.workflow_snooze else 'Snooze ended'
            owner.workflow_updated=None;owner.update_workflow();self.render_report()
        elif key=='workflow-save':
            try:
                limits={k:float(self.query_one('#workflow-'+k,Input).value) for k in ('tokens','cost','minutes')}
                if any(not math.isfinite(v) or v<0 for v in limits.values()):raise ValueError()
            except ValueError:owner.notify('Use finite, nonnegative thresholds; 0 disables',severity='error');return
            owner.workflow_limits=limits;owner.workflow_updated=None;owner.save_preferences();owner.update_workflow();owner.notify('Task alert thresholds saved')
        elif key=='workflow-terminal':owner.workflow_terminal(item)
        elif key=='workflow-resume':
            command=resume_command(item)
            if command:owner.copy_to_clipboard(command);owner.notify('Resume command copied; check session ownership before running')
            else:owner.notify('No supported native resume command for this session')
