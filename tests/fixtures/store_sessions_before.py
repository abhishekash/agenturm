"""Frozen Store.sessions from a601ad2, evaluated against current unchanged helpers."""
from observatory import core as _core

SOURCE = '''
def sessions(self, limit=100):
    if not 1 <= int(limit) <= 500:
        raise ValueError("limit must be between 1 and 500")
    with self._connect() as db:
        db.execute('BEGIN')
        receiver=db.execute("SELECT value FROM schema_meta WHERE key='receiver_id'").fetchone()[0]
        from .storage_sessions import session_projection
        rows,session_totals,initial_tasks=session_projection(db,int(limit))
        legacy_latest=[];seen_sessions=set()
        for row in rows:
            key=(row['session_id'],row['agent_id'])
            if key in seen_sessions:continue
            seen_sessions.add(key)
            if len(legacy_latest)<500 and 'agent_count' not in json.loads(row['summary_json']):legacy_latest.append(row['trace_id'])
        legacy_agent_counts={trace:0 for trace in legacy_latest}
        for offset in range(0,len(legacy_latest),200):
            selected=legacy_latest[offset:offset+200];marks=','.join('?' for _ in selected)
            counts=db.execute(f"SELECT trace_id,COUNT(DISTINCT agent_id) FROM ("
                f"SELECT trace_id,agent_id FROM spans WHERE trace_id IN ({marks}) UNION ALL "
                f"SELECT trace_id,agent_id FROM logs WHERE trace_id IN ({marks})) GROUP BY trace_id",
                [*selected,*selected]).fetchall()
            legacy_agent_counts.update((row[0],row[1]) for row in counts)
        session_expr = _session_sql_expression("l")
        log_sessions = db.execute(f"""SELECT {session_expr} AS session_id,MAX(l.time_ns) AS last_ns
            FROM logs l WHERE {session_expr} IS NOT NULL
            GROUP BY {session_expr} ORDER BY last_ns DESC,session_id DESC LIMIT ?""",
            (int(limit),)).fetchall()
        log_rows = []
        log_usage_rows = []
        if log_sessions:
            session_ids = [session["session_id"] for session in log_sessions]
            placeholders = ",".join("?" for _ in session_ids)
            fields=('input_tokens','output_tokens','cache_read_input_tokens','cache_write_input_tokens','reasoning_output_tokens')
            selected=[]
            for field in fields:selected.extend([f'COALESCE(SUM(l.{field}),0) AS {field}',f'MAX(l.{field}_reported) AS {field}_reported'])
            log_usage_rows=db.execute('SELECT l.session_id,'+','.join(selected)+' FROM live_log_usage_v1 l WHERE l.session_id IN ('+placeholders+') AND NOT EXISTS(SELECT 1 FROM live_summaries_v1 s WHERE s.session_id=l.session_id) GROUP BY l.session_id',session_ids).fetchall()
        for session in log_sessions:
            # Filter scalar source identities before reading any captured
            # payload. Native logs already covered by a run summary need
            # neither JSON parsing nor another preview of their bodies.
            log_rows.extend(db.execute(f"""WITH candidates AS (
                SELECT activity.owner_key AS digest,activity.stamp AS stamp
                FROM activity_index_v1 activity
                WHERE activity.kind='log' AND activity.session_id=?
                  AND NOT EXISTS (SELECT 1 FROM live_summaries_v1 covered JOIN trace_activity covered_time USING(trace_id)
                    WHERE activity.agent_id IS NOT NULL AND covered.trace_id=activity.trace_id
                      AND covered.agent_id=activity.agent_id AND covered_time.last_ns>=activity.stamp)
                UNION ALL
                SELECT l.digest,l.time_ns AS stamp FROM logs l
                WHERE {session_expr}=? AND l.event_name!='agent.activity'
                ORDER BY stamp DESC,digest DESC LIMIT 50)
                SELECT l.digest,l.span_id,l.trace_id,l.time_ns,l.service,l.event_name,l.severity_number,
                    l.attributes,l.resource_attributes,l.body,{session_expr} AS session_id
                FROM candidates JOIN logs l USING(digest)
                ORDER BY candidates.stamp DESC,candidates.digest DESC""",
                (session['session_id'],session['session_id'])).fetchall())
        # Error reminders use bounded concrete evidence, never lifetime totals.
        # Read only the newest two traces for each execution identity.
        recent_traces = set()
        trace_counts = {}
        for row in rows:
            key = (row['session_id'], row['agent_id'])
            trace_counts[key] = trace_counts.get(key, 0) + 1
            if trace_counts[key] <= 2:
                recent_traces.add(row['trace_id'])
        reminder_events = []
        selected_traces = sorted(recent_traces)
        for offset in range(0, len(selected_traces), 200):
            selected = selected_traces[offset:offset + 200]
            marks = ','.join('?' for _ in selected)
            for row in db.execute(f'SELECT error_refs_json FROM trace_summaries WHERE trace_id IN ({marks})', selected):
                reminder_events.extend(json.loads(row['error_refs_json']))
    grouped = {}
    for row in rows:
        summary = json.loads(row["summary_json"])
        agent_key = row["agent_id"] or summary.get("agent_id")
        key = (row["session_id"], agent_key)
        item = grouped.setdefault(key, {
            "session_id": row["session_id"], "agent_id": agent_key,
            "trace_count": 0, "errors": 0,
            "failed": 0, "incomplete": 0, "issues": 0,
            "input_tokens": 0, "output_tokens": 0, "span_count": 0,
            "cache_read_input_tokens": 0, "cache_write_input_tokens": 0,
            "reasoning_output_tokens": 0,
            "input_tokens_reported": False, "output_tokens_reported": False,
            "cache_read_input_tokens_reported": False,
            "cache_write_input_tokens_reported": False,
            "reasoning_output_tokens_reported": False,
            "known_cost_usd": 0.0, "unknown_cost_runs": 0, "cost_complete": True,
            "services": set(), "models": set(), "latest_trace_id": row["trace_id"],
            "last_unix_nano": row["last_ns"], "latest_status": "UNKNOWN",
            "latest_task": None, "latest_waiting_for": None, "project": None})
        item["usage_complete"] = item.get("usage_complete", True) and summary.get("usage_complete", True)
        item["status_complete"] = item.get("status_complete", True) and summary.get("status_complete", True)
        if item["trace_count"] == 0:
            usage_fields=("trace_id","task","input_tokens","output_tokens",
                          "cache_read_input_tokens","cache_write_input_tokens","reasoning_output_tokens",
                          "input_tokens_reported","output_tokens_reported","cache_read_input_tokens_reported",
                          "cache_write_input_tokens_reported","reasoning_output_tokens_reported",
                          "known_cost_usd","cost_complete","usage_complete","task_outcome","task_outcome_ns",
                          "agent_id","agent_count","agent_count_complete","review_owner","waiting_for","turn_id")
            item["latest_run"]={key:summary[key] for key in usage_fields if key in summary}
            item["latest_run"]["trace_id"]=row["trace_id"]
            if row['trace_id'] in legacy_agent_counts:
                item['latest_run'].update(agent_count=legacy_agent_counts[row['trace_id']],agent_count_complete=False)
            item["latest_status"] = summary.get("status") or "UNKNOWN"
            item["latest_task_outcome"] = summary.get("task_outcome") or "unknown"
            item["task_outcome_ns"] = summary.get("task_outcome_ns") or 0
            item["task_waiting_for"] = summary.get("waiting_for")
            item["latest_task"] = summary.get("task")
            item["latest_waiting_for"] = summary.get("waiting_for")
            item["project"] = summary.get("project")
            for key in ("agent_id", "agent_native_id", "parent_agent_native_id",
                        "agent_namespace", "agent_worker_id", "relationship_kind",
                        "relationship_state", "relationship_provenance"):
                if summary.get(key):
                    item[key] = summary[key]
        if summary.get("initial_task") and not item.get("initial_task"):
            item["initial_task"] = summary["initial_task"]
        item["trace_count"] += 1
        item["span_count"] += int(summary.get("span_count") or 0)
        status = summary.get("status")
        item["errors"] += status == "ERROR"
        item["failed"] += status == "FAILED" or summary.get("outcome_passed") is False
        item["incomplete"] += status == "INCOMPLETE"
        item["issues"] += status in {"ERROR", "FAILED", "INCOMPLETE"} or summary.get("outcome_passed") is False
        item["input_tokens"] += int(summary.get("input_tokens") or 0)
        item["output_tokens"] += int(summary.get("output_tokens") or 0)
        item["cache_read_input_tokens"] += int(summary.get("cache_read_input_tokens") or 0)
        item["cache_write_input_tokens"] += int(summary.get("cache_write_input_tokens") or 0)
        item["reasoning_output_tokens"] += int(summary.get("reasoning_output_tokens") or 0)
        for field in ("cache_read_input_tokens", "cache_write_input_tokens", "reasoning_output_tokens"):
            item[f"{field}_reported"] |= bool(summary.get(f"{field}_reported"))
        item["input_tokens_reported"] |= bool(summary.get("input_tokens_reported"))
        item["output_tokens_reported"] |= bool(summary.get("output_tokens_reported"))
        item["known_cost_usd"] += float(summary.get("known_cost_usd") or 0)
        cost_complete = bool(summary.get("cost_complete", False))
        item["unknown_cost_runs"] += not cost_complete
        item["cost_complete"] &= cost_complete
        from .storage_derived import merge_labels
        merge_labels(item, "services", summary.get("services", []), summary.get("services_complete", True))
        merge_labels(item, "models", summary.get("models", []), summary.get("models_complete", True))

    for key,totals in session_totals.items():
        if key in grouped:
            grouped[key].update(totals)
            # Recover the most recent recorded original task even if the
            # latest two runs did not repeat that optional metadata.
            if not grouped[key].get('initial_task'):
                if key in initial_tasks:grouped[key]['initial_task']=initial_tasks[key]

    # Some runtimes emit session-tagged events without trace context. Keep
    # those sessions visible in the roster instead of making Activity the
    # only place their work can be found.
    for row in log_rows:
        session_id = _session_value(row["session_id"])
        if not session_id:
            continue
        attrs = json.loads(row["attributes"])
        resource = json.loads(row["resource_attributes"])
        try:
            body = json.loads(row["body"])
        except (TypeError, ValueError):
            body = ""
        if row['event_name'] == 'agent.activity':
            event = _activity_row(attrs, row['time_ns'], 'log', row['digest'],
                                  row['span_id'],row['service'],body,
                                  resource_attributes=resource)
            reminder_events.append({**event, 'trace_id': row['trace_id'],
                'id': (row['trace_id'] or 'log') + ':' + row['digest'],'receiver_id':receiver})
        log_agent_id = _agent_id_from_attributes(attrs, resource, row["service"])
        key = (session_id, log_agent_id)
        item = grouped.get(key)
        if item is None:
            item = grouped[key] = {
                "session_id": session_id, "trace_count": 0, "errors": 0,
                "agent_id": log_agent_id,
                "agent_native_id": attrs.get("agent.id"),
                "parent_agent_native_id": attrs.get("agent.parent_id"),
                "agent_namespace": attrs.get("agent.namespace"),
                "agent_worker_id": attrs.get("agent.worker_id"),
                "relationship_kind": attrs.get("agent.relationship.kind"),
                "relationship_state": attrs.get("agent.relationship.state"),
                "relationship_provenance": attrs.get("agent.relationship.provenance"),
                "failed": 0, "incomplete": 0, "issues": 0,
                "input_tokens": 0, "output_tokens": 0, "span_count": 0,
                "cache_read_input_tokens": 0, "cache_write_input_tokens": 0,
                "reasoning_output_tokens": 0,
                "cache_read_input_tokens_reported": False,
                "cache_write_input_tokens_reported": False,
                "reasoning_output_tokens_reported": False,
                "input_tokens_reported": False, "output_tokens_reported": False,
                "known_cost_usd": 0.0, "unknown_cost_runs": 0, "cost_complete": True,
                "services": set(), "models": set(), "latest_trace_id": row["trace_id"],
                "last_unix_nano": row["time_ns"], "latest_status": "RUNNING",
                "latest_task_outcome": "unknown", "task_outcome_ns": 0,
                "latest_task": None, "latest_waiting_for": None, "project": None,
                "_latest_log_ns": row["time_ns"]}
        item.setdefault("_latest_log_ns", 0)
        trace_last_ns = item.get("last_unix_nano", 0) if item["trace_count"] else 0
        item.setdefault("_trace_last_ns", trace_last_ns)
        item.setdefault("_latest_signal_ns", trace_last_ns)
        item.setdefault("_latest_task_ns", trace_last_ns if item["trace_count"] else 0)
        item["services"].add(row["service"])
        item["last_unix_nano"] = max(item["last_unix_nano"], row["time_ns"])
        if row["time_ns"] >= item.get("_latest_log_ns", 0):
            item["_latest_log_ns"] = row["time_ns"]
            if row["trace_id"]:
                item["latest_trace_id"] = row["trace_id"]
            if row["time_ns"] >= item.get("_trace_last_ns", 0):
                item["latest_waiting_for"] = _waiting_for(attrs)
        initial_task = attrs.get("agent.session.task")
        if initial_task and not item.get("initial_task"):
            from .session_text import task_description
            item["initial_task"] = task_description(initial_task)[:500]
        task = attrs.get("agent.task") or attrs.get("task")
        source_event = attrs.get("observatory.source.event_name") or row["event_name"]
        if (not task and source_event == "cursor.conversation.user_message"
                and isinstance(body, str)):
            task = body
        if task and row["time_ns"] >= item.get("_latest_task_ns", 0):
            item["_latest_task_ns"] = row["time_ns"]
            item["latest_task"] = str(task)[:2000]
        model = (attrs.get("gen_ai.request.model") or attrs.get("model") or
                 attrs.get("model_name"))
        if model:
            item["models"].add(str(model))
        if not item["project"]:
            item["project"] = next((resource.get(key) for key in
                ("vcs.repository.name", "project.name", "repository.name")
                if resource.get(key)), None)
            if not item["project"]:
                repo_url = attrs.get("vcs.repository.url") or resource.get("vcs.repository.url")
                if isinstance(repo_url, str) and repo_url.strip():
                    item["project"] = repo_url.rstrip("/").removesuffix(".git").rsplit("/", 1)[-1]

        result = str(attrs.get("agent.activity.result") or "").casefold()
        is_error = (source_event == "gemini_cli.api_error" or result in
                    {"error", "failed", "denied", "blocked"} or row["severity_number"] >= 17)
        is_lifecycle = source_event in {"gemini_cli.agent.start", "gemini_cli.agent.finish"}
        waiting_for = _waiting_for(attrs)
        # Tool failures remain error evidence. Only lifecycle/explicit input
        # signals can change the task outcome, including log-only sessions.
        task_signal = _task_outcome_from_log(attrs, source_event)
        if task_signal and (row["time_ns"] > item.get("task_outcome_ns", 0) or
                            (row["time_ns"] == item.get("task_outcome_ns", 0) and
                             not item["trace_count"] and not item.get("_task_signal_seen"))):
            item["latest_task_outcome"] = task_signal
            item["task_outcome_ns"] = row["time_ns"]
            item["task_waiting_for"] = waiting_for if task_signal == "waiting" else ""
            item["_task_signal_seen"] = True
        if is_error and not item["trace_count"]:
            item["_log_error"] = True
        if is_lifecycle or is_error or waiting_for or row["time_ns"] > item.get("_trace_last_ns", 0):
            if row["time_ns"] >= item.get("_latest_signal_ns", 0):
                item["_latest_signal_ns"] = row["time_ns"]
                item["latest_waiting_for"] = waiting_for
                if waiting_for:
                    item["latest_status"] = "WAITING"
                elif source_event == "gemini_cli.agent.finish":
                    item["latest_status"] = "OK"
                elif source_event == "gemini_cli.agent.start":
                    item["latest_status"] = "RUNNING"
                elif source_event == "cursor.grok_bot.routine_run":
                    item["latest_status"] = ("ERROR" if result == "error" else
                        "INCOMPLETE" if result == "cancelled" else "OK")
                elif source_event == "gemini_cli.agent.start":
                    item["latest_status"] = "RUNNING"
                elif is_error:
                    item["latest_status"] = "ERROR"
                else:
                    item["latest_status"] = "RUNNING"

    # Aggregate every log-only usage event. The visible event preview above
    # is intentionally bounded to 50 rows per session and must not cap the
    # session's cumulative token totals.
    for usage in log_usage_rows:
        candidates = [item for key, item in grouped.items()
                      if key[0] == usage["session_id"] and item["trace_count"] == 0]
        if not candidates:
            continue
        if len(candidates) > 1:
            # Shared session IDs are not sufficient to split log-only usage
            # across workers. Do not invent a division.
            for item in candidates:
                item["usage_complete"] = False
            continue
        item = candidates[0]
        for field in ("input_tokens", "output_tokens", "cache_read_input_tokens",
                      "cache_write_input_tokens", "reasoning_output_tokens"):
            item[field] += int(usage[field] or 0)
            if f"{field}_reported" in usage.keys():
                item[f"{field}_reported"] |= bool(usage[f"{field}_reported"])
    for item in grouped.values():
        if item["trace_count"] == 0 and item.pop("_log_error", False):
            item["errors"] = 1
            item["issues"] = 1
        item.pop("_latest_log_ns", None)
        item.pop("_latest_signal_ns", None)
        item.pop("_trace_last_ns", None)
        item.pop("_latest_task_ns", None)
        item.pop("_task_signal_seen", None)
        latest_wait = item.pop("latest_waiting_for", None) or ""
        task_wait = item.pop("task_waiting_for", None)
        item["waiting_for"] = (task_wait or latest_wait
                               if item.get("latest_task_outcome") == "waiting" else latest_wait)
    from .error_reminders import error_reference, pending_failures
    errors_by_owner = {}
    for event in reminder_events:
        key = (event.get('session_id'), event.get('agent_id'))
        errors_by_owner.setdefault(key, []).append(event)
    result = []
    for item in grouped.values():
        # Metadata references only; full captured errors stay in Activity.
        item['recent_errors'] = [error_reference(event)
            for event in pending_failures(errors_by_owner.get((item['session_id'],item.get('agent_id')),[]),
                                          receiver=receiver)]
        item["known_cost_usd"] = round(item["known_cost_usd"], 8)
        from .storage_derived import finalize_usage
        finalize_usage(item)
        item["services"] = sorted(item["services"])
        item["models"] = sorted(item["models"])
        result.append(item)
    result.sort(key=lambda item: (item["last_unix_nano"], item["session_id"]), reverse=True)
    for item in result[:int(limit)]:
        latest=item.get('latest_run') or {}
        if latest.get('turn_id'):
            latest['turn_usage']=self.review_turn(latest['trace_id'])
    return result[:int(limit)]
'''

_namespace = dict(vars(_core))
exec(SOURCE, _namespace)
sessions = _namespace['sessions']
