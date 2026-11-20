"""OpenCode's bounded, authoritative native turn review query.

The bundled OpenCode adapter contributes this legacy SQL reader; process
adapters expose turn facts through the host-owned evidence contract instead.
"""
from __future__ import annotations

import json

from .opencode_turn_proof import proof_sql, usage_sql, _scalar, turn_coverage


def review_turn(db, trace_id):
    """Return exact OpenCode user-parent membership and disjoint step usage."""
    root=db.execute(f"SELECT {proof_sql('attributes')} AS attributes,{_scalar('attributes','agent.turn.id')} AS turn_id,{_scalar('attributes','session.id')} AS session_id,service,agent_id FROM spans WHERE trace_id=? AND name='agent.run' AND scope_name='observatory.opencode' LIMIT 1",(trace_id,)).fetchone()
    if not root or root['service']!='opencode':return None
    attrs=json.loads(root['attributes']);turn=root['turn_id']
    session=root['session_id'];agent=root['agent_id']
    if not turn or not session or not agent:return None
    cursor=db.execute(f"SELECT s.trace_id,{proof_sql('s.attributes')} AS attributes,{usage_sql('c.summary_json')} AS summary_json,EXISTS(SELECT 1 FROM finding_dirty_v1 f WHERE f.trace_id=s.trace_id) AS usage_pending FROM spans s LEFT JOIN live_summaries_v1 c USING(trace_id) WHERE s.service='opencode' AND s.name='agent.run' AND s.scope_name='observatory.opencode' AND json_extract(s.attributes,'$.\"agent.turn.id\"')=? AND s.agent_id=? AND json_extract(s.attributes,'$.\"session.id\"')=? ORDER BY s.start_ns,s.trace_id LIMIT 1001",(turn,agent,session))
    members=[];retained_bytes=0;payload_limited=False
    for row in cursor:
        entry=dict(row)
        size=len(entry['attributes'].encode('utf-8'))+len(entry['summary_json'].encode('utf-8'))
        if retained_bytes+size>2*1024*1024:
            payload_limited=True;entry.update(attributes='{}',summary_json='{}')
        else:retained_bytes+=size
        members.append(entry)
    clipped=len(members)>1000
    members=members[:1000]
    rows=[json.loads(row['summary_json']) if row['summary_json'] else {} for row in members]
    owner=dict(producer='opencode',agent_id=agent,session_id=session)
    reasons=turn_coverage(attrs,[json.loads(row['attributes']) for row in members],
        [row['trace_id'] for row in members],clipped=clipped)
    if payload_limited:reasons.append('Turn proof and usage metadata exceed the retained byte limit')
    if any(row['usage_pending'] for row in members):reasons.append('Turn usage summaries are catching up with recorded source changes')
    owned=bool(rows) and all(row.get('review_owner')==owner and row.get('turn_id')==turn and row.get('usage_complete') is True for row in rows)
    if not owned:reasons.append('Turn step ownership, usage reporting or indexing is incomplete')
    if clipped and not reasons:reasons.append('Turn membership exceeds the retained member limit')
    complete=owned and not clipped and not reasons
    usage=dict(trace_id=trace_id,turn_id=turn,scope_owner=owner,turn_members_clipped=clipped,turn_usage_complete=complete,turn_coverage_reasons=reasons,turn_closed=attrs.get('observatory.opencode.turn.closed'),turn_membership='recorded idle-boundary interval' if attrs else 'recorded user parent',step_trace_ids=[row['trace_id'] for row in members],
        agent_count=1,agent_count_complete=complete,review_owner=owner if complete else {},
        usage_allocation='recorded turn interval; disjoint assistant step usage' if complete else 'turn source membership or step ownership incomplete; usage unavailable')
    if complete:
        for field in ('input_tokens','output_tokens','cache_read_input_tokens','cache_write_input_tokens','reasoning_output_tokens'):
            reported=all(row.get(field+'_reported') is True for row in rows)
            usage[field]=sum(row.get(field) or 0 for row in rows) if reported else None
            usage[field+'_reported']=reported
        costs=[row.get('known_cost_usd') for row in rows]
        usage.update(known_cost_usd=sum(value for value in costs if value is not None) if any(value is not None for value in costs) else None,
            cost_complete=all(row.get('cost_complete') for row in rows),usage_complete=all(usage.get(field+'_reported') for field in ('input_tokens','output_tokens')))
    return usage
