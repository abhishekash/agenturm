//! Clock proofs and current native request reads from a bound HTTP receiver.

use super::{Delivery, Inventory, notify, project_inventory};
use crate::HistoryReader;
use crate::attention::Snapshot;
use crate::remote_history::RemoteStore;
use agenturm_kernel::{ObservationQuery, ObservationStore, ReadSnapshot};
use agenturm_store::native_attention::NativeAttentionQuery;
use agenturm_store::{DesktopAttentionRequest, WorkspaceLedger};
use std::sync::atomic::{AtomicBool, Ordering};

const CLOCK_REQUEST: NativeAttentionQuery = NativeAttentionQuery {
    source_id: None,
    session_id: None,
    snapshot_id: None,
    limit: 1,
    include_receiver_clock: true,
};
const MISSING_CLOCK: &str = "Desktop attention needs a native receiver with authenticated clock reads. Update the history receiver.";

struct Sample {
    identity: String,
    snapshot: ReadSnapshot,
    clock: u64,
    inventory: Inventory,
}

impl Sample {
    fn read(store: &RemoteStore, ledger: &WorkspaceLedger) -> Result<Self, String> {
        let identity = store
            .workspace_history_identity()
            .ok_or("Desktop attention requires an established history identity")?;
        let timed = store
            .query_native_attention(&CLOCK_REQUEST)
            .map_err(|error| format!("{MISSING_CLOCK} {error}"))?;
        let clock = timed
            .receiver_time_unix_ns
            .filter(|time| (1..=i64::MAX as u64).contains(time))
            .ok_or(MISSING_CLOCK)?;
        let snapshot = timed.snapshot_id;
        let binding = if let Some(incarnation) = snapshot.incarnation() {
            format!("{identity}:{incarnation}")
        } else {
            identity.clone()
        };
        if store.read_binding() != binding {
            return Err("Desktop attention receiver clock belongs to a different history".into());
        }
        let page = store.query_observations(&ObservationQuery {
            snapshot_id: Some(snapshot),
            ..ObservationQuery::latest(super::HISTORY_PAGE_SIZE)
        })?;
        if page.snapshot_id != snapshot {
            return Err(
                "Desktop attention receiver clock and observations belong to different snapshots"
                    .into(),
            );
        }
        let inventory = project_inventory(&page, ledger, &identity, clock)?;
        Ok(Self {
            identity,
            snapshot,
            clock,
            inventory,
        })
    }

    fn verify_current(
        &self,
        previous: &Self,
        candidates: &[DesktopAttentionRequest],
    ) -> Result<(), Delivery> {
        if self.identity != previous.identity
            || self.snapshot.incarnation() != previous.snapshot.incarnation()
            || self.snapshot.watermark() < previous.snapshot.watermark()
        {
            return Err(Delivery::DeferredHistoryChanged);
        }
        if self.clock < previous.clock {
            return Err(Delivery::DeferredSourceClock);
        }
        if candidates
            .iter()
            .any(|row| row.observed_at_unix_ns < self.clock.saturating_sub(super::FRESHNESS_NS))
        {
            return Err(Delivery::DeferredSourceClock);
        }
        let eligible = self
            .inventory
            .candidates
            .as_ref()
            .map_err(|deferred| *deferred)?;
        if candidates.iter().any(|row| {
            !eligible.iter().any(|current| {
                current.anchor == row.anchor
                    && current.event_id == row.event_id
                    && current.revision == row.revision
                    && current.observed_at_unix_ns == row.observed_at_unix_ns
            })
        }) {
            return Err(Delivery::DeferredHistoryChanged);
        }
        Ok(())
    }
}

pub(in crate::attention) fn snapshot_remote(
    store: &RemoteStore,
    ledger: &WorkspaceLedger,
    stop: &AtomicBool,
) -> Result<Snapshot, String> {
    let initial = Sample::read(store, ledger)?;
    let candidates = match &initial.inventory.candidates {
        Ok(candidates) => candidates,
        Err(deferred) => {
            let deferred = *deferred;
            return Ok(initial.inventory.report(Some(deferred)));
        }
    };
    if stop.load(Ordering::Relaxed) {
        return Ok(initial.inventory.report(None));
    }
    let verified = Sample::read(store, ledger)?;
    if let Err(deferred) = verified.verify_current(&initial, candidates) {
        return Ok(verified.inventory.report(Some(deferred)));
    }
    if stop.load(Ordering::Relaxed) {
        return Ok(verified.inventory.report(None));
    }
    let claimed = ledger
        .claim_desktop_attention(
            &verified.identity,
            verified.snapshot,
            verified.clock,
            candidates,
        )
        .map_err(|error| format!("Desktop attention claim failed: {error}"))?;
    if claimed == 0 || stop.load(Ordering::Relaxed) {
        return Ok(verified.inventory.report(None));
    }
    let current = Sample::read(store, ledger)?;
    let delivery = match current.verify_current(&verified, candidates) {
        Err(deferred) => Some(deferred),
        Ok(()) => (!stop.load(Ordering::Relaxed)).then(|| notify(claimed, stop)),
    };
    Ok(current.inventory.report(delivery))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::ExtensionId;
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
            identity: "a".repeat(32),
            snapshot,
            clock,
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
