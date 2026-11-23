//! The broker's metadata-only interface for a dedicated reader principal.

use crate::native_admin::Role;
use crate::native_read::{self, ReadFailure};
use crate::native_store::{NativeSelection, verify_read_binding};
use agenturm_kernel::retained_history::{
    MetricHistoryPage, MetricHistoryQuery, UsageHistoryPage, UsageHistoryQuery,
};
use agenturm_kernel::{ExtensionId, ObservationPage, ObservationQuery};
use agenturm_kernel::{ObservationKind, is_host_source};
use agenturm_store::{
    EvalInventory, EvalInventoryQuery, McpTraceQuery, ResolvedTraceReview, SelectedEvalPage,
    SelectedEvalQuery, SessionPage, SessionQuery, Store, TraceReviewQuery, WorkspaceAnchor,
    WorkspaceHistory,
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
    McpTraces {
        query: McpTraceQuery,
    },
    MetricHistory {
        query: MetricHistoryQuery,
    },
    UsageHistory {
        query: UsageHistoryQuery,
    },
    SelectedEvals {
        source_id: ExtensionId,
        query: SelectedEvalQuery,
    },
    EvalInventory {
        query: EvalInventoryQuery,
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
    McpTraces(Option<ObservationPage>),
    MetricHistory(MetricHistoryPage),
    UsageHistory(Box<UsageHistoryPage>),
    SelectedEvals(Option<SelectedEvalPage>),
    EvalInventory(EvalInventory),
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
        ReaderHistoryRequest::McpTraces { query } => {
            native_read::query_mcp_traces(store, allowed, query).map(ReaderHistoryReply::McpTraces)
        }
        ReaderHistoryRequest::MetricHistory { query } => {
            native_read::query_metric_history(store, allowed, &query)
                .map(ReaderHistoryReply::MetricHistory)
        }
        ReaderHistoryRequest::UsageHistory { query } => {
            native_read::query_usage_history(store, allowed, &query)
                .map(Box::new)
                .map(ReaderHistoryReply::UsageHistory)
        }
        ReaderHistoryRequest::SelectedEvals { source_id, query } => {
            native_read::query_selected_evals(store, allowed, &source_id, &query)
                .map(ReaderHistoryReply::SelectedEvals)
        }
        ReaderHistoryRequest::EvalInventory { query } => {
            native_read::query_eval_inventory(store, allowed, &query)
                .map(ReaderHistoryReply::EvalInventory)
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
    use agenturm_kernel::ReadSnapshot;
    use agenturm_kernel::retained_history::{MetricSeriesKey, MetricSeriesPosition};

    #[test]
    fn typed_history_reads_require_reader_role_current_binding_and_allowed_sources() {
        use crate::native_store::{NativeOperation, NativeReply};
        let directory =
            std::env::temp_dir().join(format!("agenturm-typed-reader-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let mut store = Store::open_collector(directory.join("history.sqlite")).unwrap();
        let identity = store.ensure_history_identity().unwrap();
        let source = ExtensionId::parse("otlp.fixture").unwrap();
        let native = ExtensionId::parse("harness.codex.fixture").unwrap();
        let denied = ExtensionId::parse("otlp.denied").unwrap();
        let allowed = BTreeSet::from([source.clone(), native.clone()]);
        let snapshot = store
            .query_observations(&ObservationQuery::latest(1))
            .unwrap()
            .snapshot_id;
        let mcp = |source_id| ReaderHistoryRequest::McpTraces {
            query: McpTraceQuery {
                source_id,
                event_id: "call-fixture".into(),
                revision: 0,
                snapshot_id: snapshot,
                limit: 1,
                before: None,
            },
        };
        let metrics = |source_id, snapshot_id, after| ReaderHistoryRequest::MetricHistory {
            query: MetricHistoryQuery::Series {
                source_id,
                snapshot_id,
                limit: 1,
                after,
            },
        };
        let usage = |source_id| ReaderHistoryRequest::UsageHistory {
            query: UsageHistoryQuery {
                source_id,
                session_id: None,
                snapshot_id: Some(snapshot),
                end_unix_ns: 7 * 86_400_000_000_000,
            },
        };
        let dispatch_reader =
            |request| dispatch(&store, &allowed, Role::Reader, Some(&identity), request);
        assert!(matches!(
            dispatch_reader(mcp(native.clone())),
            Ok(ReaderHistoryReply::McpTraces(None))
        ));
        assert!(
            matches!(dispatch_reader(metrics(Some(source.clone()), Some(snapshot), None)),
            Ok(ReaderHistoryReply::MetricHistory(MetricHistoryPage::Series { snapshot_id, rows, .. })) if snapshot_id == snapshot && rows.is_empty())
        );
        assert!(matches!(dispatch_reader(usage(Some(native.clone()))),
            Ok(ReaderHistoryReply::UsageHistory(page)) if page.snapshot_id == snapshot && page.days.len() == 7));

        let outside_cursor = MetricSeriesPosition {
            series: MetricSeriesKey {
                source_id: denied.clone(),
                series_id: "1".repeat(64),
            },
            snapshot_id: snapshot,
        };
        for request in [
            mcp(denied.clone()),
            metrics(Some(denied.clone()), Some(snapshot), None),
            metrics(None, Some(snapshot), Some(outside_cursor)),
            usage(Some(denied)),
        ] {
            assert!(matches!(
                dispatch_reader(request),
                Err(ReadFailure::Unauthorized)
            ));
        }
        for request in [
            mcp(native.clone()),
            metrics(None, Some(snapshot), None),
            usage(None),
        ] {
            assert!(matches!(
                dispatch(&store, &allowed, Role::Collector, Some(&identity), request),
                Err(ReadFailure::Unauthorized)
            ));
        }
        let replaced =
            ReadSnapshot::new(snapshot.watermark(), Some("1".repeat(32).parse().unwrap()));
        assert!(matches!(
            dispatch_reader(metrics(None, Some(replaced), None)),
            Err(ReadFailure::SnapshotReplaced)
        ));
        let other = if identity == "0".repeat(32) {
            "1".repeat(32)
        } else {
            "0".repeat(32)
        };
        assert!(matches!(
            dispatch(&store, &allowed, Role::Reader, Some(&other), usage(None)),
            Err(ReadFailure::HistoryReplaced)
        ));
        assert!(matches!(
            dispatch(&store, &allowed, Role::Reader, None, mcp(native)),
            Err(ReadFailure::InvalidRequest)
        ));

        let ReaderHistoryRequest::MetricHistory { query } = metrics(None, Some(snapshot), None)
        else {
            unreachable!();
        };
        let direct = || NativeOperation::QueryMetricHistory {
            query: query.clone(),
            expected_history_identity: identity.clone(),
        };
        assert!(matches!(
            crate::native_store::dispatch(&mut store, &allowed, Role::Reader, direct(), None, None),
            NativeReply::MetricHistory(Ok(MetricHistoryPage::Series { .. }))
        ));
        assert!(matches!(
            crate::native_store::dispatch(
                &mut store,
                &allowed,
                Role::Maintenance,
                direct(),
                None,
                None
            ),
            NativeReply::MetricHistory(Err(ReadFailure::Unauthorized))
        ));
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

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
