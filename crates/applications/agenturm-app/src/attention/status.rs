//! Read-only join of current native Workflow evidence and local delivery receipts.

use super::{Snapshot, desktop, open_history, snapshot_report};
use crate::{
    HistoryReader,
    cockpit_workflows::{request_key, roster_pending},
};
use agenturm_kernel::workflow_alerts::{
    WorkflowAlert, WorkflowAlertClock, WorkflowAlertCoverage, WorkflowAlertGaps, WorkflowAlertKind,
    WorkflowAlertScope,
};
use agenturm_kernel::workflow_notifications::{
    WorkflowDelivery, WorkflowDeliveryState, WorkflowNotificationAuthority,
    WorkflowNotificationPolicy,
};
use agenturm_store::{DesktopAttentionRequest, WorkspaceLedger};
use agenturm_ui::RequestEntry;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Serialize)]
pub(super) struct WorkflowSnapshot {
    #[serde(flatten)]
    snapshot: Snapshot,
    workflow: WorkflowStatus,
}

#[derive(Serialize)]
struct WorkflowStatus {
    history_identity: String,
    snapshot_id: String,
    policy: Policy,
    partial: bool,
    gaps: WorkflowAlertGaps,
    incidents: Vec<Incident>,
    delivery_scope: &'static str,
    request_deliveries: Vec<RequestDelivery>,
}

#[derive(Serialize)]
struct Policy {
    revision: String,
    desktop_enabled: Option<bool>,
    token_threshold: String,
    usd_nanos_threshold: String,
    elapsed_ns_threshold: String,
    snoozed_until_unix_ns: Option<String>,
}

impl From<WorkflowNotificationPolicy> for Policy {
    fn from(policy: WorkflowNotificationPolicy) -> Self {
        Self {
            revision: policy.revision.to_string(),
            desktop_enabled: policy.desktop_enabled,
            token_threshold: policy.thresholds.tokens.to_string(),
            usd_nanos_threshold: policy.thresholds.usd_nanos.to_string(),
            elapsed_ns_threshold: policy.thresholds.duration_ns.to_string(),
            snoozed_until_unix_ns: policy.snoozed_until_unix_ns.map(|time| time.to_string()),
        }
    }
}

#[derive(Serialize)]
struct Incident {
    id: String,
    kind: WorkflowAlertKind,
    scope: WorkflowAlertScope,
    evidence: Evidence,
    measured: Option<String>,
    threshold: Option<String>,
    coverage: WorkflowAlertCoverage,
    quiet: bool,
    delivery: Option<Delivery>,
}

#[derive(Serialize)]
struct Evidence {
    event_id: String,
    revision: u32,
    observed_at_unix_ns: String,
    clock: WorkflowAlertClock,
}

impl Incident {
    fn new(alert: WorkflowAlert, delivery: Option<Delivery>) -> Self {
        Self {
            id: alert.id,
            kind: alert.kind,
            scope: alert.scope,
            evidence: Evidence {
                event_id: alert.evidence.event_id,
                revision: alert.evidence.revision,
                observed_at_unix_ns: alert.evidence.observed_at_unix_ns.to_string(),
                clock: alert.evidence.clock,
            },
            measured: alert.measured.map(|value| value.to_string()),
            threshold: alert.threshold.map(|value| value.to_string()),
            coverage: alert.coverage,
            quiet: alert.quiet,
            delivery,
        }
    }
}

#[derive(Serialize)]
struct RequestDelivery {
    source_id: String,
    session_id: String,
    turn_id: Option<String>,
    worker_id: Option<String>,
    event_id: String,
    revision: u32,
    #[serde(flatten)]
    delivery: Delivery,
}

#[derive(Serialize)]
struct Delivery {
    state: &'static str,
    attempts: String,
}

impl From<WorkflowDelivery> for Delivery {
    fn from(receipt: WorkflowDelivery) -> Self {
        let state = match receipt.state {
            WorkflowDeliveryState::Claimed => "claimed",
            WorkflowDeliveryState::Delivered => "delivered",
            WorkflowDeliveryState::Failed => "failed",
            WorkflowDeliveryState::Unavailable => "unavailable",
            WorkflowDeliveryState::Uncertain => "uncertain",
        };
        Self {
            state,
            attempts: receipt.attempt.to_string(),
        }
    }
}

