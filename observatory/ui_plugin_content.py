"""Pure validation for one explicitly requested process Activity text preview.

The text is deliberately not an Activity field or a ReviewHistory event. UI
callers keep it only in transient inspector state; copy and export continue to
use the content-free projected row.
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Callable
from urllib.parse import urlencode

from .plugin_activity import _EVENT_KEY
from .workflow_evidence import safe_text

_PROVIDER_ID = re.compile(r"[a-z][a-z0-9-]{0,31}\Z")
_LOCAL_ROUTE = re.compile(r"/api/plugin-[a-z][a-z0-9-]{0,63}\Z")
MAX_PROVIDERS = 8


@dataclass(frozen=True)
class ContentPreviewProvider:
    """Trusted UI declaration; it never receives raw source text or a Store."""

    id: str
    matches: Callable[[dict], bool]
    route: str
    label: str
    hint: str
    clipped_note: str
    empty_text: str = '(empty message)'

    def path(self, record: dict) -> str:
        event_id = record.get('id') if isinstance(record, dict) else None
        if (not isinstance(record, dict) or record.get('source') != 'plugin'
                or not isinstance(event_id, str) or not _EVENT_KEY.fullmatch(event_id)
                or self.matches(record) is not True):
            raise ValueError('content preview requires one eligible Activity event')
        return self.route + '?' + urlencode({'event_id': event_id})

    def section(self, state: dict | None) -> tuple[str, str]:
        if state and state['status'] == 'available':
            text = state['text'] or self.empty_text
            return (self.label + ' · redacted', text +
                    ('\n' + self.clipped_note if state['clipped'] else '') +
                    '\nLoaded snapshot · expires in 20s · z Reload')
        if state and state['status'] == 'loading':
            return self.label, 'Loading exact source proof…'
        if state and state['status'] == 'unavailable':
            return self.label, 'Unavailable now. Capture, source, or evidence may have changed. z Retry.'
        return self.label, self.hint


class ContentPreviewRegistry:
    """A bounded, trusted registry; ambiguous matching fails closed."""

    def __init__(self, providers: tuple[ContentPreviewProvider, ...] = ()):
        self._providers: dict[str, ContentPreviewProvider] = {}
        for provider in providers:
            self.register(provider)

    def register(self, provider: ContentPreviewProvider) -> None:
        if not isinstance(provider, ContentPreviewProvider):
            raise TypeError('content preview provider declaration required')
        if (not isinstance(provider.id, str) or not _PROVIDER_ID.fullmatch(provider.id)
                or provider.id in self._providers
                or len(self._providers) >= MAX_PROVIDERS or not callable(provider.matches)
                or not isinstance(provider.route, str)
                or not _LOCAL_ROUTE.fullmatch(provider.route)
                or not all(isinstance(value, str) and 0 < len(value) <= 160
                           and safe_text(value) == value
                           for value in (provider.label, provider.hint,
                                         provider.clipped_note, provider.empty_text))):
            raise ValueError('invalid content preview provider')
        self._providers[provider.id] = provider

    def get(self, provider_id: str) -> ContentPreviewProvider | None:
        return self._providers.get(provider_id)

    def for_record(self, record: dict | None) -> ContentPreviewProvider | None:
        if (not isinstance(record, dict) or record.get('source') != 'plugin'
                or not isinstance(record.get('id'), str)
                or not _EVENT_KEY.fullmatch(record['id'])):
            return None
        matches = []
        try:
            for provider in self._providers.values():
                selected = provider.matches(record)
                if type(selected) is not bool:
                    return None
                if selected:
                    matches.append(provider)
        except Exception:
            return None
        return matches[0] if len(matches) == 1 else None


def eligible(record) -> bool:
    return (isinstance(record, dict) and record.get('source') == 'plugin'
            and record.get('domain') == 'message' and record.get('action') == 'user'
            and isinstance(record.get('id'), str)
            and bool(_EVENT_KEY.fullmatch(record['id'])))


def accepted_preview(response, record, receiver_id, before, after, *, matches=eligible):
    """Require exact selected row and unchanged receiver revision around HTTP I/O."""
    if (not isinstance(record, dict) or record.get('source') != 'plugin'
            or not isinstance(record.get('id'), str)
            or not _EVENT_KEY.fullmatch(record['id'])):
        return None
    try:
        selected = matches(record)
    except Exception:
        return None
    if selected is not True or before is None or before != after or not receiver_id:
        return None
    if not isinstance(response, dict) or response.get('receiver_id') != receiver_id \
            or response.get('event_id') != record['id']:
        return None
    revision = response.get('revision')
    text = response.get('redacted_text')
    clipped = response.get('clipped')
    if (not isinstance(revision, list) or len(revision) != 3
            or any(type(value) is not int or value < 0 for value in revision)
            or tuple(revision) != before or type(text) is not str
            or len(text.encode('utf-8')) > 4096 or safe_text(text) != text
            or type(clipped) is not bool):
        return None
    return text, clipped


PI_USER_TEXT = ContentPreviewProvider(
    'pi-user-text', eligible, '/api/plugin-content', 'Captured user text',
    'z Check for captured redacted text for this exact event.',
    '… Excerpt clipped; source text remains in the original Pi file.')
