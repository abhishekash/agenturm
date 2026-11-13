"""Indexed activity projection, maintained transactionally from source changes.

SQLite triggers queue changes even from an older writer. Readers repair a bounded
queue or report not-ready; they never fall back to expanding historical JSON.
"""

from __future__ import annotations
import json

MCP_PREDICATE = "(json_type(attributes,'$.\"mcp.method.name\"') IS NOT NULL OR json_type(attributes,'$.\"mcp.server.name\"') IS NOT NULL OR json_type(attributes,'$.\"mcp.tool.name\"') IS NOT NULL OR json_extract(attributes,'$.\"rpc.system\"')='mcp')"


def initialize(db):
    db.executescript("""
    CREATE TABLE IF NOT EXISTS activity_index_v1 (
        event_key TEXT PRIMARY KEY, kind TEXT NOT NULL, owner_key TEXT NOT NULL,
        stamp INTEGER NOT NULL, trace_id TEXT, session_id TEXT, agent_id TEXT,
        service TEXT, domain TEXT, action TEXT, result TEXT, turn_id TEXT,
        payload TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS activity_order_v1 ON activity_index_v1(stamp DESC,event_key DESC);
    CREATE INDEX IF NOT EXISTS activity_trace_v1 ON activity_index_v1(trace_id,stamp DESC,event_key DESC);
    CREATE INDEX IF NOT EXISTS activity_agent_v1 ON activity_index_v1(agent_id,stamp DESC,event_key DESC);
    CREATE INDEX IF NOT EXISTS activity_session_v1 ON activity_index_v1(session_id,stamp DESC,event_key DESC);
    CREATE INDEX IF NOT EXISTS activity_turn_v1 ON activity_index_v1(service,turn_id,stamp DESC,event_key DESC);
    CREATE INDEX IF NOT EXISTS activity_context_v1 ON activity_index_v1(trace_id,domain,action,stamp DESC,event_key DESC);
    CREATE INDEX IF NOT EXISTS activity_turn_context_v1 ON activity_index_v1(service,turn_id,domain,action,stamp DESC,event_key DESC);
    CREATE INDEX IF NOT EXISTS activity_owner_v1 ON activity_index_v1(kind,owner_key);
    CREATE TABLE IF NOT EXISTS activity_dirty_v1(kind TEXT,owner_key TEXT,PRIMARY KEY(kind,owner_key));
    """)
    for table, kind, key in (
        ("spans", "span", "NEW.trace_id||':'||NEW.span_id"),
        ("logs", "log", "NEW.digest"),
    ):
        oldkey = key.replace("NEW.", "OLD.")
        for operation in ("INSERT", "UPDATE", "DELETE"):
            owner = oldkey if operation == "DELETE" else key
            extra = ""
            if table == "logs":
                alias = "OLD" if operation == "DELETE" else "NEW"
                extra = f"INSERT INTO activity_dirty_v1 SELECT 'span',{alias}.trace_id||':'||{alias}.span_id WHERE {alias}.trace_id IS NOT NULL AND {alias}.span_id IS NOT NULL ON CONFLICT DO NOTHING;"
                if operation == "UPDATE":
                    extra += "INSERT INTO activity_dirty_v1 SELECT 'span',OLD.trace_id||':'||OLD.span_id WHERE OLD.trace_id IS NOT NULL AND OLD.span_id IS NOT NULL ON CONFLICT DO NOTHING;"
            db.execute(f"""CREATE TRIGGER IF NOT EXISTS activity_{kind}_{operation.lower()}_v1
                AFTER {operation} ON {table} BEGIN
                INSERT INTO activity_dirty_v1 VALUES('{kind}',{owner}) ON CONFLICT DO NOTHING;{extra} END""")
    for table, stamp in (("spans", "end_ns"), ("logs", "time_ns")):
        db.execute(
            f"CREATE INDEX IF NOT EXISTS {table}_mcp_time_v1 ON {table}({stamp} DESC) WHERE {MCP_PREDICATE}"
        )
    from .core import INPUT_TOKEN_KEYS, OUTPUT_TOKEN_KEYS, TOKEN_BREAKDOWN_KEYS

    usage_fields = {
        "input_tokens": INPUT_TOKEN_KEYS,
        "output_tokens": OUTPUT_TOKEN_KEYS,
        **TOKEN_BREAKDOWN_KEYS,
    }
    declarations = [
        "trace_id TEXT PRIMARY KEY",
        "session_id TEXT",
        "status TEXT",
        "active INTEGER",
        "span_count INTEGER",
        "usage_complete INTEGER",
        "status_complete INTEGER",
        "known_cost_usd REAL",
        "cost_complete INTEGER",
    ]
    declarations.extend(
        [
            "agent_id TEXT",
            "outcome_failed INTEGER",
            "services_json TEXT",
            "models_json TEXT",
            "services_complete INTEGER",
            "models_complete INTEGER",
            "initial_task TEXT",
        ]
    )
    for field in usage_fields:
        declarations.extend([field + " INTEGER", field + "_reported INTEGER"])
    db.execute(
        "CREATE TABLE IF NOT EXISTS live_run_stats_v1(" + ",".join(declarations) + ")"
    )
    existing = {row[1] for row in db.execute("PRAGMA table_info(live_run_stats_v1)")}
    for declaration in declarations:
        if declaration.split()[0] not in existing:
            db.execute("ALTER TABLE live_run_stats_v1 ADD COLUMN " + declaration)
    db.execute(
        "CREATE INDEX IF NOT EXISTS live_run_session_v2 ON live_run_stats_v1(session_id,agent_id)"
    )
    values = [
        "NEW.trace_id",
        "NEW.session_id",
        "json_extract(NEW.summary_json,'$.status')",
        "COALESCE(json_extract(NEW.summary_json,'$.active'),0)",
        "COALESCE(json_extract(NEW.summary_json,'$.span_count'),0)",
        "COALESCE(json_extract(NEW.summary_json,'$.usage_complete'),1)",
        "COALESCE(json_extract(NEW.summary_json,'$.status_complete'),1)",
        "json_extract(NEW.summary_json,'$.known_cost_usd')",
        "COALESCE(json_extract(NEW.summary_json,'$.cost_complete'),0)",
    ]
    values.extend(
        [
            "NEW.agent_id",
            "COALESCE(json_extract(NEW.summary_json,'$.outcome_passed')=0,0)",
            "COALESCE(json_extract(NEW.summary_json,'$.services'),'[]')",
            "COALESCE(json_extract(NEW.summary_json,'$.models'),'[]')",
            "COALESCE(json_extract(NEW.summary_json,'$.services_complete'),1)",
            "COALESCE(json_extract(NEW.summary_json,'$.models_complete'),1)",
            "json_extract(NEW.summary_json,'$.initial_task')",
        ]
    )
    for field in usage_fields:
        values.extend(
            [
                f"COALESCE(json_extract(NEW.summary_json,'$.{field}'),0)",
                f"COALESCE(json_extract(NEW.summary_json,'$.{field}_reported'),0)",
            ]
        )
    columns = [declaration.split()[0] for declaration in declarations]
    upsert = " ON CONFLICT(trace_id) DO UPDATE SET " + ",".join(
        column + "=excluded." + column for column in columns[1:]
    )
    if not db.in_transaction:
        db.execute("BEGIN IMMEDIATE")
    for operation in ("INSERT", "UPDATE"):
        db.execute(f"DROP TRIGGER IF EXISTS run_stats_{operation.lower()}_v1")
        db.execute(f"DROP TRIGGER IF EXISTS run_stats_{operation.lower()}_v2")
        db.execute(
            f"CREATE TRIGGER IF NOT EXISTS run_stats_{operation.lower()}_v3 AFTER {operation} ON trace_summaries BEGIN INSERT INTO live_run_stats_v1("
            + ",".join(columns)
            + ") VALUES("
            + ",".join(values)
            + ")"
            + upsert
            + "; END"
        )
    db.execute(
        "CREATE TRIGGER IF NOT EXISTS run_stats_delete_v1 AFTER DELETE ON trace_summaries BEGIN DELETE FROM live_run_stats_v1 WHERE trace_id=OLD.trace_id; END"
    )
    db.execute(
        "CREATE TABLE IF NOT EXISTS live_log_usage_v1(digest TEXT PRIMARY KEY,session_id TEXT,service TEXT,"
        + ",".join(
            field + " INTEGER," + field + "_reported INTEGER" for field in usage_fields
        )
        + ")"
    )
    db.execute(
        "CREATE INDEX IF NOT EXISTS live_log_usage_session_v1 ON live_log_usage_v1(session_id)"
    )
    db.execute(
        "CREATE INDEX IF NOT EXISTS spans_opencode_turn_v1 ON spans(service,json_extract(attributes,'$.\"agent.turn.id\"'),agent_id,json_extract(attributes,'$.\"session.id\"'),start_ns,trace_id) WHERE name='agent.run' AND scope_name='observatory.opencode'"
    )
    db.execute("CREATE INDEX IF NOT EXISTS logs_service_v1 ON logs(service)")
    db.execute("CREATE INDEX IF NOT EXISTS metric_service_v1 ON metric_points(service)")
    db.execute("CREATE INDEX IF NOT EXISTS progress_service_v1 ON progress(service)")
    if not db.execute("SELECT 1 FROM schema_meta WHERE key='live_stats_v3'").fetchone():
        db.execute(
            "INSERT OR REPLACE INTO live_run_stats_v1("
            + ",".join(columns)
            + ") SELECT "
            + ",".join(value.replace("NEW.", "") for value in values)
            + " FROM trace_summaries"
        )
        db.execute("INSERT OR REPLACE INTO schema_meta VALUES('live_stats_v3','1')")
    db.execute(
        "CREATE INDEX IF NOT EXISTS live_initial_task_v1 ON live_run_stats_v1(session_id,agent_id) WHERE initial_task IS NOT NULL"
    )
    from .core import _session_sql_expression

    session = _session_sql_expression()
    db.execute(
        f"CREATE INDEX IF NOT EXISTS logs_session_preview_v2 ON logs({session},time_ns DESC,digest DESC,trace_id,agent_id) WHERE {session} IS NOT NULL"
    )
    db.execute(
        "CREATE INDEX IF NOT EXISTS activity_log_preview_v2 ON activity_index_v1(session_id,stamp DESC,event_key DESC,owner_key,agent_id,trace_id) WHERE kind='log'"
    )
    db.execute(
        f"CREATE INDEX IF NOT EXISTS logs_non_activity_session_v1 ON logs({session},time_ns DESC,digest DESC) WHERE {session} IS NOT NULL AND event_name!='agent.activity'"
    )
    # No payload copies in compact live summaries. Traces retain full search data.
    db.executescript("""
    CREATE TABLE IF NOT EXISTS live_summaries_v1(trace_id TEXT PRIMARY KEY,session_id TEXT,agent_id TEXT,summary_json TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS live_sessions_v1 ON live_summaries_v1(session_id);
    CREATE TRIGGER IF NOT EXISTS live_summary_insert_v1 AFTER INSERT ON trace_summaries BEGIN
      INSERT INTO live_summaries_v1 VALUES(NEW.trace_id,NEW.session_id,NEW.agent_id,json_remove(NEW.summary_json,'$.search_text','$.search_terms','$.events','$.activity','$.activity_terms','$.span_terms','$.log_terms')) ON CONFLICT(trace_id) DO UPDATE SET session_id=excluded.session_id,agent_id=excluded.agent_id,summary_json=excluded.summary_json;
    END;
    CREATE TRIGGER IF NOT EXISTS live_summary_update_v1 AFTER UPDATE ON trace_summaries BEGIN
      INSERT INTO live_summaries_v1 VALUES(NEW.trace_id,NEW.session_id,NEW.agent_id,json_remove(NEW.summary_json,'$.search_text','$.search_terms','$.events','$.activity','$.activity_terms','$.span_terms','$.log_terms')) ON CONFLICT(trace_id) DO UPDATE SET session_id=excluded.session_id,agent_id=excluded.agent_id,summary_json=excluded.summary_json;
    END;
    CREATE TRIGGER IF NOT EXISTS live_summary_delete_v1 AFTER DELETE ON trace_summaries BEGIN
      DELETE FROM live_summaries_v1 WHERE trace_id=OLD.trace_id;
    END;
    """)
    if not db.execute(
        "SELECT 1 FROM schema_meta WHERE key='activity_sources_v2'"
    ).fetchone():
        db.execute(
            "INSERT OR IGNORE INTO activity_dirty_v1 SELECT 'span',trace_id||':'||span_id FROM spans"
        )
        db.execute(
            "INSERT OR IGNORE INTO activity_dirty_v1 SELECT 'log',digest FROM logs"
        )
        db.execute(
            "INSERT OR REPLACE INTO live_summaries_v1 SELECT trace_id,session_id,agent_id,json_remove(summary_json,'$.search_text','$.search_terms','$.events','$.activity','$.activity_terms','$.span_terms','$.log_terms') FROM trace_summaries"
        )
        db.execute(
            "INSERT OR REPLACE INTO schema_meta VALUES('activity_sources_v2','1')"
        )
        db.execute(
            "INSERT OR REPLACE INTO live_run_stats_v1("
            + ",".join(columns)
            + ") SELECT "
            + ",".join(value.replace("NEW.", "") for value in values)
            + " FROM trace_summaries"
        )


