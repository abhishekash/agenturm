//! Native history admission operations for an authenticated collector.
//!
//! The socket layer must authenticate both peers and establish the source
//! allowlist before invoking these operations.

use agenturm_kernel::ExtensionId;
use agenturm_store::{CommitOutcome, ObservationBatch, Store, StoreError};
#[cfg(target_os = "linux")]
use agenturm_store::{TelemetryBinding, TelemetryPayloadRecord};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdmissionRequest {
    SourceCursor { source_id: String },
    Admit { batch: ObservationBatch },
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdmissionReply {
    Cursor { cursor: Option<String> },
    Committed { inserted: usize },
    Replay,
    PayloadPolicyEnabled,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionFailure {
    Unauthorized,
    InvalidRequest,
    Conflict,
    StaleCursor,
    Unavailable,
}

pub fn dispatch(
    store: &mut Store,
    allowed_sources: &BTreeSet<ExtensionId>,
    request: AdmissionRequest,
) -> Result<AdmissionReply, AdmissionFailure> {
    let source_id = match &request {
        AdmissionRequest::SourceCursor { source_id } => source_id,
        AdmissionRequest::Admit { batch } => &batch.source_id,
    };
    let source =
        ExtensionId::parse(source_id.clone()).map_err(|_| AdmissionFailure::InvalidRequest)?;
    if !allowed_sources.contains(&source) {
        return Err(AdmissionFailure::Unauthorized);
    }

    match request {
        AdmissionRequest::SourceCursor { source_id } => store
            .source_cursor(&source_id)
            .map(|cursor| AdmissionReply::Cursor { cursor })
            .map_err(failure),
        AdmissionRequest::Admit { batch } => {
            let prepared = prepare(batch)?;
            committed(store.commit_batch(&prepared))
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn admit_telemetry(
    store: &mut Store,
    binding: &TelemetryBinding,
    batch: ObservationBatch,
    records: &[TelemetryPayloadRecord],
) -> Result<AdmissionReply, AdmissionFailure> {
    let prepared = prepare(batch)?;
    committed(store.commit_telemetry_batch(&prepared, binding, records))
}

fn prepare(batch: ObservationBatch) -> Result<ObservationBatch, AdmissionFailure> {
    let prepared = ObservationBatch::new_deterministic(
        batch.source_id,
        batch.expected_cursor,
        batch.next_cursor,
        batch.observations,
    )
    .map_err(failure)?;
    if batch.request_id != prepared.request_id {
        return Err(AdmissionFailure::Conflict);
    }
    Ok(prepared)
}

fn committed(
    result: agenturm_store::Result<CommitOutcome>,
) -> Result<AdmissionReply, AdmissionFailure> {
    match result.map_err(failure)? {
        CommitOutcome::Committed { inserted } => Ok(AdmissionReply::Committed { inserted }),
        CommitOutcome::Replay => Ok(AdmissionReply::Replay),
    }
}

fn failure(error: StoreError) -> AdmissionFailure {
    match error {
        StoreError::Invalid(_) => AdmissionFailure::InvalidRequest,
        StoreError::RequestConflict | StoreError::EventConflict { .. } => {
            AdmissionFailure::Conflict
        }
        StoreError::StaleCursor => AdmissionFailure::StaleCursor,
        _ => AdmissionFailure::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_store::{ObservationIdentity, ObservationKind, StoredObservation, StoredScope};

    fn batch(source: &str, expected: Option<&str>, next: &str) -> ObservationBatch {
        ObservationBatch::new_deterministic(
            source,
            expected.map(str::to_owned),
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
                observed_at_unix_ns: 1,
                kind: ObservationKind::SessionStarted,
            }],
        )
        .unwrap()
    }

    #[test]
    fn authenticated_source_admission_replays_after_reopen_and_later_commit() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "agenturm-native-admission-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("history.sqlite");
        let source = "harness.authorized";
        let allowed = BTreeSet::from([ExtensionId::parse(source).unwrap()]);
        let mut store = Store::open_collector(&path).unwrap();
        let first = batch(source, None, "cursor-1");
        let mut admit = |batch| dispatch(&mut store, &allowed, AdmissionRequest::Admit { batch });
        assert_eq!(
            admit(batch("harness.denied", None, "cursor-1")),
            Err(AdmissionFailure::Unauthorized)
        );
        let mut changed = first.clone();
        changed.next_cursor = "different".into();
        assert_eq!(admit(changed), Err(AdmissionFailure::Conflict));
        assert_eq!(
            admit(first.clone()),
            Ok(AdmissionReply::Committed { inserted: 1 })
        );
        assert_eq!(
            admit(batch(source, None, "stale")),
            Err(AdmissionFailure::StaleCursor)
        );
        assert_eq!(
            admit(batch(source, Some("cursor-1"), "cursor-2")),
            Ok(AdmissionReply::Committed { inserted: 1 })
        );
        drop(store);
        let mut store = Store::open_collector(&path).unwrap();
        assert_eq!(
            dispatch(
                &mut store,
                &allowed,
                AdmissionRequest::Admit { batch: first }
            ),
            Ok(AdmissionReply::Replay)
        );
        assert_eq!(
            dispatch(
                &mut store,
                &allowed,
                AdmissionRequest::SourceCursor {
                    source_id: source.into(),
                }
            ),
            Ok(AdmissionReply::Cursor {
                cursor: Some("cursor-2".into())
            })
        );
        assert_eq!(store.observations().unwrap().len(), 2);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
