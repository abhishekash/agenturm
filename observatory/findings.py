"""Deterministic, evidence-linked observations over captured agent activity."""
from __future__ import annotations

import hashlib
import json
import re


_SENSITIVE_FIELD = re.compile(
    r"(?:^|[._-])(?:api[_-]?key|access[_-]?token|refresh[_-]?token|"
    r"client[_-]?secret|[A-Za-z0-9_]+[_-](?:pass|token)(?=$)|password|passwd|private[_-]?key|authorization|"
    r"credential|secret|bearer)(?:$|[._-])", re.IGNORECASE)
_TOKEN_FIELD = re.compile(r"(?:^|[.-])(?:auth[_-]?)?token(?:$|[.-])", re.IGNORECASE)
_ASSIGNED_SECRET = re.compile(
    r"(?i)\b(api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|"
    r"password|passwd|private[_-]?key|authorization|credential|secret|bearer|"
    r"[A-Za-z][A-Za-z0-9_]*(?:_SECRET_ACCESS_KEY|_API_TOKEN|_PASS|_PASSWORD|_SECRET|_TOKEN))"
    r"(\s*[:=]\s*)([\"']?)([^\s\"'`,;]{1,})")
_DATABASE_CREDENTIAL = re.compile(r"(?i)(\b[a-z][a-z0-9+.-]*://[^\s/@:]*:)[^\s/@]+(@)")
_MYSQL_PASSWORD = re.compile(r"(\bmysql(?:\s+[^\s;]+)*?\s+-p)(?!\s|$)([^\s;]+)")
_BEARER_VALUE = re.compile(r"(?i)\bBearer\s+[A-Za-z0-9._~+/-]{8,}={0,}")
_PRIVATE_KEY = re.compile(
    r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
    re.IGNORECASE)
