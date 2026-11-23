//! Resolve only explicitly attributed, retained OTLP execution scope.

use super::*;
use agenturm_kernel::ReadSnapshot;

const MAX_TRACE_SCOPE_ROWS: usize = 4096;

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

impl Store {
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
            query.linked_traces,
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
            || query.trace_id.len() != 32
            || !query
                .trace_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || !query.trace_id.bytes().any(|byte| byte != b'0')
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

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{ObservationIdentity, TraceSpan, TraceStatus};

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
