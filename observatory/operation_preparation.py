"""Private, trace-local reuse of already sanitized operation facts."""
from __future__ import annotations
import json

MAX_EVENTS=512
MAX_BYTES=2*1024*1024


def canonical_terminal_candidate(event):
    """Only immutable canonical bodies can be inspected ahead of full facts.

    Mutable/compact envelopes retain their original evaluation order, including
    normalized_evidence's legacy mutation of a shared detail dictionary.
    """
    return (type(event)is dict and event.get('operation_reference')is not True
        and not isinstance(event.get('operation_evidence'),dict)
        and type(event.get('detail_role'))is str and event['detail_role']=='operation_v1'
        and type(event.get('detail'))is str
        and all(type(event.get(key))in(str,type(None))for key in ('result','status','reported_result')))


def canonical_terminal(event):
    if not canonical_terminal_candidate(event):return None
    from .operation_facts import VERSION,decode,_terminal_projection
    try:
        detail=decode(event['detail'])
        if not isinstance(detail,dict)or type(detail.get('version'))is not int or detail['version']!=VERSION:return None
        # detail_clipped changes input/output completeness only. Neither flag
        # participates in the terminal projection; this decoded dict is new.
        return _terminal_projection(event,detail)
    except Exception:
        # The full path remains responsible for its original output/exception.
        return None


def unstructured_terminal(event):
    """Prove the label only when no typed terminal evidence can be present.

    Printed output cannot supply an exit/error flag. Mutable, compact and
    explicit envelopes keep the full path and its existing mutation order.
    """
    if (type(event)is not dict or
            any(key in event for key in ('attributes','operation_evidence','detail_role','exit_code','operation_reference'))or
            any(type(event.get(key))not in(str,type(None))for key in ('result','status','reported_result'))):return None
    from .operation_facts import _terminal_projection
    return _terminal_projection(event,{})


def terminal_without_input(event):
    """Return terminal metadata only when an operation key is impossible.

    Inspect immutable canonical envelopes only. A label recorded as subject
    is never a qualified command/input, regardless of printed output.
    """
    terminal=unstructured_terminal(event)
    if terminal is not None:return terminal
    if not canonical_terminal_candidate(event):return None
    from .operation_facts import VERSION,decode,_terminal_projection
    detail=decode(event['detail'])
    if not isinstance(detail,dict)or type(detail.get('version'))is not int or detail['version']!=VERSION:return None
    inputs=detail.get('inputs');commands=detail.get('commands')
    if (not isinstance(inputs,(list,tuple))or inputs or not isinstance(commands,(list,tuple))or
            any(not isinstance(row,(list,tuple))or len(row)!=2 or row[0]!='subject'for row in commands)):
        return None
    return _terminal_projection(event,detail)


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
