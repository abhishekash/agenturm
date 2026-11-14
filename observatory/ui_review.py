"""Lazy, frozen review of captured work, with explicit paging and exports."""
from __future__ import annotations
from copy import deepcopy
import json
from rich.text import Text
from textual import on, work
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, DataTable, RichLog, Static,Input
from .feed_requests import shared_get
from .workflow_evidence import ReviewHistory, scope_for, render_review
from .review_facts import refreshed_review_item, review_capture_policy
from .ui_export import ReceiptExportScreen


class ReviewLog(RichLog):
    can_focus = True


class ReviewScreen(ModalScreen):
    DEFAULT_CSS = '''
    ReviewScreen { background: $surface; }
    #review-frame { width: 100%; height: 100%; padding: 0 1; }
    #review-heading { height: auto; max-height: 3; text-style: bold; }
    #review-status { height: auto; max-height: 2; }
    #review-body { height: 1fr; border-left: solid transparent; }
    #review-body:focus { border-left: solid $primary; }
    #review-controls { height: 3; }
    #review-controls Button { width: 1fr; min-width: 8; }
    #review-hints { height: 1; }
    #review-floor { display: none; height: 1fr; content-align: center middle; }
    ReviewScreen.too-small #review-frame { display: none; }
    ReviewScreen.too-small #review-floor { display: block; }
    '''
    BINDINGS = [Binding('escape','dismiss','Back'), Binding('c','copy','Copy brief'), Binding('j','copy_json','Copy JSON'),
                Binding('x','export','Export'), Binding('R','refresh','Refresh'),Binding('g','git','Capture Git'),Binding('m','map_workspace','Map checkout'),Binding('d','draft_task','Draft task')]

    def __init__(self, owner, item):
        super().__init__()
        self.owner, self.item = owner, deepcopy(item)
        snapshot = owner.feed.snapshot
        self.scope = scope_for(item, snapshot, getattr(owner.feed.api, 'endpoint', ''))
        self.item.update(review_capture_policy(self.item,snapshot,self.scope))
        self.history = owner.review_cache.get(self.scope, self.item.get('stamp'), self.item.get('capture_revision',self.item.get('capture_content')))
        if self.item.get('capture_content') is False:
            self.item['task']='Selected execution (metadata-only)'
            self.history=ReviewHistory(self.scope,self.item.get('stamp'),False,loaded=True)
        self.projection = None
        self.generation = 0
        self.started_snapshot = snapshot.updated
        self.workspace_observation=None;self.git_generation=0;self.git_busy=False
        if not hasattr(owner,'review_git_comparisons'):owner.review_git_comparisons={}

    def compose(self):
        yield Static('Review needs at least 60 columns × 18 rows.\nResize to continue · Esc Back',id='review-floor',markup=False)
        with Vertical(id='review-frame'):
            yield Static('Review recorded work',id='review-heading',markup=False)
            yield Static('',id='review-status',markup=False)
            yield ReviewLog(id='review-body',min_width=1,wrap=True,markup=False,auto_scroll=False)
            with Horizontal(id='review-controls'):
                yield Button('Earlier',id='review-earlier')
                yield Button('Refresh',id='review-refresh')
                yield Button('Copy',id='review-copy')
                yield Button('Export',id='review-export')
                yield Button('Save task',id='review-save')
                yield Button('Draft · d',id='review-draft')
                yield Button('Git · g',id='review-git')
                yield Button('Excerpt',id='review-excerpt')
                yield Button('Back',id='review-back')
            yield Static('↑↓ Scroll · Tab · g Git · d Draft · m Map · c Brief · j JSON · Esc Back',id='review-hints',markup=False)

    def on_mount(self):
        self.on_resize()
        self.render_projection()
        self.query_one('#review-body').focus()
        if not self.history.loaded:self.request_page()
        self.set_interval(2,self.freshness)

    def on_resize(self):
        self.set_class(self.app.size.width < 60 or self.app.size.height < 18,'too-small')
        if self.is_mounted:
            for button in self.query('#review-controls Button'):button.styles.min_width=0
            compact=self.app.size.width<80
            for key,wide,narrow in (('earlier','Earlier','Older'),('refresh','Refresh','Reload'),('save','Save task','Save'),('excerpt','Excerpt','Draft')):
                self.query_one('#review-'+key,Button).label=narrow if compact else wide
            self.query_one('#review-hints',Static).update('↑↓ Scroll · Tab · g Git · d Draft · m Map · c Brief · j JSON · Esc Back' if compact else '↑↓ PgUp/PgDn · Tab · g Git · d Draft · m Map · c Brief · j JSON · x Export · Esc Back')
            self.call_after_refresh(self.render_projection)

    def on_unmount(self):
        self.generation += 1;self.git_generation+=1
        self.history.busy = False

    def freshness(self):
        self.apply_capture_policy()
        if self.history.busy: status='Loading exactly scoped recorded evidence…'
        elif self.history.error: status='Evidence unavailable: ' + self.history.error + ' · Refresh to retry'
        elif self.owner.feed.snapshot.updated != self.started_snapshot: status='Frozen review · newer snapshot available · Refresh to update'
        else: status='Frozen review · Refresh to update'
        self.query_one('#review-status',Static).update(status)

    def apply_capture_policy(self):
        policy=review_capture_policy(self.item,self.owner.feed.snapshot,self.scope)
        if policy.get('capture_content') is False and (self.item.get('capture_content') is not False or self.history.events):
            self.item.update(policy);self.generation+=1;self.git_generation+=1;self.workspace_observation=None;self.git_busy=False
            self.item['task']='Selected execution (metadata-only)'
            self.history=ReviewHistory(self.scope,self.history.revision,False,loaded=True)
            self.owner.review_cache.entries.clear()
            self.owner.review_git_comparisons.clear()
            self.render_projection()
        if self.owner.feed.snapshot.receiver_id and self.owner.feed.snapshot.receiver_id!=self.scope.receiver:
            self.workspace_observation=None;self.git_generation+=1;self.git_busy=False
            if self.projection:self.projection.pop('workspace_observation',None);self.projection.pop('parallel_path_observations',None)
            self.owner.review_git_comparisons.clear()
        if self.workspace_observation and self.projection and self.is_mounted:
            before=self.projection.pop('parallel_path_observations',None)
            self.attach_parallel_observations()
            if before!=self.projection.get('parallel_path_observations'):self.render_projection()
        return policy

    def render_projection(self):
        self.projection = self.history.projection(self.item)
        if self.workspace_observation and self.item.get('capture_content') is not False:
            from .review_continuity import freeze_workspace
            self.projection=freeze_workspace(self.projection,self.workspace_observation)
            self.attach_parallel_observations()
        self.query_one('#review-heading',Static).update(self.projection['title']+' · Review')
        self.owner.last_review={'item_id':self.item.get('id'),'projection':self.projection}
        log=self.query_one('#review-body',RichLog)
        scroll=log.scroll_y
        text=render_review(self.projection).split('\n',1)[-1].lstrip()
        if self.workspace_observation:
            from .review_continuity import render_workspace
            text+='\n\n'+render_workspace(self.workspace_observation)
            if self.projection.get('parallel_path_observations'):
                from .review_continuity import render_parallel
                text+='\n\n'+render_parallel(self.projection['parallel_path_observations'])
        log.clear();log.write(Text(text),width=max(1,log.scrollable_content_region.width))
        log.scroll_to(y=scroll,animate=False)
        self.query_one('#review-earlier',Button).disabled=not bool(self.history.cursor) or self.history.busy
        self.query_one('#review-refresh',Button).disabled=self.history.busy
        self.query_one('#review-git',Button).disabled=self.git_busy or self.history.busy or self.item.get('capture_content') is False
        self.query_one('#review-excerpt',Button).disabled=self.item.get('capture_content') is False or not any(self.projection[k] for k in ('checks','edits','errors','context_events'))
        self.freshness()

    def request_page(self, *, refresh=False):
        if self.history.busy:return
        candidate=refreshed_review_item(self.item,self.owner.feed.snapshot,self.scope) if refresh else dict(self.item)
        request_scope=scope_for(candidate,self.owner.feed.snapshot,getattr(self.owner.feed.api,'endpoint',''))
        if request_scope.scope_key!=self.scope.scope_key:
            self.history.error='Receiver or execution identity changed; reopen the review.';self.render_projection();return
        if self.scope.kind=='unavailable':
            self.history.error='No exact agent or producer/session identity is available.';self.render_projection();return
        if candidate.get('capture_content') is False:
            self.item=candidate;self.scope=request_scope
            self.history=ReviewHistory(request_scope,self.history.revision,False,loaded=True)
            self.render_projection();return
        if request_scope.run_start_ns and not request_scope.run_id and not request_scope.run_end_ns:
            self.history.error='No exact trace or recorded lifecycle end is available to isolate this run.';self.render_projection();return
        if self.scope.run_id and not getattr(self.owner.feed.snapshot,'review_trace_filter',False):
            self.history.error='This receiver cannot filter an exact trace. Update the receiver to review this run safely.';self.render_projection();return
        cursor=None if refresh else self.history.failed_cursor or self.history.cursor
        candidate['_review_snapshot_updated']=self.owner.feed.snapshot.updated if refresh else self.started_snapshot
        self.history.busy=True
        self.generation+=1
        self.render_projection()
        self.read_page(self.generation,cursor,refresh,candidate,request_scope)

    @work(thread=True,exclusive=True,group='review-page',exit_on_error=False)
    def read_page(self,generation,cursor,refresh,candidate,request_scope):
        response=None;error=''
        try:
            if scope_for(self.item,self.owner.feed.snapshot,getattr(self.owner.feed.api,'endpoint','')).receiver!=self.scope.receiver:
                raise ValueError('Receiver identity changed; reopen the review before loading more evidence')
            response=shared_get(self.owner.feed.api,request_scope.path(cursor),review=True,cancelled=lambda:not self.is_mounted or generation!=self.generation)
        except Exception as exc:error=str(exc)[:500] or 'Receiver unavailable'
        try:self.app.call_from_thread(self.finish_page,generation,cursor,refresh,response,error,candidate,request_scope)
        except RuntimeError:pass

    def finish_page(self,generation,cursor,refresh,response,error,candidate,request_scope):
        if not self.is_mounted or generation!=self.generation:return
        self.history.busy=False
        current=refreshed_review_item(self.item,self.owner.feed.snapshot,self.scope)
        if current.get('capture_content') is False:
            self.item=current
            self.history=ReviewHistory(self.scope,self.history.revision,False,loaded=True)
            self.owner.review_cache.entries.clear()
            self.render_projection();return
        if current.get('capture_revision')!=candidate.get('capture_revision'):
            self.item=current
            self.history=ReviewHistory(self.scope,self.history.revision,current.get('capture_revision'))
            self.history.error='Capture policy changed during the read; refresh to load matching evidence.'
            self.owner.review_cache.entries.clear()
            self.render_projection();return
        if scope_for(self.item,self.owner.feed.snapshot,getattr(self.owner.feed.api,'endpoint','')).receiver!=self.scope.receiver:
            error='Receiver identity changed; reopen the review before loading more evidence'
        if error:self.history.error=error;self.history.failed_cursor=cursor
        else:
            try:
                target=ReviewHistory(request_scope,self.history.revision,candidate.get('capture_revision',candidate.get('capture_content'))) if refresh else self.history
                target.accept(response)
                self.item=candidate
                self.scope=request_scope
                self.history=target
                self.owner.review_cache.entries[target.key]=target
                self.owner.review_cache.trim(keep=target.key)
                self.started_snapshot=candidate.get('_review_snapshot_updated',self.started_snapshot)
            except (ValueError,TypeError) as exc:
                self.history.error=str(exc);self.history.failed_cursor=cursor
        self.render_projection()

    def attach_parallel_observations(self):
        from .review_continuity import remember_comparison,overlap_observations
        from .workflow_evidence import ExecutionScope
        cache=self.owner.review_git_comparisons
        current_receiver=getattr(self.owner.feed.snapshot,'receiver_id','')
        for key,row in list(cache.items()):
            scope=ExecutionScope(**row['scope'])
            if scope.receiver!=current_receiver or review_capture_policy({},self.owner.feed.snapshot,scope).get('capture_content') is False:
                cache.pop(key,None)
        try:key=remember_comparison(cache,self.projection)
        except ValueError:return
        if len(cache)<2:return
        comparisons=overlap_observations([cache[key],*[row for name,row in cache.items() if name!=key]])
        comparisons['observations']=[row for row in comparisons['observations'] if self.scope.agent_id in row['agents']]
        comparisons['cache_limits']=dict(executions=8,total_bytes=512000,entry_bytes=128000,retained='comparison metadata only; Git diff bodies are not retained')
        self.projection['parallel_path_observations']=comparisons

    def action_draft_task(self):
        self.apply_capture_policy()
        if self.current_policy().get('capture_content') is False:return
        from .ui_saved_tasks import SavedTasksScreen,capture_selected_run
        from .review_continuity import editable_draft
        try:
            context=capture_selected_run(self.owner,item_override=self.item,review_snapshot=self.projection)
            if not context or context.get('capture_content') is False:return
            context['continuity_draft']=editable_draft(self.projection)
        except ValueError as exc:self.notify(str(exc),severity='warning');return
        self.app.push_screen(SavedTasksScreen(self.owner,create=True,capture_context=context))

    def action_git(self):
        if self.git_busy or self.history.busy:return
        if self.apply_capture_policy().get('capture_content') is False:
            self.notify('Metadata-only sources cannot supply a Git observation.',severity='warning');return
        if self.workspace_observation:
            self.capture_git(self.workspace_observation['binding']);return
        self.action_map_workspace()

    def action_map_workspace(self):
        if self.git_busy or self.history.busy:return
        if self.apply_capture_policy().get('capture_content') is False:return
        self.app.push_screen(WorkspaceMappingScreen(self.item.get('working_directory') or ''),self.workspace_selected)

    def workspace_selected(self,workspace):
        if not workspace or not self.is_mounted:return
        from .review_continuity import bind_workspace
        try:binding=bind_workspace(self.projection,workspace,reported_workspace=self.item.get('working_directory') or '')
        except ValueError as exc:self.notify(str(exc),severity='warning');return
        self.capture_git(binding)

    def capture_git(self,binding):
        if self.apply_capture_policy().get('capture_content') is False:return
        self.git_busy=True;self.git_generation+=1
        self.read_git(self.git_generation,self.generation,deepcopy(self.projection),binding,
            deepcopy(self.workspace_observation),deepcopy(self.current_policy()))
        self.query_one('#review-git',Button).disabled=True

    @work(thread=True,exclusive=True,group='review-git',exit_on_error=False)
    def read_git(self,generation,source_generation,projection,binding,previous,policy):
        result=None;error=''
        try:
            if self.current_policy()!=policy or self.owner.feed.snapshot.receiver_id!=self.scope.receiver:
                raise ValueError('Capture policy or receiver changed before Git observation.')
            from .review_continuity import capture_review_workspace
            result=capture_review_workspace(projection,binding,previous)
        except Exception as exc:error=str(exc)[:500]
        try:self.app.call_from_thread(self.git_ready,generation,source_generation,result,error,policy)
        except RuntimeError:pass

    def git_ready(self,generation,source_generation,result,error,policy):
        if not self.is_mounted or generation!=self.git_generation:return
        self.git_busy=False
        if source_generation!=self.generation or self.current_policy()!=policy or self.owner.feed.snapshot.receiver_id!=self.scope.receiver:
            self.workspace_observation=None;self.notify('Git observation discarded: source, capture policy or receiver changed.',severity='warning')
        elif error:self.notify(error,severity='warning')
        else:self.workspace_observation=result
        self.render_projection()

    def action_refresh(self):self.request_page(refresh=True)

    def frozen_json(self):return json.dumps(self.projection,ensure_ascii=False,indent=2)+'\n'

    def action_copy(self):
        self.apply_capture_policy()
        self.start_receipt(False)

    def action_copy_json(self):
        self.apply_capture_policy()
        self.app.copy_to_clipboard(self.frozen_json());self.notify('Frozen review copied as JSON.')

    def action_export(self):
        self.apply_capture_policy()
        self.start_receipt(True)

    def current_policy(self):
        return review_capture_policy(self.item,self.owner.feed.snapshot,self.scope)

    def start_receipt(self,export):
        from .ui_saved_tasks import projection_policy_state
        projection=deepcopy(self.projection)
        state=projection_policy_state(self.owner,projection)
        if not state[0]:
            self.notify('Source capture changed. Refresh the current evidence before sharing.',severity='warning');return
        self.prepare_receipt(projection,export,state)

    @work(thread=True,exclusive=True,group='review-format',exit_on_error=False)
    def prepare_receipt(self,projection,export,policy):
        from .work_receipts import freeze_receipt
        from .ui_saved_tasks import projection_policy_state
        if projection_policy_state(self.owner,projection)!=policy:return
        formats=freeze_receipt(projection)
        try:self.app.call_from_thread(self.receipt_ready,formats,export,policy,projection)
        except RuntimeError:pass

    def receipt_ready(self,formats,export,policy,projection=None):
        from .ui_saved_tasks import projection_policy_state
        if not self.is_mounted:return
        projection=deepcopy(self.projection if projection is None else projection)
        invalidated=False
        def valid():
            nonlocal invalidated
            if not self.is_mounted or projection_policy_state(self.owner,projection)!=policy or not policy[0]:invalidated=True
            return not invalidated
        if not valid():
            self.apply_capture_policy();self.notify('Capture policy changed. Try again with the current evidence.',severity='warning');return
        if export:self.app.push_screen(ReceiptExportScreen(formats,valid=valid))
        else:
            self.app.copy_to_clipboard(formats['markdown']);self.notify('Frozen evidence brief copied as Markdown.')

    @on(Button.Pressed)
    def pressed(self,event):
        event.stop()
        action=event.button.id
        if action=='review-earlier':self.request_page()
        elif action=='review-refresh':self.action_refresh()
        elif action=='review-copy':self.action_copy()
        elif action=='review-export':self.action_export()
        elif action=='review-excerpt':
            self.app.push_screen(ReviewExcerptScreen(self.owner,self.projection,self.item))
        elif action=='review-git':self.action_git()
        elif action=='review-draft':self.action_draft_task()
        elif action=='review-save':
            from .ui_saved_tasks import SavedTasksScreen
            from .ui_saved_tasks import capture_selected_run
            context=capture_selected_run(self.owner,item_override=self.item,review_snapshot=self.projection)
            self.app.push_screen(SavedTasksScreen(self.owner,create=True,capture_context=context))
        elif action=='review-back':self.dismiss()


