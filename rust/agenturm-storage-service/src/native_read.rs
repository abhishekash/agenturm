//! Scoped native history reads for an authenticated broker client.

use agenturm_kernel::ReadSnapshot;
use agenturm_kernel::{
    ExtensionId, MAX_DURABLE_SNAPSHOT_OBSERVATIONS, ObservationPage, ObservationQuery,
};
use agenturm_store::{
    ResolvedTraceReview, SessionPage, SessionQuery, Store, StoreError, TraceReviewQuery,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Fits the version 1 frame even when lawful observation metadata is large.
const MAX_NATIVE_HISTORY_ROWS: usize = 2_048;

#[derive(Clone, Copy, Debug)]
pub enum HistorySelection<'a> {
    Observations,
    Turn {
        turn_id: &'a str,
        worker_id: Option<&'a str>,
    },
    AllWorkers {
        turn_id: &'a str,
    },
    LinkedTraces,
    LinkedTurnTraces {
        turn_id: &'a str,
        worker_id: Option<&'a str>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadFailure {
    Unauthorized,
    InvalidRequest,
    HistoryReplaced,
    SnapshotReplaced,
    ExpiredSnapshot { minimum_snapshot_id: ReadSnapshot },
    TraceScopeTooLarge { max_rows: usize },
    EvalTraceNotRetained,
    Unavailable,
}

pub fn query_history(
    store: &Store,
    allowed_sources: &BTreeSet<ExtensionId>,
    query: &ObservationQuery,
    selection: HistorySelection<'_>,
) -> Result<ObservationPage, ReadFailure> {
    let include_linked_traces = matches!(
        selection,
        HistorySelection::LinkedTraces | HistorySelection::LinkedTurnTraces { .. }
    );
    let query = bounded_history_query(allowed_sources, query, include_linked_traces)?;
    match selection {
        HistorySelection::Observations => {
            store.query_observations_from_sources(&query, allowed_sources)
        }
        HistorySelection::Turn { turn_id, worker_id } => {
            store.query_worker_turn_observations(&query, turn_id, worker_id)
        }
        HistorySelection::AllWorkers { turn_id } => store.query_turn_observations(&query, turn_id),
        HistorySelection::LinkedTraces => {
            store.query_linked_traces_from_sources(&query, allowed_sources)
        }
        HistorySelection::LinkedTurnTraces { turn_id, worker_id } => {
            store.query_linked_turn_traces_from_sources(&query, turn_id, worker_id, allowed_sources)
        }
    }
    .map_err(failure)
}

fn bounded_history_query(
    allowed_sources: &BTreeSet<ExtensionId>,
    query: &ObservationQuery,
    include_linked_traces: bool,
) -> Result<ObservationQuery, ReadFailure> {
    authorize(allowed_sources, query.harness_id.as_deref())?;
    authorize(
        allowed_sources,
        query
            .before
            .as_ref()
            .map(|position| position.harness_id.as_str()),
    )?;
    if !(1..=MAX_DURABLE_SNAPSHOT_OBSERVATIONS).contains(&query.limit) {
        return Err(ReadFailure::InvalidRequest);
    }
    if let Some(before) = &query.before
        && !include_linked_traces
        && query
            .harness_id
            .as_ref()
            .is_some_and(|source| *source != before.harness_id)
    {
        return Err(ReadFailure::InvalidRequest);
    }
    let mut query = query.clone();
    query.limit = query.limit.min(MAX_NATIVE_HISTORY_ROWS);
    Ok(query)
}

pub fn review_trace_turn(
    store: &Store,
    allowed_sources: &BTreeSet<ExtensionId>,
    query: TraceReviewQuery,
) -> Result<Option<ResolvedTraceReview>, ReadFailure> {
    authorize(allowed_sources, Some(&query.trace_source_id))?;
    let query = TraceReviewQuery {
        page: bounded_history_query(allowed_sources, &query.page, query.linked_traces)?,
        ..query
    };
    store
        .review_trace_turn(&query, Some(allowed_sources))
        .map_err(failure)
}

pub fn query_sessions(
    store: &Store,
    allowed_sources: &BTreeSet<ExtensionId>,
    query: &SessionQuery,
) -> Result<SessionPage, ReadFailure> {
    authorize(allowed_sources, query.harness_id.as_deref())?;
    authorize(
        allowed_sources,
        query
            .before
            .as_ref()
            .map(|position| position.harness_id.as_str()),
    )?;
    store
        .query_sessions_from_sources(query, allowed_sources)
        .map_err(failure)
}

pub fn query_host_history(
    store: &Store,
    allowed_sources: &BTreeSet<ExtensionId>,
    query: &ObservationQuery,
) -> Result<ObservationPage, ReadFailure> {
    authorize(allowed_sources, query.harness_id.as_deref())?;
    authorize(
        allowed_sources,
        query.before.as_ref().map(|row| row.harness_id.as_str()),
    )?;
    store
        .query_host_history(query, Some(allowed_sources))
        .map_err(failure)
}

fn authorize(
    allowed_sources: &BTreeSet<ExtensionId>,
    source_id: Option<&str>,
) -> Result<(), ReadFailure> {
    let Some(source_id) = source_id else {
        return Ok(());
    };
    let source = ExtensionId::parse(source_id).map_err(|_| ReadFailure::InvalidRequest)?;
    if !allowed_sources.contains(&source) {
        return Err(ReadFailure::Unauthorized);
    }
    Ok(())
}

pub(crate) fn failure(error: StoreError) -> ReadFailure {
    match error {
        StoreError::Invalid(_) => ReadFailure::InvalidRequest,
        StoreError::SnapshotReplaced => ReadFailure::SnapshotReplaced,
        StoreError::ExpiredHistorySnapshot {
            minimum_snapshot_id,
        } => ReadFailure::ExpiredSnapshot {
            minimum_snapshot_id,
        },
        StoreError::TraceScopeTooLarge { max_rows } => ReadFailure::TraceScopeTooLarge { max_rows },
        StoreError::EvalTraceNotRetained => ReadFailure::EvalTraceNotRetained,
        _ => ReadFailure::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::ObservationPosition;
    use agenturm_store::{
        ObservationBatch, ObservationIdentity, ObservationKind, StoredObservation, StoredScope,
    };

    #[test]
    fn trace_turn_resolution_requires_both_sources_on_the_allowlist() {
        let store = Store::open(":memory:").unwrap();
        let source = ExtensionId::parse("harness.authorized").unwrap();
        let trace_source = ExtensionId::parse("otlp.authorized").unwrap();
        let query = TraceReviewQuery {
            page: ObservationQuery {
                harness_id: Some(source.as_str().into()),
                session_id: Some("session-1".into()),
                ..ObservationQuery::latest(1)
            },
            trace_source_id: trace_source.as_str().into(),
            trace_id: "11111111111111111111111111111111".into(),
            expected_worker_id: None,
            linked_traces: false,
        };
        for one_source in [source.clone(), trace_source.clone()] {
            assert_eq!(
                review_trace_turn(&store, &BTreeSet::from([one_source]), query.clone()),
                Err(ReadFailure::Unauthorized),
            );
        }
        assert_eq!(
            review_trace_turn(&store, &BTreeSet::from([source, trace_source]), query),
            Ok(None),
        );
    }

    #[test]
    fn scoped_queries_preserve_the_snapshot_and_report_expiry() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "agenturm-native-read-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let mut store = Store::open_collector(directory.join("history.sqlite")).unwrap();
        let source = "harness.authorized";
        let allowed = BTreeSet::from([ExtensionId::parse(source).unwrap()]);
        let mut query = ObservationQuery::latest(1);
        assert_eq!(
            query_history(
                &store,
                &allowed,
                &query,
                HistorySelection::AllWorkers { turn_id: "turn-1" }
            )
            .unwrap_err(),
            ReadFailure::InvalidRequest
        );
        query.harness_id = Some("harness.denied".into());
        assert_eq!(
            query_history(&store, &allowed, &query, HistorySelection::Observations).unwrap_err(),
            ReadFailure::Unauthorized
        );

        let commit = |store: &mut Store, previous, next: &str, timestamp| {
            let batch = ObservationBatch::new_deterministic(
                source,
                previous,
                next,
                vec![StoredObservation {
                    event_id: next.into(),
                    revision: 0,
                    identity: ObservationIdentity {
                        scope: StoredScope::Session,
                        session_id: Some("session-1".into()),
                        worker_id: None,
                        turn_id: Some("turn-1".into()),
                        operation_id: None,
                    },
                    observed_at_unix_ns: timestamp,
                    kind: ObservationKind::SessionStarted,
                }],
            )
            .unwrap();
            store.commit_batch(&batch).unwrap();
        };
        query.harness_id = Some(source.into());
        query.session_id = Some("session-1".into());
        let selection = HistorySelection::Turn {
            turn_id: "turn-1",
            worker_id: None,
        };
        commit(&mut store, None, "cursor-1", 1);
        let initial = query_history(&store, &allowed, &query, selection).unwrap();
        query.snapshot_id = Some(initial.snapshot_id);

        commit(&mut store, Some("cursor-1".into()), "cursor-2", 2);
        let pinned = query_history(&store, &allowed, &query, selection).unwrap();
        assert_eq!(initial, pinned);
        let sessions = query_sessions(
            &store,
            &allowed,
            &SessionQuery {
                harness_id: query.harness_id.clone(),
                snapshot_id: query.snapshot_id,
                ..SessionQuery::latest(1)
            },
        )
        .unwrap();
        assert_eq!(sessions.snapshot_id, initial.snapshot_id);
        assert_eq!(sessions.sessions[0].observed_at_unix_ns, 1);

        store.prune_observations_before(2).unwrap();
        assert!(matches!(
            query_history(&store, &allowed, &query, selection),
            Err(ReadFailure::ExpiredSnapshot { .. })
        ));
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unfiltered_queries_apply_authorized_sources_before_watermark_paging() {
        let mut store = Store::open(":memory:").unwrap();
        let allowed = ["harness.a", "harness.b"]
            .map(|source| ExtensionId::parse(source).unwrap())
            .into_iter()
            .collect();
        let one = |event: &str, time, worker: Option<&str>| StoredObservation {
            event_id: event.into(),
            revision: 0,
            identity: ObservationIdentity {
                scope: StoredScope::Session,
                session_id: Some("same-session".into()),
                worker_id: worker.map(str::to_owned),
                turn_id: Some("same-turn".into()),
                operation_id: None,
            },
            observed_at_unix_ns: time,
            kind: ObservationKind::SessionStarted,
        };
        let mut commit = |source, cursor, events| {
            let batch = ObservationBatch::new_deterministic(source, None, cursor, events).unwrap();
            store.commit_batch(&batch).unwrap();
        };
        commit(
            "harness.a",
            "one",
            vec![one("a", 1, None), one("worker", 2, Some("child"))],
        );
        commit("harness.b", "one", vec![one("b", 2, None)]);
        commit("harness.denied", "one", vec![one("private", 3, None)]);

        let query = ObservationQuery::latest(1);
        let page = query_history(&store, &allowed, &query, HistorySelection::Observations).unwrap();
        assert!(page.has_more);
        assert_eq!(page.observations[0].event_id, "b");
        let before = &page.observations[0];
        let continuation = ObservationQuery {
            snapshot_id: Some(page.snapshot_id),
            before: Some(ObservationPosition {
                observed_at_unix_ns: before.observed_at_unix_ns,
                harness_id: before.harness_id.as_str().into(),
                event_id: before.event_id.clone(),
                revision: before.revision,
                snapshot_id: page.snapshot_id,
            }),
            ..query
        };
        let earlier = query_history(
            &store,
            &allowed,
            &continuation,
            HistorySelection::Observations,
        )
        .unwrap();
        assert_eq!(earlier.observations[0].event_id, "worker");
        assert_eq!(earlier.snapshot_id, page.snapshot_id);
        let sessions = query_sessions(&store, &allowed, &SessionQuery::latest(2)).unwrap();
        let sources: Vec<_> = sessions
            .sessions
            .iter()
            .map(|row| row.harness_id.as_str())
            .collect();
        assert_eq!(sources, ["harness.b", "harness.a"]);
        assert!(sessions.next_before.is_none());

        let turn = ObservationQuery {
            harness_id: Some("harness.a".into()),
            session_id: Some("same-session".into()),
            ..ObservationQuery::latest(10)
        };
        for (worker_id, expected) in [(None, "a"), (Some("child"), "worker")] {
            let page = query_history(
                &store,
                &allowed,
                &turn,
                HistorySelection::Turn {
                    turn_id: "same-turn",
                    worker_id,
                },
            )
            .unwrap();
            assert_eq!(page.observations.len(), 1);
            assert_eq!(page.observations[0].event_id, expected);
        }
        let all = query_history(
            &store,
            &allowed,
            &turn,
            HistorySelection::AllWorkers {
                turn_id: "same-turn",
            },
        )
        .unwrap();
        assert_eq!(all.observations.len(), 2);
    }
}
