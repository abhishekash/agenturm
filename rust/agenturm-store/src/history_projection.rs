//! Closed observation metadata filters sharing retained history's cursor and watermark.

use super::*;
use agenturm_kernel::retained_history::HistoryProjection;

impl Store {
    pub fn query_projected_history(
        &self,
        query: &ObservationQuery,
        projection: &HistoryProjection,
        turn_id: Option<&str>,
        worker_filter: Option<Option<&str>>,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
    ) -> Result<ObservationPage> {
        validate_projection(query, projection)?;
        if worker_filter.is_some() && turn_id.is_none() {
            return Err(StoreError::Invalid("worker history requires a native turn"));
        }
        let transaction = self.connection.unchecked_transaction()?;
        Self::query_history_in(
            &transaction,
            query,
            turn_id,
            worker_filter,
            TraceLink::Projected(projection),
            allowed_sources,
            false,
        )
    }
}

fn validate_projection(query: &ObservationQuery, projection: &HistoryProjection) -> Result<()> {
    match projection {
        HistoryProjection::Traces { trace_id: Some(id) }
        | HistoryProjection::Progress { trace_id: Some(id) } => {
            if !linked_trace::valid_trace_id(id)
                || !query
                    .harness_id
                    .as_deref()
                    .is_some_and(|source| source.starts_with("otlp."))
            {
                return Err(StoreError::Invalid(
                    "exact trace history requires a telemetry source and trace",
                ));
            }
        }
        HistoryProjection::Mcp { query: Some(term) }
            if !HistoryProjection::valid_mcp_query(term) =>
        {
            return Err(StoreError::Invalid("invalid MCP history search term"));
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn selection(projection: &HistoryProjection) -> (Option<&'static str>, Option<&str>) {
    let (name, filter) = match projection {
        HistoryProjection::Activity => ("activity", None),
        HistoryProjection::Traces { trace_id } => ("traces", trace_id.as_deref()),
        HistoryProjection::Progress { trace_id } => ("progress", trace_id.as_deref()),
        HistoryProjection::Mcp { query } => ("mcp", query.as_deref()),
        HistoryProjection::Usage => ("usage", None),
        HistoryProjection::Relationships => ("relationships", None),
    };
    (Some(name), filter)
}

pub(super) fn require_cursor(
    connection: &Connection,
    query: &ObservationQuery,
    projection: &HistoryProjection,
    turn_id: Option<&str>,
    worker_filter: Option<Option<&str>>,
    allowed_sources: Option<&BTreeSet<ExtensionId>>,
    receipt_watermark: i64,
) -> Result<()> {
    const INVALID: StoreError =
        StoreError::Invalid("history cursor does not match the projected scope");
    let Some(position) = query.before.as_ref() else {
        return Ok(());
    };
    if agenturm_kernel::is_host_source(&position.harness_id)
        || query
            .harness_id
            .as_ref()
            .is_some_and(|id| id != &position.harness_id)
        || allowed_sources
            .is_some_and(|sources| !sources.iter().any(|id| id.as_str() == position.harness_id))
    {
        return Err(INVALID);
    }
    let kind_json = connection.query_row(
        "SELECT observation.kind_json
           FROM agenturm_observations AS observation
           JOIN agenturm_history_batches AS history_batch ON history_batch.request_id=observation.request_id
          WHERE observation.source_id=?1 AND observation.event_id=?2 AND observation.revision=?3
            AND observation.observed_at_unix_ns=?4 AND history_batch.sequence<=?5
            AND (?6 IS NULL OR observation.session_id=?6)
            AND (?7 IS NULL OR observation.turn_id=?7)
            AND (?8=0 OR observation.worker_id IS ?9)",
        params![
            position.harness_id,
            position.event_id,
            i64::from(position.revision),
            position.observed_at_unix_ns as i64,
            receipt_watermark,
            query.session_id,
            turn_id,
            worker_filter.is_some(),
            worker_filter.flatten(),
        ],
        |row| row.get::<_, String>(0),
    ).optional()?.ok_or(INVALID)?;
    let kind = serde_json::from_str::<ObservationKind>(&kind_json)?;
    if !matches_projection(&kind, projection) {
        return Err(INVALID);
    }
    Ok(())
}

fn matches_projection(kind: &ObservationKind, projection: &HistoryProjection) -> bool {
    match (kind, projection) {
        (ObservationKind::HostSample(_), _) => false,
        (ObservationKind::TraceSpanSummary(_), HistoryProjection::Activity) => false,
        (_, HistoryProjection::Activity) => true,
        (ObservationKind::TraceSpan(span), HistoryProjection::Traces { trace_id }) => {
            trace_id.as_ref().is_none_or(|id| id == &span.trace_id)
        }
        (ObservationKind::TraceProgress(_), HistoryProjection::Traces { trace_id: None }) => true,
        (ObservationKind::TraceProgress(progress), HistoryProjection::Progress { trace_id }) => {
            trace_id.as_ref().is_none_or(|id| id == &progress.trace_id)
        }
        (
            ObservationKind::McpCall {
                server,
                tool,
                method,
                result,
                ..
            },
            HistoryProjection::Mcp { query },
        ) => query.as_ref().is_none_or(|query| {
            let folded = query.to_ascii_lowercase();
            [
                server.as_deref(),
                tool.as_deref(),
                Some(method.as_str()),
                Some(result.as_str()),
            ]
            .into_iter()
            .flatten()
            .any(|label| label.to_ascii_lowercase().contains(&folded))
        }),
        (
            ObservationKind::Usage { .. }
            | ObservationKind::CreditUsage { .. }
            | ObservationKind::ReportedCost(_)
            | ObservationKind::SpanTokenUsage(_),
            HistoryProjection::Usage,
        ) => true,
        (ObservationKind::Relationship { .. }, HistoryProjection::Relationships) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::telemetry_summary::TraceSpanSummary;
    use agenturm_kernel::{McpMethod, TraceSpan, TraceStatus};

    fn commit(
        store: &mut Store,
        source: &ExtensionId,
        rows: &[Observation],
        previous: Option<&str>,
        cursor: &str,
    ) {
        store
            .commit(source.as_str(), previous, cursor, rows)
            .unwrap();
    }

    fn event(
        source: &ExtensionId,
        id: &str,
        stamp: u64,
        kind: KernelObservationKind,
    ) -> Observation {
        Observation {
            harness_id: source.clone(),
            event_id: id.into(),
            revision: 0,
            identity: agenturm_kernel::ObservationIdentity::session("fixture"),
            observed_at_unix_ns: stamp,
            kind,
        }
    }

    fn span(source: &ExtensionId, trace_id: &str, span_id: &str, stamp: u64) -> [Observation; 2] {
        let id = format!("otlp:{trace_id}:{span_id}");
        [
            event(
                source,
                &id,
                stamp,
                KernelObservationKind::TraceSpan(TraceSpan {
                    trace_id: trace_id.into(),
                    span_id: span_id.into(),
                    parent_span_id: None,
                    target_source_id: None,
                    target_operation_id: None,
                    started_at_unix_ns: 1,
                    status: TraceStatus::Ok,
                }),
            ),
            event(
                source,
                &format!("{id}:summary"),
                stamp,
                KernelObservationKind::TraceSpanSummary(TraceSpanSummary {
                    trace_id: trace_id.into(),
                    span_id: span_id.into(),
                    agent_run: false,
                    reported_error: false,
                }),
            ),
        ]
    }

    #[test]
    fn exact_trace_returns_limit_one_span_through_later_summaries_and_foreign_spans() {
        let mut store = Store::open(":memory:").unwrap();
        let source = ExtensionId::parse("otlp.fixture").unwrap();
        let denied = ExtensionId::parse("otlp.denied").unwrap();
        let trace = "1".repeat(32);
        let other_trace = "2".repeat(32);
        let [older, older_summary] = span(&source, &trace, &"1".repeat(16), 19);
        let [newer, newer_summary] = span(&source, &trace, &"2".repeat(16), 20);
        let mut rows = vec![older.clone(), older_summary, newer.clone(), newer_summary];
        for stamp in 21..=30 {
            rows.extend(span(&source, &other_trace, &format!("{stamp:016x}"), stamp));
        }
        commit(&mut store, &source, &rows, None, "initial");
        commit(
            &mut store,
            &denied,
            &span(&denied, &trace, &"4".repeat(16), 40),
            None,
            "initial",
        );
        let query = ObservationQuery {
            harness_id: Some(source.as_str().into()),
            ..ObservationQuery::latest(1)
        };
        let projection = HistoryProjection::Traces {
            trace_id: Some(trace.clone()),
        };
        let allowed = BTreeSet::from([source.clone()]);
        let page = store
            .query_projected_history(&query, &projection, None, None, Some(&allowed))
            .unwrap();
        assert_eq!(page.observations, vec![newer]);
        assert!(page.has_more);
        let next_query = ObservationQuery {
            before: page.older_position(),
            snapshot_id: Some(page.snapshot_id),
            ..query.clone()
        };
        commit(
            &mut store,
            &source,
            &span(&source, &trace, &"5".repeat(16), 18),
            Some("initial"),
            "later",
        );
        let next = store
            .query_projected_history(&next_query, &projection, None, None, Some(&allowed))
            .unwrap();
        assert_eq!(next.observations, vec![older]);
        assert_eq!(next.snapshot_id, page.snapshot_id);
        assert!(!next.has_more);

        let other = HistoryProjection::Traces {
            trace_id: Some(other_trace),
        };
        assert!(matches!(
            store.query_projected_history(&next_query, &other, None, None, Some(&allowed)),
            Err(StoreError::Invalid(
                "history cursor does not match the projected scope"
            ))
        ));
        let wrong_epoch = agenturm_kernel::ReadSnapshot::new(
            page.snapshot_id.watermark(),
            Some("a".repeat(32).parse().unwrap()),
        );
        let mut replaced = next_query.clone();
        replaced.before.as_mut().unwrap().snapshot_id = wrong_epoch;
        replaced.snapshot_id = Some(wrong_epoch);
        assert!(matches!(
            store.query_projected_history(&replaced, &projection, None, None, Some(&allowed)),
            Err(StoreError::SnapshotReplaced)
        ));

        let activity = store
            .query_projected_history(
                &query,
                &HistoryProjection::Activity,
                None,
                None,
                Some(&allowed),
            )
            .unwrap();
        assert!(matches!(
            activity.observations[0].kind,
            KernelObservationKind::TraceSpan(_)
        ));
    }

    fn call(
        source: &ExtensionId,
        id: &str,
        stamp: u64,
        server: &str,
        tool: &str,
        result: &str,
        worker: Option<&str>,
    ) -> Observation {
        let mut row = event(
            source,
            id,
            stamp,
            KernelObservationKind::McpCall {
                server: Some(server.into()),
                tool: Some(tool.into()),
                method: McpMethod::ToolsCall,
                duration_ns: None,
                result: result.into(),
            },
        );
        row.identity.turn_id = Some("turn".into());
        row.identity.worker_id = worker.map(str::to_owned);
        row.identity.operation_id = Some(id.into());
        row
    }

    #[test]
    fn mcp_literal_search_pages_only_matches_within_exact_worker_scope_and_acl() {
        let mut store = Store::open(":memory:").unwrap();
        let allowed_source = ExtensionId::parse("harness.fixture").unwrap();
        let denied_source = ExtensionId::parse("harness.denied").unwrap();
        let older = call(&allowed_source, "older", 10, "a_B", "read", "failed", None);
        let newer = call(&allowed_source, "newer", 20, "read", "a_B", "denied", None);
        let unrelated = call(
            &allowed_source,
            "unrelated",
            30,
            "AxB",
            "write",
            "succeeded",
            None,
        );
        let sibling = call(
            &allowed_source,
            "sibling",
            40,
            "a_b",
            "read",
            "succeeded",
            Some("worker"),
        );
        let mut other_session = call(
            &allowed_source,
            "other-session",
            50,
            "a_b",
            "read",
            "succeeded",
            None,
        );
        other_session.identity.session_id = Some("second".into());
        let denied = call(
            &denied_source,
            "denied",
            60,
            "a_b",
            "read",
            "succeeded",
            None,
        );
        commit(
            &mut store,
            &allowed_source,
            &[
                older.clone(),
                newer.clone(),
                unrelated.clone(),
                sibling.clone(),
                other_session,
            ],
            None,
            "initial",
        );
        commit(&mut store, &denied_source, &[denied], None, "initial");
        let allowed = BTreeSet::from([allowed_source.clone()]);
        let query = ObservationQuery {
            harness_id: Some(allowed_source.as_str().into()),
            session_id: Some("fixture".into()),
            ..ObservationQuery::latest(1)
        };
        let projection = |query: &str| HistoryProjection::Mcp {
            query: Some(query.into()),
        };
        let search = |query: &ObservationQuery, projection, worker| {
            store.query_projected_history(query, &projection, Some("turn"), worker, Some(&allowed))
        };
        let page = search(&query, projection("A_B"), Some(None)).unwrap();
        assert_eq!(page.observations, vec![newer]);
        assert!(page.has_more);
        let next_query = ObservationQuery {
            before: page.older_position(),
            snapshot_id: Some(page.snapshot_id),
            ..query.clone()
        };
        let next = search(&next_query, projection("A_B"), Some(None)).unwrap();
        assert_eq!(next.observations, vec![older]);
        assert!(!next.has_more);
        assert!(search(&next_query, projection("AxB"), Some(None)).is_err());
        assert!(search(&next_query, projection("A_B"), Some(Some("worker"))).is_err());
        assert_eq!(
            search(&query, projection("A_B"), None)
                .unwrap()
                .observations,
            vec![sibling.clone()]
        );
        assert_eq!(
            search(&query, projection("A_B"), Some(Some("worker")))
                .unwrap()
                .observations,
            vec![sibling]
        );
        assert_eq!(
            search(&query, projection("TOOLS/CALL"), Some(None))
                .unwrap()
                .observations,
            vec![unrelated]
        );
        assert_eq!(
            search(&query, projection("FAILED"), Some(None))
                .unwrap()
                .observations[0]
                .event_id,
            "older"
        );
        assert!(
            search(&query, projection("%"), None)
                .unwrap()
                .observations
                .is_empty()
        );

        // An all-source page still binds the broker's allowlist before its limit.
        let across = store
            .query_projected_history(
                &ObservationQuery::latest(1),
                &projection("A_B"),
                None,
                None,
                Some(&allowed),
            )
            .unwrap();
        assert_eq!(across.observations[0].harness_id, allowed_source);
        assert_eq!(across.observations[0].event_id, "other-session");
    }
}