_TOKEN_PATTERNS = (
    ("Stripe", re.compile(r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}\b")),
    ("OpenAI or compatible", re.compile(r"\bsk-[A-Za-z0-9_-]{20,}\b")),
    ("GitHub", re.compile(r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b")),
    ("AWS access key", re.compile(r"\bAKIA[0-9A-Z]{16}\b")),
    ("Google API key", re.compile(r"\bAIza[0-9A-Za-z_-]{30,}\b")),
    ("Slack", re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b")),
)


def _safe_text(value):
    """Remove recognizable credential material before it enters a finding."""
    text = str(value)
    text = _DATABASE_CREDENTIAL.sub(r"\1[REDACTED]\2", text)
    text = _MYSQL_PASSWORD.sub(r"\1[REDACTED]", text)
    text = _PRIVATE_KEY.sub("[REDACTED PRIVATE KEY]", text)
    text = _BEARER_VALUE.sub("Bearer [REDACTED]", text)
    text = _ASSIGNED_SECRET.sub(lambda match: match.group(1) + match.group(2) + "[REDACTED]", text)
    for label, pattern in _TOKEN_PATTERNS:
        text = pattern.sub(f"[REDACTED {label} token]", text)
    return text


def _credential_hits(value, field_path=""):
    """Return safe field/provider labels; never return the matched values."""
    hits = set()
    if isinstance(value, dict):
        for key, child in value.items():
            key = str(key)
            path = f"{field_path}.{key}" if field_path else key
            if (_SENSITIVE_FIELD.search(key) or _TOKEN_FIELD.search(key)) and child not in (None, "", False):
                hits.add(("sensitive field", path[:160]))
            hits.update(_credential_hits(child, path))
    elif isinstance(value, (list, tuple)):
        for child in value[:100]:
            hits.update(_credential_hits(child, field_path))
    elif isinstance(value, str):
        if _PRIVATE_KEY.search(value):
            hits.add(("private key material", field_path[:160] or "captured text"))
        if _ASSIGNED_SECRET.search(value):
            hits.add(("credential assignment", field_path[:160] or "captured text"))
        if _BEARER_VALUE.search(value):
            hits.add(("bearer token", field_path[:160] or "captured text"))
        for label, pattern in _TOKEN_PATTERNS:
            if pattern.search(value):
                hits.add((label + " token pattern", field_path[:160] or "captured text"))
    return hits


def _finding(trace_id, kind, title, severity, evidence, detail):
    from .operation_facts import safe_value
    title=safe_value(title)[0];detail=safe_value(detail)[0]
    source = ",".join(sorted(f"{e['type']}:{e['id']}" for e in evidence))
    digest = hashlib.sha256(f"{trace_id}:{kind}:{source}".encode()).hexdigest()[:16]
    return {"id": digest, "trace_id": trace_id, "kind": kind,
            "title": _safe_text(title), "severity": severity,
            "detail": _safe_text(detail), "evidence": evidence}


def _blocked_activity(attrs):
    result = str(attrs.get("agent.activity.result") or attrs.get("activity.result") or "").lower()
    return result in {"blocked", "denied", "policy_denied"}


def mcp_result_error(attrs):
    """Return safe text when a structured MCP tool result reports an error."""
    attrs = attrs if isinstance(attrs, dict) else {}
    if any(attrs.get(key) is True for key in ("mcp.tool.is_error", "mcp.is_error")):
        return "MCP tool response marked as an error."
    for key in ("gcp.vertex.agent.tool_response", "gen_ai.tool.call.result",
                "tool.result", "output.value"):
        value = attrs.get(key)
        if isinstance(value, str):
            try:
                value = json.loads(value)
            except (json.JSONDecodeError, TypeError):
                continue
        if not isinstance(value, dict) or not (
            value.get("isError") is True or value.get("is_error") is True
        ):
            continue
        content = value.get("content")
        if isinstance(content, list):
            messages = [str(item["text"]) for item in content
                        if isinstance(item, dict) and isinstance(item.get("text"), str)]
            if messages:
                return _safe_text("; ".join(messages)[:500])
        return "MCP tool response marked as an error."
    return None


def _activity_detail(attrs):
    domain = attrs.get("agent.activity.domain") or attrs.get("activity.domain") or "activity"
    action = attrs.get("agent.activity.action") or attrs.get("activity.action") or "action"
    subject = (attrs.get("agent.activity.subject") or attrs.get("agent.activity.target") or
               attrs.get("activity.subject") or attrs.get("activity.target"))
    return f"{domain}: {action}" + (f" · {subject}" if subject else "")


def analyze(spans, logs, outcome=None, *, activity=None, receiver='', outcome_owner=None):
    """Occurrence-owned findings; trace membership never establishes ownership."""
    if not spans and not logs and not outcome:return []
    from .core import _activity_for,_activity_row,_event_activity_identity,_tool_activity
    from .operation_facts import operation_facts,operation_key,operation_metadata,digest
    trace_id = spans[0]['trace_id'] if spans else logs[0]['trace_id'] if logs else outcome['trace_id']
    if activity is None:activity=_activity_for(spans,logs)
    findings=[]
    represented=set();repeats={}
    def emit(kind,title,severity,evidence,detail,owner,*,facts=None):
        item=_finding(trace_id,kind,title,severity,evidence,detail)
        item.update({key:owner.get(key) for key in ('service','agent_id','session_id','turn_id')})
        item.update(producer=owner.get('service'),receiver_id=receiver,
            ownership_version=1,task='',run_status='UNKNOWN',
            time_unix_nano=owner.get('time_unix_nano') or 0)
        key=operation_key({**owner,'trace_id':trace_id,'receiver_id':receiver},facts=facts) if facts else None
        # Incomplete inputs/signatures remain individual observations.
        item['group_key']=digest((kind,key,facts['error_fingerprint'])) if key and facts['error_fingerprint'] else item['id']
        if facts:
            item.update(operation_metadata(owner,facts=facts))
            item.update(command=facts['command'],exit_code=facts['exit_code'],
                output_kind=facts['output_kind'],output_origin=facts['output_origin'],
                result_basis=facts['result_basis'],detail_clipped=facts['output_clipped'])
        findings.append(item)
        return item
    def source_owner(row,source,source_id,attrs=None,resource=None,stamp=None):
        return _activity_row(attrs if attrs is not None else row.get('attributes') or {},
            stamp if stamp is not None else row.get('end_unix_nano') or row.get('time_unix_nano') or 0,
            source,source_id,row.get('span_id'),row.get('service','unknown'),
            resource_attributes=resource if resource is not None else row.get('resource_attributes') or {})
    def credentials(value,owner,source,source_id):
        hits=_credential_hits(value)
        if not hits:return
        labels=', '.join(sorted({label for label,_ in hits}))
        fields=sorted({field for _,field in hits})
        severity='high' if any(label=='private key material' or label.endswith('token pattern') for label,_ in hits) else 'medium'
        emit('credential_exposure','Possible credential material captured',severity,
            [{'type':source,'id':source_id}],
            f"Detected {labels} in {', '.join(fields[:8]) or 'captured content'}. Values are omitted; inspect the linked source.",owner)
    for event in activity:
        facts=operation_facts(event)
        raw=str(event.get('result') or '').casefold()
        failed=raw in {'error','failed','blocked','denied','policy_denied','incomplete'} or facts['result']=='failed' or facts['result_conflict']
        if not failed and not facts['expected_nonzero']:continue
        source=event.get('source') or 'activity';source_id=event.get('source_id')
        if not source_id:continue
        represented.add((source,source_id))
        blocked=raw in {'blocked','denied','policy_denied'}
        kind='blocked_activity' if blocked else 'mcp_tool_error' if str(event.get('domain')).casefold() in {'mcp','mcptool'} else 'error_log' if source=='log' else 'span_error'
        title=('Non-zero search/comparison result' if facts['expected_nonzero'] else
            'Blocked agent activity' if blocked else 'Recorded operation has conflicting results' if facts['result_conflict'] else
            'MCP tool returned an error' if kind=='mcp_tool_error' else 'Recorded operation failed')
        detail='\n'.join(value for value in (facts['command'] or facts['display_subject'],
            'Exit code: '+str(facts['exit_code']) if facts['exit_code'] is not None else '',
            facts['output'],facts['result_basis']) if value)[:2000]
        item=emit(kind,title,'info' if facts['expected_nonzero'] else 'medium' if blocked or facts['result_conflict'] else 'high',
            [{'type':source,'id':source_id,'span_id':event.get('span_id')}],detail,event,facts=facts)
        if facts['result']=='failed' and item['group_key']!=item['id']:
            repeats.setdefault(item['group_key'],[]).append((item,event,facts))
    for repeated in repeats.values():
        if len(repeated)<2:continue
        first,event,facts=repeated[0]
        emit('repeated_tool_error','Repeated matching operation failures','medium',
            [entry for item,_,_ in repeated for entry in item['evidence']],
            str(len(repeated))+' recorded matching failures. '+first['detail'],event,facts=facts)
    for span in spans:
        attrs=span.get('attributes') or {};owner=source_owner(span,'span',span['span_id'])
        credentials(attrs,owner,'span',span['span_id'])
        for index,event in enumerate(span.get('events') or []):
            if not isinstance(event,dict) or not isinstance(event.get('attributes'),dict):continue
            event_attrs,resource=_event_activity_identity(span,event['attributes'])
            source_id=span['span_id']+':event:'+str(index)
            event_owner=source_owner(span,'span',source_id,event_attrs,resource,event.get('time_unix_nano',0))
            credentials(event['attributes'],event_owner,'span',source_id)
        # Non-tool SDK failures must survive even without an Activity mapping.
        if span.get('status')=='ERROR' and _tool_activity(span) is None and not any(event.get('source')=='span' and event.get('source_id')==span['span_id'] for event in activity):
            detail=attrs.get('exception.message') or attrs.get('error.type') or 'Span status is ERROR; operation details were not supplied.'
            emit('span_error','Error span recorded','high',[{'type':'span','id':span['span_id']}],str(detail)[:1500],owner)
        stopped=attrs.get('agent.stopped_reason') if span.get('name')=='agent.run' else None
        if stopped and stopped!='completed':
            emit('incomplete_run','Run stopped before completion','medium',[{'type':'span','id':span['span_id']}],
                'Recorded stop reason: '+str(stopped)[:500],owner)
    for log in logs:
        source_id=log.get('id') or log.get('digest');owner=source_owner(log,'log',source_id)
        credentials([log.get('attributes') or {},log.get('body')],owner,'log',source_id)
        if log.get('severity_number',0)>=17 and ('log',source_id) not in represented:
            # A qualified informational operation must not return as a raw error.
            projected=next((event for event in activity if event.get('source')=='log' and event.get('source_id')==source_id),None)
            if projected and operation_facts(projected)['expected_nonzero']:continue
            emit('error_log','Error log recorded','medium',[{'type':'log','id':source_id}],str(log.get('body') or '')[:1500],owner)
    if outcome:
        owner={**(outcome_owner or {}),'service':(outcome_owner or {}).get('producer'),
            'time_unix_nano':outcome.get('time_unix_nano') or 0}
        if not outcome.get('passed'):
            failed=[str(score.get('scorer') or 'assertion') for score in outcome.get('scores',[]) if score.get('passed') is False]
            detail='Failed checks: '+', '.join(failed) if failed else str(outcome.get('error') or 'Evaluation did not pass')
            emit('failed_outcome','Task outcome failed','high',[{'type':'outcome','id':trace_id}],detail[:500],owner)
        credentials(outcome,owner,'outcome',trace_id)
    return findings
