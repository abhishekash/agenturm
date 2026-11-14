"""Bounded exact-source reads for findings, within the caller's source snapshot."""
import json
import re

from .operation_facts import safe_value
from .storage_derived import _bounded_columns


def read_sources(db, finding):
    from .core import _activity_row,_event_activity_identity
    result=[];references=finding.get('evidence') or [];remaining=60000
    if not isinstance(references,list):references=[]
    for reference in references[:8]:
        if not isinstance(reference,dict):continue
        kind=reference.get('type');identity=reference.get('id');record=None;complete=False;scope={};reason='Source is no longer retained or available'
        valid=(isinstance(identity,str) and (
            kind=='span' and re.fullmatch(r'[0-9a-f]{16}(?::event:[0-9]{1,5})?',identity) or
            kind=='log' and re.fullmatch(r'[0-9a-f]{64}',identity) or
            kind=='outcome' and re.fullmatch(r'[0-9a-f]{32}',identity)))
        if not valid:
            entry=dict(type='unknown',id='omitted',record=None,complete=False,reason='Invalid source reference')
            result.append(entry);remaining-=len(json.dumps(entry).encode());continue
        if kind=='span':
            parts=identity.split(':event:')
            if not re.fullmatch(r'[0-9a-f]{16}',parts[0]):continue
            row=db.execute('SELECT '+_bounded_columns('spans')+' FROM spans WHERE trace_id=? AND span_id=?',
                (finding['trace_id'],parts[0])).fetchone()
            if row:
                complete=bool(row['payload_complete'])
                if not complete:reason='Source payload exceeds the 64 KiB record limit'
                resource=json.loads(row['resource_attributes'])
                attrs=json.loads(row['attributes'])
                if len(parts)==2 and re.fullmatch(r'[0-9]{1,5}',parts[1]):
                    index=int(parts[1]);events=json.loads(row['events'])
                    record=events[index] if index<len(events) else None
                    if isinstance(record,dict):
                        attrs,resource=_event_activity_identity(dict(attributes=attrs,resource_attributes=resource,service=row['service']),record.get('attributes') or {})
                elif len(parts)==1:
                    # Parent span selection never reveals another worker's events.
                    record=dict(name=row['name'],status=row['status'],attributes=json.loads(row['attributes']),
                        resource_attributes=resource)
                scope=_activity_row(attrs,0,'span',identity,parts[0],row['service'],resource_attributes=resource)
        elif kind=='log':
            row=db.execute('SELECT '+_bounded_columns('logs')+' FROM logs WHERE digest=? AND trace_id=?',
                (identity,finding['trace_id'])).fetchone()
            if row:
                complete=bool(row['payload_complete'])
                if not complete:reason='Source payload exceeds the 64 KiB record limit'
                scope=_activity_row(json.loads(row['attributes']),0,'log',identity,service=row['service'],resource_attributes=json.loads(row['resource_attributes']))
                record=dict(event_name=row['event_name'],severity=row['severity_text'],body=json.loads(row['body']),
                    attributes=json.loads(row['attributes']),resource_attributes=json.loads(row['resource_attributes']))
        elif kind=='outcome' and identity==finding['trace_id']:
            row=db.execute('''SELECT passed,CASE WHEN length(CAST(scores AS BLOB))+length(CAST(error AS BLOB))<=65536
                THEN scores ELSE '[]' END AS scores,
                CASE WHEN length(CAST(scores AS BLOB))+length(CAST(error AS BLOB))<=65536 THEN error ELSE '' END AS error,
                length(CAST(scores AS BLOB))+length(CAST(error AS BLOB))<=65536 AS complete
                FROM outcomes WHERE trace_id=?''',(finding['trace_id'],)).fetchone()
            if row:
                complete=bool(row['complete']);scope=finding;record=dict(passed=bool(row['passed']),scores=json.loads(row['scores']),error=row['error'])
        owner={key:scope.get(key) for key in ('service','agent_id','session_id')}
        if record is not None:
            bounded,unclipped=safe_value(record)
            encoded=json.dumps(bounded,ensure_ascii=False)
            encoded_clipped=len(encoded.encode())>8000
            if encoded_clipped:
                bounded=dict(excerpt=encoded.encode()[:2000].decode('utf-8','replace'),format='Truncated JSON excerpt')
                unclipped=False
            entry=dict(type=kind,id=identity,record=bounded,complete=complete and unclipped,scope=owner)
            if not entry['complete']:entry['reason']=reason if not complete else 'Encoded projection exceeds the 8,000-byte limit' if encoded_clipped else 'Sanitized projection was clipped or redacted'
            size=len(json.dumps(entry,ensure_ascii=False).encode())
            if size>remaining:
                entry=dict(type=kind,id=identity,record=None,complete=False,scope=owner,reason='Combined response budget reached')
            remaining-=len(json.dumps(entry,ensure_ascii=False).encode())
            result.append(entry)
        else:
            entry=dict(type=kind,id=identity,record=None,complete=False,scope=owner,reason=reason)
            remaining-=len(json.dumps(entry).encode());result.append(entry)
    return dict(sources=result,references_total=len(references),references_clipped=len(references)>8,
        coverage='At most 8 exact source references; 64 KiB source record and 8,000-byte sanitized projection limits apply.')