def drain(db, limit=200):
    from .core import (
        _activity_for,
        SESSION_KEYS,
        _session_value,
        INPUT_TOKEN_KEYS,
        OUTPUT_TOKEN_KEYS,
        TOKEN_BREAKDOWN_KEYS,
    )

    jobs = db.execute(
        "SELECT kind,owner_key FROM activity_dirty_v1 ORDER BY kind,owner_key LIMIT ?",
        (limit,),
    ).fetchall()
    for kind, key in jobs:
        db.execute(
            "DELETE FROM activity_index_v1 WHERE kind=? AND owner_key=?", (kind, key)
        )
        if kind == "log":
            db.execute("DELETE FROM live_log_usage_v1 WHERE digest=?", (key,))
        if kind == "span":
            trace, span = key.split(":", 1)
            r = db.execute(
                "SELECT * FROM spans WHERE trace_id=? AND span_id=?", (trace, span)
            ).fetchone()
            rows = []
            if r:
                source = dict(r)
                attrs = json.loads(r["attributes"])
                resource = json.loads(r["resource_attributes"])
                source.update(
                    attributes=attrs,
                    resource_attributes=resource,
                    events=json.loads(r["events"]),
                    start_unix_nano=r["start_ns"],
                    end_unix_nano=r["end_ns"],
                )
                linked = [
                    dict(
                        event_name="agent.activity",
                        span_id=span,
                        attributes={},
                        time_unix_nano=0,
                        id="",
                        service=r["service"],
                        body="",
                    )
                ]
                has_link = db.execute(
                    "SELECT 1 FROM logs WHERE trace_id=? AND span_id=? AND event_name='agent.activity' LIMIT 1",
                    (trace, span),
                ).fetchone()
                rows = [
                    row
                    for row in _activity_for([source], linked if has_link else [])
                    if row["source"] == "span"
                ]
        else:
            r = db.execute("SELECT * FROM logs WHERE digest=?", (key,)).fetchone()
            rows = []
            if r and r["event_name"] == "agent.activity":
                attrs = json.loads(r["attributes"])
                resource = json.loads(r["resource_attributes"])
                source = dict(
                    attributes=attrs,
                    resource_attributes=resource,
                    event_name=r["event_name"],
                    span_id=r["span_id"],
                    time_unix_nano=r["time_ns"],
                    id=r["digest"],
                    service=r["service"],
                    body=json.loads(r["body"]),
                )
                rows = _activity_for([], [source])
                fields = {
                    "input_tokens": INPUT_TOKEN_KEYS,
                    "output_tokens": OUTPUT_TOKEN_KEYS,
                    **TOKEN_BREAKDOWN_KEYS,
                }
                if any(name in attrs for names in fields.values() for name in names):
                    session = next(
                        (
                            _session_value(values.get(key))
                            for key in SESSION_KEYS
                            for values in (attrs, resource)
                            if _session_value(values.get(key))
                        ),
                        None,
                    )
                    numeric = [r["digest"], session, r["service"]]
                    for names in fields.values():
                        value = next(
                            (attrs[name] for name in names if name in attrs), None
                        )
                        try:
                            amount = max(0, int(value or 0))
                        except (ValueError, TypeError, OverflowError):
                            amount = 0
                        numeric.extend((amount, int(value is not None)))
                    db.execute(
                        "INSERT OR REPLACE INTO live_log_usage_v1 VALUES("
                        + ",".join("?" for _ in numeric)
                        + ")",
                        numeric,
                    )

        if r:
            for row in rows:
                # Explicit event ownership remains authoritative inside a shared trace.
                agent = row.get("agent_id")
                # _activity_row has already resolved explicit event ownership;
                # unknown/malformed identities must remain unknown.
                session = row.get("session_id")
                turn = row.get("turn_id") or None
                row.update(
                    agent_id=agent,
                    session_id=session,
                    trace_id=r["trace_id"],
                    turn_id=turn,
                )
                event_key = (r["trace_id"] or "log") + ":" + row["source_id"]
                row["id"] = event_key
                db.execute(
                    "INSERT OR REPLACE INTO activity_index_v1 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    (
                        event_key,
                        kind,
                        key,
                        row["time_unix_nano"],
                        r["trace_id"],
                        session,
                        agent,
                        row["service"],
                        row["domain"],
                        row["action"],
                        row["result"],
                        turn,
                        json.dumps(
                            {
                                **row,
                                "detail": "",
                                "detail_present": bool(row.get("detail")),
                                "subject": str(row.get("subject") or "")[:8000],
                            },
                            separators=(",", ":"),
                        ),
                    ),
                )
        db.execute(
            "DELETE FROM activity_dirty_v1 WHERE kind=? AND owner_key=?", (kind, key)
        )
    return len(jobs)