class ReviewExcerptScreen(ModalScreen):
    DEFAULT_CSS = """
    ReviewExcerptScreen { background: $surface; padding: 0 1; }
    #review-source-heading { height: auto; max-height: 3; }
    #review-source-list { height: 1fr; }
    #review-source-hint { height: 2; }
    """
    BINDINGS=[('escape','dismiss','Back')]

    def __init__(self,owner,projection,item):
        super().__init__();self.owner=owner;self.projection=projection;self.item=item;self.rows={}

    def compose(self):
        yield Static('Choose recorded evidence to draft a decision. Saving remains explicit.',id='review-source-heading',markup=False)
        yield DataTable(id='review-source-list',cursor_type='row')
        yield Static('↑↓ Choose · Enter Save excerpt to a task · Esc Back',id='review-source-hint',markup=False)

    def on_mount(self):
        table=self.query_one(DataTable);table.add_columns('Recorded source / excerpt')
        for kind in ('checks','edits','errors','context_events'):
            for row in self.projection[kind]:
                text=row.get('command') or row.get('path') or row.get('subject') or ''
                key=str(len(self.rows));self.rows[key]=(row['source'],text)
                table.add_row(Text(kind.replace('_',' ')+' · '+text,overflow='ellipsis',no_wrap=True),key=key)
        table.focus()

    @on(DataTable.RowSelected)
    def choose(self,event):
        from .ui_saved_tasks import SavedTasksScreen,capture_selected_run
        ref,text=self.rows[event.row_key.value];scope=self.projection['scope']
        source=dict(receiver=scope['receiver'],producer=scope['producer'],agent_id=scope['agent_id'],
            session_id=scope['session_id'],trace_id=str(ref.get('trace_id') or ''),
            event_id=str(ref.get('source_id') or ref.get('id') or ''),
            run_start_ns=scope.get('run_start_ns'),
            coverage=self.projection['window']['coverage'],
            captured_ns=int(self.projection['captured_at']*1e9),content_available=True)
        if source.get('run_start_ns') is None:source.pop('run_start_ns')
        context=capture_selected_run(self.owner,item_override=self.item,review_snapshot=self.projection)
        self.app.push_screen(SavedTasksScreen(self.owner,decision_excerpt={'source':source,'text':text[:4000]},capture_context=context))


