//! The broker's metadata-only interface for a dedicated reader principal.

use crate::native_admin::Role;
use crate::native_read::{self, ReadFailure};
use crate::native_store::{NativeSelection, verify_read_binding};
use agenturm_kernel::{ExtensionId, ObservationPage, ObservationQuery};
use agenturm_kernel::{ObservationKind, is_host_source};
use agenturm_store::{
    ResolvedTraceReview, SelectedEvalPage, SelectedEvalQuery, SessionPage, SessionQuery, Store,
    TraceReviewQuery, WorkspaceAnchor, WorkspaceHistory,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReaderHistoryRequest {
    Identity,
    Sessions {
        query: SessionQuery,
    },
    History {
        query: ObservationQuery,
        selection: NativeSelection,
    },
    HostHistory {
        query: ObservationQuery,
    },
    TraceReview {
        query: TraceReviewQuery,
    },
    SelectedEvals {
        source_id: ExtensionId,
        query: SelectedEvalQuery,
    },
    CheckAnchor {
        anchor: WorkspaceAnchor,
    },
    CheckEvent {
        anchor: WorkspaceAnchor,
        event_id: String,
        revision: u32,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ReaderHistoryReply {
    Identity(String),
    Sessions(SessionPage),
    History(ObservationPage),
    HostHistory(ObservationPage),
    TraceReview(Option<ResolvedTraceReview>),
    SelectedEvals(Option<SelectedEvalPage>),
    Contains(bool),
}

pub(crate) fn dispatch(
    store: &Store,
    allowed: &BTreeSet<ExtensionId>,
    role: Role,
    expected_identity: Option<&str>,
    request: ReaderHistoryRequest,
) -> Result<ReaderHistoryReply, ReadFailure> {
    if role != Role::Reader {
        return Err(ReadFailure::Unauthorized);
    }
    match expected_identity {
        Some(expected) => verify_read_binding(store, expected)?,
        None if !matches!(request, ReaderHistoryRequest::Identity) => {
            return Err(ReadFailure::InvalidRequest);
        }
        None => {}
    }
    let host_history = matches!(&request, ReaderHistoryRequest::HostHistory { .. });
    let allowed = &allowed
        .iter()
        .filter(|source| is_host_source(source.as_str()) == host_history)
        .cloned()
        .collect();
    match request {
        ReaderHistoryRequest::Identity => store
            .history_identity()
            .map(ReaderHistoryReply::Identity)
            .map_err(|_| ReadFailure::Unavailable),
        ReaderHistoryRequest::Sessions { query } => {
            native_read::query_sessions(store, allowed, &query).map(ReaderHistoryReply::Sessions)
        }
        ReaderHistoryRequest::History { query, selection } => {
            native_read::query_history(store, allowed, &query, selection.as_history())
                .and_then(|page| {
                    if page
                        .observations
                        .iter()
                        .any(|row| matches!(row.kind, ObservationKind::HostSample(_)))
                    {
                        return Err(ReadFailure::Unavailable);
                    }
                    Ok(page)
                })
                .map(ReaderHistoryReply::History)
        }
        ReaderHistoryRequest::HostHistory { query } => {
            native_read::query_host_history(store, allowed, &query)
                .map(ReaderHistoryReply::HostHistory)
        }
        ReaderHistoryRequest::TraceReview { query } => {
            native_read::review_trace_turn(store, allowed, query)
                .map(ReaderHistoryReply::TraceReview)
        }
        ReaderHistoryRequest::SelectedEvals { source_id, query } => {
            native_read::query_selected_evals(store, allowed, &source_id, &query)
                .map(ReaderHistoryReply::SelectedEvals)
        }
        ReaderHistoryRequest::CheckAnchor { anchor } => {
            allow_anchor(allowed, &anchor)?;
            store
                .contains_anchor(&anchor)
                .map(ReaderHistoryReply::Contains)
                .map_err(|_| ReadFailure::Unavailable)
        }
        ReaderHistoryRequest::CheckEvent {
            anchor,
            event_id,
            revision,
        } => {
            allow_anchor(allowed, &anchor)?;
            store
                .contains_event(&anchor, &event_id, revision)
                .map(ReaderHistoryReply::Contains)
                .map_err(|_| ReadFailure::Unavailable)
        }
    }
}

fn allow_anchor(
    allowed: &BTreeSet<ExtensionId>,
    anchor: &WorkspaceAnchor,
) -> Result<(), ReadFailure> {
    if allowed.contains(&anchor.harness_id) {
        Ok(())
    } else {
        Err(ReadFailure::Unauthorized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_does_not_accept_collector_or_administrative_principals() {
        let store = Store::open(":memory:").unwrap();
        for role in [Role::Collector, Role::GrantAdministrator, Role::Maintenance] {
            assert!(matches!(
                dispatch(
                    &store,
                    &BTreeSet::new(),
                    role,
                    None,
                    ReaderHistoryRequest::Identity
                ),
                Err(ReadFailure::Unauthorized)
            ));
        }
    }
}
