"""Bounded, on-demand prompt history for the selected session inspector."""
from dataclasses import dataclass, field
from urllib.parse import urlencode


def prompt_scope(item):
    """Never broaden a session inspector into an all-agent prompt query."""
    if not (item.get('session_id') or item.get('agent_id')) or (
            item.get('agent_id_inferred') and not item.get('session_id')):
        return None
    return (item.get('service') or '', item.get('session_id') or '', item.get('agent_id') or '',
            bool(item.get('agent_id_inferred')))


def scoped_prompts(events, scope):
    if scope is None:
        return []
    service, session, agent, legacy = scope
    return [event for event in events
            if event.get('domain') == 'message' and event.get('action') == 'user'
            and (not service or not event.get('service') or event['service'] == service)
            and (not session or event.get('session_id') == session)
            and (not agent or event.get('agent_id') == agent or (legacy and not event.get('agent_id')))]


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

    def merge(self, events):
        for event in scoped_prompts(events, self.scope):
            self.events[prompt_key(event)] = event

    def ordered(self):
        return sorted(self.events.values(), key=lambda event:
                      (event.get('time_unix_nano') or 0, prompt_key(event)))

    def path(self, cursor=None):
        service, session, agent, legacy = self.scope
        return '/api/activity?' + urlencode(dict(limit=100, domain='message', action='user',
            service=service, session_id=session, agent_id='' if legacy else agent,
            subtree=0, cursor=cursor or ''))

    def accept(self, response):
        self.merge(response.get('activity') or [])
        self.cursor = response.get('next_cursor')
        self.loaded = True
        self.error = ''
