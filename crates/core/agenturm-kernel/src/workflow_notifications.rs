//! Typed settings and receipts for local Workflow desktop delivery.

use crate::ReadSnapshot;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkflowThresholds {
    pub tokens: u64,
    pub usd_nanos: u64,
    pub duration_ns: u64,
}

impl WorkflowThresholds {
    pub const MAX_VALUE: u64 = i64::MAX as u64;

    pub fn is_valid(self) -> bool {
        [self.tokens, self.usd_nanos, self.duration_ns]
            .into_iter()
            .all(|value| value <= Self::MAX_VALUE)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkflowNotificationPolicy {
    pub revision: u64,
    pub desktop_enabled: Option<bool>,
    pub thresholds: WorkflowThresholds,
    pub snoozed_until_unix_ns: Option<u64>,
}

impl WorkflowNotificationPolicy {
    pub fn can_deliver(self, desktop_opt_in: bool, now_unix_ns: u64) -> bool {
        self.desktop_enabled.unwrap_or(desktop_opt_in)
            && self
                .snoozed_until_unix_ns
                .is_none_or(|until| until <= now_unix_ns)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkflowPolicyChange {
    SetDesktop(bool),
    SetThresholds(WorkflowThresholds),
    ToggleSnooze,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowNotificationAuthority {
    pub history_identity: String,
    pub snapshot_id: ReadSnapshot,
}

impl WorkflowNotificationAuthority {
    pub fn includes(&self, prior: &Self) -> bool {
        self.history_identity == prior.history_identity
            && self.snapshot_id.incarnation() == prior.snapshot_id.incarnation()
            && self.snapshot_id.watermark() >= prior.snapshot_id.watermark()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkflowDeliveryState {
    Claimed,
    Delivered,
    Failed,
    Unavailable,
    Uncertain,
}

impl WorkflowDeliveryState {
    pub fn retryable(self) -> bool {
        matches!(self, Self::Failed | Self::Unavailable)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowDelivery {
    pub authority: WorkflowNotificationAuthority,
    pub key: [u8; 32],
    pub policy_revision: u64,
    pub attempt: u64,
    pub state: WorkflowDeliveryState,
}
