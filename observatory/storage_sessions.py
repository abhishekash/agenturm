"""Narrow per-run facts feed session totals; only recent payloads are decoded."""

FIELDS = (
    "input_tokens",
    "output_tokens",
    "cache_read_input_tokens",
    "cache_write_input_tokens",
    "reasoning_output_tokens",
)


def session_projection(db, limit):
    selected = """SELECT c.session_id FROM live_run_stats_v1 c JOIN trace_activity a USING(trace_id)
        WHERE c.session_id IS NOT NULL GROUP BY c.session_id
        ORDER BY MAX(a.last_ns) DESC,c.session_id DESC LIMIT ?"""
    heads = db.execute(
        """WITH ranked AS (
        SELECT c.trace_id,c.session_id,c.agent_id,a.last_ns,
            ROW_NUMBER() OVER(PARTITION BY c.session_id,c.agent_id ORDER BY a.last_ns DESC,c.trace_id DESC) AS position
        FROM live_run_stats_v1 c JOIN trace_activity a USING(trace_id)
        WHERE c.session_id IN ("""
        + selected
        + """))
        SELECT r.session_id,r.trace_id,c.summary_json,r.agent_id,r.last_ns
        FROM ranked r JOIN live_summaries_v1 c USING(trace_id)
        WHERE position<=2 ORDER BY r.last_ns DESC,r.session_id DESC,r.trace_id DESC""",
        (limit,),
    ).fetchall()
    values = [
        "COUNT(*) AS trace_count",
        "SUM(s.span_count) AS span_count",
        "SUM(s.status='ERROR') AS errors",
        "SUM(s.status='FAILED' OR s.outcome_failed) AS failed",
        "SUM(s.status='INCOMPLETE') AS incomplete",
        "SUM(s.status IN ('ERROR','FAILED','INCOMPLETE') OR s.outcome_failed) AS issues",
        "MIN(s.usage_complete) AS usage_complete",
        "MIN(s.status_complete) AS status_complete",
        "SUM(COALESCE(s.known_cost_usd,0)) AS known_cost_usd",
        "SUM(NOT s.cost_complete) AS unknown_cost_runs",
        "MIN(s.cost_complete) AS cost_complete",
    ]
    for field in FIELDS:
        values.extend(
            [
                f"SUM(s.{field}) AS {field}",
                f"MAX(s.{field}_reported) AS {field}_reported",
            ]
        )
    for field in ("services", "models"):
        values.append(f"MIN(s.{field}_complete) AS {field}_complete")
    aggregates = db.execute(
        "SELECT s.session_id,s.agent_id,"
        + ",".join(values)
        + " FROM live_run_stats_v1 s WHERE s.session_id IN ("
        + selected
        + ") GROUP BY s.session_id,s.agent_id",
        (limit,),
    ).fetchall()
    result = {}
    for row in aggregates:
        item = dict(row)
        for field in ("services", "models"):
            labels = db.execute(
                f"SELECT DISTINCT substr(CAST(label.value AS TEXT),1,128) AS label,length(CAST(label.value AS TEXT))<=128 AS complete FROM live_run_stats_v1 source,json_each(source.{field}_json) label WHERE source.session_id=? AND source.agent_id IS ? ORDER BY label LIMIT 129",
                (item["session_id"], item["agent_id"]),
            ).fetchall()
            item[field] = {row["label"] for row in labels[:128]}
            item[field + "_complete"] = (
                bool(item[field + "_complete"])
                and len(labels) <= 128
                and all(row["complete"] for row in labels)
            )
        for name in tuple(item):
            if name.endswith(("_complete", "_reported")):
                item[name] = bool(item[name])
        result[(item.pop("session_id"), item.pop("agent_id"))] = item
    # Keep original-task lookup in this same ownership/content snapshot. One
    # indexed batch also avoids opening a connection for every displayed actor.
    initial_tasks = {}
    for row in db.execute("""WITH original AS (
        SELECT s.trace_id,s.session_id,s.agent_id,
            ROW_NUMBER() OVER(PARTITION BY s.session_id,s.agent_id ORDER BY a.last_ns DESC,s.trace_id DESC) AS position
        FROM live_run_stats_v1 s JOIN trace_activity a USING(trace_id)
        WHERE s.initial_task IS NOT NULL AND s.initial_task!='' AND s.session_id IN ("""
        + selected + ")) SELECT r.session_id,r.agent_id,s.initial_task FROM original r JOIN live_run_stats_v1 s USING(trace_id) WHERE r.position=1", (limit,)):
        initial_tasks[(row['session_id'],row['agent_id'])]=row['initial_task']
    return heads, result, initial_tasks
