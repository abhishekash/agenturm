"""Private, trace-local reuse of already sanitized operation facts."""
from __future__ import annotations
import json

MAX_EVENTS=512
MAX_BYTES=2*1024*1024


class OperationPreparation:
    """Own no source objects; the caller keeps its bound activity list alive.

    Bind once after canonical Activity is constructed and do not mutate those
    objects. Only their Python object identities and sanitized JSON are held.
    """
    def __init__(self):
        self._allowed=frozenset();self._facts={};self._bytes=0;self._bound=False;self._closed=False

    def __enter__(self):return self
    def __exit__(self,*_):self.clear()

    def bind(self,activity):
        if self._bound or self._closed:
            self.clear();raise RuntimeError('Operation preparation is bound to one trace only')
        if not isinstance(activity,(list,tuple)):raise TypeError('Canonical Activity must remain alive as a sequence')
        self._allowed=frozenset(id(event) for event in activity[:MAX_EVENTS] if isinstance(event,dict))
        self._bound=True

    def full(self,event):
        from .operation_facts import operation_facts
        identity=id(event)
        allowed=not self._closed and identity in self._allowed
        if allowed and identity in self._facts:return json.loads(self._facts[identity])
        facts=operation_facts(event)
        if allowed and self._bytes<MAX_BYTES:
            try:encoded=json.dumps(facts,ensure_ascii=False,separators=(',',':'),allow_nan=False).encode('utf-8')
            except (TypeError,ValueError,UnicodeError):return facts
            if len(encoded)<=MAX_BYTES-self._bytes:
                self._facts[identity]=encoded;self._bytes+=len(encoded)
        return facts

    def correlation(self,event):
        # Findings deliberately uses full facts even for compact references.
        # Error correlation must retain the separate compact metadata contract.
        if event.get('operation_reference') is True:
            from .operation_facts import correlation_facts
            return correlation_facts(event)
        return self.full(event)

    def clear(self):
        self._allowed=frozenset();self._facts.clear();self._bytes=0;self._closed=True
