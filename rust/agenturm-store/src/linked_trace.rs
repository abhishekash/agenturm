//! Resolve only explicitly attributed, retained OTLP execution scope.

use super::*;
use agenturm_kernel::{ObservationPosition, ReadSnapshot};

const MAX_TRACE_SCOPE_ROWS: usize = 4096;

#[derive(Clone, Copy)]
pub(super) enum TraceLink<'a> {
    Unlinked,
    Session,
    McpOperation(&'a str),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceTurnQuery {
    pub harness_id: String,
    pub session_id: String,
    pub trace_source_id: String,
    pub trace_id: String,
    pub snapshot_id: Option<ReadSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedTraceTurn {
    pub turn_id: String,
    pub worker_id: Option<String>,
    pub snapshot_id: ReadSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceReviewQuery {
    pub page: ObservationQuery,
    pub trace_source_id: String,
    pub trace_id: String,
    pub expected_worker_id: Option<String>,
    pub linked_traces: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedTraceReview {
    pub turn_id: String,
    pub worker_id: Option<String>,
    pub page: ObservationPage,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpTraceQuery {
    pub source_id: ExtensionId,
    pub event_id: String,
    pub revision: u32,
    pub snapshot_id: ReadSnapshot,
    pub limit: usize,
    pub before: Option<ObservationPosition>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StandaloneTraceQuery {
    pub source_id: ExtensionId,
    pub session_id: String,
    pub trace_id: String,
    pub worker_id: Option<String>,
    pub snapshot_id: Option<ReadSnapshot>,
    pub before: Option<ObservationPosition>,
    pub limit: usize,
}

impl Store {
    /// Reads the exact retained telemetry trace and its span metadata companions.
    /// The producer source owns the trace. Native links remain recorded metadata.
    pub fn query_standalone_trace(
        &self,
        query: &StandaloneTraceQuery,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
    ) -> Result<ObservationPage> {
        if allowed_sources.is_some_and(|allowed| !allowed.contains(&query.source_id)) {
            return Err(StoreError::Invalid("trace source is unavailable"));
        }
        if !query.source_id.as_str().starts_with("otlp.")
            || !valid_trace_id(&query.trace_id)
            || !(1..=MAX_DURABLE_SNAPSHOT_OBSERVATIONS).contains(&query.limit)
        {
            return Err(StoreError::Invalid("standalone trace review scope is invalid"));
        }
        validate_identifier(&query.session_id, MAX_SESSION_KEY_BYTES, "session ID")?;
        if let Some(worker) = &query.worker_id {
            validate_identifier(worker, MAX_SESSION_KEY_BYTES, "worker ID")?;
        }
        if let Some(cursor) = &query.before {
            validate_identifier(&cursor.event_id, MAX_EVENT_ID_BYTES, "trace event ID")?;
            if cursor.harness_id != query.source_id.as_str()
                || query.snapshot_id != Some(cursor.snapshot_id)
                || cursor.observed_at_unix_ns > i64::MAX as u64
            {
                return Err(StoreError::Invalid("trace cursor does not match the review scope"));
            }
        }
        let transaction = self.connection.unchecked_transaction()?;
        let (snapshot_id, watermark) = retention::resolve_read_snapshot(&transaction, query.snapshot_id)?;
        let prefix = format!("otlp:{}:", query.trace_id);
        let before = query.before.as_ref();
        let mut statement = transaction.prepare(STANDALONE_TRACE_SQL)?;
        let mut results = statement.query(rusqlite::named_params! {
            ":source": query.source_id.as_str(),
            ":session": query.session_id,
            ":worker": query.worker_id,
            ":trace": query.trace_id,
            ":prefix": prefix,
            ":prefix_end": format!("{prefix}~"),
            ":watermark": watermark,
            ":scan_limit": MAX_TRACE_SCOPE_ROWS + 1,
            ":limit": query.limit + 1,
            ":before_time": before.map(|item| item.observed_at_unix_ns as i64),
            ":before_event": before.map(|item| item.event_id.as_str()),
            ":before_revision": before.map(|item| item.revision),
        })?;
        let mut observations = Vec::new();
        while let Some(row) = results.next()? {
            if row.get::<_, usize>(0)? > MAX_TRACE_SCOPE_ROWS {
                return Err(StoreError::TraceScopeTooLarge { max_rows: MAX_TRACE_SCOPE_ROWS });
            }
            if !row.get::<_, bool>(1)? {
                return Err(StoreError::Invalid("trace cursor occurrence is outside the review scope"));
            }
            let Some(event_id) = row.get::<_, Option<String>>(2)? else { break };
            let kind = serde_json::from_str::<KernelObservationKind>(&row.get::<_, String>(7)?)?;
            observations.push(Observation {
                harness_id: query.source_id.clone(),
                event_id,
                revision: row.get(3)?,
                identity: agenturm_kernel::ObservationIdentity {
                    scope: agenturm_kernel::ObservationScope::Session,
                    session_id: Some(query.session_id.clone()),
                    worker_id: row.get(4)?,
                    turn_id: row.get(5)?,
                    operation_id: None,
                },
                observed_at_unix_ns: row.get(6)?,
                kind,
            });
        }
        let has_more = observations.len() > query.limit;
        observations.truncate(query.limit);
        observations.reverse();
        Ok(ObservationPage { observations, snapshot_id, has_more })
    }

    /// Revalidates the selected MCP receipt and reads its explicitly linked
    /// spans in one SQLite snapshot. An absent or inaccessible call has no
    /// page, while a retained call can have a page with no linked spans.
    pub fn query_mcp_traces(
        &self,
        query: &McpTraceQuery,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
    ) -> Result<Option<ObservationPage>> {
        validate_identifier(
            query.source_id.as_str(),
            MAX_SOURCE_ID_BYTES,
            "MCP source ID",
        )?;
        validate_identifier(&query.event_id, MAX_EVENT_ID_BYTES, "MCP event ID")?;
        if query.source_id.as_str().starts_with("otlp.")
            || allowed_sources.is_some_and(|sources| !sources.contains(&query.source_id))
        {
            return Ok(None);
        }
        let transaction = self.connection.unchecked_transaction()?;
        let (snapshot_id, watermark) =
            retention::resolve_read_snapshot(&transaction, Some(query.snapshot_id))?;
        let anchor: Option<(String, String, Option<String>, String)> = transaction
            .query_row(
                "SELECT call.session_id, call.turn_id, call.worker_id, call.operation_id
                 FROM agenturm_observations AS call
                 JOIN agenturm_history_batches AS batch ON batch.request_id=call.request_id
                 WHERE call.source_id=?1 AND call.event_id=?2 AND call.revision=?3
                   AND call.scope='session' AND call.session_id IS NOT NULL
                   AND call.turn_id IS NOT NULL AND call.operation_id IS NOT NULL
                   AND json_extract(call.kind_json,'$.kind')='mcp_call'
                   AND batch.sequence<=?4",
                params![
                    query.source_id.as_str(),
                    query.event_id,
                    query.revision,
                    watermark
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((session_id, turn_id, worker_id, operation_id)) = anchor else {
            return Ok(None);
        };
        validate_identifier(&operation_id, MAX_SESSION_KEY_BYTES, "MCP operation ID")?;
        let page = ObservationQuery {
            limit: query.limit,
            harness_id: Some(query.source_id.as_str().to_owned()),
            session_id: Some(session_id),
            before: query.before.clone(),
            snapshot_id: Some(snapshot_id),
        };
        Self::query_history_in(
            &transaction,
            &page,
            Some(&turn_id),
            Some(worker_id.as_deref()),
            TraceLink::McpOperation(&operation_id),
            allowed_sources,
            false,
        )
        .map(Some)
    }

    /// Reads the unique explicit turn of a trace at the selected watermark.
    /// A sibling scope, absent native anchor, or session-only trace yields no match.
    pub fn resolve_trace_turn(&self, query: &TraceTurnQuery) -> Result<Option<ResolvedTraceTurn>> {
        let transaction = self.connection.unchecked_transaction()?;
        Self::resolve_trace_turn_in(&transaction, query)
    }

    /// Resolves the native turn and reads one exact page in a single snapshot.
    pub fn review_trace_turn(
        &self,
        query: &TraceReviewQuery,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
    ) -> Result<Option<ResolvedTraceReview>> {
        let page = &query.page;
        let anchor = TraceTurnQuery {
            harness_id: page
                .harness_id
                .clone()
                .ok_or(StoreError::Invalid("review requires a native source ID"))?,
            session_id: page
                .session_id
                .clone()
                .ok_or(StoreError::Invalid("review requires a session ID"))?,
            trace_source_id: query.trace_source_id.clone(),
            trace_id: query.trace_id.clone(),
            snapshot_id: page.snapshot_id,
        };
        let transaction = self.connection.unchecked_transaction()?;
        let Some(turn) = Self::resolve_trace_turn_in(&transaction, &anchor)? else {
            return Ok(None);
        };
        if query
            .expected_worker_id
            .as_ref()
            .is_some_and(|worker| turn.worker_id.as_ref() != Some(worker))
        {
            return Ok(None);
        }
        let page = Self::query_history_in(
            &transaction,
            page,
            Some(&turn.turn_id),
            Some(turn.worker_id.as_deref()),
            if query.linked_traces {
                TraceLink::Session
            } else {
                TraceLink::Unlinked
            },
            allowed_sources,
            false,
        )?;
        debug_assert_eq!(page.snapshot_id, turn.snapshot_id);
        Ok(Some(ResolvedTraceReview {
            turn_id: turn.turn_id,
            worker_id: turn.worker_id,
            page,
        }))
    }

    fn resolve_trace_turn_in(
        connection: &Connection,
        query: &TraceTurnQuery,
    ) -> Result<Option<ResolvedTraceTurn>> {
        validate_identifier(&query.session_id, MAX_SESSION_KEY_BYTES, "session ID")?;
        validate_identifier(&query.harness_id, MAX_SOURCE_ID_BYTES, "native source ID")?;
        validate_identifier(
            &query.trace_source_id,
            MAX_SOURCE_ID_BYTES,
            "trace source ID",
        )?;
        if query.harness_id.starts_with("otlp.")
            || !query.trace_source_id.starts_with("otlp.")
            || ExtensionId::parse(&query.harness_id).is_err()
            || ExtensionId::parse(&query.trace_source_id).is_err()
            || !valid_trace_id(&query.trace_id)
        {
            return Err(StoreError::Invalid(
                "trace source, owner, or identity is invalid",
            ));
        }
        let (snapshot_id, watermark) =
            retention::resolve_read_snapshot(connection, query.snapshot_id)?;
        let event_prefix = format!("otlp:{}:", query.trace_id);
        // Bound the retained rows before checking their distinct scopes. A
        // sentinel row rejects oversized traces without choosing a partial scope.
        let (count, turn_id, worker_id): (usize, Option<String>, Option<String>) = connection
            .query_row(
                "WITH retained AS MATERIALIZED (
                     SELECT span.kind_json, span.session_id, span.turn_id, span.worker_id
                     FROM agenturm_observations AS span
                     CROSS JOIN agenturm_history_batches AS span_batch
                       ON span_batch.request_id=span.request_id
                     WHERE span.source_id=?1 AND span.event_id>=?2 AND span.event_id<?3
                       AND span.scope='session' AND span_batch.sequence<=?4
                     LIMIT ?8
                 ), scopes AS (
                     SELECT DISTINCT json_extract(kind_json,'$.target_source_id') AS target,
                            session_id, turn_id, worker_id
                     FROM retained
                     WHERE turn_id IS NOT NULL
                       AND json_extract(kind_json,'$.kind')='trace_span'
                       AND json_extract(kind_json,'$.trace_id')=?5
                     LIMIT 2
                 )
                 SELECT (SELECT count(*) FROM retained), scopes.turn_id, scopes.worker_id
                 FROM (SELECT 1) LEFT JOIN scopes
                 ON (SELECT count(*) FROM retained)<?8 AND (SELECT count(*) FROM scopes)=1
                   AND scopes.target=?6 AND scopes.session_id=?7
                   AND EXISTS (
                       SELECT 1 FROM agenturm_observations AS anchor
                       JOIN agenturm_history_batches AS anchor_batch
                         ON anchor_batch.request_id=anchor.request_id
                       WHERE anchor.source_id=?6 AND anchor.session_id=?7
                         AND anchor.turn_id=scopes.turn_id AND anchor.worker_id IS scopes.worker_id
                         AND anchor.scope='session' AND anchor_batch.sequence<=?4
                         AND json_extract(anchor.kind_json,'$.kind')<>'trace_span'
                   )",
                params![
                    query.trace_source_id,
                    event_prefix,
                    format!("{event_prefix}~"),
                    watermark,
                    query.trace_id,
                    query.harness_id,
                    query.session_id,
                    MAX_TRACE_SCOPE_ROWS + 1,
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        if count > MAX_TRACE_SCOPE_ROWS {
            return Err(StoreError::TraceScopeTooLarge {
                max_rows: MAX_TRACE_SCOPE_ROWS,
            });
        }
        Ok(turn_id.map(|turn_id| ResolvedTraceTurn {
            turn_id,
            worker_id,
            snapshot_id,
        }))
    }
}

fn valid_trace_id(value: &str) -> bool {
    value.len() == 32
        && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        && value.bytes().any(|byte| byte != b'0')
}

// The source/event primary key bounds the initial trace-range scan. Each
// companion is looked up by its exact event ID and matched to its retained
// structural span before the cursor or result page is evaluated.
const STANDALONE_TRACE_SQL: &str = "
    WITH spans AS MATERIALIZED (
        SELECT span.event_id, span.revision, span.worker_id, span.turn_id,
               span.observed_at_unix_ns, span.kind_json,
               json_extract(span.kind_json,'$.span_id') AS span_id
        FROM agenturm_observations AS span
        CROSS JOIN agenturm_history_batches AS admission ON admission.request_id=span.request_id
        WHERE span.source_id=:source AND span.event_id>=:prefix AND span.event_id<:prefix_end
          AND span.scope='session' AND span.session_id=:session AND span.operation_id IS NULL
          AND (:worker IS NULL OR span.worker_id=:worker)
          AND admission.sequence<=:watermark
          AND json_extract(span.kind_json,'$.kind')='trace_span'
          AND json_extract(span.kind_json,'$.trace_id')=:trace
          AND span.event_id=:prefix || json_extract(span.kind_json,'$.span_id')
        LIMIT :scan_limit
    ), bounds AS (
        SELECT count(*) AS span_count FROM spans
    ), candidates AS MATERIALIZED (
        SELECT event_id, revision, worker_id, turn_id, observed_at_unix_ns, kind_json FROM spans
        WHERE (SELECT span_count FROM bounds)<:scan_limit
        UNION ALL
        SELECT companion.event_id, companion.revision, companion.worker_id, companion.turn_id,
               companion.observed_at_unix_ns, companion.kind_json
        FROM spans
        CROSS JOIN agenturm_observations AS companion
          ON companion.source_id=:source AND companion.revision=spans.revision
         AND companion.event_id IN (spans.event_id || ':summary',
             'otlp-usage:' || :trace || ':' || spans.span_id,
             'otlp-cost:' || :trace || ':' || spans.span_id)
        CROSS JOIN agenturm_history_batches AS admission
          ON admission.request_id=companion.request_id
        WHERE (SELECT span_count FROM bounds)<:scan_limit
          AND companion.scope='session' AND companion.session_id=:session
          AND companion.worker_id IS spans.worker_id AND companion.turn_id IS spans.turn_id
          AND companion.operation_id IS NULL
          AND companion.observed_at_unix_ns=spans.observed_at_unix_ns
          AND admission.sequence<=:watermark
          AND (
               (companion.event_id=spans.event_id || ':summary'
                AND json_extract(companion.kind_json,'$.kind')='trace_span_summary'
                AND json_extract(companion.kind_json,'$.trace_id')=:trace
                AND json_extract(companion.kind_json,'$.span_id')=spans.span_id)
            OR (companion.event_id='otlp-usage:' || :trace || ':' || spans.span_id
                AND json_extract(companion.kind_json,'$.kind')='span_token_usage'
                AND json_extract(companion.kind_json,'$.span.trace_id')=:trace
                AND json_extract(companion.kind_json,'$.span.span_id')=spans.span_id)
            OR (companion.event_id='otlp-cost:' || :trace || ':' || spans.span_id
                AND json_extract(companion.kind_json,'$.kind')='reported_cost'
                AND json_extract(companion.kind_json,'$.span.trace_id')=:trace
                AND json_extract(companion.kind_json,'$.span.span_id')=spans.span_id)
          )
    ), page AS (
        SELECT event_id, revision, worker_id, turn_id, observed_at_unix_ns, kind_json
        FROM candidates
        WHERE :before_time IS NULL OR (observed_at_unix_ns,event_id,revision)
             < (:before_time,:before_event,:before_revision)
        ORDER BY observed_at_unix_ns DESC,event_id DESC,revision DESC
        LIMIT :limit
    )
    SELECT bounds.span_count, (:before_time IS NULL OR EXISTS (
               SELECT 1 FROM candidates AS anchor WHERE anchor.observed_at_unix_ns=:before_time
                 AND anchor.event_id=:before_event AND anchor.revision=:before_revision
           )), page.event_id, page.revision, page.worker_id, page.turn_id,
           page.observed_at_unix_ns, page.kind_json
    FROM bounds LEFT JOIN page ON true
    ORDER BY page.observed_at_unix_ns DESC,page.event_id DESC,page.revision DESC";

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{McpMethod, ObservationIdentity, TraceSpan, TraceStatus};

    #[test]
    fn mcp_trace_page_matches_the_exact_retained_call_and_both_source_permissions() {
        let mut store = Store::open(":memory:").unwrap();
        let native = ExtensionId::parse("harness.codex.fixture").unwrap();
        let otlp = ExtensionId::parse("otlp.mcp.fixture").unwrap();
        let mut turn = ObservationIdentity::session("session-1");
        turn.turn_id = Some("turn-1".into());
        let mut call = turn.clone();
        call.operation_id = Some("operation-1".into());
        store
            .commit(
                native.as_str(),
                None,
                "calls",
                &[
                    Observation {
                        harness_id: native.clone(),
                        event_id: "call-1".into(),
                        revision: 2,
                        identity: call,
                        observed_at_unix_ns: 1,
                        kind: KernelObservationKind::McpCall {
                            server: Some("server".into()),
                            tool: Some("tool".into()),
                            method: McpMethod::ToolsCall,
                            duration_ns: None,
                            result: "succeeded".into(),
                        },
                    },
                    Observation {
                        harness_id: native.clone(),
                        event_id: "turn-start".into(),
                        revision: 2,
                        identity: turn.clone(),
                        observed_at_unix_ns: 1,
                        kind: KernelObservationKind::SessionStarted,
                    },
                ],
            )
            .unwrap();
        let span =
            |number: u64, identity: ObservationIdentity, target: &str, operation: Option<&str>| {
                let trace_id = format!("{number:032x}");
                let span_id = format!("{number:016x}");
                Observation {
                    harness_id: otlp.clone(),
                    event_id: format!("otlp:{trace_id}:{span_id}"),
                    revision: 0,
                    identity,
                    observed_at_unix_ns: number + 2,
                    kind: KernelObservationKind::TraceSpan(TraceSpan {
                        trace_id,
                        span_id,
                        parent_span_id: None,
                        target_source_id: Some(target.into()),
                        target_operation_id: operation.map(str::to_owned),
                        started_at_unix_ns: 2,
                        status: TraceStatus::Ok,
                    }),
                }
            };
        let first = span(1, turn.clone(), native.as_str(), Some("operation-1"));
        let wrong_worker = ObservationIdentity {
            worker_id: Some("sibling".into()),
            ..turn.clone()
        };
        let wrong_session = ObservationIdentity {
            session_id: Some("session-2".into()),
            ..turn.clone()
        };
        let wrong_turn = ObservationIdentity {
            turn_id: Some("turn-2".into()),
            ..turn.clone()
        };
        store
            .commit(
                otlp.as_str(),
                None,
                "first",
                &[
                    first.clone(),
                    span(2, turn.clone(), native.as_str(), Some("operation-2")),
                    span(3, wrong_worker, native.as_str(), Some("operation-1")),
                    span(4, wrong_session, native.as_str(), Some("operation-1")),
                    span(5, wrong_turn, native.as_str(), Some("operation-1")),
                    span(6, turn.clone(), "harness.codex.other", Some("operation-1")),
                    span(7, turn.clone(), native.as_str(), None),
                ],
            )
            .unwrap();
        let pinned = store
            .query_observations(&ObservationQuery::latest(1))
            .unwrap()
            .snapshot_id;
        let mut query = McpTraceQuery {
            source_id: native.clone(),
            event_id: "call-1".into(),
            revision: 2,
            snapshot_id: pinned,
            limit: 1,
            before: None,
        };
        let allowed = BTreeSet::from([native.clone(), otlp.clone()]);
        let first_page = store
            .query_mcp_traces(&query, Some(&allowed))
            .unwrap()
            .unwrap();
        assert_eq!(
            first_page.observations.as_slice(),
            std::slice::from_ref(&first)
        );
        assert!(!first_page.has_more);
        assert!(
            store
                .query_mcp_traces(&query, Some(&BTreeSet::from([otlp.clone()])))
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .query_mcp_traces(&query, Some(&BTreeSet::from([native.clone()])))
                .unwrap()
                .unwrap()
                .observations
                .is_empty()
        );
        assert!(
            store
                .query_mcp_traces(
                    &McpTraceQuery {
                        revision: 3,
                        ..query.clone()
                    },
                    None
                )
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .query_mcp_traces(
                    &McpTraceQuery {
                        event_id: "turn-start".into(),
                        ..query.clone()
                    },
                    None
                )
                .unwrap()
                .is_none()
        );
        let replaced = ReadSnapshot::new(
            pinned.watermark(),
            Some("11111111111111111111111111111111".parse().unwrap()),
        );
        assert!(matches!(
            store.query_mcp_traces(
                &McpTraceQuery {
                    snapshot_id: replaced,
                    ..query.clone()
                },
                None
            ),
            Err(StoreError::SnapshotReplaced)
        ));

        let next = span(8, turn, native.as_str(), Some("operation-1"));
        store
            .commit(
                otlp.as_str(),
                Some("first"),
                "later",
                std::slice::from_ref(&next),
            )
            .unwrap();
        assert_eq!(
            store.query_mcp_traces(&query, None).unwrap(),
            Some(first_page)
        );
        query.snapshot_id = store
            .query_observations(&ObservationQuery::latest(1))
            .unwrap()
            .snapshot_id;
        let latest = store.query_mcp_traces(&query, None).unwrap().unwrap();
        assert_eq!(latest.observations, [next]);
        query.before = latest.older_position();
        assert!(query.before.is_some());
        let older = store.query_mcp_traces(&query, None).unwrap().unwrap();
        assert_eq!(older.observations, [first]);
        assert!(!older.has_more);
        assert_eq!(older.snapshot_id, latest.snapshot_id);
    }

    #[test]
    fn trace_resolution_refuses_a_partial_scope_over_budget() {
        let mut store = Store::open(":memory:").unwrap();
        let harness = ExtensionId::parse("harness.codex").unwrap();
        let trace_source = ExtensionId::parse("otlp.scope-budget").unwrap();
        let trace_id = "11111111111111111111111111111111";
        let mut identity = ObservationIdentity::session("session-1");
        identity.turn_id = Some("turn-1".into());
        store
            .commit(
                harness.as_str(),
                None,
                "first",
                &[Observation {
                    harness_id: harness.clone(),
                    event_id: "native".into(),
                    revision: 0,
                    identity: identity.clone(),
                    observed_at_unix_ns: 1,
                    kind: KernelObservationKind::SessionStarted,
                }],
            )
            .unwrap();
        let trace = |index: usize| {
            let span_id = format!("{index:016x}");
            Observation {
                harness_id: trace_source.clone(),
                event_id: format!("otlp:{trace_id}:{span_id}"),
                revision: 0,
                identity: identity.clone(),
                observed_at_unix_ns: 3,
                kind: KernelObservationKind::TraceSpan(TraceSpan {
                    trace_id: trace_id.into(),
                    span_id,
                    parent_span_id: None,
                    target_source_id: Some(harness.as_str().into()),
                    target_operation_id: None,
                    started_at_unix_ns: 2,
                    status: TraceStatus::Ok,
                }),
            }
        };
        let mut previous = None;
        for start in (1..=MAX_TRACE_SCOPE_ROWS).step_by(MAX_BATCH_EVENTS) {
            let end = (start + MAX_BATCH_EVENTS - 1).min(MAX_TRACE_SCOPE_ROWS);
            let rows = (start..=end).map(&trace).collect::<Vec<_>>();
            let cursor = end.to_string();
            store
                .commit(trace_source.as_str(), previous.as_deref(), &cursor, &rows)
                .unwrap();
            previous = Some(cursor);
        }
        let mut query = TraceTurnQuery {
            harness_id: harness.as_str().into(),
            session_id: "session-1".into(),
            trace_source_id: trace_source.as_str().into(),
            trace_id: trace_id.into(),
            snapshot_id: None,
        };
        let resolved = store.resolve_trace_turn(&query).unwrap().unwrap();
        assert_eq!(resolved.turn_id, "turn-1");
        store
            .commit(
                trace_source.as_str(),
                previous.as_deref(),
                "overflow",
                &[trace(MAX_TRACE_SCOPE_ROWS + 1)],
            )
            .unwrap();
        assert!(matches!(
            store.resolve_trace_turn(&query),
            Err(StoreError::TraceScopeTooLarge {
                max_rows: MAX_TRACE_SCOPE_ROWS
            })
        ));
        query.snapshot_id = Some(resolved.snapshot_id);
        assert_eq!(store.resolve_trace_turn(&query).unwrap(), Some(resolved));
    }
}
