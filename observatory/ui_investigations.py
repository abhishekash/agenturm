"""Typed retained-history search and pinned exact evidence in one bounded modal."""
from __future__ import annotations
import asyncio
from copy import deepcopy
import json
from itertools import chain
import re

from rich.text import Text
from textual import on, work
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, Checkbox, DataTable, Input, RichLog, Select, Static

from .investigation import decode_reference, encode_reference, reference_matches, validate_query
from .investigation_cli import collect_investigation, collect_source
from .review_facts import review_capture_policy
from .ui_findings import source_fields
from .workflow_evidence import scope_for

MAX_ROWS=250
MAX_PAGES=5
MAX_BYTES=2*1024*1024
SOURCE_BYTES=65536
IDENTITY=('service','agent_id','session_id','trace_id','span_id','turn_id')
_CONTROLS=re.compile(r'[\x00-\x08\x0b-\x1f\x7f-\x9f]')


def display_text(value,*,single=False):
    text=_CONTROLS.sub('',str(value or ''))
    return ' '.join(text.split()) if single else text


def policy_state(owner,receiver,references):
    """Known live policy, keyed by full recorded identity; never a new namespace."""
    snapshot=owner.feed.snapshot
    if not receiver or getattr(snapshot,'receiver_id','')!=receiver:return False,('receiver-changed',)
    local=getattr(snapshot,'local_status',{}) or {}
    if local.get('capture_content') is False:return False,('capture-disabled',)
    cache={};states=[]
    for reference in references:
        if not isinstance(reference,dict) or reference.get('receiver_id')!=receiver:return False,('foreign-reference',)
        scope=scope_for(reference,snapshot)
        key=(scope.producer,scope.agent_id,scope.session_id)
        if key not in cache:cache[key]=review_capture_policy(reference,snapshot,scope)
        policy=cache[key]
        if policy.get('capture_content') is False:return False,('source-revoked',)
        states.append((json.dumps({key:reference.get(key) for key in ('receiver_id',*IDENTITY)},sort_keys=True),
            json.dumps(policy,sort_keys=True)))
    return True,tuple(sorted(set(states)))


def source_references(response,expected):
    """Defense at publication in addition to collect_source's authenticated checks."""
    if response.get('event_id')!=expected['event_id'] or response.get('receiver_id')!=expected['receiver_id'] or not reference_matches(expected,response.get('source_reference')):
        raise ValueError('Exact source reference changed; evidence is withheld.')
    sources=response.get('sources')
    if not isinstance(sources,list) or len(sources)>8:raise ValueError('Malformed exact source response.')
    references=[expected]
    for entry in sources:
        scope=entry.get('scope') if isinstance(entry,dict) else None
        if (not isinstance(scope,dict) or entry.get('id')!=expected['source_id'] or entry.get('type')!=expected['source']
            or any(key not in scope or scope[key]!=expected.get(key) for key in IDENTITY)):
            raise ValueError('Exact source owner/execution changed; evidence is withheld.')
        references.append({**scope,'receiver_id':expected['receiver_id']})
    return references


def source_lines(response):
    """Bound field labels and recorded text together; recorded content is never markup."""
    lines=[];used=0;clipped=False
    for entry in response.get('sources') or []:
        scope=entry['scope']
        values=[('Recorded source',str(entry.get('type'))+' · '+' / '.join(str(scope.get(key) or 'Not reported') for key in ('service','agent_id','session_id')))]
        recorded=[('Availability',entry.get('reason') or 'Exact source no longer retained or unavailable.')] if entry.get('record') is None else source_fields(entry['record'])
        metadata=[('Exact source ID',str(entry.get('id') or 'Not reported')),
            ('Recorded owner / execution',' / '.join(str(scope.get(key) or 'Not reported') for key in IDENTITY))]
        tail=chain(recorded,metadata)
        for label,value in chain(values,tail):
            label=display_text(label,single=True)[:512]
            text=display_text(value)
            size=len((label+'\n'+text).encode())
            if used+size>SOURCE_BYTES-2048 or len(lines)>=796:clipped=True;break
            lines.append((label,text));used+=size
        if clipped:break
    lines.append(('Coverage',display_text(response.get('coverage') or 'Exact retained source only.').encode()[:1024].decode('utf-8','ignore')))
    if response.get('unavailable'):lines.append(('Availability','Source no longer retained, capture revoked, or unavailable; this is not an empty recorded output.'))
    if clipped:lines.append(('Display limit','Source display clipped at 64 KiB / 800 fields; omitted content is not interpreted.'))
    return lines


