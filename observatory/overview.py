"""Source-backed work summaries; counts never certify a whole execution."""
from itertools import islice
from .workflow_evidence import ReviewHistory,event_key,scope_for

EVENT_LIMIT=200
CANDIDATE_LIMIT=1000


def overview_facts(item,snapshot,events,*,loaded=False,partial=False,usage=None,endpoint=''):
    selected=dict(item)
    owner=(usage or {}).get('scope_owner') or {}
    if (usage or {}).get('turn_id') and all(isinstance(owner.get(key),str) and bool(owner[key]) and owner.get(key)==item.get(field) for key,field in
            (('producer','service'),('agent_id','agent_id'),('session_id','session_id'))):
        selected['_review_usage']=usage
    scope=scope_for(selected,snapshot,endpoint)
    candidates=[] if item.get('capture_content') is False else list(islice(events,CANDIDATE_LIMIT+1))
    candidates_clipped=len(candidates)>CANDIDATE_LIMIT
    rows=[event for event in candidates[:CANDIDATE_LIMIT] if scope.matches(event)]
    def stamp(event):
        value=event.get('time_unix_nano')
        return value if type(value) is int and value>0 else 0
    unique={}
    for event in rows:
        key=event_key(event)
        if key not in unique or stamp(event)>=stamp(unique[key]):unique[key]=event
    rows=sorted(unique.values(),key=stamp,reverse=True)[:EVENT_LIMIT]
    clipped=len(unique)>EVENT_LIMIT or partial or candidates_clipped
    history=ReviewHistory(scope,events={event_key(event):event for event in rows},loaded=loaded,
        cursor='earlier' if clipped else None,receiver_confirmed=bool(rows) and
            all(event.get('receiver_id')==scope.receiver for event in rows))
    projection=history.projection({**selected,'_review_usage':usage or {}})
    edits=projection['edits'];checks=projection['checks'];errors=projection['errors']
    paths=[]
    for edit in reversed(edits):
        path=edit.get('path')
        if path and path not in paths:paths.append(path)
    results={result:sum(check['result']==result for check in checks) for result in ('passed','failed','unknown')}
    return dict(scope=scope.kind,projection=projection,paths=paths,edit_count=len(edits),
        checks=checks,check_results=results,issues=errors,
        work_records=sum(event.get('domain') in {'file','shell','tool','mcp','browser'} for event in rows),
        source_count=len(rows),partial=clipped or not loaded,
        coverage=('Current snapshot preview; full history has not been loaded. ' if not loaded else '')+
            'Loaded captured records only; whole-run capture and current worktree correctness are not established.')


def overview_summary(facts):
    lines=[]
    if facts['edit_count']:
        lines.append(str(facts['edit_count'])+' file operation'+('s' if facts['edit_count']!=1 else '')+
            ' · '+str(len(facts['paths']))+' recorded path'+('s' if len(facts['paths'])!=1 else ''))
    counts=facts['check_results']
    if facts['checks']:lines.append('Checks: '+' · '.join(str(counts[key])+' '+key for key in counts if counts[key]))
    if facts['issues']:lines.append(str(len(facts['issues']))+' recorded issue'+('s' if len(facts['issues'])!=1 else ''))
    if not lines and facts['work_records']:lines.append(str(facts['work_records'])+' recorded work events · no file/check evidence in this preview')
    return '\n'.join(lines)