pub(super) fn read(db: &Path) -> Result<WorkflowSnapshot, String> {
    let store = open_history(db)?;
    let ledger = store.workspace_ledger(db)?;
    let expected = desktop::current_position(&store)?;
    let authority = WorkflowNotificationAuthority {
        history_identity: expected.0.clone(),
        snapshot_id: expected.1,
    };
    let policy = ledger
        .workflow_notification_policy()
        .map_err(|error| error.to_string())?;
    let (requests, partial) = roster_pending(&store, db, authority.snapshot_id)?;
    if requests.iter().any(|row| {
        row.proof.snapshot_id != authority.snapshot_id
            || row.proof.history_identity.as_deref() != Some(&authority.history_identity)
    }) {
        return Err(history_changed());
    }
    let request_deliveries = request_deliveries(&ledger, &authority, policy.revision, &requests)?;
    let (page, receipts) = desktop::workflow::alert_deliveries(&store, db)?;
    if page.history_identity != authority.history_identity
        || page.snapshot_id != authority.snapshot_id
        || receipts.iter().any(|(_, receipt)| {
            receipt.authority != authority || receipt.policy_revision != policy.revision
        })
        || desktop::current_position(&open_history(db)?)? != expected
        || ledger
            .workflow_notification_policy()
            .map_err(|error| error.to_string())?
            != policy
    {
        return Err(history_changed());
    }
    let mut receipts = receipts.into_iter().collect::<BTreeMap<_, _>>();
    let incidents = page
        .alerts
        .into_iter()
        .map(|alert| {
            let delivery = receipts.remove(&alert.id).map(Delivery::from);
            Incident::new(alert, delivery)
        })
        .collect();
    Ok(WorkflowSnapshot {
        snapshot: snapshot_report(requests, partial, None),
        workflow: WorkflowStatus {
            history_identity: authority.history_identity,
            snapshot_id: authority.snapshot_id.to_string(),
            policy: policy.into(),
            partial: page.partial,
            gaps: page.gaps,
            incidents,
            delivery_scope: "matched_current_evidence",
            request_deliveries,
        },
    })
}

fn request_deliveries(
    ledger: &WorkspaceLedger,
    authority: &WorkflowNotificationAuthority,
    policy_revision: u64,
    requests: &[RequestEntry],
) -> Result<Vec<RequestDelivery>, String> {
    let keys = requests
        .iter()
        .filter(|row| row.timestamp > 0)
        .map(|row| {
            let (anchor, event_id, revision) = request_key(row)?;
            Ok(DesktopAttentionRequest {
                anchor,
                event_id,
                revision,
                observed_at_unix_ns: row.timestamp,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let receipts = ledger
        .workflow_delivery_states(authority, &keys)
        .map_err(|error| error.to_string())?;
    if receipts.iter().any(|receipt| {
        &receipt.authority != authority || receipt.policy_revision != policy_revision
    }) {
        return Err(history_changed());
    }
    let mut receipts = receipts
        .into_iter()
        .map(|receipt| (receipt.key, receipt))
        .collect::<BTreeMap<_, _>>();
    keys.into_iter()
        .map(|request| {
            let key = WorkspaceLedger::workflow_delivery_key(&request)
                .map_err(|error| error.to_string())?;
            Ok(receipts.remove(&key).map(|receipt| RequestDelivery {
                source_id: request.anchor.harness_id.as_str().to_owned(),
                session_id: request.anchor.session_id,
                turn_id: request.anchor.turn_id,
                worker_id: request.anchor.worker_id,
                event_id: request.event_id,
                revision: request.revision,
                delivery: receipt.into(),
            }))
        })
        .filter_map(Result::transpose)
        .collect()
}

fn history_changed() -> String {
    "Workflow history or policy changed during the read. Retry the snapshot.".into()
}
