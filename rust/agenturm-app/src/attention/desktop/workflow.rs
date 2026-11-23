//! Notification side effects are confined to current, positively verified requests.

use super::{Delivery, Inventory, project_inventory, start_notifier};
use crate::{
    HistoryReader,
    attention::Snapshot,
    workflow_alerts::{self, WorkflowAlertSample, WorkflowAlertSamplePage},
};
use agenturm_kernel::ObservationQuery;
use agenturm_kernel::workflow_alerts::{WorkflowAlertKind, WorkflowAlertPage};
use agenturm_kernel::workflow_notifications::{
    WorkflowDelivery, WorkflowDeliveryState, WorkflowNotificationAuthority,
    WorkflowNotificationPolicy, WorkflowPolicyChange, WorkflowThresholds,
};
use agenturm_store::native_attention::NativeAttentionQuery;
use agenturm_store::{
    DesktopAttentionRequest, Store, WorkflowAlertCandidate, WorkflowNotificationRead,
    WorkspaceLedger,
};
use agenturm_ui::RequestEntry;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const CLOCK_QUERY: NativeAttentionQuery = NativeAttentionQuery {
    source_id: None,
    session_id: None,
    snapshot_id: None,
    limit: 1,
    include_receiver_clock: true,
};

fn clock(store: &impl HistoryReader) -> Result<(WorkflowNotificationAuthority, u64), String> {
    let identity = store
        .workspace_history_identity()
        .ok_or("Workflow requires an established history identity")?;
    let timed = store.query_native_attention(&CLOCK_QUERY)?;
    let now = timed
        .receiver_time_unix_ns
        .filter(|time| (1..=i64::MAX as u64).contains(time))
        .ok_or("Workflow desktop delivery requires an authenticated receiver clock")?;
    if store.workspace_history_identity().as_deref() != Some(&identity) {
        return Err("Workflow history changed. Refresh Workflow.".into());
    }
    Ok((
        WorkflowNotificationAuthority {
            history_identity: identity,
            snapshot_id: timed.snapshot_id,
        },
        now,
    ))
}

fn verify_location(
    store: &impl HistoryReader,
    db: &Path,
    expected: &WorkflowNotificationAuthority,
) -> Result<(), String> {
    // A local reader can still hold the previous SQLite inode after a path replacement.
    // Broker and HTTP clients already authenticate and pin their bound live receiver.
    let (current, _) = if store.session_title_store().is_some() {
        clock(&Store::open_read_only(db).map_err(|error| error.to_string())?)?
    } else {
        clock(store)?
    };
    if !current.includes(expected) {
        return Err("Workflow history changed. Refresh Workflow.".into());
    }
    Ok(())
}

struct Sample {
    authority: WorkflowNotificationAuthority,
    now: u64,
    inventory: Inventory,
}

impl Sample {
    fn read(store: &impl HistoryReader, ledger: &WorkspaceLedger) -> Result<Self, String> {
        let (authority, now) = clock(store)?;
        let page = store.query_observations(&ObservationQuery {
            snapshot_id: Some(authority.snapshot_id),
            ..ObservationQuery::latest(super::HISTORY_PAGE_SIZE)
        })?;
        if page.snapshot_id != authority.snapshot_id {
            return Err("Workflow clock and evidence selected different snapshots".into());
        }
        let inventory = project_inventory(&page, ledger, &authority.history_identity, now)?;
        Ok(Self {
            authority,
            now,
            inventory,
        })
    }

    fn verify_current(
        &self,
        earlier: &Self,
        requests: &[DesktopAttentionRequest],
    ) -> Result<(), Delivery> {
        if !self.authority.includes(&earlier.authority) {
            return Err(Delivery::DeferredHistoryChanged);
        }
        let freshness = self.now.saturating_sub(super::FRESHNESS_NS)..=self.now;
        if self.now < earlier.now
            || requests
                .iter()
                .any(|row| !freshness.contains(&row.observed_at_unix_ns))
        {
            return Err(Delivery::DeferredSourceClock);
        }
        let eligible = self
            .inventory
            .candidates
            .as_ref()
            .map_err(|reason| *reason)?;
        if requests.iter().any(|request| !eligible.contains(request)) {
            return Err(Delivery::DeferredHistoryChanged);
        }
        Ok(())
    }
}

