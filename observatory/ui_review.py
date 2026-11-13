"""Lazy, frozen review of captured work, with explicit paging and exports."""
from __future__ import annotations
from copy import deepcopy
import json
from rich.text import Text
from textual import on, work
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, DataTable, RichLog, Static
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
                Binding('x','export','Export'), Binding('R','refresh','Refresh')]

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
                yield Button('Excerpt',id='review-excerpt')
                yield Button('Back',id='review-back')
            yield Static('↑↓ Scroll · Tab · c Brief · j JSON · x Export · Esc Back',id='review-hints',markup=False)

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
            self.query_one('#review-hints',Static).update('↑↓ Scroll · Tab · c Brief · j JSON · x Export · Esc Back' if compact else '↑↓ PgUp/PgDn Scroll · Tab Controls · c Brief · j JSON · x Export · Esc Back')
            self.call_after_refresh(self.render_projection)

    def on_unmount(self):
        self.generation += 1
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
            self.item.update(policy);self.generation+=1
            self.item['task']='Selected execution (metadata-only)'
            self.history=ReviewHistory(self.scope,self.history.revision,False,loaded=True)
            self.owner.review_cache.entries.clear()
            self.render_projection()
        return policy

    def render_projection(self):
        self.projection = self.history.projection(self.item)
        self.query_one('#review-heading',Static).update(self.projection['title']+' · Review')
        self.owner.last_review={'item_id':self.item.get('id'),'projection':self.projection}
        log=self.query_one('#review-body',RichLog)
        scroll=log.scroll_y
        log.clear();log.write(Text(render_review(self.projection).split('\n',1)[-1].lstrip()),width=max(1,log.scrollable_content_region.width))
        log.scroll_to(y=scroll,animate=False)
        self.query_one('#review-earlier',Button).disabled=not bool(self.history.cursor) or self.history.busy
        self.query_one('#review-refresh',Button).disabled=self.history.busy
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

    def action_refresh(self):self.request_page(refresh=True)

    def frozen_json(self):return json.dumps(self.projection,ensure_ascii=False,indent=2)+'\n'

    def action_copy(self):
        self.apply_capture_policy()
        self.prepare_receipt(self.projection,False,self.current_policy())

    def action_copy_json(self):
        self.apply_capture_policy()
        self.app.copy_to_clipboard(self.frozen_json());self.notify('Frozen review copied as JSON.')

    def action_export(self):
        self.apply_capture_policy()
        self.prepare_receipt(self.projection,True,self.current_policy())

    def current_policy(self):
        return review_capture_policy(self.item,self.owner.feed.snapshot,self.scope)

    @work(thread=True,exclusive=True,group='review-format',exit_on_error=False)
    def prepare_receipt(self,projection,export,policy):
        from .work_receipts import freeze_receipt
        formats=freeze_receipt(projection)
        try:self.app.call_from_thread(self.receipt_ready,formats,export,policy)
        except RuntimeError:pass

    def receipt_ready(self,formats,export,policy):
        if not self.is_mounted:return
        if self.current_policy()!=policy:
            self.apply_capture_policy();self.notify('Capture policy changed. Try again with the current evidence.',severity='warning');return
        if export:self.app.push_screen(ReceiptExportScreen(formats,valid=lambda:self.is_mounted and self.current_policy()==policy))
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
            session_id=scope['session_id'],trace_id=scope['run_id'],
            event_id=str(ref.get('source_id') or ref.get('id') or ''),
            run_start_ns=scope.get('run_start_ns'),
            coverage=self.projection['window']['coverage'],
            captured_ns=int(self.projection['captured_at']*1e9),content_available=True)
        if source.get('run_start_ns') is None:source.pop('run_start_ns')
        context=capture_selected_run(self.owner,item_override=self.item,review_snapshot=self.projection)
        self.app.push_screen(SavedTasksScreen(self.owner,decision_excerpt={'source':source,'text':text[:4000]},capture_context=context))
