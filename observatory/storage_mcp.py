"""MCP source identities and conservative transport/execution association."""


def session_sql(alias=''):
    from .core import SESSION_KEYS
    prefix=alias+'.' if alias else ''
    values=[]
    for key in SESSION_KEYS:
        for column in ('attributes','resource_attributes'):
            value=f"json_extract({prefix}{column},'$.\"{key}\"')"
            kind=f"json_type({prefix}{column},'$.\"{key}\"')"
            values.append(f"CASE WHEN {kind}='text' AND length(trim({value})) BETWEEN 1 AND 512 THEN trim({value}) END")
    return 'COALESCE('+','.join(values)+')'


def text_sql(alias,key):
    prefix=alias+'.' if alias else ''
    value=f"json_extract({prefix}attributes,'$.\"{key}\"')"
    return f"CASE WHEN json_type({prefix}attributes,'$.\"{key}\"')='text' AND length({value}) BETWEEN 1 AND 512 THEN {value} END"


def text_value(attrs,*keys,default=None):
    return next((attrs[key] for key in keys if isinstance(attrs.get(key),str) and 1<=len(attrs[key])<=512),default)


def call_key_sql(kind,alias=''):
    from .core import SESSION_KEYS
    prefix=alias+'.' if alias else ''
    if kind=='span':base=f"{prefix}trace_id || ':' || {prefix}span_id";source=base
    else:
        call=text_sql(alias,'observatory.mcp.call_id')
        server=f"COALESCE({text_sql(alias,'mcp.server.name')},{text_sql(alias,'server.address')})"
        base=f"CASE WHEN {call} IS NOT NULL THEN 'cursor-mcp:' || json_array({server},{prefix}trace_id,{call}) WHEN {prefix}trace_id IS NOT NULL AND {prefix}span_id IS NOT NULL THEN {prefix}trace_id || ':' || {prefix}span_id ELSE {prefix}digest END"
        source=prefix+'digest'
    # Call IDs are producer-local. Keep explicit ownership in every merge key,
    # including unscoped reads, instead of silently merging colliding workers.
    ambiguous=f"(({prefix}agent_id IS NULL AND {identity_claim_sql(alias,('agent.id','agent.worker_id','agent.parent_id'))}) OR ({session_sql(alias)} IS NULL AND {identity_claim_sql(alias,SESSION_KEYS)}))"
    return f"'mcp-v3:' || json_array({prefix}service,{prefix}agent_id,{session_sql(alias)},CASE WHEN {ambiguous} THEN {source} END,{base})"


def tool_sql(alias):
    def value(key):return text_sql(alias,key)
    return f"COALESCE({value('mcp.tool.name')},{value('gen_ai.tool.name')},{value('tool.name')},CASE WHEN {value('mcp.method.name')}='tools/call' AND lower({alias}.name) LIKE 'mcp send tools/call %' THEN trim(substr({alias}.name,instr(lower({alias}.name),'tools/call')+length('tools/call'))) END)"


def identity_claim_sql(alias,keys):
    prefix=alias+'.' if alias else ''
    return '('+' OR '.join(f"COALESCE(json_type({prefix}{column},'$.\"{key}\"'),'null')!='null'" for key in keys for column in ('attributes','resource_attributes'))+')'


def execution_match_sql(call='spans',execution='execution',*,_unique=True):
    """Explicit linkage, or one compatible pair; time proximity is never proof.

    Missing ownership remains unallocated in a trace with known other owners.
    Legacy unowned pairs remain inspectable only in unambiguous trace context.
    """
    from .storage_activity import MCP_PREDICATE
    from .core import SESSION_KEYS
    csession,esession=session_sql(call),session_sql(execution)
    cserver=f"COALESCE({text_sql(call,'mcp.server.name')},{text_sql(call,'server.address')})"
    eserver=f"COALESCE({text_sql(execution,'mcp.server.name')},{text_sql(execution,'server.address')})"
    ctool=tool_sql(call)
    linked=f"({execution}.parent_span_id={call}.span_id OR {call}.parent_span_id={execution}.span_id)"
    unique_link=f"(SELECT COUNT(*) FROM spans other WHERE other.trace_id={call}.trace_id AND {MCP_PREDICATE.replace('attributes','other.attributes')} AND (other.parent_span_id={execution}.span_id OR {execution}.parent_span_id=other.span_id))=1"
    explicit=f'({linked} AND {unique_link})'
    owner_claim=identity_claim_sql('known',('agent.id','agent.worker_id','agent.parent_id'))
    legacy_agents=f"NOT EXISTS(SELECT 1 FROM spans known WHERE known.trace_id={call}.trace_id AND (known.agent_id IS NOT NULL OR {owner_claim}) UNION ALL SELECT 1 FROM logs known WHERE known.trace_id={call}.trace_id AND (known.agent_id IS NOT NULL OR {owner_claim}))"
    invalid_session=f"({identity_claim_sql('known',SESSION_KEYS)} AND {session_sql('known')} IS NULL)"
    valid_context=f"NOT EXISTS(SELECT 1 FROM spans known WHERE known.trace_id={call}.trace_id AND {invalid_session} UNION ALL SELECT 1 FROM logs known WHERE known.trace_id={call}.trace_id AND {invalid_session})"
    sessions=f"((SELECT COUNT(DISTINCT value) FROM (SELECT {session_sql('known')} AS value FROM spans known WHERE known.trace_id={call}.trace_id UNION ALL SELECT {session_sql('known')} AS value FROM logs known WHERE known.trace_id={call}.trace_id))<=1 AND {valid_context})"
    unique_call=f"(SELECT COUNT(*) FROM spans other WHERE other.trace_id={call}.trace_id AND other.service={call}.service AND other.agent_id IS {call}.agent_id AND {MCP_PREDICATE.replace('attributes','other.attributes')} AND {tool_sql('other')}={ctool})=1"
    unique_execution=f"(SELECT COUNT(*) FROM spans other WHERE other.trace_id={call}.trace_id AND other.service={call}.service AND other.agent_id IS {call}.agent_id AND lower(json_extract(other.attributes,'$.\"gen_ai.tool.type\"'))='mcptool' AND json_extract(other.attributes,'$.\"gen_ai.tool.name\"')={ctool})=1"
    predicate=f"""{execution}.trace_id={call}.trace_id AND {execution}.service={call}.service
        AND {execution}.agent_id IS {call}.agent_id
        AND ({cserver} IS NULL OR {eserver} IS NULL OR {cserver}={eserver})
        AND lower(json_extract({execution}.attributes,'$."gen_ai.tool.type"'))='mcptool'
        AND json_extract({execution}.attributes,'$."gen_ai.tool.name"')={ctool}
        AND ({call}.agent_id IS NOT NULL OR {legacy_agents})
        AND (({csession} IS NOT NULL AND {esession} IS NOT NULL AND {csession}={esession})
            OR (({csession} IS NULL OR {esession} IS NULL) AND {sessions}))
        AND ({explicit} OR ({unique_call} AND {unique_execution}))"""
    if _unique:
        candidate=execution_match_sql(call,'candidate_execution',_unique=False)
        predicate+=f' AND (SELECT COUNT(*) FROM spans candidate_execution WHERE {candidate})=1'
    return predicate