fn threshold_key(
    sample: &WorkflowAlertSample,
    kind: WorkflowAlertKind,
    threshold: u64,
) -> Option<[u8; 32]> {
    let kind = match kind {
        WorkflowAlertKind::Tokens => b"tokens".as_slice(),
        WorkflowAlertKind::UsdCost => b"cost".as_slice(),
        WorkflowAlertKind::Elapsed => b"elapsed".as_slice(),
        _ => return None,
    };
    let mut hash = Sha256::new();
    hash.update(b"agenturm-workflow-threshold-delivery-v1\0");
    hash.update(kind);
    hash.update([0]);
    hash.update(threshold.to_be_bytes());
    hash.update(sample.started_at_unix_ns.to_be_bytes());
    for part in [
        sample.scope.source_id.as_str(),
        &sample.scope.session_id,
        &sample.scope.turn_id,
        sample.scope.worker_id.as_deref().unwrap_or_default(),
        &sample.start_event_id,
    ] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    Some(hash.finalize().into())
}

fn alert_candidates(
    page: &WorkflowAlertSamplePage,
    thresholds: WorkflowThresholds,
) -> Vec<WorkflowAlertCandidate> {
    let mut candidates = Vec::new();
    for sample in &page.samples {
        for (kind, amount, threshold) in [
            (WorkflowAlertKind::Tokens, sample.tokens, thresholds.tokens),
            (
                WorkflowAlertKind::UsdCost,
                sample.usd_nanos,
                thresholds.usd_nanos,
            ),
        ] {
            if threshold > 0
                && let Some(key) = threshold_key(sample, kind, threshold)
            {
                candidates.push(WorkflowAlertCandidate::Measured {
                    key,
                    exceeded: amount.map(|value| value >= threshold),
                });
            }
        }
        if thresholds.duration_ns > 0
            && let Some(crossed_at_unix_ns) = sample
                .started_at_unix_ns
                .checked_add(thresholds.duration_ns)
                .filter(|time| (1..=i64::MAX as u64).contains(time))
            && let Some(key) =
                threshold_key(sample, WorkflowAlertKind::Elapsed, thresholds.duration_ns)
        {
            candidates.push(WorkflowAlertCandidate::Elapsed {
                key,
                crossed_at_unix_ns,
            });
        }
    }
    candidates
}

fn read_alert_candidates(
    store: &impl HistoryReader,
    recent: &Sample,
    thresholds: WorkflowThresholds,
) -> Option<(u64, Vec<WorkflowAlertCandidate>)> {
    if thresholds == WorkflowThresholds::default() {
        return None;
    }
    let (_, page) = workflow_alerts::load_with_samples(store, &thresholds).ok()?;
    let page = page?;
    if page.history_identity != recent.authority.history_identity
        || page.snapshot_id != recent.authority.snapshot_id
        || page.receiver_time_unix_ns < recent.now
    {
        return None;
    }
    Some((
        page.receiver_time_unix_ns,
        alert_candidates(&page, thresholds),
    ))
}

pub(in crate::attention) struct Retry<'a> {
    pub authority: &'a WorkflowNotificationAuthority,
    pub expected_revision: u64,
    pub key: [u8; 32],
    pub expected_attempt: u64,
}

