"""Frozen Store._mcp_page from da90e73; only SQL indentation is dedented."""
from observatory import core as _core

SOURCE = r'''
def _mcp_page(self, db, limit, service, session_id, query, cursor,
              include_reviews, agent_id, subtree, scope_revision):
    limit = max(1, min(200, int(limit)))
    # Scoped provider IDs may each occupy 512 characters. SQL JSON escapes
    # control characters, with the server/call tuple nested once more in
    # the outer ownership key; the decoded key can exceed 10 KiB.
    before = self._event_cursor(cursor,max_key=16384)
    scope = None
    mcp_method = "json_extract(attributes,'$.\"mcp.method.name\"')"
    # Keep all MCP protocol methods, not only tool execution. Discovery,
    # prompts, and resource reads are useful agent activity too.
    from .storage_activity import MCP_PREDICATE
    from .storage_mcp import call_key_sql, session_sql, execution_match_sql, text_sql, text_value
    match = MCP_PREDICATE
    clauses, params = [match], []
    if service:
        clauses.append("service=?")
        params.append(service)
    if session_id:
        clauses.append(session_sql()+'=?')
        params.append(session_id)
    if agent_id:
        scope = self.resolve_agent_scope(agent_id, include_subagents=bool(subtree),_db=db)
        if scope_revision and scope_revision != scope["revision"]:
            raise ValueError("hierarchy scope changed")
        if not scope["agent_ids"]:
            return {'calls': [], 'next_cursor': None, 'summary': {}, 'agent_scope': scope}
        marks = ','.join('?' for _ in scope["agent_ids"])
        clauses.append(f"agent_id IN ({marks})")
        params.extend(scope['agent_ids'])
    if not include_reviews:
        clauses.append("service!='codex-review'")
    where = ' AND '.join(clauses)
    span_key,log_key=call_key_sql('span'),call_key_sql('log')
    def mcp_identity(alias):
        def value(key):
            return text_sql(alias,key)
        if alias == 'spans':
            tool_from_name = (
                f"CASE WHEN {value('mcp.method.name')}='tools/call' "
                f"AND lower({alias}.name) LIKE 'mcp send tools/call %' "
                f"THEN trim(substr({alias}.name, "
                f"instr(lower({alias}.name),'tools/call')+length('tools/call'))) END"
            )
        else:
            tool_from_name = "NULL"
        return (f"COALESCE({value('mcp.server.name')},{value('server.address')},'Not reported')",
                f"COALESCE({value('mcp.tool.name')},{value('gen_ai.tool.name')},"
                f"{value('tool.name')},{value('agent.activity.action')},"
                f"{tool_from_name},'Not reported')")
    server,tool=mcp_identity('logs')
    # A timestamp/tool match cannot establish that two source calls are
    # duplicates. Explicit scoped call keys already merge linked I/O.
    unique_log='1'
    query_filter = ' HAVING MAX(instr(lower(evidence),lower(?)))>0' if query else ''
    cursor_filter = ' WHERE (stamp,call_key) < (?,?)' if before else ''
    values = [*params, *params, *([query] if query else []), *(before or ()), limit+1]
    span_server, span_tool = mcp_identity('spans')
    mcp_result_error_sql = " OR ".join(
        f"(json_type(execution.attributes,'$.\"{flag}\"')='true')"
        for flag in ("mcp.tool.is_error", "mcp.is_error")
    )
    for result_key in ("gcp.vertex.agent.tool_response", "gen_ai.tool.call.result",
                       "tool.result", "output.value"):
        result_value = f"json_extract(execution.attributes,'$.\"{result_key}\"')"
        # MCP tool results are often stored as JSON strings. Guard nested
        # extraction so arbitrary plain-text tool output cannot make the
        # inventory query fail with malformed JSON.
        mcp_result_error_sql += (
            f" OR CASE WHEN json_valid({result_value}) THEN "
            f"(json_type({result_value},'$.isError')='true' "
            f"OR json_type({result_value},'$.is_error')='true') ELSE 0 END"
        )
    mcp_tool_failed = f"EXISTS (SELECT 1 FROM spans execution WHERE {execution_match_sql()} AND ({mcp_result_error_sql}))"
    body_value="json_extract(body,'$')"
    direction_sql="json_extract(attributes,'$.\"observatory.mcp.io.direction\"')"
    no_output_attributes=' AND '.join(
        f"json_type(attributes,'$.\"{key}\"') IS NULL"
        for key in ('output.value','gen_ai.tool.call.result','gcp.vertex.agent.tool_response','tool.result'))
    chosen_body_result=f"""CASE WHEN {direction_sql}='result' THEN {body_value}
        WHEN COALESCE({direction_sql},'')='' AND json_valid({body_value})
        THEN json_extract({body_value},'$.result') END"""
    body_errors=' OR '.join(f"json_type(({chosen_body_result}),'$.{key}')='true'" for key in ('isError','is_error'))
    log_body_error=f"""CASE WHEN ({no_output_attributes}) AND json_valid(({chosen_body_result}))
        THEN ({body_errors}) ELSE 0 END"""
    summary_sql = f"""WITH evidence AS (
        SELECT {span_key} AS call_key, {span_server} AS server, {span_tool} AS tool,
               (status IN ('ERROR','FAILED') OR COALESCE(({mcp_result_error_sql.replace('execution.','spans.')}),0) OR {mcp_tool_failed}) AS failed
        FROM spans WHERE {where}
        UNION ALL
        SELECT {log_key}, {server}, {tool},
               COALESCE(json_extract(attributes,'$."agent.activity.result"') IN ('error','failed','denied','blocked')
                 OR ({mcp_result_error_sql.replace('execution.','logs.')}) OR ({log_body_error}),0)
        FROM logs WHERE {where} AND {unique_log}
    ), calls AS (
        SELECT call_key, COALESCE(MAX(NULLIF(server,'Not reported')),'Not reported') AS server,
               COALESCE(MAX(NULLIF(tool,'Not reported')),'Not reported') AS tool, MAX(failed) AS failed
        FROM evidence GROUP BY call_key
    ) SELECT server, COUNT(*) AS calls, SUM(failed) AS errors,
             COUNT(DISTINCT NULLIF(tool,'Not reported')) AS tools
      FROM calls GROUP BY server ORDER BY server"""
    inventory = [dict(row) for row in db.execute(summary_sql, [*params, *params])]
    metric_servers = []
    if not session_id and not agent_id:
        metric_filter = "" if not service else " AND service=?"
        metric_params = [] if not service else [service]
        metric_rows = db.execute(f"""SELECT digest,time_ns,temporality,attributes,
                resource_attributes,scope_name,data,
                COALESCE(NULLIF(json_extract(attributes,'$.\"cursor.mcp.server.name\"'),''),'Not reported') AS server
            FROM metric_points
            WHERE name='cursor.tool.calls' AND service='cursor'
              AND kind='sum' AND temporality IN (1,2)
              AND json_extract(data,'$.isMonotonic')=1
              AND json_extract(attributes,'$.\"cursor.tool.kind\"')='mcp'{metric_filter}
            ORDER BY time_ns,digest""", metric_params).fetchall()
        # DELTA (1) values cover disjoint collection intervals, so add
        # every point. CUMULATIVE (2) values are running totals; retain
        # only the latest point in each attribute/resource time series.
        cumulative = {}
        totals = {}

        def add_metric_point(row):
            attributes = json.loads(row['attributes'])
            data = json.loads(row['data'])
            try:
                if int(data.get('flags', 0)) & 1:
                    return
                calls = max(0, int(float(data.get('value', 0))))
            except (TypeError, ValueError, OverflowError):
                return
            server = row['server']
            bucket = totals.setdefault(server, {'server':server,'calls':0,'errors':0,'tools':set()})
            bucket['calls'] += calls
            if str(attributes.get('cursor.tool.status','')).casefold() in {'failure','failed','error'}:
                bucket['errors'] += calls
            tool = attributes.get('cursor.tool.name')
            if tool:
                bucket['tools'].add(str(tool))

        for row in metric_rows:
            if row['temporality'] == 1:
                add_metric_point(row)
                continue
            identity = (row['server'],
                json.dumps(json.loads(row['attributes']),sort_keys=True,separators=(',',':')),
                json.dumps(json.loads(row['resource_attributes']),sort_keys=True,separators=(',',':')),
                row['scope_name'])
            previous = cumulative.get(identity)
            if previous is None or (row['time_ns'],row['digest']) > (previous['time_ns'],previous['digest']):
                cumulative[identity] = row
        for row in cumulative.values():
            add_metric_point(row)
        metric_servers = [{**row,'tools':len(row['tools'])}
                          for row in sorted(totals.values(),key=lambda item:item['server'].casefold())]
    summary = {'calls': sum(row['calls'] for row in inventory),
               'errors': sum(row['errors'] for row in inventory),
               'tools': sum(row['tools'] for row in inventory),
               'servers': [row for row in inventory if row['server'] != 'Not reported'],
               'unknown_server_calls': sum(row['calls'] for row in inventory if row['server'] == 'Not reported'),
               'unknown_server_errors': sum(row['errors'] for row in inventory if row['server'] == 'Not reported'),
               'unknown_server_tools': sum(row['tools'] for row in inventory if row['server'] == 'Not reported'),
               'metric_calls': sum(row['calls'] for row in metric_servers),
               'metric_servers': metric_servers,
               'metric_scope': 'all sessions; Cursor metrics have no session IDs'}
    candidates = db.execute(f"""WITH evidence AS (
        SELECT {span_key} AS call_key,end_ns AS stamp,attributes || name || service AS evidence FROM spans WHERE {where}
        UNION ALL
        SELECT {log_key} AS call_key,time_ns AS stamp,attributes || body || service AS evidence FROM logs WHERE {where} AND {unique_log}
    ), calls AS (
        SELECT call_key,MAX(stamp) AS stamp FROM evidence GROUP BY call_key {query_filter}
    ) SELECT * FROM calls {cursor_filter} ORDER BY stamp DESC,call_key DESC LIMIT ?""", values).fetchall()
    selected_keys = [row['call_key'] for row in candidates[:limit]]
    if not selected_keys:
        result = {'calls': [], 'next_cursor': None, 'summary': summary}
        if scope is not None:
            result.update(scope='agent subtree' if subtree else 'this agent', agent_scope=scope)
        return result
    placeholders = ','.join('?' for _ in selected_keys)
    spans = db.execute(f"SELECT *,{span_key} AS mcp_key,{session_sql()} AS mcp_session FROM spans WHERE {span_key} IN ({placeholders}) AND {where}",[*selected_keys,*params]).fetchall()
    logs = db.execute(f"SELECT *,{log_key} AS mcp_key,{session_sql()} AS mcp_session FROM logs WHERE {log_key} IN ({placeholders}) AND {where} ORDER BY time_ns DESC,digest DESC",[*selected_keys,*params]).fetchall()
    def decode_tool_payload(value):
        if not isinstance(value, str):
            return value
        try:
            return json.loads(value)
        except (json.JSONDecodeError, TypeError):
            return value

    def recorded_payload(attrs,keys):
        return next((decode_tool_payload(attrs[key]) for key in keys if key in attrs), '')

    argument_keys=('input.value','gen_ai.tool.call.arguments','gcp.vertex.agent.tool_call_args')
    output_keys=('output.value','gen_ai.tool.call.result','gcp.vertex.agent.tool_response','tool.result')

    calls = {}
    for span in spans:
        attrs = json.loads(span['attributes'])
        key = span['mcp_key']
        tool = text_value(attrs,'mcp.tool.name','gen_ai.tool.name','tool.name')
        method = text_value(attrs,'mcp.method.name','rpc.method',default='tools/call')
        if not tool and method == 'tools/call':
            name_prefix = 'mcp send tools/call '
            if span['name'].lower().startswith(name_prefix):
                tool = span['name'][len(name_prefix):].strip()
        elif not tool:
            tool = text_value(attrs,'agent.activity.action',default='Not reported')
        calls[key] = {'id': key, 'trace_id': span['trace_id'], 'span_id': span['span_id'],
            'session_id': span['mcp_session'],'agent_id':span['agent_id'],
            'service': span['service'],
            'server': text_value(attrs,'mcp.server.name','server.address',default='Not reported'),
            'tool': tool or text_value(attrs,'agent.activity.action',default=span['name']),
            'method': method, 'status': 'ERROR' if mcp_result_error(attrs) else span['status'],
            'time_unix_nano': span['end_ns'],
            'duration_ms': None if attrs.get('observatory.duration_unknown') else max(0, (span['end_ns']-span['start_ns'])/1e6),
            'arguments': recorded_payload(attrs,argument_keys),
            'arguments_reported': any(name in attrs for name in argument_keys),
            'output': recorded_payload(attrs,output_keys),
            'output_reported': any(name in attrs for name in output_keys),
            'source': 'span', 'log_id': None}
    for log in reversed(logs):
        attrs = json.loads(log['attributes'])
        call_id = attrs.get('observatory.mcp.call_id')
        key = log['mcp_key']
        body = json.loads(log['body'])
        try:
            details = json.loads(body) if isinstance(body, str) else body
        except ValueError:
            details = {}
        if not isinstance(details, dict):
            details = {}
        old = calls.get(key, {})
        result = str(attrs.get('agent.activity.result') or '').lower()
        state = 'ERROR' if result in {'error','failed','denied','blocked'} else 'RUNNING' if result=='started' else 'OK' if result in {'success','completed'} else old.get('status') or 'OK'
        if old.get('time_unix_nano', 0) > log['time_ns']:
            state = old.get('status') or state
        own_arguments=any(name in attrs for name in argument_keys)
        own_output=any(name in attrs for name in output_keys)
        direction=attrs.get('observatory.mcp.io.direction')
        envelope=direction is None or direction==''
        calls[key] = {**old, 'id': key, 'trace_id': log['trace_id'], 'span_id': log['span_id'],
            'session_id': log['mcp_session'],'agent_id':log['agent_id'],
            'service': log['service'],
            'server': text_value(attrs,'mcp.server.name','server.address',default=old.get('server') or 'Not reported'),
            'tool': text_value(attrs,'mcp.tool.name','gen_ai.tool.name','tool.name',default=old.get('tool') or text_value(attrs,'agent.activity.action',default='Not reported')),
            'method': text_value(attrs,'mcp.method.name','rpc.method',default=old.get('method') or 'tools/call'),
            'status': state,
            'time_unix_nano': max(log['time_ns'],old.get('time_unix_nano',0)), 'duration_ms': attrs.get('duration_ms', old.get('duration_ms')),
            'arguments': (recorded_payload(attrs,argument_keys) if own_arguments else
                          decode_tool_payload(body) if direction=='arguments' else
                          details['arguments'] if envelope and 'arguments' in details else old.get('arguments','')),
            'arguments_reported': own_arguments or direction=='arguments' or (envelope and 'arguments' in details) or old.get('arguments_reported',False),
            'output': (recorded_payload(attrs,output_keys) if own_output else
                       decode_tool_payload(body) if direction=='result' else
                       details['result'] if envelope and 'result' in details else old.get('output','')),
            'output_reported': own_output or direction=='result' or (envelope and 'result' in details) or old.get('output_reported',False),
            'metadata': {**old.get('metadata', {}), **{
                name: value for name, value in attrs.items()
                if name.startswith('cursor.') and not name.startswith('cursor.user.')
                and name != 'cursor.team.id' and value is not None}},
            'source': 'session log + span' if 'span' in old.get('source', '') else 'session log', 'log_id': log['digest']}
        if mcp_result_error(attrs) or mcp_result_error({'tool.result':calls[key]['output']}):
            calls[key]['status']='ERROR'
    # Some agent frameworks emit the MCP transport and the agent's tool
    # execution as separate spans. Join those records so the MCP view can
    # show arguments and results that the transport span itself omits.
    execution_spans = []
    span_keys=[row['mcp_key'] for row in spans]
    if span_keys:
        placeholders=','.join('?' for _ in span_keys)
        execution_spans=db.execute(f"""SELECT {call_key_sql('span','spans')} AS call_key,
            execution.trace_id,execution.span_id,execution.attributes,
            CASE WHEN execution.parent_span_id=spans.span_id OR spans.parent_span_id=execution.span_id
                THEN 'explicit span linkage' ELSE 'single compatible recorded pair' END AS basis
            FROM spans JOIN spans execution ON {execution_match_sql()}
            WHERE {call_key_sql('span','spans')} IN ({placeholders})""",span_keys).fetchall()
    executions_by_call = {}
    for execution in execution_spans:
        executions_by_call.setdefault(execution['call_key'],[]).append(execution)

    for key, call in calls.items():
        candidates_for_call=executions_by_call.get(key,[])
        if len(candidates_for_call)!=1:
            call['execution_basis']='Not associated; execution evidence is missing or ambiguous'
            continue
        execution=candidates_for_call[0]
        attrs=json.loads(execution['attributes'])
        call['execution_basis']=execution['basis']
        call['execution_source']={'trace_id':execution['trace_id'],'span_id':execution['span_id']}
        arguments_reported=any(name in attrs for name in argument_keys)
        output_reported=any(name in attrs for name in output_keys)
        if not call.get('arguments_reported') and arguments_reported:
            call['arguments'] = recorded_payload(attrs,argument_keys)
            call['arguments_reported']=True
        if not call.get('output_reported') and output_reported:
            call['output'] = recorded_payload(attrs,output_keys)
            call['output_reported']=True
        if mcp_result_error(attrs):
            call['status'] = 'ERROR'
        if arguments_reported or output_reported:
            call['source'] = 'agent + MCP spans' if call.get('source') == 'span' else call.get('source')
    items = sorted(calls.values(),key=lambda item:item['time_unix_nano'],reverse=True)
    by_id = {item['id']: item for item in items}
    items = [by_id[key] for key in selected_keys if key in by_id]
    next_cursor = self._next_event_cursor(candidates[limit-1]['stamp'],candidates[limit-1]['call_key']) if len(candidates)>limit else None
    result = {'calls': items, 'next_cursor': next_cursor, 'summary': summary}
    if scope is not None:
        result.update(scope='agent subtree' if subtree else 'this agent', agent_scope=scope)
    return result
'''

_namespace = dict(vars(_core))
exec(SOURCE, _namespace)
mcp_page = _namespace['_mcp_page']
