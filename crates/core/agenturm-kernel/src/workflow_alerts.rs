//! Body-free, snapshot-bound Workflow incidents independent of pending requests.

use crate::{ExtensionId, ReadSnapshot};
use serde::{Deserialize, Serialize};

pub const MAX_WORKFLOW_ALERTS: usize = 128;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAlertKind {
    Failure,
    Interruption,
    Tokens,
    UsdCost,
    Elapsed,
    Completion,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAlertScope {
    pub source_id: ExtensionId,
    pub session_id: String,
    pub turn_id: String,
    pub worker_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAlertEvidence {
    pub event_id: String,
    pub revision: u32,
    pub observed_at_unix_ns: u64,
    pub clock: WorkflowAlertClock,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAlertClock {
    SourceReported,
    ReceiverReceipt,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAlertCoverage {
    ExactFact,
    ReportedLowerBound,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAlert {
    /// Stable within an exact logical history and read incarnation.
    pub id: String,
    pub scope: WorkflowAlertScope,
    pub kind: WorkflowAlertKind,
    pub evidence: WorkflowAlertEvidence,
    /// USD and duration use nano-units. Token counts use whole tokens.
    pub measured: Option<u64>,
    pub threshold: Option<u64>,
    pub coverage: WorkflowAlertCoverage,
    pub quiet: bool,
}

/// Missing evidence counts are independent of thresholds below the limit.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAlertGaps {
    pub no_turn: usize,
    pub tokens_unreported: usize,
    pub usd_unreported: usize,
    pub elapsed_unproven: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAlertPage {
    pub history_identity: String,
    pub snapshot_id: ReadSnapshot,
    pub alerts: Vec<WorkflowAlert>,
    pub partial: bool,
    pub gaps: WorkflowAlertGaps,
}