class WorkspaceMappingScreen(ModalScreen):
    """Explicitly bind a local path; never trust a receiver cwd by address."""
    DEFAULT_CSS='''
    WorkspaceMappingScreen { align: center middle; background: $background 70%; }
    #workspace-mapping { width: 90%; max-width: 90; height: auto; padding: 1; border: round $primary; background: $surface; }
    #workspace-mapping Static { height: auto; margin-bottom: 1; }
    #workspace-mapping Input { height: 3; }
    #workspace-map-actions { height: 3; }
    #workspace-map-actions Button { width: 1fr; min-width: 0; }
    '''
    BINDINGS=[Binding('escape','cancel','Back'),Binding('ctrl+s','map','Map and capture',priority=True)]

    def __init__(self,reported):super().__init__();self.reported=reported
    def compose(self):
        with Vertical(id='workspace-mapping'):
            yield Static('Map this recorded execution to a local checkout.\nReceiver cwd is unverified: '+str(self.reported or 'Not reported'),markup=False)
            yield Input(placeholder='Explicit local checkout path',id='workspace-local')
            yield Static('Read-only Git capture only. Current changes may include pre-existing or concurrent work. Ctrl+S maps and captures; Esc cancels.',markup=False)
            with Horizontal(id='workspace-map-actions'):
                yield Button('Map + capture · Ctrl+S',id='workspace-map')
                yield Button('Cancel · Esc',id='workspace-cancel')
    def on_mount(self):self.query_one(Input).focus()
    def action_cancel(self):self.dismiss(None)
    def action_map(self):
        value=self.query_one(Input).value.strip()
        if value:self.dismiss(value)
        else:self.notify('Choose an explicit local checkout path.',severity='warning')
    @on(Button.Pressed)
    def pressed(self,event):
        event.stop()
        if event.button.id=='workspace-map':self.action_map()
        else:self.action_cancel()
