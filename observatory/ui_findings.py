"""Exact finding sources, loaded off the UI loop and fenced by current policy."""
import asyncio
import json

from rich.text import Text
from textual import on,work
from textual.containers import Vertical,Horizontal
from textual.screen import ModalScreen
from textual.widgets import Button,RichLog,Static

from .review_facts import review_capture_policy
from .workflow_evidence import scope_for


def source_fields(record,path=''):
    """Keep field provenance while displaying recorded strings as readable text."""
    if isinstance(record,dict) and record:
        for key,value in record.items():yield from source_fields(value,(path+' · ' if path else '')+str(key))
    elif isinstance(record,list) and record:
        for index,value in enumerate(record):yield from source_fields(value,path+' ['+str(index)+']')
    else:yield path or 'Recorded content',record if isinstance(record,str) else json.dumps(record,ensure_ascii=False)


class FindingSourceScreen(ModalScreen):
    DEFAULT_CSS='''
    FindingSourceScreen { align: center middle; background: $background 70%; }
    #finding-source-dialog { width: 95%; height: 95%; border: round $primary; background: $surface; padding: 1; }
    #finding-source-status { height: auto; max-height: 4; }
    #finding-source-log { height: 1fr; }
    #finding-source-actions { height: 3; }
    #finding-source-actions Button { width: 1fr; }
    '''
    BINDINGS=[('escape','dismiss','Back'),('r','refresh','Refresh')]

    def __init__(self,owner,item):
        super().__init__();self.owner=owner;self.item=dict(item)
        self.receiver=item.get('receiver_id') or owner.feed.snapshot.receiver_id
        self.sources=[];self.generation=0

    def compose(self):
        with Vertical(id='finding-source-dialog'):
            yield Static('Exact recorded evidence',id='finding-source-status')
            yield RichLog(id='finding-source-log',wrap=True,markup=False,auto_scroll=False,min_width=1)
            with Horizontal(id='finding-source-actions'):
                yield Button('Refresh · r',id='finding-source-refresh')
                yield Button('Back · Esc',id='finding-source-close')

    def allowed(self):
        snapshot=self.owner.feed.snapshot
        if not self.receiver or snapshot.receiver_id!=self.receiver:return False
        for source in [{'scope':self.item},*self.sources]:
            item=source.get('scope') or {}
            if review_capture_policy(item,snapshot,scope_for(item,snapshot)).get('capture_content') is False:return False
        return True

    def on_mount(self):
        self.query_one(RichLog).focus();self.set_interval(.25,self.check_policy);self.action_refresh()

    def check_policy(self):
        if not self.allowed():
            self.generation+=1;self.sources=[]
            self.query_one(RichLog).clear()
            self.query_one('#finding-source-status',Static).update('Source content withheld: capture policy or receiver changed.')

    def action_refresh(self):
        if not self.allowed():self.check_policy();return
        self.generation+=1;self.load_sources(self.generation)

    @work(group='finding-source',exclusive=True)
    async def load_sources(self,generation):
        self.query_one('#finding-source-status',Static).update('Loading exact recorded evidence…')
        try:
            response=await asyncio.to_thread(self.owner.feed.api.get,'/api/findings/'+self.item['id']+'/source')
            if not self.is_mounted or self.app.screen is not self or generation!=self.generation:return
            if response.get('receiver_id')==self.receiver and response.get('finding_id')==self.item['id'] and response.get('unavailable'):
                self.sources=[];self.query_one(RichLog).clear()
                self.query_one('#finding-source-status',Static).update('Finding no longer retained, was revoked, or is unavailable. Refresh Findings; Esc to return.')
                return
            identity=response.get('finding_scope')
            same_scope=isinstance(identity,dict) and all((identity.get(key) or None)==(self.item.get(key) or None)
                for key in ('service','agent_id','session_id','trace_id','turn_id'))
            if response.get('receiver_id')!=self.receiver or response.get('finding_id')!=self.item['id'] or not same_scope:
                self.sources=[];self.query_one(RichLog).clear()
                self.query_one('#finding-source-status',Static).update('Source content withheld: receiver, finding or owner identity changed.');return
            self.sources=response.get('sources') or []
            if not self.allowed():self.check_policy();return
            log=self.query_one(RichLog);log.clear()
            for source in self.sources:
                scope=source.get('scope') or {}
                log.write(Text(source.get('type','Source')+' · '+str(source.get('id') or 'Unknown reference'),style='bold'))
                log.write(Text('Producer / actor / session: '+ ' / '.join(str(scope.get(key) or 'Not reported') for key in ('service','agent_id','session_id'))))
                if source.get('record') is None:log.write('Source unavailable.')
                else:
                    for label,text in source_fields(source['record']):
                        log.write(Text(label,style='bold'));log.write(Text(text))
                if source.get('reason'):log.write(Text(source['reason']))
                log.write('')
            status=('Source no longer retained, was revoked, or is unavailable.' if response.get('unavailable') else
                response.get('coverage') or 'Exact source evidence')
            if response.get('references_clipped'):status+=' Additional references omitted.'
            log.write(Text(status))
            self.query_one('#finding-source-status',Static).update('Exact evidence · '+str(len(self.sources))+'/'+str(response.get('references_total',len(self.sources)))+' references\n↑/↓ scroll · PgUp/PgDn · Home/End')
        except Exception:
            if self.is_mounted and generation==self.generation:
                self.query_one('#finding-source-status',Static).update('Source read unavailable or index catching up. Press r to retry; Esc to return.')

    @on(Button.Pressed,'#finding-source-refresh')
    def refresh_pressed(self):self.action_refresh()

    @on(Button.Pressed,'#finding-source-close')
    def close_pressed(self):self.dismiss()


def finding_capture_allowed(item,snapshot):
    if item.get('receiver_id') and item['receiver_id']!=snapshot.receiver_id:return False
    return review_capture_policy(item,snapshot,scope_for(item,snapshot)).get('capture_content') is not False