pub(in crate::attention) fn observe(
    store: &impl HistoryReader,
    ledger: &WorkspaceLedger,
    stop: &AtomicBool,
    desktop_opt_in: bool,
    retry: Option<Retry<'_>>,
    verify_location: impl Fn(&WorkflowNotificationAuthority) -> Result<(), String>,
) -> Result<Snapshot, String> {
    let policy = ledger.workflow_notification_policy().map_err(string)?;
    let initial = Sample::read(store, ledger)?;
    if let Some(retry) = &retry {
        if !initial.authority.includes(retry.authority) {
            return Err("Workflow history changed. Refresh Workflow.".into());
        }
        if policy.revision != retry.expected_revision {
            return Err("Workflow settings changed. Reload settings and try again.".into());
        }
    }
    let candidates = match &initial.inventory.candidates {
        Ok(candidates) => candidates,
        Err(reason) if retry.is_none() => {
            let reason = *reason;
            return Ok(initial.inventory.report(Some(reason)));
        }
        Err(_) => return Err("Retry needs a complete current request inventory.".into()),
    };
    if stop.load(Ordering::Relaxed) {
        return Ok(initial.inventory.report(None));
    }
    verify_location(&initial.authority)?;
    let verified = if candidates.is_empty() {
        initial
    } else {
        let next = Sample::read(store, ledger)?;
        if let Err(deferred) = next.verify_current(&initial, candidates) {
            if retry.is_some() {
                return Err("The selected request changed. Refresh Workflow.".into());
            }
            return Ok(next.inventory.report(Some(deferred)));
        }
        next
    };
    let candidates = verified
        .inventory
        .candidates
        .as_ref()
        .expect("complete current inventory");
    let (now, alerts) = if policy.can_deliver(desktop_opt_in, verified.now) {
        read_alert_candidates(store, &verified, policy.thresholds)
            .unwrap_or((verified.now, Vec::new()))
    } else {
        (verified.now, Vec::new())
    };
    let read = WorkflowNotificationRead {
        authority: &verified.authority,
        clock_unix_ns: now,
        policy_revision: policy.revision,
        desktop_opt_in,
    };
    let attempts = ledger
        .claim_workflow_delivery(
            read,
            candidates,
            &alerts,
            retry
                .as_ref()
                .map(|retry| (retry.key, retry.expected_attempt)),
        )
        .map_err(string)?;
    if attempts.is_empty() || stop.load(Ordering::Relaxed) {
        return Ok(verified.inventory.report(None));
    }
    let claimed_keys = attempts
        .iter()
        .map(|attempt| attempt.key())
        .collect::<std::collections::BTreeSet<_>>();
    let claimed_requests = candidates
        .iter()
        .filter_map(|candidate| {
            let key = WorkspaceLedger::workflow_delivery_key(candidate).ok()?;
            claimed_keys.contains(&key).then_some(candidate.clone())
        })
        .collect::<Vec<_>>();
    let claimed_alerts = alerts
        .iter()
        .copied()
        .filter(|alert| claimed_keys.contains(&alert.key()))
        .collect::<Vec<_>>();
    if claimed_requests.len() + claimed_alerts.len() != attempts.len() {
        return Err("Workflow delivery claim did not match its verified inventory".into());
    }
    let started = ledger
        .start_workflow_delivery(read, &attempts, || {
            verify_location(&verified.authority)?;
            let current = Sample::read(store, ledger)?;
            if let Err(deferred) = current.verify_current(&verified, &claimed_requests) {
                return Ok(Err(deferred));
            }
            let final_clock = if claimed_alerts.is_empty() {
                current.now
            } else {
                let Some((clock, alerts)) =
                    read_alert_candidates(store, &current, policy.thresholds)
                else {
                    return Ok(Err(Delivery::DeferredIncomplete));
                };
                if !claimed_alerts.iter().all(|alert| alerts.contains(alert)) {
                    return Ok(Err(Delivery::DeferredHistoryChanged));
                }
                clock
            };
            if final_clock < now
                || attempts
                    .iter()
                    .any(|attempt| !attempt.is_fresh_at(final_clock))
            {
                return Ok(Err(Delivery::DeferredSourceClock));
            }
            if stop.load(Ordering::Relaxed) {
                return Ok(Err(Delivery::DeferredHistoryChanged));
            }
            Ok(Ok(start_notifier(
                attempts.len(),
                !claimed_alerts.is_empty(),
            )))
        })
        .map_err(string)?;
    let delivery = match started {
        Some(Ok(process)) => {
            let (delivery, state) = process.wait(stop);
            ledger
                .finish_workflow_delivery(attempts, state)
                .map_err(string)?;
            delivery
        }
        Some(Err(deferred)) => deferred,
        None => Delivery::DeferredPolicyChanged,
    };
    Ok(verified.inventory.report(Some(delivery)))
}

fn string(error: impl ToString) -> String {
    error.to_string()
}

pub(crate) fn settings(
    store: &impl HistoryReader,
    db: &Path,
) -> Result<(WorkflowNotificationPolicy, WorkflowNotificationAuthority), String> {
    let ledger = store.workspace_ledger(db)?;
    let (authority, _) = clock(store)?;
    verify_location(store, db, &authority)?;
    Ok((
        ledger.workflow_notification_policy().map_err(string)?,
        authority,
    ))
}

