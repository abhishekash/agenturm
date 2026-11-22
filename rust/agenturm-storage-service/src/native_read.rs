//! Scoped native history reads for an authenticated broker client.

use agenturm_kernel::{ExtensionId, ObservationPage, ObservationQuery};
use agenturm_store::{SessionPage, SessionQuery, Store, StoreError};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug)]
pub enum HistorySelection<'a> {
    Observations,
    Turn {
        turn_id: &'a str,
        worker_id: Option<&'a str>,
    },
    LinkedTraces,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadFailure {
    Unauthorized,
    InvalidRequest,
    ExpiredSnapshot { minimum_snapshot_id: u64 },
    Unavailable,
}

pub fn query_history(
    store: &Store,
    allowed_sources: &BTreeSet<ExtensionId>,
    query: &ObservationQuery,
    selection: HistorySelection<'_>,
) -> Result<ObservationPage, ReadFailure> {
    authorize(allowed_sources, query.harness_id.as_deref())?;
    match selection {
        HistorySelection::Observations => store.query_observations(query),
        HistorySelection::Turn { turn_id, worker_id } => {
            store.query_worker_turn_observations(query, turn_id, worker_id)
        }
        HistorySelection::LinkedTraces => store.query_linked_traces(query),
    }
    .map_err(failure)
}

pub fn query_sessions(
    store: &Store,
    allowed_sources: &BTreeSet<ExtensionId>,
    query: &SessionQuery,
) -> Result<SessionPage, ReadFailure> {
    authorize(allowed_sources, query.harness_id.as_deref())?;
    store.query_sessions(query).map_err(failure)
}

fn authorize(
    allowed_sources: &BTreeSet<ExtensionId>,
    source_id: Option<&str>,
) -> Result<(), ReadFailure> {
    let source_id = source_id.ok_or(ReadFailure::InvalidRequest)?;
    let source = ExtensionId::parse(source_id).map_err(|_| ReadFailure::InvalidRequest)?;
    if !allowed_sources.contains(&source) {
        return Err(ReadFailure::Unauthorized);
    }
    Ok(())
}

fn failure(error: StoreError) -> ReadFailure {
    match error {
        StoreError::Invalid(_) => ReadFailure::InvalidRequest,
        StoreError::ExpiredHistorySnapshot {
            minimum_snapshot_id,
        } => ReadFailure::ExpiredSnapshot {
            minimum_snapshot_id,
        },
        _ => ReadFailure::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_store::{
        ObservationBatch, ObservationIdentity, ObservationKind, StoredObservation, StoredScope,
    };

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
            query_history(&store, &allowed, &query, HistorySelection::Observations).unwrap_err(),
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
}
