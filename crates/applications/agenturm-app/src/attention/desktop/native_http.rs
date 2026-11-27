//! Native HTTP notification delivery with authenticated receiver clock and binding.

use super::workflow;
use crate::attention::Snapshot;
use crate::remote_history::RemoteStore;
use agenturm_store::WorkspaceLedger;
use std::sync::atomic::AtomicBool;

pub(in crate::attention) fn snapshot_remote(
    store: &RemoteStore,
    ledger: &WorkspaceLedger,
    stop: &AtomicBool,
) -> Result<Snapshot, String> {
    workflow::observe(store, ledger, stop, true, None, |authority| {
        let binding = if let Some(incarnation) = authority.snapshot_id.incarnation() {
            format!("{}:{incarnation}", authority.history_identity)
        } else {
            authority.history_identity.clone()
        };
        if store.read_binding() != binding {
            return Err("Desktop attention receiver clock belongs to a different history".into());
        }
        Ok(())
    })
}