class InvestigationTable(DataTable):
    BINDINGS=[Binding('home','first_row','First'),Binding('end','last_row','Last')]
    def action_first_row(self):self.move_cursor(row=0,animate=False);self.scroll_home(animate=False)
    def action_last_row(self):self.move_cursor(row=max(0,self.row_count-1),animate=False);self.scroll_end(animate=False)


class InvestigationsScreen(ModalScreen):
    DEFAULT_CSS='''
    InvestigationsScreen { background: $surface; }
    #investigation-frame { height: 100%; width: 100%; padding: 0 1; }
    #investigation-heading { height: 1; text-style: bold; }
    #investigation-form, #investigation-filters { height: 3; }
    #investigation-kind { width: 19; }
    #investigation-query { width: 1fr; }
    #investigation-search { width: 8; min-width: 8; height: 3; padding: 0; }
    #investigation-service { width: 1fr; }
    #investigation-failed { width: 19; height: 3; }
    #investigation-status { height: 2; max-height: 2; }
    #investigation-results, #investigation-source { height: 1fr; }
    #investigation-source:focus { border-left: solid $primary; }
    #investigation-controls { height: 1; }
    #investigation-controls Button { width: 1fr; min-width: 8; height: 1; padding: 0; border: none; }
    #investigation-hints { height: 2; }
    #investigation-floor { display: none; height: 1fr; content-align: center middle; }
    InvestigationsScreen.too-small #investigation-frame { display: none; }
    InvestigationsScreen.too-small #investigation-floor { display: block; }
    '''
    BINDINGS=[Binding('escape','back','Back'),Binding('ctrl+c','back','Cancel',priority=True),
        Binding('c','read_only','',show=False),Binding('x','read_only','',show=False)]

    def __init__(self,owner,*,kind='path',query='',service='',failed=False):
        super().__init__();self.owner=owner
        self.initial=(kind,query,service,failed)
        self.receiver=getattr(owner.feed.snapshot,'receiver_id','')
        self.rows=[];self.sources=[];self.references=[];self.selected_reference=None
        self.generation=0;self.busy=False;self.closed=False;self.invalidated=False
        self.request=None;self.cursor=None;self.cursors=set();self.pages=0;self.payload_bytes=0
        self.source_mode=False;self.filters_visible=False;self.frozen_policy=None

    def compose(self):
        yield Static('Investigations need 60 columns × 18 rows.\nResize to continue · Esc Back',id='investigation-floor',markup=False)
        with Vertical(id='investigation-frame'):
            yield Static('Investigate retained activity',id='investigation-heading',markup=False)
            with Horizontal(id='investigation-form'):
                yield Select([('Path','path'),('Command','command'),('Instruction','instruction')],value=self.initial[0],allow_blank=False,id='investigation-kind')
                yield Input(self.initial[1],placeholder='Exact recorded path',id='investigation-query')
                yield Button('Search',id='investigation-search')
            with Horizontal(id='investigation-filters'):
                yield Input(self.initial[2],placeholder='Exact producer (optional)',id='investigation-service')
                yield Checkbox('Failures only',self.initial[3],id='investigation-failed')
            yield Static('Search retained source observations. No match does not prove no activity.',id='investigation-status',markup=False)
            yield InvestigationTable(id='investigation-results',cursor_type='row',zebra_stripes=True)
            yield RichLog(id='investigation-source',wrap=True,markup=False,auto_scroll=False,min_width=1)
            with Horizontal(id='investigation-controls'):
                yield Button('More',id='investigation-more',disabled=True)
                yield Button('Filters',id='investigation-toggle-filters')
                yield Button('Back · Esc',id='investigation-back')
            yield Static('Enter Search / open · Tab Controls · ↑↓ PgUp/PgDn Home/End\nEsc Back · Ctrl+C Cancel · Bounded retained evidence',id='investigation-hints',markup=False)

    def on_mount(self):
        self.set_mode(False);self.update_floor();self.query_one('#investigation-query',Input).focus()
        self.set_interval(.25,self.check_policy)

    def on_unmount(self):
        self.closed=True;self.generation+=1;self.rows=[];self.sources=[];self.references=[]
        self.workers.cancel_group(self,'typed-investigation')

    def update_floor(self):self.set_class(self.size.width<60 or self.size.height<18,'too-small')
    def on_resize(self):
        self.update_floor()
        if self.is_mounted:self.call_after_refresh(self.render_rows)

    def status(self,text):self.query_one('#investigation-status',Static).update(display_text(text))

    def set_mode(self,source):
        self.source_mode=source
        self.query_one('#investigation-heading',Static).update('Exact recorded evidence' if source else 'Investigate retained activity')
        hints=('↑↓ PgUp/PgDn Home/End Read · Tab Back\nEsc Results · Ctrl+C Cancel / Back' if source else
            'Enter Search / open · Tab Controls · ↑↓ PgUp/PgDn Home/End\nEsc Back · Ctrl+C Cancel · Bounded retained evidence')
        self.query_one('#investigation-hints',Static).update(hints)
        self.query_one('#investigation-form').display=not source
        self.query_one('#investigation-filters').display=not source and self.filters_visible
        self.query_one('#investigation-results').display=not source
        self.query_one('#investigation-source').display=source
        for identity in ('investigation-more','investigation-toggle-filters'):self.query_one('#'+identity).display=not source
        self.query_one('#investigation-back',Button).label='Results · Esc' if source else 'Back · Esc'

    def cancel_request(self):
        self.generation+=1;self.busy=False;self.workers.cancel_group(self,'typed-investigation')

    def clear_derived(self):
        self.rows=[];self.sources=[];self.references=[];self.selected_reference=None
        self.cursor=None;self.cursors=set();self.pages=0;self.payload_bytes=0;self.frozen_policy=None
        self.query_one('#investigation-results',DataTable).clear()
        self.query_one('#investigation-source',RichLog).clear()
        self.query_one('#investigation-more',Button).disabled=True

    def invalidate(self,message='Capture policy or receiver changed. Reopen or deliberately search current permitted evidence.'):
        self.cancel_request();self.invalidated=True;self.clear_derived();self.status(message)

    def check_policy(self):
        if self.closed:return
        state=policy_state(self.owner,self.receiver,self.references)
        if not state[0] or self.frozen_policy is not None and state!=self.frozen_policy:
            if not self.invalidated:self.invalidate()

    def live(self,generation):
        return not self.closed and self.is_mounted and self.app.screen is self and generation==self.generation

    def query_values(self):
        return (self.query_one('#investigation-kind',Select).value,self.query_one('#investigation-query',Input).value,
            self.query_one('#investigation-service',Input).value or None,self.query_one('#investigation-failed',Checkbox).value)

    def action_search(self):
        values=self.query_values()
        try:
            validate_query(values[0],values[1])
            if values[2] is not None and (len(values[2])>512 or any(ord(char)<32 or ord(char)==127 for char in values[2])):
                raise ValueError('Use an exact single-line producer name of at most 512 characters.')
        except ValueError as exc:self.status(str(exc));return
        if not policy_state(self.owner,self.receiver,[])[0]:self.invalidate();return
        self.cancel_request();self.clear_derived();self.set_mode(False);self.invalidated=False;self.request=values
        self.busy=True;self.status('Reading retained observations… Ctrl+C cancels.');self.load_page(self.generation,None)

    @on(Input.Submitted)
    def submit_query(self,event):event.stop();self.action_search()

    @on(Input.Changed)
    @on(Select.Changed)
    @on(Checkbox.Changed)
    def query_edited(self,event):
        if not self.is_mounted:return
        if isinstance(event,Select.Changed):
            placeholders={'path':'Exact recorded path','command':'Literal command phrase','instruction':'Literal user-instruction phrase'}
            self.query_one('#investigation-query',Input).placeholder=placeholders.get(event.value,'Recorded query')
        if self.rows or self.busy:
            self.cancel_request();self.clear_derived();self.status('Query changed. Press Enter or Search to read retained evidence.')

    def action_more(self):
        self.check_policy()
        if self.busy or self.invalidated or not self.cursor or self.pages>=MAX_PAGES or len(self.rows)>=MAX_ROWS:return
        self.busy=True;self.query_one('#investigation-more',Button).disabled=True
        self.status('Loading the next bounded page…');self.load_page(self.generation,self.cursor)

    @work(group='typed-investigation',exclusive=True,exit_on_error=False)
    async def load_page(self,generation,cursor):
        request=self.request
        try:
            kind,query,service,failed=request
            response=await asyncio.to_thread(collect_investigation,self.owner.feed.api,kind=kind,query=query,
                service=service,failed=failed,limit=50,pages=1,cursor=cursor)
            if not self.live(generation) or request!=self.request:return
            self.check_policy()
            if not self.live(generation):return
            if response['receiver_id']!=self.receiver:raise ValueError('Receiver changed; reopen investigations.')
            references=[]
            for row in response['matches']:
                reference=decode_reference(encode_reference(row['source_reference']))
                if reference['receiver_id']!=self.receiver or reference['event_id']!=row['event_id']:
                    raise ValueError('Result source reference changed; evidence is withheld.')
                references.append(reference)
            combined_refs=[*self.references,*references]
            state=policy_state(self.owner,self.receiver,combined_refs)
            if not state[0]:self.invalidate();return
            size=len(json.dumps(response,ensure_ascii=False).encode())
            if self.payload_bytes+size>MAX_BYTES:
                self.cursor=None;self.status('2 MiB investigation limit reached. Narrow the query or start a fresh search.');return
            ids={row['event_id'] for row in self.rows}
            if any(row['event_id'] in ids for row in response['matches']):raise ValueError('Receiver repeated an event; refresh this bounded search.')
            next_cursor=response.get('next_cursor')
            if next_cursor and next_cursor in self.cursors:raise ValueError('Receiver repeated a cursor; search stopped.')
            self.rows.extend(deepcopy(response['matches']));self.references=combined_refs;self.frozen_policy=state
            self.pages+=1;self.payload_bytes+=size;self.cursor=next_cursor
            if next_cursor:self.cursors.add(next_cursor)
            self.render_rows();self.query_one('#investigation-results').focus()
            suffix=(' More remains; 250-row / five-page limit.' if self.cursor and self.pages>=MAX_PAGES else ' More results available.' if self.cursor else '')
            self.status(f'{len(self.rows)} retained observations · {response.get("match_mode") or "typed source query"}'+suffix+'\n'+(response.get('coverage') or 'No matches does not prove no activity.'))
        except asyncio.CancelledError:raise
        except Exception as exc:
            if self.live(generation):self.clear_derived();self.status('Investigation unavailable: '+display_text(str(exc),single=True)[:1000]+' Retry Search; no empty-result conclusion was made.')
        finally:
            if self.live(generation):
                self.busy=False;self.query_one('#investigation-more',Button).disabled=not(self.cursor and self.pages<MAX_PAGES and len(self.rows)<MAX_ROWS)

    def render_rows(self):
        table=self.query_one('#investigation-results',DataTable)
        selected=table.cursor_row;table.clear(columns=True)
        width=max(32,table.size.width or self.size.width-2)
        table.add_column('Recorded input / path',width=max(5,width-27));table.add_column('Producer',width=12);table.add_column('Result',width=9)
        for row in self.rows:
            reference=row['source_reference']
            table.add_row(Text(display_text(row.get('text'),single=True)[:4096]),Text(display_text(reference.get('service') or 'Unknown',single=True)),
                Text(display_text(row.get('result') or 'unknown',single=True)),key=row['event_id'])
        if self.rows:table.move_cursor(row=min(selected,len(self.rows)-1),animate=False)

    @on(DataTable.RowSelected,'#investigation-results')
    def open_row(self,event):
        event.stop();self.check_policy()
        if self.busy or self.invalidated:return
        row=next((row for row in self.rows if row['event_id']==event.row_key.value),None)
        if row is None:return
        self.cancel_request();self.selected_reference=decode_reference(encode_reference(row['source_reference']))
        self.set_mode(True);self.query_one('#investigation-source',RichLog).clear();self.busy=True
        self.status('Loading pinned exact source… Ctrl+C cancels; Esc returns to results.')
        self.load_source(self.generation,deepcopy(self.selected_reference))

    @work(group='typed-investigation',exclusive=True,exit_on_error=False)
    async def load_source(self,generation,reference):
        try:
            response=await asyncio.to_thread(collect_source,self.owner.feed.api,reference['event_id'],encode_reference(reference))
            lines=await asyncio.to_thread(source_lines,response)
            if not self.live(generation) or not self.source_mode:return
            self.check_policy()
            if not self.live(generation):return
            references=[*self.references,*source_references(response,reference)]
            state=policy_state(self.owner,self.receiver,references)
            if not state[0]:self.invalidate();return
            self.sources=deepcopy(response['sources']);self.references=references;self.frozen_policy=state
            log=self.query_one('#investigation-source',RichLog);log.clear()
            for label,text in lines:log.write(Text(label,style='bold'));log.write(Text(text));log.write('')
            log.focus();log.scroll_home(animate=False)
            self.status('Exact recorded source · '+('unavailable / not retained' if response.get('unavailable') else str(len(self.sources))+' reference(s)')+'\n'+display_text(response.get('coverage') or 'Retained source evidence only.'))
        except asyncio.CancelledError:raise
        except Exception as exc:
            if self.live(generation):self.sources=[];self.query_one('#investigation-source',RichLog).clear();self.status('Exact source unavailable: '+display_text(str(exc),single=True)[:1000]+' Esc returns to results.')
        finally:
            if self.live(generation):self.busy=False

    def action_back(self):
        if self.busy:
            self.cancel_request()
            if self.source_mode:self.set_mode(False);self.query_one('#investigation-results').focus()
            self.status('Read cancelled. Authored query retained; Enter searches again.');return
        if self.source_mode:
            self.cancel_request();self.sources=[];self.selected_reference=None;self.query_one('#investigation-source',RichLog).clear()
            self.set_mode(False);self.query_one('#investigation-results').focus();self.status(f'{len(self.rows)} retained observations. Enter opens the pinned source.');return
        self.closed=True;self.cancel_request();self.clear_derived();self.dismiss()

    def action_read_only(self):self.check_policy()

    @on(Button.Pressed)
    def buttons(self,event):
        event.stop()
        if event.button.id=='investigation-search':self.action_search()
        elif event.button.id=='investigation-more':self.action_more()
        elif event.button.id=='investigation-back':self.action_back()
        elif event.button.id=='investigation-toggle-filters':
            self.filters_visible=not self.filters_visible;self.query_one('#investigation-filters').display=self.filters_visible