pub(crate) fn change_settings(
    store: &impl HistoryReader,
    db: &Path,
    prior: &WorkflowNotificationAuthority,
    expected_revision: u64,
    change: WorkflowPolicyChange,
) -> Result<(WorkflowNotificationPolicy, WorkflowNotificationAuthority), String> {
    let ledger = store.workspace_ledger(db)?;
    let (authority, now) = clock(store)?;
    if !authority.includes(prior) {
        return Err("Workflow history changed. Refresh Workflow.".into());
    }
    let updated = ledger
        .change_workflow_notification_policy(&authority, expected_revision, now, change, || {
            verify_location(store, db, &authority)
        })
        .map_err(string)?;
    Ok((updated, authority))
}

pub(crate) fn deliveries(
    store: &impl HistoryReader,
    db: &Path,
    items: &[RequestEntry],
) -> Result<Vec<(RequestEntry, WorkflowDelivery)>, String> {
    let Some(first) = items.first() else {
        return Ok(Vec::new());
    };
    let Some(history_identity) = &first.proof.history_identity else {
        return Ok(Vec::new());
    };
    let authority = WorkflowNotificationAuthority {
        history_identity: history_identity.clone(),
        snapshot_id: first.proof.snapshot_id,
    };
    let ledger = store.workspace_ledger(db)?;
    let (current, now) = clock(store)?;
    if !current.includes(&authority) {
        return Ok(Vec::new());
    }
    verify_location(store, db, &authority)?;
    let candidates = items
        .iter()
        .filter(|item| {
            item.proof.history_identity.as_ref() == Some(history_identity)
                && item.proof.snapshot_id == authority.snapshot_id
                && !item.seen
                && (now.saturating_sub(super::FRESHNESS_NS)..=now).contains(&item.timestamp)
        })
        .map(|item| {
            let (anchor, event_id, revision) = crate::cockpit_workflows::request_key(item)?;
            Ok((
                item,
                DesktopAttentionRequest {
                    anchor,
                    event_id,
                    revision,
                    observed_at_unix_ns: item.timestamp,
                },
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let (items, requests): (Vec<_>, Vec<_>) = candidates.into_iter().unzip();
    let states = ledger
        .workflow_delivery_states(&authority, &requests)
        .map_err(string)?;
    let states = states
        .into_iter()
        .map(|state| (state.key, state))
        .collect::<std::collections::HashMap<_, _>>();
    items
        .into_iter()
        .zip(requests)
        .map(|(item, request)| {
            let key = WorkspaceLedger::workflow_delivery_key(&request).map_err(string)?;
            Ok(states.get(&key).cloned().map(|state| (item.clone(), state)))
        })
        .filter_map(Result::transpose)
        .collect()
}

pub(crate) fn alert_deliveries(
    store: &impl HistoryReader,
    db: &Path,
) -> Result<(WorkflowAlertPage, Vec<(String, WorkflowDelivery)>), String> {
    let ledger = store.workspace_ledger(db)?;
    let policy = ledger.workflow_notification_policy().map_err(string)?;
    let (page, sampled) = workflow_alerts::load_with_samples(store, &policy.thresholds)?;
    let Some(sampled) = sampled else {
        return Ok((page, Vec::new()));
    };
    let authority = WorkflowNotificationAuthority {
        history_identity: sampled.history_identity,
        snapshot_id: sampled.snapshot_id,
    };
    if page.history_identity != authority.history_identity
        || page.snapshot_id != authority.snapshot_id
    {
        return Ok((page, Vec::new()));
    }
    verify_location(store, db, &authority)?;
    let keys = page
        .alerts
        .iter()
        .filter_map(|alert| {
            let sample = sampled
                .samples
                .iter()
                .find(|sample| sample.scope == alert.scope)?;
            Some((
                threshold_key(sample, alert.kind, alert.threshold?)?,
                alert.id.clone(),
            ))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let digests = keys.keys().copied().collect::<Vec<_>>();
    let deliveries = ledger
        .workflow_alert_delivery_states(&authority, &digests)
        .map_err(string)?;
    Ok((
        page,
        deliveries
            .into_iter()
            .filter_map(|receipt| keys.get(&receipt.key).cloned().map(|id| (id, receipt)))
            .collect(),
    ))
}

pub(crate) fn poll(store: &impl HistoryReader, db: &Path, stop: &AtomicBool) -> Result<(), String> {
    let ledger = store.workspace_ledger(db)?;
    // Unset preferences remain silent in the cockpit.
    if ledger
        .workflow_notification_policy()
        .map_err(string)?
        .desktop_enabled
        != Some(true)
    {
        return Ok(());
    }
    observe(store, &ledger, stop, false, None, |authority| {
        verify_location(store, db, authority)
    })?;
    Ok(())
}

pub(crate) fn retry(
    store: &impl HistoryReader,
    db: &Path,
    authority: &WorkflowNotificationAuthority,
    expected_revision: u64,
    key: [u8; 32],
    expected_attempt: u64,
    stop: &AtomicBool,
) -> Result<WorkflowDeliveryState, String> {
    let ledger = store.workspace_ledger(db)?;
    let report = observe(
        store,
        &ledger,
        stop,
        false,
        Some(Retry {
            authority,
            expected_revision,
            key,
            expected_attempt,
        }),
        |authority| verify_location(store, db, authority),
    )?;
    match report.desktop_delivery {
        Some(Delivery::Delivered) => Ok(WorkflowDeliveryState::Delivered),
        Some(Delivery::Unavailable) => Ok(WorkflowDeliveryState::Unavailable),
        Some(Delivery::Failed) => Ok(WorkflowDeliveryState::Failed),
        Some(Delivery::Uncertain) => Ok(WorkflowDeliveryState::Uncertain),
        _ => Err("Delivery deferred. Refresh Workflow.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{ExtensionId, ReadSnapshot};
    use agenturm_store::WorkspaceAnchor;

    fn candidate(worker: &str) -> DesktopAttentionRequest {
        DesktopAttentionRequest {
            anchor: WorkspaceAnchor {
                harness_id: ExtensionId::parse("harness.codex").unwrap(),
                session_id: "session".into(),
                turn_id: Some("turn".into()),
                worker_id: Some(worker.into()),
            },
            event_id: "codex:fixture:1:lifecycle".into(),
            revision: 0,
            observed_at_unix_ns: 11,
        }
    }

    fn sample(
        snapshot: ReadSnapshot,
        clock: u64,
        requests: Vec<DesktopAttentionRequest>,
    ) -> Sample {
        Sample {
            authority: WorkflowNotificationAuthority {
                history_identity: "a".repeat(32),
                snapshot_id: snapshot,
            },
            now: clock,
            inventory: Inventory {
                requests: Vec::new(),
                partial: false,
                candidates: Ok(requests),
            },
        }
    }

    #[test]
    fn later_watermark_is_safe_only_for_the_same_current_uncorrected_request() {
        let pending = [candidate("worker")];
        let baseline = sample(12.into(), 13, vec![candidate("worker")]);
        let later = sample(14.into(), 15, vec![candidate("worker")]);
        assert_eq!(later.verify_current(&baseline, &pending), Ok(()));
        let sibling = sample(14.into(), 15, vec![candidate("sibling")]);
        assert_eq!(
            sibling.verify_current(&baseline, &pending),
            Err(Delivery::DeferredHistoryChanged)
        );
        assert_eq!(
            baseline.verify_current(&later, &pending),
            Err(Delivery::DeferredHistoryChanged)
        );
        let regrown = sample(
            ReadSnapshot::new(14, Some("1".repeat(32).parse().unwrap())),
            15,
            vec![candidate("worker")],
        );
        assert_eq!(
            regrown.verify_current(&baseline, &pending),
            Err(Delivery::DeferredHistoryChanged)
        );
        let clock_regressed = sample(14.into(), 12, vec![candidate("worker")]);
        assert_eq!(
            clock_regressed.verify_current(&baseline, &pending),
            Err(Delivery::DeferredSourceClock)
        );
        let aged = sample(
            14.into(),
            super::super::FRESHNESS_NS + 12,
            vec![candidate("worker")],
        );
        assert_eq!(
            aged.verify_current(&baseline, &pending),
            Err(Delivery::DeferredSourceClock)
        );
    }
}