def hydrate(db, records):
    """Dereference text only after indexed pagination, without copying it at rest."""
    from .core import _activity_for

    logs = {}
    spans = {}
    log_ids = list(
        {
            row["source_id"]
            for row in records
            if row.get("source") == "log" and row.get("detail_present")
        }
    )
    if log_ids:
        for row in db.execute(
            "SELECT digest,body,attributes FROM logs WHERE digest IN ("
            + ",".join("?" for _ in log_ids)
            + ")",
            log_ids,
        ):
            logs[row["digest"]] = row
    owners = list(
        {
            (row.get("trace_id"), row.get("span_id"))
            for row in records
            if row.get("source") == "span" and row.get("detail_present")
        }
    )
    for offset in range(0, len(owners), 100):
        batch = owners[offset : offset + 100]
        predicate = " OR ".join("(trace_id=? AND span_id=?)" for _ in batch)
        for row in db.execute(
            "SELECT * FROM spans WHERE " + predicate,
            [value for pair in batch for value in pair],
        ):
            source = dict(row)
            source.update(
                attributes=json.loads(row["attributes"]),
                resource_attributes=json.loads(row["resource_attributes"]),
                events=json.loads(row["events"]),
                start_unix_nano=row["start_ns"],
                end_unix_nano=row["end_ns"],
            )
            spans[(row["trace_id"], row["span_id"])] = {
                event["source_id"]: event.get("detail", "")
                for event in _activity_for([source], [])
            }
    for row in records:
        if not row.pop("detail_present", False):
            continue
        detail = ""
        if row["source"] == "log" and row["source_id"] in logs:
            source = logs[row["source_id"]]
            body = json.loads(source["body"])
            attrs = json.loads(source["attributes"])
            detail = attrs.get("agent.activity.detail") or (
                body if isinstance(body, str) else ""
            )
        elif row["source"] == "span":
            detail = spans.get((row.get("trace_id"), row.get("span_id")), {}).get(
                row["source_id"], ""
            )
        row["detail"] = str(detail)[:8000]
        if len(str(detail)) > 8000:
            row["detail_clipped"] = True
    return records
