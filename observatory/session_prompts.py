"""Bounded, on-demand prompt history for the selected session inspector."""
from dataclasses import dataclass, field
from urllib.parse import urlencode

MAX_PROMPTS = 300
MAX_PROMPT_CHARS = 16000


def event_in_scope(event, scope):
    """An explicit worker never inherits untagged or sibling evidence."""
    if scope is None:
        return False
    service, session, agent, legacy = scope
    return ((not service or event.get('service') == service)
            and (not session or event.get('session_id') == session)
            and (not agent or event.get('agent_id') == agent or (legacy and not event.get('agent_id'))))


def scoped_events(events, scope):
    return [event for event in events if event_in_scope(event, scope)]


def prompt_scope(item):
    """Never broaden a session inspector into an all-agent prompt query."""
    if not (item.get('session_id') or item.get('agent_id')) or (
            item.get('agent_id_inferred') and not item.get('session_id')):
        return None
    return (item.get('service') or '', item.get('session_id') or '', item.get('agent_id') or '',
            bool(item.get('agent_id_inferred')))


def scoped_prompts(events, scope):
    return [event for event in scoped_events(events, scope)
            if event.get('domain') == 'message' and event.get('action') == 'user']


def prompt_key(event):
    if event.get('source_id'):
        return str(event.get('trace_id') or 'log')+':'+str(event['source_id'])
    return str(event.get('id') or event.get('source_id') or
               (event.get('time_unix_nano'), event.get('subject'), event.get('detail')))


@dataclass
class PromptHistory:
    scope: tuple | None = None
    events: dict = field(default_factory=dict)
    cursor: str | None = None
    loaded: bool = False
    busy: bool = False
    error: str = ''
    selected: str | None = None
    limited: bool = False
    revision: int | None = None
    requested_revision: int | None = None
    older_loaded: bool = False
    failed_cursor: str | None = None
    selection_touched: bool = False

    def merge(self, events):
        for event in scoped_prompts(events, self.scope):
            key = prompt_key(event)
            if key not in self.events and len(self.events) >= MAX_PROMPTS:
                self.limited = True
                oldest = next((row for row in self.ordered() if prompt_key(row)!=self.selected),None)
                if oldest is None or (event.get('time_unix_nano') or 0)<=(oldest.get('time_unix_nano') or 0):
                    continue
                self.events.pop(prompt_key(oldest))
            bounded = dict(event)
            for field in ('detail', 'subject'):
                value = bounded.get(field)
                if isinstance(value, str) and len(value) > MAX_PROMPT_CHARS:
                    bounded[field] = value[:MAX_PROMPT_CHARS] + '\n[Reader limit: text clipped; open Activity for the captured record.]'
                    bounded['_reader_clipped'] = True
            self.events[key] = bounded

    def ordered(self):
        return sorted(self.events.values(), key=lambda event:
                      (event.get('time_unix_nano') or 0, prompt_key(event)))

    def path(self, cursor=None):
        service, session, agent, legacy = self.scope
        return '/api/activity?' + urlencode(dict(limit=100, domain='message', action='user',
            service=service, session_id=session, agent_id='' if legacy else agent,
            subtree=0, cursor=cursor or ''))

    def accept(self, response, *, older=False):
        if not self.loaded and not self.selection_touched:self.selected=None
        self.merge(response.get('activity') or [])
        if older or not self.older_loaded:self.cursor = response.get('next_cursor')
        if older:self.older_loaded=True
        else:self.revision=self.requested_revision
        self.loaded = True
        self.error = ''
        self.failed_cursor=None


@dataclass
class SessionEvidence:
    """One bounded recent window, replaced on refresh rather than accumulated."""
    scope: tuple | None = None
    events: list = field(default_factory=list)
    revision: int | None = None
    requested_revision: int | None = None
    loaded: bool = False
    busy: bool = False
    error: str = ''
    partial: bool = False

    def path(self):
        service, session, agent, legacy = self.scope
        return '/api/activity?' + urlencode(dict(limit=100, service=service,
            session_id=session, agent_id='' if legacy else agent, subtree=0))

    def accept(self, response):
        events = response.get('activity') or []
        self.events = scoped_events(events[:100], self.scope)
        self.partial = bool(response.get('next_cursor')) or len(events) >= 100
        self.revision = self.requested_revision
        self.loaded = True
        self.error = ''
