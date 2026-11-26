//! A small Rust-owned SQLite store for kernel observations.
//!
//! The store provides typed atomic persistence, request replay, durable reads
//! and online SQLite backup. Linux collectors can hold a cooperative writer
//! lease for the lifetime of their connection.
//!
//! Kernel observations carry adapter-native event ID/revision, nanosecond
//! timestamps, and typed session/worker/turn/operation identity. The store
//! implements the kernel's atomic admission port; IDs remain source-scoped and
//! must never be synthesized from timestamps or session keys.

#![forbid(unsafe_code)]

mod automatic_retention;
mod backup;
mod content_grant;
mod eval_detail;
mod eval_inventory;
mod eval_outcome;
mod history_projection;
mod host_inventory;
mod linked_trace;
pub mod metric_history;
pub mod native_attention;
#[cfg(target_os = "linux")]
mod native_codex_metadata;
#[cfg(target_os = "linux")]
pub mod native_outbox;
mod private_text;
mod retention;
mod review_ledger;
mod session_catalog;
mod telemetry_log_text;
mod telemetry_payload;
pub mod usage_history;
mod workspace_ledger;

pub use automatic_retention::{
    AutomaticRetentionPolicy, AutomaticRetentionSchedule, AutomaticRetentionStatus,
    DEFAULT_RETENTION_AGE_NS,
};
pub use backup::BackupError;
pub use content_grant::{
    CodexOperationEvidence, CodexOperationSelection, ContentGrant, ContentGrantScope, ContentKind,
    MAX_PROMPT_REFERENCES, UserTextEvidence,
};
pub use eval_detail::{
    EvalDetailRecord, EvalDetailSelection, EvalFindingSelection, EvalResultDetail, EvalScoreDetail,
    eval_detail_grant_scope,
};
pub use eval_inventory::{
    EvalInventory, EvalInventoryQuery, EvalInventoryRow, EvaluationMetadata,
    MAX_EVAL_INVENTORY_SOURCES,
};
pub use eval_outcome::{
    EvalImportReport, EvalOutcome, EvalPage, EvalQuery, EvalScore, MAX_EVAL_OUTCOMES,
    RetainedEvalOutcome, SelectedEvalPage, SelectedEvalQuery, deserialize_eval_batch,
};
use linked_trace::TraceLink;
pub use linked_trace::{
    McpTraceQuery, ResolvedTraceReview, ResolvedTraceTurn, StandaloneTraceQuery, TraceReviewQuery,
    TraceTurnQuery,
};
#[cfg(target_os = "linux")]
pub use native_codex_metadata::{CodexThreadMetadata, read_codex_thread_metadata};
pub use private_text::bounded_text_preview;
pub use retention::RetentionOutcome;
pub use review_ledger::{ReviewDecisionNote, ReviewError, ReviewKey, ReviewLedger, ReviewVersion};
pub use session_catalog::{
    MAX_SESSION_PAGE_SIZE, SessionLifecycle, SessionPage, SessionPosition, SessionQuery,
    SessionSummary,
};
pub use telemetry_log_text::{
    TelemetryLogText, TelemetryLogTextBinding, TelemetryLogTextSelection,
    deserialize_telemetry_log_records,
};
pub use telemetry_payload::{
    MAX_TELEMETRY_PAYLOAD_BATCH_BYTES, MAX_TELEMETRY_PAYLOAD_TEXT_BYTES, TelemetryBinding,
    TelemetryPayload, TelemetryPayloadRecord, TelemetryPayloadSelection,
};
pub use workspace_ledger::{
    ApprovedTaskDraft, ApprovedTaskSource, DesktopAttentionRequest, LegacyActivityId,
    LegacyDesktopAttentionRequest, LegacyTaskOccurrence, LegacyTaskOccurrenceAuthority,
    LegacyTaskScope, LegacyTaskSourceProof, MAX_WORKSPACE_GIT_DIFF_BYTES, MAX_WORKSPACE_PAGE_SIZE,
    SavedHandoff, SavedTask, SavedTaskDecision, TaskBrief, TaskBriefUpdate, TaskDecisionInput,
    TaskDecisionKind, TaskDraftApproval, TaskHandoff, TaskSourceProof, WorkflowAlertCandidate,
    WorkflowDeliveryAttempt, WorkflowNotificationRead, WorkspaceAnchor, WorkspaceError,
    WorkspaceGitDiff, WorkspaceGitSnapshot, WorkspaceHistory, WorkspaceLedger, WorkspacePage,
};

use agenturm_kernel::ReadSnapshot;
use agenturm_kernel::{
    ExtensionId, MAX_DURABLE_SNAPSHOT_OBSERVATIONS, Observation,
    ObservationKind as KernelObservationKind, ObservationPage, ObservationQuery,
    ObservationScope as KernelObservationScope, ObservationStore, StoreCommit,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::Path;

const SCHEMA_VERSION: i64 = 27;
pub const MAX_HOST_HISTORY_PAGE: usize = 64;
const MAX_BATCH_EVENTS: usize = 2_048;
pub const MAX_OTLP_BATCH_SPANS: usize = 1_024;
const MAX_SOURCE_ID_BYTES: usize = 96;
const MAX_REQUEST_ID_BYTES: usize = 128;
const MAX_EVENT_ID_BYTES: usize = 256;
const MAX_SESSION_KEY_BYTES: usize = 512;
const MAX_CATEGORY_BYTES: usize = 64;
const MAX_CURSOR_BYTES: usize = 800;
const STORE_MARKER_V10: &str = "agenturm.observation-store.v10";
const STORE_MARKER_V11: &str = "agenturm.observation-store.v11";
const STORE_MARKER_V12: &str = "agenturm.observation-store.v12";
const STORE_MARKER_V13: &str = "agenturm.observation-store.v13";
const STORE_MARKER_V14: &str = "agenturm.observation-store.v14";
const STORE_MARKER_V15: &str = "agenturm.observation-store.v15";
const STORE_MARKER_V16: &str = "agenturm.observation-store.v16";
const STORE_MARKER_V17: &str = "agenturm.observation-store.v17";
const STORE_MARKER_V18: &str = "agenturm.observation-store.v18";
const STORE_MARKER_V19: &str = "agenturm.observation-store.v19";
const STORE_MARKER_V20: &str = "agenturm.observation-store.v20";
const STORE_MARKER_V21: &str = "agenturm.observation-store.v21";
const STORE_MARKER_V22: &str = "agenturm.observation-store.v22";
const STORE_MARKER_V23: &str = "agenturm.observation-store.v23";
const STORE_MARKER_V24: &str = "agenturm.observation-store.v24";
const STORE_MARKER_V25: &str = "agenturm.observation-store.v25";
const STORE_MARKER_V26: &str = "agenturm.observation-store.v26";
const STORE_MARKER: &str = "agenturm.observation-store.v27";
const STORE_MARKER_V9: &str = "agenturm.observation-store.v9";
const STORE_MARKER_V8: &str = "agenturm.observation-store.v8";
const STORE_MARKER_V7: &str = "agenturm.observation-store.v7";
const STORE_MARKER_V6: &str = "agenturm.observation-store.v6";
const STORE_MARKER_V5: &str = "agenturm.observation-store.v5";
const STORE_MARKER_V4: &str = "agenturm.observation-store.v4";
const STORE_MARKER_V3: &str = "agenturm.observation-store.v3";
const STORE_MARKER_V2: &str = "agenturm.observation-store.v2";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservationKind {
    SessionStarted,
    SessionEnded,
    Activity {
        category: String,
    },
    Tool {
        category: String,
        result: String,
    },
    FileEdit(agenturm_kernel::FileEdit),
    FileChange(agenturm_kernel::FileChange),
    McpCall {
        server: Option<String>,
        tool: Option<String>,
        method: agenturm_kernel::McpMethod,
        duration_ns: Option<u64>,
        result: String,
    },
    TraceSpan(agenturm_kernel::TraceSpan),
    TraceProgress(agenturm_kernel::TraceProgress),
    HostSample(agenturm_kernel::HostSample),
    MetricPoint(agenturm_kernel::MetricPoint),
    MetricDescriptor(agenturm_kernel::MetricDescriptor),
    TraceSpanSummary(agenturm_kernel::telemetry_summary::TraceSpanSummary),
    OtlpActivityLog(agenturm_kernel::telemetry_summary::OtlpActivityLog),
    OtlpErrorLog(agenturm_kernel::telemetry_summary::OtlpErrorLog),
    InternalReview(agenturm_kernel::native_review::InternalReview),
    NativeRequest(agenturm_kernel::native_review::NativeRequest),
    Lifecycle {
        state: String,
        reason_code: Option<String>,
    },
    Check {
        category: String,
        result: String,
    },
    PullRequestRaised {
        repository: String,
        number: u64,
    },
    Relationship {
        relationship_kind: agenturm_kernel::RelationshipKind,
        parent_session_id: String,
    },
    WorkerSpawn(agenturm_kernel::WorkerSpawn),
    WorkerLaunch(agenturm_kernel::WorkerLaunch),
    Message {
        role: String,
    },
    Diagnostic {
        code: String,
        severity: String,
    },
    Usage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cache_read_tokens: Option<u64>,
        cache_write_tokens: Option<u64>,
        total_tokens: Option<u64>,
    },
    CreditUsage {
        nanocredits: u64,
    },
    ReportedCost(agenturm_kernel::ReportedCost),
    SpanTokenUsage(agenturm_kernel::SpanTokenUsage),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredObservation {
    pub event_id: String,
    pub revision: u32,
    pub identity: ObservationIdentity,
    pub observed_at_unix_ns: u64,
    pub kind: ObservationKind,
}

/// Source-native session hierarchy associated with one stored fact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationIdentity {
    pub scope: StoredScope,
    pub session_id: Option<String>,
    pub worker_id: Option<String>,
    pub turn_id: Option<String>,
    pub operation_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredScope {
    Source,
    Session,
}

impl StoredScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Session => "session",
        }
    }
}

fn kernel_scope(scope: StoredScope) -> KernelObservationScope {
    match scope {
        StoredScope::Source => KernelObservationScope::Source,
        StoredScope::Session => KernelObservationScope::Session,
    }
}

fn stored_scope(scope: KernelObservationScope) -> StoredScope {
    match scope {
        KernelObservationScope::Source => StoredScope::Source,
        KernelObservationScope::Session => StoredScope::Session,
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationBatch {
    pub request_id: String,
    pub source_id: String,
    pub expected_cursor: Option<String>,
    pub next_cursor: String,
    pub observations: Vec<StoredObservation>,
}

impl ObservationBatch {
    /// Builds a batch with a repeatable request ID derived from its complete
    /// typed contents and opaque cursor transition. Event input order does not
    /// affect the ID.
    pub fn new_deterministic(
        source_id: impl Into<String>,
        expected_cursor: Option<String>,
        next_cursor: impl Into<String>,
        observations: Vec<StoredObservation>,
    ) -> Result<Self> {
        let mut batch = Self {
            // The request ID is excluded from the digest input, so validate
            // every caller-controlled field and collection bound before
            // cloning or serializing the observations below.
            request_id: "pending".into(),
            source_id: source_id.into(),
            expected_cursor,
            next_cursor: next_cursor.into(),
            observations,
        };
        validate_batch(&batch)?;
        batch.request_id = batch_digest(&batch)?;
        validate_batch(&batch)?;
        Ok(batch)
    }

    /// Converts kernel observations into the deterministic admission batch
    /// shared by local and broker-backed storage.
    pub fn from_observations(
        source_id: &str,
        expected_cursor: Option<&str>,
        next_cursor: &str,
        observations: &[Observation],
    ) -> std::result::Result<Self, String> {
        let mut stored = Vec::with_capacity(observations.len());
        for observation in observations {
            if observation.harness_id.as_str() != source_id {
                return Err("observation source identity mismatch".into());
            }
            stored.push(StoredObservation {
                event_id: observation.event_id.clone(),
                revision: observation.revision,
                identity: ObservationIdentity {
                    scope: stored_scope(observation.identity.scope),
                    session_id: observation.identity.session_id.clone(),
                    worker_id: observation.identity.worker_id.clone(),
                    turn_id: observation.identity.turn_id.clone(),
                    operation_id: observation.identity.operation_id.clone(),
                },
                observed_at_unix_ns: observation.observed_at_unix_ns,
                kind: match &observation.kind {
                    KernelObservationKind::SessionStarted => ObservationKind::SessionStarted,
                    KernelObservationKind::SessionEnded => ObservationKind::SessionEnded,
                    KernelObservationKind::Activity { category } => ObservationKind::Activity {
                        category: category.clone(),
                    },
                    KernelObservationKind::Tool { category, result } => ObservationKind::Tool {
                        category: category.clone(),
                        result: result.clone(),
                    },
                    KernelObservationKind::FileEdit(edit) => ObservationKind::FileEdit(*edit),
                    KernelObservationKind::FileChange(change) => {
                        ObservationKind::FileChange(change.clone())
                    }
                    KernelObservationKind::McpCall {
                        server,
                        tool,
                        method,
                        duration_ns,
                        result,
                    } => ObservationKind::McpCall {
                        server: server.clone(),
                        tool: tool.clone(),
                        method: *method,
                        duration_ns: *duration_ns,
                        result: result.clone(),
                    },
                    KernelObservationKind::TraceSpan(span) => {
                        ObservationKind::TraceSpan(span.clone())
                    }
                    KernelObservationKind::TraceProgress(progress) => {
                        ObservationKind::TraceProgress(progress.clone())
                    }
                    KernelObservationKind::InternalReview(review) => {
                        ObservationKind::InternalReview(review.clone())
                    }
                    KernelObservationKind::NativeRequest(request) => {
                        ObservationKind::NativeRequest(request.clone())
                    }
                    KernelObservationKind::TraceSpanSummary(summary) => {
                        ObservationKind::TraceSpanSummary(summary.clone())
                    }
                    KernelObservationKind::OtlpActivityLog(log) => {
                        ObservationKind::OtlpActivityLog(log.clone())
                    }
                    KernelObservationKind::OtlpErrorLog(log) => {
                        ObservationKind::OtlpErrorLog(log.clone())
                    }
                    KernelObservationKind::MetricPoint(point) => {
                        ObservationKind::MetricPoint(point.clone())
                    }
                    KernelObservationKind::MetricDescriptor(descriptor) => {
                        ObservationKind::MetricDescriptor(descriptor.clone())
                    }
                    KernelObservationKind::HostSample(sample) => {
                        ObservationKind::HostSample(sample.clone())
                    }
                    KernelObservationKind::Lifecycle { state, reason_code } => {
                        ObservationKind::Lifecycle {
                            state: state.clone(),
                            reason_code: reason_code.clone(),
                        }
                    }
                    KernelObservationKind::Check { category, result } => ObservationKind::Check {
                        category: category.clone(),
                        result: result.clone(),
                    },
                    KernelObservationKind::PullRequestRaised { repository, number } => {
                        ObservationKind::PullRequestRaised {
                            repository: repository.clone(),
                            number: *number,
                        }
                    }
                    KernelObservationKind::Relationship {
                        relationship_kind,
                        parent_session_id,
                    } => ObservationKind::Relationship {
                        relationship_kind: *relationship_kind,
                        parent_session_id: parent_session_id.clone(),
                    },
                    KernelObservationKind::WorkerSpawn(spawn) => {
                        ObservationKind::WorkerSpawn(spawn.clone())
                    }
                    KernelObservationKind::WorkerLaunch(launch) => {
                        ObservationKind::WorkerLaunch(launch.clone())
                    }
                    KernelObservationKind::Message { role } => {
                        ObservationKind::Message { role: role.clone() }
                    }
                    KernelObservationKind::CreditUsage { nanocredits } => {
                        ObservationKind::CreditUsage {
                            nanocredits: *nanocredits,
                        }
                    }
                    KernelObservationKind::ReportedCost(cost) => {
                        ObservationKind::ReportedCost(cost.clone())
                    }
                    KernelObservationKind::SpanTokenUsage(usage) => {
                        ObservationKind::SpanTokenUsage(usage.clone())
                    }
                    KernelObservationKind::Diagnostic { code, severity } => {
                        ObservationKind::Diagnostic {
                            code: code.clone(),
                            severity: severity.clone(),
                        }
                    }
                    KernelObservationKind::Usage {
                        input_tokens,
                        output_tokens,
                        cache_read_tokens,
                        cache_write_tokens,
                        total_tokens,
                    } => ObservationKind::Usage {
                        input_tokens: *input_tokens,
                        output_tokens: *output_tokens,
                        cache_read_tokens: *cache_read_tokens,
                        cache_write_tokens: *cache_write_tokens,
                        total_tokens: *total_tokens,
                    },
                },
            });
        }
        Self::new_deterministic(
            source_id,
            expected_cursor.map(str::to_owned),
            next_cursor,
            stored,
        )
        .map_err(|error| error.to_string())
    }

    /// Returns the canonical content digest used as this batch's request ID.
    pub fn deterministic_request_id(&self) -> Result<String> {
        batch_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitOutcome {
    Committed { inserted: usize },
    Replay,
}

#[derive(Debug)]
pub enum StoreError {
    Invalid(&'static str),
    RequestConflict,
    StaleCursor,
    StaleContentGrant,
    EvalTraceNotRetained,
    EvalInventoryTooLarge {
        max_trace_keys: usize,
        max_row_bytes: usize,
    },
    ExpiredHistorySnapshot {
        minimum_snapshot_id: ReadSnapshot,
    },
    SnapshotReplaced,
    TraceScopeTooLarge {
        max_rows: usize,
    },
    EventConflict {
        event_id: String,
    },
    UnrecognizedDatabase,
    UnsupportedSchema(i64),
    Sqlite(rusqlite::Error),
    Serialization(serde_json::Error),
    InjectedFailure,
    WriterLease(std::io::Error),
}

impl Display for StoreError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid observation batch: {message}"),
            Self::RequestConflict => write!(f, "request ID was reused with different content"),
            Self::StaleCursor => write!(f, "source cursor changed since batch preparation"),
            Self::StaleContentGrant => write!(f, "content grant is stale or denied"),
            Self::EvalTraceNotRetained => {
                write!(
                    f,
                    "an evaluation trace is absent from the selected retained source"
                )
            }
            Self::EvalInventoryTooLarge {
                max_trace_keys,
                max_row_bytes,
            } => write!(
                f,
                "evaluation inventory exceeds its {max_trace_keys} candidate trace key or {max_row_bytes} outcome byte budget",
            ),
            Self::ExpiredHistorySnapshot {
                minimum_snapshot_id,
            } => write!(
                f,
                "history snapshot expired after retention; start a new query at snapshot {minimum_snapshot_id} or later"
            ),
            Self::SnapshotReplaced => f.write_str(
                "history snapshot belongs to a restored or replaced history; start a new query",
            ),
            Self::TraceScopeTooLarge { max_rows } => {
                write!(f, "trace exceeds the {max_rows}-row retained scope limit")
            }
            Self::EventConflict { event_id } => {
                write!(f, "event ID was reused with different content: {event_id}")
            }
            Self::UnrecognizedDatabase => {
                write!(f, "database is not a fresh or recognized Agenturm store")
            }
            Self::UnsupportedSchema(version) => {
                write!(f, "unsupported Agenturm store schema version: {version}")
            }
            Self::Sqlite(error) => Display::fmt(error, f),
            Self::Serialization(error) => Display::fmt(error, f),
            Self::InjectedFailure => write!(f, "injected transaction failure"),
            Self::WriterLease(error) => write!(f, "collector writer lease unavailable: {error}"),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Serialization(error) => Some(error),
            Self::WriterLease(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

fn validate_marked_shape(connection: &Connection, version: i64) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT type, name FROM sqlite_schema
         WHERE name NOT LIKE 'sqlite_%' AND type IN ('table', 'index', 'view', 'trigger')
         ORDER BY type, name",
    )?;
    let objects = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut expected = vec![
        (
            "index".to_owned(),
            "agenturm_observations_by_session".to_owned(),
        ),
        ("table".to_owned(), "agenturm_ingest_receipts".to_owned()),
        ("table".to_owned(), "agenturm_observations".to_owned()),
        ("table".to_owned(), "agenturm_source_state".to_owned()),
        ("table".to_owned(), "agenturm_store_meta".to_owned()),
    ];
    if version >= 3 {
        expected.push(("table".to_owned(), "agenturm_history_batches".to_owned()));
    }
    if version >= 9 {
        expected.push(("table".to_owned(), "agenturm_content_grants".to_owned()));
    }
    if version >= 10 {
        expected.push(("table".to_owned(), "agenturm_retention_state".to_owned()));
        expected.push((
            "table".to_owned(),
            "agenturm_retired_observations".to_owned(),
        ));
    }
    if version >= 11 {
        expected.push((
            "table".to_owned(),
            "agenturm_content_cursor_proofs".to_owned(),
        ));
    }
    if version >= 15 {
        expected.push((
            "index".to_owned(),
            "agenturm_observations_by_trace".to_owned(),
        ));
        expected.push(("table".to_owned(), "agenturm_eval_outcomes".to_owned()));
    }
    if version >= 16 {
        expected.push(("table".to_owned(), "agenturm_telemetry_payloads".to_owned()));
    }
    if version >= 17 {
        expected.push(("table".into(), "agenturm_eval_detail_admissions".into()));
        expected.push(("table".into(), "agenturm_eval_details".into()));
        eval_detail::validate_schema(connection)?;
    }
    if version >= 18 {
        expected.push(("index".into(), "agenturm_metric_series_points".into()));
        expected.push(("index".into(), "agenturm_usage_history_by_time".into()));
        metric_history::validate_index(connection)?;
        usage_history::validate_index(connection)?;
    }
    if version >= 19 {
        expected.extend([
            ("table".into(), "agenturm_native_attention_senders".into()),
            ("table".into(), "agenturm_native_attention_events".into()),
            (
                "index".into(),
                "agenturm_native_attention_by_request".into(),
            ),
            ("index".into(), "agenturm_native_attention_by_target".into()),
        ]);
        native_attention::validate_schema(connection)?;
    }
    if version >= 21 {
        expected.push((
            "index".into(),
            "agenturm_metric_descriptions_by_series".into(),
        ));
        metric_history::validate_descriptor_index(connection)?;
    }
    expected.sort();
    if objects != expected {
        return Err(StoreError::UnrecognizedDatabase);
    }
    let marker: Option<String> = connection
        .query_row(
            "SELECT value FROM agenturm_store_meta WHERE key = 'format'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let expected_marker = match version {
        2 => STORE_MARKER_V2,
        3 => STORE_MARKER_V3,
        4 => STORE_MARKER_V4,
        5 => STORE_MARKER_V5,
        6 => STORE_MARKER_V6,
        7 => STORE_MARKER_V7,
        8 => STORE_MARKER_V8,
        9 => STORE_MARKER_V9,
        10 => STORE_MARKER_V10,
        11 => STORE_MARKER_V11,
        12 => STORE_MARKER_V12,
        13 => STORE_MARKER_V13,
        14 => STORE_MARKER_V14,
        15 => STORE_MARKER_V15,
        16 => STORE_MARKER_V16,
        17 => STORE_MARKER_V17,
        18 => STORE_MARKER_V18,
        19 => STORE_MARKER_V19,
        20 => STORE_MARKER_V20,
        21 => STORE_MARKER_V21,
        22 => STORE_MARKER_V22,
        23 => STORE_MARKER_V23,
        24 => STORE_MARKER_V24,
        25 => STORE_MARKER_V25,
        26 => STORE_MARKER_V26,
        27..=SCHEMA_VERSION => STORE_MARKER,
        _ => return Err(StoreError::UnrecognizedDatabase),
    };
    if marker.as_deref() != Some(expected_marker) {
        return Err(StoreError::UnrecognizedDatabase);
    }
    let mut expected_tables = vec![
        (
            "agenturm_store_meta",
            vec![("key", "TEXT", 1, 1), ("value", "TEXT", 1, 0)],
        ),
        (
            "agenturm_ingest_receipts",
            vec![
                ("request_id", "TEXT", 1, 1),
                ("source_id", "TEXT", 1, 0),
                ("batch_digest", "TEXT", 1, 0),
                ("event_count", "INTEGER", 1, 0),
            ],
        ),
        (
            "agenturm_source_state",
            vec![("source_id", "TEXT", 1, 1), ("cursor", "TEXT", 1, 0)],
        ),
        (
            "agenturm_observations",
            vec![
                ("source_id", "TEXT", 1, 1),
                ("event_id", "TEXT", 1, 2),
                ("revision", "INTEGER", 1, 3),
                ("request_id", "TEXT", 1, 0),
                ("event_digest", "TEXT", 1, 0),
                ("session_id", "TEXT", 1, 0),
                ("worker_id", "TEXT", 0, 0),
                ("turn_id", "TEXT", 0, 0),
                ("operation_id", "TEXT", 0, 0),
                ("observed_at_unix_ns", "INTEGER", 1, 0),
                ("kind_json", "TEXT", 1, 0),
            ],
        ),
    ];
    if version >= 3 {
        let mut columns = vec![("sequence", "INTEGER", 0, 1), ("request_id", "TEXT", 1, 0)];
        if version >= 22 {
            columns.push(("admitted_at_unix_ns", "INTEGER", 1, 0));
        }
        expected_tables.push(("agenturm_history_batches", columns));
    }
    if version >= 9 {
        expected_tables.push((
            "agenturm_content_grants",
            vec![
                ("source_id", "TEXT", 1, 1),
                ("kind", "TEXT", 1, 2),
                ("source_fingerprint", "BLOB", 1, 0),
                ("epoch", "INTEGER", 1, 0),
                ("nonce", "BLOB", 1, 0),
                ("enabled", "INTEGER", 1, 0),
            ],
        ));
    }
    if version >= 10 {
        expected_tables.push((
            "agenturm_retention_state",
            vec![
                ("singleton", "INTEGER", 1, 1),
                ("cutoff_unix_ns", "INTEGER", 1, 0),
                ("snapshot_offset", "INTEGER", 1, 0),
                ("minimum_snapshot_id", "INTEGER", 1, 0),
            ],
        ));
        expected_tables.push((
            "agenturm_retired_observations",
            vec![
                ("source_id", "TEXT", 1, 1),
                ("event_id", "TEXT", 1, 2),
                ("revision", "INTEGER", 1, 3),
                ("event_digest", "TEXT", 1, 0),
            ],
        ));
    }
    if version >= 11 {
        expected_tables.push((
            "agenturm_content_cursor_proofs",
            vec![("request_id", "TEXT", 1, 1), ("cursor", "TEXT", 1, 0)],
        ));
    }
    if version >= 15 {
        expected_tables.push((
            "agenturm_eval_outcomes",
            vec![
                ("trace_source_id", "TEXT", 1, 1),
                ("trace_id", "TEXT", 1, 2),
                ("revision", "INTEGER", 1, 3),
                ("outcome_digest", "BLOB", 1, 0),
                ("outcome_json", "TEXT", 0, 0),
            ],
        ));
        eval_outcome::validate_trace_index(connection)?;
    }
    if version >= 16 {
        expected_tables.push((
            "agenturm_telemetry_payloads",
            vec![
                ("source_id", "TEXT", 1, 1),
                ("event_id", "TEXT", 1, 2),
                ("revision", "INTEGER", 1, 3),
                ("request_id", "TEXT", 1, 0),
                ("payload_digest", "BLOB", 1, 0),
                ("payload_json", "TEXT", 0, 0),
            ],
        ));
    }
    for (table, expected_columns) in expected_tables {
        let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let expected_columns = if table == "agenturm_observations" && version >= 8 {
            vec![
                ("source_id", "TEXT", 1, 1),
                ("event_id", "TEXT", 1, 2),
                ("revision", "INTEGER", 1, 3),
                ("request_id", "TEXT", 1, 0),
                ("event_digest", "TEXT", 1, 0),
                ("scope", "TEXT", 1, 0),
                ("session_id", "TEXT", 0, 0),
                ("worker_id", "TEXT", 0, 0),
                ("turn_id", "TEXT", 0, 0),
                ("operation_id", "TEXT", 0, 0),
                ("observed_at_unix_ns", "INTEGER", 1, 0),
                ("kind_json", "TEXT", 1, 0),
            ]
        } else {
            expected_columns
        }
        .into_iter()
        .map(|(name, column_type, not_null, primary_key)| {
            (
                name.to_owned(),
                column_type.to_owned(),
                not_null,
                primary_key,
            )
        })
        .collect::<Vec<_>>();
        if columns != expected_columns {
            return Err(StoreError::UnrecognizedDatabase);
        }
    }
    let foreign_keys = connection
        .prepare("PRAGMA foreign_key_list(agenturm_observations)")?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if foreign_keys
        != [(
            "agenturm_ingest_receipts".into(),
            "request_id".into(),
            "request_id".into(),
        )]
    {
        return Err(StoreError::UnrecognizedDatabase);
    }
    if version >= 3 {
        let history_sql: String = connection.query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='agenturm_history_batches'",
            [],
            |row| row.get(0),
        )?;
        if !history_sql.contains("AUTOINCREMENT") {
            return Err(StoreError::UnrecognizedDatabase);
        }
        let history_foreign_keys = connection
            .prepare("PRAGMA foreign_key_list(agenturm_history_batches)")?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if history_foreign_keys
            != [(
                "agenturm_ingest_receipts".into(),
                "request_id".into(),
                "request_id".into(),
            )]
        {
            return Err(StoreError::UnrecognizedDatabase);
        }
        let unique_request_id = connection
            .prepare("PRAGMA index_list(agenturm_history_batches)")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|(_, unique)| *unique == 1)
            .any(|(index, _)| {
                connection
                    .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
                    .and_then(|mut statement| {
                        statement
                            .query_map([index], |row| row.get::<_, String>(0))
                            .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>())
                    })
                    .is_ok_and(|columns| columns == ["request_id"])
            });
        if !unique_request_id {
            return Err(StoreError::UnrecognizedDatabase);
        }
    }
    let index_columns = connection
        .prepare("PRAGMA index_info(agenturm_observations_by_session)")?
        .query_map([], |row| row.get::<_, String>(2))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if index_columns != ["session_id", "observed_at_unix_ns", "event_id", "revision"] {
        return Err(StoreError::UnrecognizedDatabase);
    }
    Ok(())
}

fn migrate_v7_to_v8(connection: &mut Connection) -> Result<()> {
    validate_marked_shape(connection, 7)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "CREATE TABLE agenturm_observations_v8 (
            source_id TEXT NOT NULL,
            event_id TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK (revision >= 0),
            request_id TEXT NOT NULL,
            event_digest TEXT NOT NULL,
            scope TEXT NOT NULL CHECK(scope IN ('source', 'session')),
            session_id TEXT,
            worker_id TEXT,
            turn_id TEXT,
            operation_id TEXT,
            observed_at_unix_ns INTEGER NOT NULL CHECK (observed_at_unix_ns >= 0),
            kind_json TEXT NOT NULL,
            CHECK ((scope='source' AND session_id IS NULL AND worker_id IS NULL
                    AND turn_id IS NULL AND operation_id IS NULL)
                OR (scope='session' AND session_id IS NOT NULL)),
            PRIMARY KEY (source_id, event_id, revision),
            FOREIGN KEY (request_id) REFERENCES agenturm_ingest_receipts(request_id)
                DEFERRABLE INITIALLY DEFERRED
        );
        INSERT INTO agenturm_observations_v8
            (source_id, event_id, revision, request_id, event_digest, scope,
             session_id, worker_id, turn_id, operation_id, observed_at_unix_ns, kind_json)
        SELECT source_id, event_id, revision, request_id, event_digest, 'session',
               session_id, worker_id, turn_id, operation_id, observed_at_unix_ns, kind_json
          FROM agenturm_observations;
        DROP TABLE agenturm_observations;
        ALTER TABLE agenturm_observations_v8 RENAME TO agenturm_observations;
        CREATE INDEX agenturm_observations_by_session
            ON agenturm_observations (session_id, observed_at_unix_ns, event_id, revision);
        UPDATE agenturm_store_meta
            SET value='agenturm.observation-store.v8' WHERE key='format';
        PRAGMA user_version = 8;",
    )?;
    transaction.commit()?;
    validate_marked_shape(connection, 8)
}

/// One SQLite connection owned by the caller. On Linux, writable connections
/// to the same database share one process lease and SQLite serializes their
/// transactions. Read-only connections do not take the lease.
pub struct Store {
    connection: Connection,
    created_new: bool,
    #[cfg(target_os = "linux")]
    writer_lease: Option<agenturm_storage_lease::WriterLease>,
}

impl Store {
    /// Opens or creates a store at `path`. A new database receives the current
    /// schema; an unknown or unrelated database is rejected without changing
    /// its journal mode or schema. On Linux the lease is acquired before any
    /// SQLite writes, including schema initialization and migration.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        #[cfg(target_os = "linux")]
        if !path.as_os_str().is_empty() && path != Path::new(":memory:") {
            use std::fs::OpenOptions;
            use std::io::ErrorKind;
            use std::os::unix::fs::OpenOptionsExt;

            let path = std::path::absolute(path).map_err(StoreError::WriterLease)?;
            // The lease binds an inode, so make the fresh database file before
            // attempting the lease. SQLite schema writes happen after acquisition.
            let created_new = match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(_) => true,
                Err(error) if error.kind() == ErrorKind::AlreadyExists => false,
                Err(error) => return Err(StoreError::WriterLease(error)),
            };
            let lease = agenturm_storage_lease::WriterLease::acquire(&path)
                .map_err(StoreError::WriterLease)?;
            let connection = Self::open_leased_connection(&lease)?;
            let connection = Self::initialize_writable_connection(connection)?;
            lease.verify_admission().map_err(StoreError::WriterLease)?;
            return Ok(Self {
                connection,
                created_new,
                writer_lease: Some(lease),
            });
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        Ok(Self {
            connection: Self::initialize_writable_connection(connection)?,
            created_new: false,
            #[cfg(target_os = "linux")]
            writer_lease: None,
        })
    }

    /// Opens a writable store for collection using the same ownership rules
    /// as other store writers. Collection always uses a filesystem path.
    pub fn open_collector(path: impl AsRef<Path>) -> Result<Self> {
        #[cfg(target_os = "linux")]
        let path = std::path::absolute(path).map_err(StoreError::WriterLease)?;
        Self::open(path)
    }

    /// Initializes the broker history identity under its retained write lease.
    /// A SQLite backup preserves this metadata along with the history.
    #[cfg(target_os = "linux")]
    pub fn ensure_history_identity(&mut self) -> Result<String> {
        if self.writer_lease.is_none() {
            return Err(StoreError::WriterLease(std::io::Error::other(
                "history identity requires a writer lease",
            )));
        }
        self.verify_write_admission()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        initialize_history_identity(&transaction)?;
        let identity = read_history_identity(&transaction)?;
        transaction.commit()?;
        Ok(identity)
    }

    /// Returns the established history identity without initializing a store.
    pub fn history_identity(&self) -> Result<String> {
        read_history_identity(&self.connection)
    }

    /// Read provenance changes on backup publication while the logical identity
    /// used for workspace state stays stable.
    pub fn read_incarnation(&self) -> Result<Option<agenturm_kernel::ReadIncarnation>> {
        retention::read_incarnation(&self.connection)
    }

    /// Opens a supported Rust store through a read-only connection. This path
    /// never initializes, migrates, changes journal mode, or creates a DB.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let transaction = connection.unchecked_transaction()?;
        let schema_version: i64 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if !matches!(schema_version, 8..=SCHEMA_VERSION) {
            return Err(StoreError::UnsupportedSchema(schema_version));
        }
        validate_marked_shape(&transaction, schema_version)?;
        transaction.commit()?;
        Ok(Self {
            connection,
            created_new: false,
            #[cfg(target_os = "linux")]
            writer_lease: None,
        })
    }

    #[cfg(target_os = "linux")]
    fn open_leased_connection(lease: &agenturm_storage_lease::WriterLease) -> Result<Connection> {
        lease.verify_admission().map_err(StoreError::WriterLease)?;
        // The lease already requires an existing file. Open the bound
        // canonical name without creating a replacement or following a
        // symlink if the target changed after that check.
        let connection = Connection::open_with_flags(
            lease.database_path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        lease.verify_admission().map_err(StoreError::WriterLease)?;
        Ok(connection)
    }

    fn initialize_writable_connection(mut connection: Connection) -> Result<Connection> {
        let schema_version: i64 =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if schema_version == 0 {
            let user_objects: i64 = connection.query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE name NOT LIKE 'sqlite_%' AND type IN ('table', 'index', 'view', 'trigger')",
                [],
                |row| row.get(0),
            )?;
            if user_objects != 0 {
                return Err(StoreError::UnrecognizedDatabase);
            }
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "CREATE TABLE agenturm_store_meta (
                    key TEXT PRIMARY KEY NOT NULL,
                    value TEXT NOT NULL
                );
                CREATE TABLE agenturm_ingest_receipts (
                    request_id TEXT PRIMARY KEY NOT NULL,
                    source_id TEXT NOT NULL,
                    batch_digest TEXT NOT NULL,
                    event_count INTEGER NOT NULL CHECK (event_count >= 0)
                );
                CREATE TABLE agenturm_source_state (
                    source_id TEXT PRIMARY KEY NOT NULL,
                    cursor TEXT NOT NULL
                );
                CREATE TABLE agenturm_history_batches (
                    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                    request_id TEXT NOT NULL UNIQUE,
                    admitted_at_unix_ns INTEGER NOT NULL DEFAULT 0 CHECK (admitted_at_unix_ns >= 0),
                    FOREIGN KEY (request_id) REFERENCES agenturm_ingest_receipts(request_id)
                );
                CREATE TABLE agenturm_observations (
                    source_id TEXT NOT NULL,
                    event_id TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision >= 0),
                    request_id TEXT NOT NULL,
                    event_digest TEXT NOT NULL,
                    scope TEXT NOT NULL CHECK(scope IN ('source', 'session')),
                    session_id TEXT,
                    worker_id TEXT,
                    turn_id TEXT,
                    operation_id TEXT,
                    observed_at_unix_ns INTEGER NOT NULL CHECK (observed_at_unix_ns >= 0),
                    kind_json TEXT NOT NULL,
                    CHECK ((scope='source' AND session_id IS NULL AND worker_id IS NULL
                            AND turn_id IS NULL AND operation_id IS NULL)
                        OR (scope='session' AND session_id IS NOT NULL)),
                    PRIMARY KEY (source_id, event_id, revision),
                    FOREIGN KEY (request_id) REFERENCES agenturm_ingest_receipts(request_id)
                        DEFERRABLE INITIALLY DEFERRED
                );
                CREATE INDEX agenturm_observations_by_session
                    ON agenturm_observations
                        (session_id, observed_at_unix_ns, event_id, revision);",
            )?;
            transaction.execute(
                "INSERT INTO agenturm_store_meta (key, value) VALUES ('format', ?1)",
                [STORE_MARKER],
            )?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            content_grant::create_schema(&transaction, SCHEMA_VERSION)?;
            content_grant::create_receipt_proof_schema(&transaction)?;
            retention::create_schema(&transaction)?;
            eval_outcome::create_schema(&transaction)?;
            telemetry_payload::create_schema(&transaction)?;
            eval_detail::create_schema(&transaction)?;
            metric_history::create_index(&transaction)?;
            metric_history::create_descriptor_index(&transaction)?;
            usage_history::create_index(&transaction)?;
            native_attention::create_schema(&transaction)?;
            initialize_history_identity(&transaction)?;
            transaction.commit()?;
        } else if schema_version == 2 {
            validate_marked_shape(&connection, 2)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "CREATE TABLE agenturm_history_batches (
                    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                    request_id TEXT NOT NULL UNIQUE,
                    FOREIGN KEY (request_id) REFERENCES agenturm_ingest_receipts(request_id)
                );
                INSERT INTO agenturm_history_batches(request_id)
                    SELECT request_id FROM agenturm_ingest_receipts ORDER BY rowid;
                UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v3' WHERE key='format';
                PRAGMA user_version = 3;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 3)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v4' WHERE key='format';
                PRAGMA user_version = 4;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 4)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v5' WHERE key='format';
                PRAGMA user_version = 5;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 5)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v6' WHERE key='format';
                PRAGMA user_version = 6;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 6)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v7' WHERE key='format';
                PRAGMA user_version = 7;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 7)?;
        } else if schema_version == 3 {
            validate_marked_shape(&connection, 3)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v4' WHERE key='format';
                PRAGMA user_version = 4;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 4)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v5' WHERE key='format';
                PRAGMA user_version = 5;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 5)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v6' WHERE key='format';
                PRAGMA user_version = 6;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 6)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v7' WHERE key='format';
                PRAGMA user_version = 7;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 7)?;
        } else if schema_version == 4 {
            validate_marked_shape(&connection, 4)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v5' WHERE key='format';
                PRAGMA user_version = 5;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 5)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v6' WHERE key='format';
                PRAGMA user_version = 6;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 6)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v7' WHERE key='format';
                PRAGMA user_version = 7;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 7)?;
        } else if schema_version == 5 {
            validate_marked_shape(&connection, 5)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v6' WHERE key='format';
                PRAGMA user_version = 6;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 6)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v7' WHERE key='format';
                PRAGMA user_version = 7;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 7)?;
        } else if schema_version == 6 {
            validate_marked_shape(&connection, 6)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v7' WHERE key='format';
                PRAGMA user_version = 7;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 7)?;
        } else if schema_version == 7 {
            validate_marked_shape(&connection, 7)?;
        } else if (8..SCHEMA_VERSION).contains(&schema_version) {
            validate_marked_shape(&connection, schema_version)?;
        } else if schema_version != SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema(schema_version));
        } else {
            validate_marked_shape(&connection, SCHEMA_VERSION)?;
        }
        if (2..8).contains(&schema_version) {
            migrate_v7_to_v8(&mut connection)?;
        }
        if (2..9).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            content_grant::create_schema(&transaction, 9)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V9],
            )?;
            transaction.pragma_update(None, "user_version", 9)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 9)?;
        }
        if (2..10).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            retention::create_schema(&transaction)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V10],
            )?;
            transaction.pragma_update(None, "user_version", 10)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 10)?;
        }
        if (2..11).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            content_grant::migrate_v10(&transaction)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V11],
            )?;
            transaction.pragma_update(None, "user_version", 11)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 11)?;
        }
        if (2..12).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            content_grant::migrate_v11(&transaction)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V12],
            )?;
            transaction.pragma_update(None, "user_version", 12)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 12)?;
        }
        if (2..13).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            content_grant::migrate_v12(&transaction)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V13],
            )?;
            transaction.pragma_update(None, "user_version", 13)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 13)?;
        }
        if (2..14).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            content_grant::migrate_v13(&transaction)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V14],
            )?;
            transaction.pragma_update(None, "user_version", 14)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 14)?;
        }
        if (2..15).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            eval_outcome::create_schema(&transaction)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V15],
            )?;
            transaction.pragma_update(None, "user_version", 15)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 15)?;
        }
        if (2..16).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            content_grant::migrate_grant_schema(&transaction, 16)?;
            telemetry_payload::create_schema(&transaction)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V16],
            )?;
            transaction.pragma_update(None, "user_version", 16)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 16)?;
        }
        if (2..17).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            content_grant::migrate_grant_schema(&transaction, 17)?;
            eval_detail::create_schema(&transaction)?;
            initialize_history_identity(&transaction)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V17],
            )?;
            transaction.pragma_update(None, "user_version", 17)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 17)?;
        }
        if (2..18).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            metric_history::create_index(&transaction)?;
            usage_history::create_index(&transaction)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V18],
            )?;
            transaction.pragma_update(None, "user_version", 18)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 18)?;
        }
        if (2..19).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            native_attention::create_schema(&transaction)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V19],
            )?;
            transaction.pragma_update(None, "user_version", 19)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 19)?;
        }
        if (2..20).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            content_grant::migrate_grant_schema(&transaction, 20)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V20],
            )?;
            transaction.pragma_update(None, "user_version", 20)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 20)?;
        }
        if (2..21).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            metric_history::create_descriptor_index(&transaction)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V21],
            )?;
            transaction.pragma_update(None, "user_version", 21)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 21)?;
        }
        if (2..22).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "ALTER TABLE agenturm_history_batches
                 ADD COLUMN admitted_at_unix_ns INTEGER NOT NULL DEFAULT 0
                 CHECK (admitted_at_unix_ns >= 0);",
            )?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V22],
            )?;
            transaction.pragma_update(None, "user_version", 22)?;
            validate_marked_shape(&transaction, 22)?;
            transaction.commit()?;
        }
        if (2..25).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            content_grant::migrate_grant_schema(&transaction, 25)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V25],
            )?;
            transaction.pragma_update(None, "user_version", 25)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 25)?;
        }
        if (2..26).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            content_grant::migrate_grant_schema(&transaction, 26)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V26],
            )?;
            transaction.pragma_update(None, "user_version", 26)?;
            transaction.commit()?;
            validate_marked_shape(&connection, 26)?;
        }
        if (2..27).contains(&schema_version) {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            content_grant::migrate_grant_schema(&transaction, 27)?;
            transaction.execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER],
            )?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction.commit()?;
            validate_marked_shape(&connection, SCHEMA_VERSION)?;
        }
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "secure_delete", "ON")?;
        Ok(connection)
    }

    fn verify_write_admission(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        if let Some(lease) = &self.writer_lease {
            lease.verify_admission().map_err(StoreError::WriterLease)?;
        }
        Ok(())
    }

    /// Atomically records one source batch and its replay receipt. Repeating
    /// the same request returns `Replay`; changing content under a request or
    /// existing source/event identity fails without partial writes.
    pub fn commit_batch(&mut self, batch: &ObservationBatch) -> Result<CommitOutcome> {
        self.commit_batch_inner(
            batch,
            None,
            None,
            #[cfg(test)]
            None,
        )
    }

    pub fn commit_telemetry_batch(
        &mut self,
        batch: &ObservationBatch,
        binding: &TelemetryBinding,
        payloads: &[TelemetryPayloadRecord],
    ) -> Result<CommitOutcome> {
        self.verify_telemetry_storage()?;
        self.commit_batch_inner(
            batch,
            Some(telemetry_payload::Admission::Spans(binding, payloads)),
            None,
            #[cfg(test)]
            None,
        )
    }

    pub fn commit_telemetry_log_batch(
        &mut self,
        batch: &ObservationBatch,
        binding: &TelemetryLogTextBinding,
        records: &[TelemetryPayloadRecord],
    ) -> Result<CommitOutcome> {
        self.verify_telemetry_storage()?;
        self.commit_batch_inner(
            batch,
            Some(telemetry_payload::Admission::Logs(binding, records)),
            None,
            #[cfg(test)]
            None,
        )
    }

    fn commit_batch_inner(
        &mut self,
        batch: &ObservationBatch,
        telemetry: Option<telemetry_payload::Admission<'_>>,
        fail_after_insert: Option<usize>,
        #[cfg(test)] before_commit: Option<fn()>,
    ) -> Result<CommitOutcome> {
        validate_batch(batch)?;
        let payloads = telemetry
            .map(|admission| admission.encode(batch))
            .transpose()?
            .unwrap_or_default();
        let digest = batch_digest(batch)?;
        self.verify_write_admission()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: Option<(String, String)> = transaction
            .query_row(
                "SELECT source_id, batch_digest FROM agenturm_ingest_receipts WHERE request_id = ?1",
                [&batch.request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((source_id, prior_digest)) = prior {
            if source_id == batch.source_id && prior_digest == digest {
                for (event_id, payload) in &payloads {
                    payload.verify_replay(&transaction, &batch.source_id, event_id)?;
                }
                return Ok(CommitOutcome::Replay);
            }
            return Err(StoreError::RequestConflict);
        }

        let current_cursor: Option<String> = transaction
            .query_row(
                "SELECT cursor FROM agenturm_source_state WHERE source_id = ?1",
                [&batch.source_id],
                |row| row.get(0),
            )
            .optional()?;
        if current_cursor != batch.expected_cursor {
            return Err(StoreError::StaleCursor);
        }

        let validating_telemetry_replay =
            !payloads.is_empty() && current_cursor.as_deref() == Some(batch.next_cursor.as_str());
        let admitted_at = retention::batch_admission_time();
        let retention_cutoff = retention::read_state(&transaction)?.cutoff_unix_ns;
        let mut prior_event_digest = transaction.prepare(
            "SELECT COALESCE(
                (SELECT event_digest FROM agenturm_observations
                 WHERE source_id = ?1 AND event_id = ?2 AND revision = ?3),
                (SELECT event_digest FROM agenturm_retired_observations
                 WHERE source_id = ?1 AND event_id = ?2 AND revision = ?3))",
        )?;
        let mut insert_retired = transaction.prepare(
            "INSERT INTO agenturm_retired_observations
                (source_id, event_id, revision, event_digest) VALUES (?1, ?2, ?3, ?4)",
        )?;
        let mut insert_observation = transaction.prepare(
            "INSERT INTO agenturm_observations
                (source_id, event_id, revision, request_id, event_digest, scope,
                 session_id, worker_id, turn_id, operation_id, observed_at_unix_ns, kind_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )?;
        let mut inserted = 0;
        for observation in &batch.observations {
            let payload = payloads.get(observation.event_id.as_str());
            let event_hash = event_digest(&batch.source_id, observation)?;
            let prior_digest: Option<String> = prior_event_digest.query_row(
                params![batch.source_id, observation.event_id, observation.revision],
                |row| row.get(0),
            )?;
            if let Some(prior_digest) = prior_digest {
                if prior_digest == event_hash {
                    if let Some(payload) = payload {
                        payload.verify_replay(
                            &transaction,
                            &batch.source_id,
                            &observation.event_id,
                        )?;
                    }
                    continue;
                }
                return Err(StoreError::EventConflict {
                    event_id: observation.event_id.clone(),
                });
            }
            if validating_telemetry_replay {
                return Err(StoreError::RequestConflict);
            }
            let timestamp = i64::try_from(observation.observed_at_unix_ns)
                .map_err(|_| StoreError::Invalid("timestamp exceeds SQLite INTEGER"))?;
            let retained = retention::observation_age_time(observation, timestamp, admitted_at)
                >= retention_cutoff;
            if let Some(payload) = payload {
                payload.insert(&transaction, batch, &observation.event_id, retained)?;
            }
            if !retained {
                insert_retired.execute(params![
                    batch.source_id,
                    observation.event_id,
                    observation.revision,
                    event_hash
                ])?;
                continue;
            }
            insert_observation.execute(params![
                batch.source_id,
                observation.event_id,
                i64::from(observation.revision),
                batch.request_id,
                event_hash,
                observation.identity.scope.as_str(),
                observation.identity.session_id,
                observation.identity.worker_id,
                observation.identity.turn_id,
                observation.identity.operation_id,
                timestamp,
                serde_json::to_string(&observation.kind)?,
            ])?;
            inserted += 1;
            if fail_after_insert == Some(inserted) {
                return Err(StoreError::InjectedFailure);
            }
        }
        if validating_telemetry_replay {
            return Ok(CommitOutcome::Replay);
        }
        drop((prior_event_digest, insert_retired, insert_observation));
        transaction.execute(
            "INSERT INTO agenturm_ingest_receipts (request_id, source_id, batch_digest, event_count)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                batch.request_id,
                batch.source_id,
                digest,
                i64::try_from(batch.observations.len())
                    .map_err(|_| StoreError::Invalid("event count exceeds SQLite INTEGER"))?,
            ],
        )?;
        if batch.observations.iter().any(|row| {
            matches!(&row.kind, ObservationKind::Message { role } if role == "user")
                || matches!(&row.kind, ObservationKind::Tool { category, .. } if category == "shell")
                || matches!(
                    &row.kind,
                    ObservationKind::McpCall { .. }
                        | ObservationKind::FileEdit(_)
                        | ObservationKind::FileChange(_)
                )
        }) {
            content_grant::record_receipt_proof(
                &transaction,
                &batch.request_id,
                &batch.next_cursor,
            )?;
        }
        transaction.execute(
            "INSERT INTO agenturm_history_batches(sequence, request_id, admitted_at_unix_ns)
             VALUES (?1, ?2, ?3)",
            params![
                retention::next_receipt_sequence(&transaction)?,
                batch.request_id,
                admitted_at
            ],
        )?;
        transaction.execute(
            "INSERT INTO agenturm_source_state (source_id, cursor) VALUES (?1, ?2)
             ON CONFLICT(source_id) DO UPDATE SET cursor = excluded.cursor",
            params![batch.source_id, batch.next_cursor],
        )?;
        #[cfg(test)]
        if let Some(before_commit) = before_commit {
            before_commit();
        }
        transaction.commit()?;
        Ok(CommitOutcome::Committed { inserted })
    }

    /// Returns all observations in stable chronological/source/event order.
    pub fn observations(&self) -> Result<Vec<(String, StoredObservation)>> {
        let mut statement = self.connection.prepare(
            "SELECT source_id, event_id, revision, scope, session_id, worker_id, turn_id,
                    operation_id, observed_at_unix_ns, kind_json
             FROM agenturm_observations
             ORDER BY observed_at_unix_ns, source_id, event_id, revision",
        )?;
        let rows = statement.query_map([], |row| {
            let revision: i64 = row.get(2)?;
            let scope: String = row.get(3)?;
            let timestamp: i64 = row.get(8)?;
            let kind_json: String = row.get(9)?;
            let kind = serde_json::from_str(&kind_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    9,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok((
                row.get::<_, String>(0)?,
                StoredObservation {
                    event_id: row.get(1)?,
                    revision: u32::try_from(revision).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    identity: ObservationIdentity {
                        scope: match scope.as_str() {
                            "source" => StoredScope::Source,
                            "session" => StoredScope::Session,
                            _ => {
                                return Err(rusqlite::Error::FromSqlConversionFailure(
                                    3,
                                    rusqlite::types::Type::Text,
                                    "unknown observation scope".into(),
                                ));
                            }
                        },
                        session_id: row.get(4)?,
                        worker_id: row.get(5)?,
                        turn_id: row.get(6)?,
                        operation_id: row.get(7)?,
                    },
                    observed_at_unix_ns: u64::try_from(timestamp).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            8,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    kind,
                },
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// Reads the newest bounded durable page, returned in chronological order.
    /// `has_more` tells callers that older history is outside this snapshot.
    pub fn observation_page(&self, limit: usize) -> Result<ObservationPage> {
        self.query_observations(&ObservationQuery::latest(limit))
    }

    /// Reads one filtered, keyset-paginated page in chronological order.
    /// `before` is exclusive and uses the full stable sort key.
    pub fn query_observations(&self, query: &ObservationQuery) -> Result<ObservationPage> {
        self.query_history(query, None, None, false, None)
    }

    /// Applies the broker's source allowlist before reading and paging rows.
    pub fn query_observations_from_sources(
        &self,
        query: &ObservationQuery,
        allowed_sources: &BTreeSet<ExtensionId>,
    ) -> Result<ObservationPage> {
        self.query_history(query, None, None, false, Some(allowed_sources))
    }

    /// Reads exact retained host samples under the normal history watermark.
    pub fn query_host_history(
        &self,
        query: &ObservationQuery,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
    ) -> Result<ObservationPage> {
        if query.limit > MAX_HOST_HISTORY_PAGE
            || query.session_id.is_some()
            || query
                .harness_id
                .as_deref()
                .is_some_and(|id| !agenturm_kernel::is_host_source(id))
            || query.before.as_ref().is_some_and(|position| {
                !agenturm_kernel::is_host_source(position.harness_id.as_str())
                    || query
                        .harness_id
                        .as_deref()
                        .is_some_and(|id| id != position.harness_id.as_str())
            })
        {
            return Err(StoreError::Invalid(
                "invalid host history scope or page limit",
            ));
        }
        let transaction = self.connection.unchecked_transaction()?;
        Self::query_history_in(
            &transaction,
            query,
            None,
            None,
            TraceLink::Unlinked,
            allowed_sources,
            true,
        )
    }

    /// Reads a bounded history page for one exact native turn.
    pub fn query_turn_observations(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
    ) -> Result<ObservationPage> {
        self.query_history(query, Some(turn_id), None, false, None)
    }

    /// Reads a turn for one exact worker anchor. `None` selects rows whose
    /// native worker identity is absent.
    pub fn query_worker_turn_observations(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
        worker_id: Option<&str>,
    ) -> Result<ObservationPage> {
        self.query_history(query, Some(turn_id), Some(worker_id), false, None)
    }

    /// Reads imported traces explicitly attached to the selected native
    /// source/session at the same history snapshot as the session evidence.
    pub fn query_linked_traces(&self, query: &ObservationQuery) -> Result<ObservationPage> {
        self.query_linked_traces_impl(query, None, None)
    }

    pub fn query_linked_traces_from_sources(
        &self,
        query: &ObservationQuery,
        allowed_sources: &BTreeSet<ExtensionId>,
    ) -> Result<ObservationPage> {
        self.query_linked_traces_impl(query, None, Some(allowed_sources))
    }

    /// Reads traces attached to one native turn and its exact worker scope.
    pub fn query_linked_turn_traces(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
        worker_id: Option<&str>,
    ) -> Result<ObservationPage> {
        self.query_linked_traces_impl(query, Some((turn_id, worker_id)), None)
    }

    pub fn query_linked_turn_traces_from_sources(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
        worker_id: Option<&str>,
        allowed_sources: &BTreeSet<ExtensionId>,
    ) -> Result<ObservationPage> {
        self.query_linked_traces_impl(query, Some((turn_id, worker_id)), Some(allowed_sources))
    }

    fn query_linked_traces_impl(
        &self,
        query: &ObservationQuery,
        turn: Option<(&str, Option<&str>)>,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
    ) -> Result<ObservationPage> {
        if query
            .harness_id
            .as_deref()
            .is_none_or(|source| source.starts_with("otlp."))
            || query.session_id.is_none()
        {
            return Err(StoreError::Invalid(
                "linked traces require a harness source and session",
            ));
        }
        self.query_history(
            query,
            turn.map(|(turn_id, _)| turn_id),
            turn.map(|(_, worker_id)| worker_id),
            true,
            allowed_sources,
        )
    }

    fn query_history(
        &self,
        query: &ObservationQuery,
        turn_id: Option<&str>,
        worker_filter: Option<Option<&str>>,
        linked_traces: bool,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
    ) -> Result<ObservationPage> {
        let transaction = self.connection.unchecked_transaction()?;
        Self::query_history_in(
            &transaction,
            query,
            turn_id,
            worker_filter,
            if linked_traces {
                TraceLink::Session
            } else {
                TraceLink::Unlinked
            },
            allowed_sources,
            false,
        )
    }

    fn query_history_in(
        connection: &Connection,
        query: &ObservationQuery,
        turn_id: Option<&str>,
        worker_filter: Option<Option<&str>>,
        trace_link: TraceLink<'_>,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
        host_history: bool,
    ) -> Result<ObservationPage> {
        let linked_traces = matches!(trace_link, TraceLink::Session | TraceLink::McpOperation(_));
        let operation = match trace_link {
            TraceLink::McpOperation(operation) => Some(operation),
            _ => None,
        };
        let (projection, filter) = match trace_link {
            TraceLink::Projected(projection) => history_projection::selection(projection),
            _ => (None, None),
        };
        if query.limit == 0 || query.limit > MAX_DURABLE_SNAPSHOT_OBSERVATIONS {
            return Err(StoreError::Invalid("observation page limit is invalid"));
        }
        if let Some(source_id) = &query.harness_id {
            validate_identifier(source_id, MAX_SOURCE_ID_BYTES, "harness ID")?;
            agenturm_kernel::ExtensionId::parse(source_id.clone())
                .map_err(|_| StoreError::Invalid("harness ID"))?;
        }
        if let Some(session_id) = &query.session_id {
            validate_identifier(session_id, MAX_SESSION_KEY_BYTES, "session ID")?;
        }
        if let Some(turn_id) = turn_id {
            if query.harness_id.is_none() || query.session_id.is_none() {
                return Err(StoreError::Invalid(
                    "turn history requires a source and session",
                ));
            }
            validate_identifier(turn_id, MAX_SESSION_KEY_BYTES, "turn ID")?;
        }
        if let Some(Some(worker_id)) = worker_filter {
            validate_identifier(worker_id, MAX_SESSION_KEY_BYTES, "worker ID")?;
        }
        if let Some(before) = &query.before {
            validate_identifier(&before.harness_id, MAX_SOURCE_ID_BYTES, "harness ID")?;
            agenturm_kernel::ExtensionId::parse(before.harness_id.clone())
                .map_err(|_| StoreError::Invalid("harness ID"))?;
            validate_identifier(&before.event_id, MAX_EVENT_ID_BYTES, "event ID")?;
            if before.observed_at_unix_ns > i64::MAX as u64 {
                return Err(StoreError::Invalid(
                    "query timestamp exceeds SQLite INTEGER",
                ));
            }
            if query.snapshot_id != Some(before.snapshot_id) {
                return Err(StoreError::Invalid("query cursor snapshot does not match"));
            }
        }
        // Use the same SQLite snapshot for the retention barrier, receipt head,
        // and result rows so a concurrent prune cannot shift an active page.
        let (snapshot_id, receipt_watermark) =
            retention::resolve_read_snapshot(connection, query.snapshot_id)?;
        if let TraceLink::Projected(projection) = trace_link {
            history_projection::require_cursor(
                connection,
                query,
                projection,
                turn_id,
                worker_filter,
                allowed_sources,
                receipt_watermark,
            )?;
        }
        let workflow_filter = if projection == Some("workflow_alerts") {
            history_projection::WORKFLOW_ALERTS_FILTER
        } else {
            "1"
        };
        let observation_sql = format!(
            "SELECT observation.source_id, observation.event_id, observation.revision,
                    observation.scope, observation.session_id, observation.worker_id, observation.turn_id,
                    observation.operation_id, observation.observed_at_unix_ns,
                    observation.kind_json
             FROM agenturm_observations AS observation
             JOIN agenturm_ingest_receipts AS receipt
               ON receipt.request_id=observation.request_id
             JOIN agenturm_history_batches AS history_batch
               ON history_batch.request_id=receipt.request_id
             WHERE history_batch.sequence <= ?1
               AND ((?12 = 0 AND (?2 IS NULL OR observation.source_id=?2))
                    OR (?12 = 1
                        AND substr(observation.source_id,1,5)='otlp.'
                        AND json_extract(observation.kind_json,'$.kind') IN ('trace_span','trace_progress')
                        AND json_extract(observation.kind_json,'$.target_source_id')=?2))
               AND (?3 IS NULL OR observation.session_id=?3)
               AND (?12 = 0 OR EXISTS (
                    SELECT 1
                    FROM agenturm_observations AS anchor
                    JOIN agenturm_history_batches AS anchor_batch
                      ON anchor_batch.request_id=anchor.request_id
                    WHERE anchor.source_id=?2 AND anchor.session_id=?3
                      AND anchor.scope='session' AND anchor_batch.sequence<=?1
                      AND json_extract(anchor.kind_json,'$.kind')<>'trace_span'
                      AND (observation.turn_id IS NULL
                           OR (anchor.turn_id=observation.turn_id
                               AND anchor.worker_id IS observation.worker_id))
               ))
               AND (?9 IS NULL OR observation.turn_id=?9)
               AND (?10 = 0 OR observation.worker_id IS ?11)
               AND (?15 IS NULL OR (observation.scope='session'
                    AND json_extract(observation.kind_json,'$.kind')='trace_span'
                    AND json_extract(observation.kind_json,'$.target_operation_id')=?15))
               AND (?13 IS NULL OR observation.source_id IN (SELECT value FROM json_each(?13)))
               AND ((?14 = 1 AND substr(observation.source_id,1,5)='host.'
                     AND json_extract(observation.kind_json,'$.kind')='host_sample')
                    OR (?14 = 0 AND (substr(observation.source_id,1,5)<>'host.'
                          OR json_extract(observation.kind_json,'$.kind')<>'host_sample')))
               AND CASE ?16
                   WHEN 'activity' THEN json_extract(observation.kind_json,'$.kind') NOT IN ('trace_span_summary','metric_descriptor')
                   WHEN 'traces' THEN (
                       (json_extract(observation.kind_json,'$.kind')='trace_span'
                           AND (?17 IS NULL OR json_extract(observation.kind_json,'$.trace_id')=?17))
                       OR (?17 IS NULL AND json_extract(observation.kind_json,'$.kind')='trace_progress'))
                   WHEN 'progress' THEN (json_extract(observation.kind_json,'$.kind')='trace_progress'
                       AND (?17 IS NULL OR json_extract(observation.kind_json,'$.trace_id')=?17))
                   WHEN 'mcp' THEN (json_extract(observation.kind_json,'$.kind')='mcp_call' AND (
                       ?17 IS NULL
                       OR instr(lower(json_extract(observation.kind_json,'$.server')),lower(?17))>0
                       OR instr(lower(json_extract(observation.kind_json,'$.tool')),lower(?17))>0
                       OR instr(lower(json_extract(observation.kind_json,'$.method')),lower(?17))>0
                       OR instr(lower(json_extract(observation.kind_json,'$.result')),lower(?17))>0))
                   WHEN 'usage' THEN json_extract(observation.kind_json,'$.kind') IN (
                       'usage','credit_usage','reported_cost','span_token_usage')
                   WHEN 'relationships' THEN json_extract(observation.kind_json,'$.kind') IN ('relationship','worker_spawn','worker_launch')
                   WHEN 'workflow_alerts' THEN ({workflow_filter})
                   ELSE ?16 IS NULL
               END
               AND (?4 IS NULL OR
                    (observation.observed_at_unix_ns,observation.source_id,
                     observation.event_id,observation.revision) <
                    (?4,?5,?6,?7))
             ORDER BY observation.observed_at_unix_ns DESC, observation.source_id DESC,
                      observation.event_id DESC, observation.revision DESC
             LIMIT ?8",
        );
        let mut statement = connection.prepare(&observation_sql)?;
        let read_limit = i64::try_from(query.limit + 1)
            .map_err(|_| StoreError::Invalid("observation page limit is invalid"))?;
        let before = query.before.as_ref();
        let rows = statement.query_map(
            params![
                receipt_watermark,
                query.harness_id,
                query.session_id,
                before.map(|position| position.observed_at_unix_ns as i64),
                before.map(|position| position.harness_id.as_str()),
                before.map(|position| position.event_id.as_str()),
                before.map(|position| i64::from(position.revision)),
                read_limit,
                turn_id,
                worker_filter.is_some(),
                worker_filter.flatten(),
                linked_traces,
                allowed_sources.map(serde_json::to_string).transpose()?,
                host_history,
                operation,
                projection,
                filter,
            ],
            |row| {
                let revision: i64 = row.get(2)?;
                let scope: String = row.get(3)?;
                let timestamp: i64 = row.get(8)?;
                let kind_json: String = row.get(9)?;
                let kind = serde_json::from_str(&kind_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        9,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    u32::try_from(revision).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    ObservationIdentity {
                        scope: match scope.as_str() {
                            "source" => StoredScope::Source,
                            "session" => StoredScope::Session,
                            _ => {
                                return Err(rusqlite::Error::FromSqlConversionFailure(
                                    3,
                                    rusqlite::types::Type::Text,
                                    "unknown observation scope".into(),
                                ));
                            }
                        },
                        session_id: row.get(4)?,
                        worker_id: row.get(5)?,
                        turn_id: row.get(6)?,
                        operation_id: row.get(7)?,
                    },
                    u64::try_from(timestamp).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            8,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    kind,
                ))
            },
        )?;
        let mut rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        let has_more = rows.len() > query.limit;
        rows.truncate(query.limit);
        rows.reverse();
        let mut observations = Vec::with_capacity(rows.len());
        for (source_id, event_id, revision, identity, observed_at_unix_ns, kind) in rows {
            let harness_id = agenturm_kernel::ExtensionId::parse(source_id)
                .map_err(|_| StoreError::Invalid("stored source ID is invalid"))?;
            observations.push(Observation {
                harness_id,
                event_id,
                revision,
                identity: agenturm_kernel::ObservationIdentity {
                    scope: kernel_scope(identity.scope),
                    session_id: identity.session_id,
                    worker_id: identity.worker_id,
                    turn_id: identity.turn_id,
                    operation_id: identity.operation_id,
                },
                observed_at_unix_ns,
                kind: match kind {
                    ObservationKind::SessionStarted => KernelObservationKind::SessionStarted,
                    ObservationKind::SessionEnded => KernelObservationKind::SessionEnded,
                    ObservationKind::Activity { category } => {
                        KernelObservationKind::Activity { category }
                    }
                    ObservationKind::Tool { category, result } => {
                        KernelObservationKind::Tool { category, result }
                    }
                    ObservationKind::FileEdit(edit) => KernelObservationKind::FileEdit(edit),
                    ObservationKind::FileChange(change) => {
                        KernelObservationKind::FileChange(change.clone())
                    }
                    ObservationKind::McpCall {
                        server,
                        tool,
                        method,
                        duration_ns,
                        result,
                    } => KernelObservationKind::McpCall {
                        server,
                        tool,
                        method,
                        duration_ns,
                        result,
                    },
                    ObservationKind::TraceSpan(span) => KernelObservationKind::TraceSpan(span),
                    ObservationKind::TraceProgress(progress) => {
                        KernelObservationKind::TraceProgress(progress)
                    }
                    ObservationKind::InternalReview(review) => {
                        KernelObservationKind::InternalReview(review)
                    }
                    ObservationKind::NativeRequest(request) => {
                        KernelObservationKind::NativeRequest(request)
                    }
                    ObservationKind::TraceSpanSummary(summary) => {
                        KernelObservationKind::TraceSpanSummary(summary)
                    }
                    ObservationKind::OtlpActivityLog(log) => {
                        KernelObservationKind::OtlpActivityLog(log)
                    }
                    ObservationKind::OtlpErrorLog(log) => KernelObservationKind::OtlpErrorLog(log),
                    ObservationKind::MetricPoint(point) => {
                        KernelObservationKind::MetricPoint(point)
                    }
                    ObservationKind::MetricDescriptor(descriptor) => {
                        KernelObservationKind::MetricDescriptor(descriptor)
                    }
                    ObservationKind::HostSample(sample) => {
                        KernelObservationKind::HostSample(sample)
                    }
                    ObservationKind::Lifecycle { state, reason_code } => {
                        KernelObservationKind::Lifecycle { state, reason_code }
                    }
                    ObservationKind::Check { category, result } => {
                        KernelObservationKind::Check { category, result }
                    }
                    ObservationKind::PullRequestRaised { repository, number } => {
                        KernelObservationKind::PullRequestRaised { repository, number }
                    }
                    ObservationKind::Relationship {
                        relationship_kind,
                        parent_session_id,
                    } => KernelObservationKind::Relationship {
                        relationship_kind,
                        parent_session_id,
                    },
                    ObservationKind::WorkerSpawn(spawn) => {
                        KernelObservationKind::WorkerSpawn(spawn)
                    }
                    ObservationKind::WorkerLaunch(launch) => {
                        KernelObservationKind::WorkerLaunch(launch)
                    }
                    ObservationKind::Message { role } => KernelObservationKind::Message { role },
                    ObservationKind::CreditUsage { nanocredits } => {
                        KernelObservationKind::CreditUsage { nanocredits }
                    }
                    ObservationKind::ReportedCost(cost) => {
                        KernelObservationKind::ReportedCost(cost)
                    }
                    ObservationKind::SpanTokenUsage(usage) => {
                        KernelObservationKind::SpanTokenUsage(usage)
                    }
                    ObservationKind::Diagnostic { code, severity } => {
                        KernelObservationKind::Diagnostic { code, severity }
                    }
                    ObservationKind::Usage {
                        input_tokens,
                        output_tokens,
                        cache_read_tokens,
                        cache_write_tokens,
                        total_tokens,
                    } => KernelObservationKind::Usage {
                        input_tokens,
                        output_tokens,
                        cache_read_tokens,
                        cache_write_tokens,
                        total_tokens,
                    },
                },
            });
        }
        Ok(ObservationPage {
            observations,
            has_more,
            snapshot_id,
        })
    }

    /// Returns a source's opaque durable cursor, if any batch has committed.
    pub fn source_cursor(&self, source_id: &str) -> Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT cursor FROM agenturm_source_state WHERE source_id = ?1",
                [source_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }
}

fn initialize_history_identity(connection: &Connection) -> Result<()> {
    connection.execute(
        "INSERT INTO agenturm_store_meta(key,value)
         VALUES ('history_identity', lower(hex(randomblob(16))))
         ON CONFLICT(key) DO NOTHING",
        [],
    )?;
    Ok(())
}

fn read_history_identity(connection: &Connection) -> Result<String> {
    let identity = connection
        .query_row(
            "SELECT CASE WHEN typeof(value) = 'text' AND length(CAST(value AS BLOB)) = 32
                         THEN value ELSE '' END
             FROM agenturm_store_meta WHERE key = 'history_identity'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    identity
        .filter(|value| {
            value.len() == 32
                && value
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
        .ok_or(StoreError::Invalid("history identity is unavailable"))
}

impl ObservationStore for Store {
    fn source_cursor(&self, source_id: &str) -> std::result::Result<Option<String>, String> {
        Store::source_cursor(self, source_id).map_err(|error| error.to_string())
    }

    fn load_observations(&self, limit: usize) -> std::result::Result<ObservationPage, String> {
        self.observation_page(limit)
            .map_err(|error| error.to_string())
    }

    fn query_observations(
        &self,
        query: &ObservationQuery,
    ) -> std::result::Result<ObservationPage, String> {
        Store::query_observations(self, query).map_err(|error| error.to_string())
    }

    fn query_turn_observations(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
    ) -> std::result::Result<ObservationPage, String> {
        Store::query_turn_observations(self, query, turn_id).map_err(|error| error.to_string())
    }

    fn query_worker_turn_observations(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
        worker_id: Option<&str>,
    ) -> std::result::Result<ObservationPage, String> {
        Store::query_worker_turn_observations(self, query, turn_id, worker_id)
            .map_err(|error| error.to_string())
    }

    fn commit(
        &mut self,
        source_id: &str,
        expected_cursor: Option<&str>,
        next_cursor: &str,
        observations: &[Observation],
    ) -> std::result::Result<StoreCommit, String> {
        let batch = ObservationBatch::from_observations(
            source_id,
            expected_cursor,
            next_cursor,
            observations,
        )?;
        match self
            .commit_batch(&batch)
            .map_err(|error| error.to_string())?
        {
            CommitOutcome::Committed { inserted } => Ok(StoreCommit {
                inserted,
                replay: false,
            }),
            CommitOutcome::Replay => Ok(StoreCommit {
                inserted: 0,
                replay: true,
            }),
        }
    }
}

fn validate_batch(batch: &ObservationBatch) -> Result<()> {
    validate_identifier(&batch.source_id, MAX_SOURCE_ID_BYTES, "source ID")?;
    validate_identifier(&batch.request_id, MAX_REQUEST_ID_BYTES, "request ID")?;
    validate_cursor(&batch.next_cursor)?;
    if let Some(cursor) = &batch.expected_cursor {
        validate_cursor(cursor)?;
    }
    if batch.observations.len() > MAX_BATCH_EVENTS
        || batch
            .observations
            .iter()
            .any(|row| matches!(row.kind, ObservationKind::SpanTokenUsage(_)))
    {
        validate_otlp_metadata_batch(batch)?;
    }
    metric_history::validate_descriptor_batch(batch)?;
    let mut event_ids = std::collections::BTreeSet::new();
    for observation in &batch.observations {
        validate_identifier(&observation.event_id, MAX_EVENT_ID_BYTES, "event ID")?;
        if !event_ids.insert((observation.event_id.as_str(), observation.revision)) {
            return Err(StoreError::Invalid(
                "duplicate event ID and revision within batch",
            ));
        }
        match observation.identity.scope {
            StoredScope::Session => validate_identifier(
                observation
                    .identity
                    .session_id
                    .as_deref()
                    .ok_or(StoreError::Invalid("session scope requires a session ID"))?,
                MAX_SESSION_KEY_BYTES,
                "session ID",
            )?,
            StoredScope::Source
                if matches!(
                    observation.kind,
                    ObservationKind::Diagnostic { .. }
                        | ObservationKind::HostSample(_)
                        | ObservationKind::MetricPoint(_)
                        | ObservationKind::MetricDescriptor(_)
                        | ObservationKind::OtlpErrorLog(_)
                        | ObservationKind::InternalReview(_)
                ) && observation.identity.session_id.is_none()
                    && observation.identity.worker_id.is_none()
                    && observation.identity.turn_id.is_none()
                    && observation.identity.operation_id.is_none() => {}
            StoredScope::Source => {
                return Err(StoreError::Invalid(
                    "source scope requires a source diagnostic without session identity",
                ));
            }
        }
        for (value, label) in [
            (observation.identity.worker_id.as_deref(), "worker ID"),
            (observation.identity.turn_id.as_deref(), "turn ID"),
            (observation.identity.operation_id.as_deref(), "operation ID"),
        ] {
            if let Some(value) = value {
                validate_identifier(value, MAX_SESSION_KEY_BYTES, label)?;
            }
        }
        if observation.observed_at_unix_ns > i64::MAX as u64 {
            return Err(StoreError::Invalid("timestamp exceeds SQLite INTEGER"));
        }
        if let ObservationKind::HostSample(sample) = &observation.kind
            && (observation.identity.scope != StoredScope::Source
                || !agenturm_kernel::is_host_source(&batch.source_id)
                || !sample.is_valid())
        {
            return Err(StoreError::Invalid(
                "host sample requires a valid host source and source-scoped metrics",
            ));
        }
        if let ObservationKind::MetricPoint(point) = &observation.kind
            && (observation.identity.scope != StoredScope::Source
                || observation.revision != 0
                || !agenturm_kernel::is_metric_source(&batch.source_id)
                || !point.is_valid_at(observation.observed_at_unix_ns)
                || !point.matches_event(&observation.event_id, observation.observed_at_unix_ns))
        {
            return Err(StoreError::Invalid(
                "metric point requires a valid OTLP source, event, and source scope",
            ));
        }
        if let ObservationKind::TraceSpanSummary(summary) = &observation.kind
            && (observation.identity.scope != StoredScope::Session || !batch.source_id.starts_with("otlp.") || observation.revision != 0 || !summary.is_valid() || !summary.matches_event(&observation.event_id) || !batch.observations.iter().any(|anchor| {
                matches!(&anchor.kind, ObservationKind::TraceSpan(span) if span.trace_id == summary.trace_id && span.span_id == summary.span_id)
                && anchor.identity == observation.identity && anchor.observed_at_unix_ns == observation.observed_at_unix_ns && anchor.revision == observation.revision
            })) {
            return Err(StoreError::Invalid("trace summary requires its exact structural span in the same batch"));
        }
        if let ObservationKind::OtlpActivityLog(log) = &observation.kind
            && (observation.identity.scope != StoredScope::Session
                || !batch.source_id.starts_with("otlp.")
                || observation.revision != 0
                || observation.identity.operation_id.is_some()
                || !log.is_valid()
                || !log.matches_event(&observation.event_id)
                || (observation.identity.turn_id.is_some() && log.target_source_id.is_none())
                || (observation.identity.worker_id.is_some()
                    && observation.identity.turn_id.is_none()))
        {
            return Err(StoreError::Invalid(
                "activity log requires exact OTLP session scope and safe metadata",
            ));
        }
        if let ObservationKind::OtlpErrorLog(log) = &observation.kind
            && (!batch.source_id.starts_with("otlp.")
                || observation.revision != 0
                || observation.identity.operation_id.is_some()
                || !log.is_valid_at(observation.observed_at_unix_ns)
                || !log.matches_event(&observation.event_id)
                || (log.target_source_id.is_some() && observation.identity.session_id.is_none())
                || (observation.identity.turn_id.is_some() && log.target_source_id.is_none())
                || (observation.identity.worker_id.is_some()
                    && observation.identity.turn_id.is_none()))
        {
            return Err(StoreError::Invalid(
                "error log requires exact typed OTLP scope and safe metadata",
            ));
        }
        let review_source =
            agenturm_kernel::native_review::is_codex_review_source(&batch.source_id);
        if review_source
            && !matches!(
                observation.kind,
                ObservationKind::InternalReview(_) | ObservationKind::Diagnostic { .. }
            )
        {
            return Err(StoreError::Invalid(
                "internal review sources may emit only review metadata",
            ));
        }
        if let ObservationKind::InternalReview(review) = &observation.kind
            && (!review_source
                || observation.identity.scope != StoredScope::Source
                || !review.is_valid())
        {
            return Err(StoreError::Invalid(
                "internal review metadata requires a valid reserved source",
            ));
        }
        if let ObservationKind::NativeRequest(request) = &observation.kind
            && (observation.identity.scope != StoredScope::Session
                || observation.identity.turn_id.is_none()
                || !request.is_valid())
        {
            return Err(StoreError::Invalid(
                "native requests require a session, turn and opaque request ID",
            ));
        }
        if let ObservationKind::Activity { category } = &observation.kind
            && (category.is_empty()
                || category.len() > MAX_CATEGORY_BYTES
                || category.chars().any(char::is_control))
        {
            return Err(StoreError::Invalid(
                "activity category is invalid or too long",
            ));
        }
        if let ObservationKind::Tool { category, result } = &observation.kind
            && (!(matches!(category.as_str(), "shell" | "file" | "mcp" | "other"))
                || !(matches!(
                    result.as_str(),
                    "started" | "succeeded" | "failed" | "denied"
                )))
        {
            return Err(StoreError::Invalid("tool category or result is invalid"));
        }
        if let ObservationKind::McpCall {
            server,
            tool,
            duration_ns,
            result,
            ..
        } = &observation.kind
            && (observation.identity.scope != StoredScope::Session
                || observation.identity.session_id.is_none()
                || observation.identity.operation_id.is_none()
                || !agenturm_kernel::is_valid_mcp_metadata(
                    server.as_deref(),
                    tool.as_deref(),
                    *duration_ns,
                    result,
                ))
        {
            return Err(StoreError::Invalid(
                "MCP call metadata or operation identity is invalid",
            ));
        }
        if let ObservationKind::TraceSpan(span) = &observation.kind
            && (!span.is_valid(observation.observed_at_unix_ns)
                || !agenturm_kernel::is_trace_source_link_valid(&batch.source_id, span)
                || !agenturm_kernel::is_trace_turn_link_valid(
                    span,
                    &observation.event_id,
                    observation.identity.turn_id.as_deref(),
                    observation.identity.worker_id.as_deref(),
                    observation.identity.operation_id.as_deref(),
                ))
        {
            return Err(StoreError::Invalid(
                "trace identity, timing, or source link is invalid",
            ));
        }
        if let ObservationKind::TraceProgress(progress) = &observation.kind
            && (observation.identity.scope != StoredScope::Session
                || observation.revision != 0
                || !batch.source_id.starts_with("otlp.")
                || !progress.is_valid(&observation.event_id, observation.observed_at_unix_ns)
                || !agenturm_kernel::is_trace_progress_scope_valid(
                    progress,
                    observation.identity.turn_id.as_deref(),
                    observation.identity.worker_id.as_deref(),
                    observation.identity.operation_id.as_deref(),
                ))
        {
            return Err(StoreError::Invalid(
                "trace progress metadata or event identity is invalid",
            ));
        }
        if let ObservationKind::Lifecycle { state, reason_code } = &observation.kind
            && (!agenturm_kernel::is_lifecycle_state(state)
                || reason_code.as_ref().is_some_and(|reason| {
                    reason.is_empty()
                        || reason.len() > 128
                        || !reason.bytes().all(|byte| {
                            byte.is_ascii_lowercase()
                                || byte.is_ascii_digit()
                                || b"._-".contains(&byte)
                        })
                }))
        {
            return Err(StoreError::Invalid("lifecycle state or reason is invalid"));
        }
        if let ObservationKind::FileEdit(edit) = observation.kind
            && (!edit.is_valid()
                || observation.identity.scope != StoredScope::Session
                || observation.identity.turn_id.is_none()
                || observation.identity.operation_id.is_none())
        {
            return Err(StoreError::Invalid(
                "file edit counts or operation identity are invalid",
            ));
        }
        if let ObservationKind::FileChange(change) = &observation.kind
            && (!change.is_valid()
                || observation.identity.scope != StoredScope::Session
                || observation.identity.turn_id.is_none()
                || observation.identity.operation_id.is_none())
        {
            return Err(StoreError::Invalid(
                "file change summary or operation identity is invalid",
            ));
        }
        if let ObservationKind::Check { category, result } = &observation.kind
            && (!matches!(category.as_str(), "test" | "lint" | "build")
                || !matches!(result.as_str(), "passed" | "failed" | "unknown"))
        {
            return Err(StoreError::Invalid("check category or result is invalid"));
        }
        if let ObservationKind::PullRequestRaised { repository, number } = &observation.kind
            && (observation.identity.scope != StoredScope::Session
                || observation.identity.turn_id.is_none()
                || observation.identity.operation_id.is_none()
                || !agenturm_kernel::is_github_pull_request(repository, *number))
        {
            return Err(StoreError::Invalid(
                "pull request observation reference or identity is invalid",
            ));
        }
        if let ObservationKind::Relationship {
            parent_session_id, ..
        } = &observation.kind
            && (observation.identity.scope != StoredScope::Session
                || observation.identity.turn_id.is_some()
                || observation.identity.operation_id.is_some()
                || !agenturm_kernel::is_valid_relationship_parent(
                    observation.identity.session_id.as_deref(),
                    parent_session_id,
                ))
        {
            return Err(StoreError::Invalid(
                "relationship parent or identity is invalid",
            ));
        }
        if let ObservationKind::WorkerSpawn(spawn) = &observation.kind
            && (observation.identity.scope != StoredScope::Session
                || observation.identity.turn_id.is_none()
                || observation.identity.operation_id.is_none()
                || !spawn.is_valid_for(observation.identity.worker_id.as_deref()))
        {
            return Err(StoreError::Invalid(
                "worker spawn ID or parent operation identity is invalid",
            ));
        }
        if let ObservationKind::WorkerLaunch(launch) = &observation.kind
            && (observation.identity.scope != StoredScope::Session
                || observation.identity.turn_id.is_none()
                || observation.identity.operation_id.is_none()
                || !launch.is_valid_for(observation.identity.worker_id.as_deref()))
        {
            return Err(StoreError::Invalid(
                "worker launch ID or parent operation identity is invalid",
            ));
        }
        if let ObservationKind::Message { role } = &observation.kind
            && !matches!(role.as_str(), "user" | "assistant")
        {
            return Err(StoreError::Invalid("message role is invalid"));
        }
        if let ObservationKind::CreditUsage { nanocredits } = &observation.kind
            && *nanocredits > i64::MAX as u64
        {
            return Err(StoreError::Invalid("credit usage exceeds supported range"));
        }
        if let ObservationKind::ReportedCost(cost) = &observation.kind
            && (observation.identity.scope != StoredScope::Session
                || !cost.is_valid()
                || !cost.matches_evidence(
                    &observation.event_id,
                    observation.identity.operation_id.as_deref(),
                )
                || (cost.span.is_some() && !batch.source_id.starts_with("otlp.")))
        {
            return Err(StoreError::Invalid(
                "reported cost metadata or event identity is invalid",
            ));
        }
        if let ObservationKind::Diagnostic { code, severity } = &observation.kind
            && (!agenturm_kernel::is_safe_diagnostic_code(code)
                || !matches!(severity.as_str(), "info" | "warning" | "error"))
        {
            return Err(StoreError::Invalid(
                "diagnostic code or severity is invalid",
            ));
        }
    }
    validate_native_cost_anchors(&batch.observations)
}

fn validate_native_cost_anchors(rows: &[StoredObservation]) -> Result<()> {
    let mut cost_rows = rows.iter().filter(|row| {
        matches!(&row.kind, ObservationKind::ReportedCost(cost) if cost.native_origin.is_some())
    }).peekable();
    if cost_rows.peek().is_none() {
        return Ok(());
    }
    let messages: std::collections::BTreeMap<_, _> = rows
        .iter()
        .filter(|row| matches!(&row.kind, ObservationKind::Message { role } if role == "assistant"))
        .map(|row| ((row.event_id.as_str(), row.revision), row))
        .collect();
    for row in cost_rows {
        let valid = row
            .identity
            .operation_id
            .as_deref()
            .and_then(|id| messages.get(&(id, row.revision)))
            .is_some_and(|anchor| {
                anchor.observed_at_unix_ns == row.observed_at_unix_ns
                    && anchor.identity.scope == row.identity.scope
                    && anchor.identity.session_id == row.identity.session_id
                    && anchor.identity.turn_id == row.identity.turn_id
                    && anchor.identity.worker_id == row.identity.worker_id
                    && anchor.identity.operation_id.is_none()
            });
        if !valid {
            return Err(StoreError::Invalid(
                "native cost has no exact assistant message",
            ));
        }
    }
    Ok(())
}

fn validate_otlp_metadata_batch(batch: &ObservationBatch) -> Result<()> {
    let rows = &batch.observations;
    if rows.len() > MAX_OTLP_BATCH_SPANS * 4 {
        return Err(StoreError::Invalid("batch exceeds event count limit"));
    }
    if !batch.source_id.starts_with("otlp.") || ExtensionId::parse(&batch.source_id).is_err() {
        return Err(StoreError::Invalid(if rows.len() > MAX_BATCH_EVENTS {
            "batch exceeds event count limit"
        } else {
            "span token usage requires an OTLP source"
        }));
    }
    let mut spans = BTreeMap::new();
    for row in rows {
        if let ObservationKind::TraceSpan(span) = &row.kind {
            let span_key = (span.trace_id.as_str(), span.span_id.as_str());
            if row.revision != 0
                || row.identity.operation_id.is_some()
                || row
                    .event_id
                    .strip_prefix("otlp:")
                    .and_then(|id| id.split_once(':'))
                    != Some(span_key)
                || spans.insert(span_key, (row, span)).is_some()
                || spans.len() > MAX_OTLP_BATCH_SPANS
            {
                return Err(StoreError::Invalid(
                    "OTLP structural spans exceed limits or have invalid identities",
                ));
            }
        }
    }
    let matching_span = |row: &StoredObservation, span: &agenturm_kernel::TraceSpanIdentity| {
        let key = (span.trace_id.as_str(), span.span_id.as_str());
        spans.get(&key).is_some_and(|(anchor, structural)| {
            row.revision == anchor.revision
                && row.identity == anchor.identity
                && row.observed_at_unix_ns == anchor.observed_at_unix_ns
                && match &row.kind {
                    ObservationKind::SpanTokenUsage(usage) => {
                        usage.target_source_id == structural.target_source_id
                    }
                    _ => true,
                }
        })
    };
    for row in rows {
        match &row.kind {
            ObservationKind::TraceSpan(_) | ObservationKind::TraceSpanSummary(_) => {}
            ObservationKind::ReportedCost(cost) => {
                if !cost.matches_event(&row.event_id)
                    || !cost
                        .span
                        .as_ref()
                        .is_some_and(|span| matching_span(row, span))
                {
                    return Err(StoreError::Invalid(
                        "OTLP reported cost has no exact structural span",
                    ));
                }
            }
            ObservationKind::SpanTokenUsage(usage) => {
                let valid = agenturm_kernel::is_span_usage_anchor_valid(
                    usage,
                    &row.event_id,
                    row.identity.turn_id.as_deref(),
                    row.identity.worker_id.as_deref(),
                    row.identity.operation_id.as_deref(),
                );
                if !valid || !matching_span(row, &usage.span) {
                    return Err(StoreError::Invalid(
                        "OTLP span token usage has no exact structural span",
                    ));
                }
            }
            _ => {
                return Err(StoreError::Invalid(
                    "OTLP metadata batch contains an unrelated observation",
                ));
            }
        }
    }
    Ok(())
}

fn validate_cursor(cursor: &str) -> Result<()> {
    if cursor.len() > MAX_CURSOR_BYTES || cursor.chars().any(char::is_control) {
        return Err(StoreError::Invalid("cursor is invalid or too long"));
    }
    Ok(())
}

fn validate_identifier(value: &str, max_bytes: usize, label: &'static str) -> Result<()> {
    if value.is_empty()
        || value.len() > max_bytes
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
    {
        return Err(StoreError::Invalid(label));
    }
    Ok(())
}

fn batch_digest(batch: &ObservationBatch) -> Result<String> {
    let mut observations = batch.observations.clone();
    observations.sort_by(|left, right| {
        left.event_id
            .cmp(&right.event_id)
            .then(left.revision.cmp(&right.revision))
    });
    let encoded = serde_json::to_vec(&DigestInput {
        source_id: &batch.source_id,
        expected_cursor: &batch.expected_cursor,
        next_cursor: &batch.next_cursor,
        observations: &observations,
    })?;
    Ok(hex_digest(&encoded))
}

#[derive(Serialize)]
struct DigestInput<'a> {
    source_id: &'a str,
    expected_cursor: &'a Option<String>,
    next_cursor: &'a str,
    observations: &'a [StoredObservation],
}

fn event_digest(source_id: &str, observation: &StoredObservation) -> Result<String> {
    let encoded = serde_json::to_vec(&EventDigestInput {
        source_id,
        observation,
    })?;
    Ok(hex_digest(&encoded))
}

#[derive(Serialize)]
struct EventDigestInput<'a> {
    source_id: &'a str,
    observation: &'a StoredObservation,
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::ObservationPosition;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDb(std::path::PathBuf);

    impl TempDb {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("agenturm-store-{}-{id}.sqlite", std::process::id()));
            Self(path)
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = fs::remove_file(format!("{}{suffix}", self.0.display()));
            }
            if let Some(name) = self.0.file_name().and_then(|name| name.to_str()) {
                let _ = fs::remove_file(
                    self.0
                        .with_file_name(format!(".{name}.agenturm-writer.lock")),
                );
            }
        }
    }

    fn downgrade_fixture_to_v14(connection: &Connection) {
        connection.execute_batch(
            "DROP TABLE agenturm_native_attention_events; DROP TABLE agenturm_native_attention_senders;
             DROP INDEX agenturm_metric_descriptions_by_series;
             ALTER TABLE agenturm_history_batches DROP COLUMN admitted_at_unix_ns;
             DROP INDEX agenturm_metric_series_points;
             DROP INDEX agenturm_usage_history_by_time;
             DROP TABLE agenturm_eval_details;
             DROP TABLE agenturm_eval_detail_admissions;
             DROP INDEX agenturm_observations_by_trace;
             DROP TABLE agenturm_eval_outcomes;
             DROP TABLE agenturm_telemetry_payloads;
             UPDATE agenturm_store_meta SET value='agenturm.observation-store.v14' WHERE key='format';
             PRAGMA user_version=14;"
        ).unwrap();
    }

    #[test]
    fn metric_description_upgrade_preserves_v20_grants_and_receipts() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let binding = TelemetryLogTextBinding {
            source_id: ExtensionId::parse("otlp.fixture").unwrap(),
            native_source_id: None,
        };
        let scope = binding.grant_scope().unwrap();
        let grant = store.grant_content(&scope).unwrap();
        let batch = ObservationBatch::new_deterministic(
            "harness.fixture",
            None,
            "retained-cursor",
            vec![event("retained-event", "session", 10)],
        )
        .unwrap();
        assert_eq!(
            store.commit_batch(&batch).unwrap(),
            CommitOutcome::Committed { inserted: 1 }
        );
        let receipt = |store: &Store| -> (String, i64, String, String) {
            store
                .connection
                .query_row(
                    "SELECT r.batch_digest, h.sequence, o.event_digest, o.kind_json
                   FROM agenturm_ingest_receipts r JOIN agenturm_history_batches h USING(request_id)
                   JOIN agenturm_observations o USING(request_id)
                  WHERE r.request_id=?1",
                    [&batch.request_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap()
        };
        let original = receipt(&store);
        store.connection.execute_batch(
            "DROP INDEX agenturm_metric_descriptions_by_series;
             ALTER TABLE agenturm_history_batches DROP COLUMN admitted_at_unix_ns;
             UPDATE agenturm_store_meta SET value='agenturm.observation-store.v20' WHERE key='format';
             PRAGMA user_version=20;",
        ).unwrap();
        drop(store);

        let read_only = Store::open_read_only(&db.0).unwrap();
        assert_eq!(receipt(&read_only), original);
        assert_eq!(
            read_only.current_content_grant(&scope).unwrap(),
            Some(grant.clone())
        );
        drop(read_only);
        let mut upgraded = Store::open(&db.0).unwrap();
        validate_marked_shape(&upgraded.connection, SCHEMA_VERSION).unwrap();
        assert_eq!(receipt(&upgraded), original);
        assert_eq!(upgraded.current_content_grant(&scope).unwrap(), Some(grant));
        assert_eq!(
            upgraded.commit_batch(&batch).unwrap(),
            CommitOutcome::Replay
        );
    }

    #[test]
    fn claude_edit_grants_upgrade_from_v22_and_keep_prior_approvals() {
        let db = TempDb::new();
        let codex = ContentGrantScope::new(
            ExtensionId::parse("harness.codex").unwrap(),
            ContentKind::CodexEditDetail,
            [1; 32],
        );
        let claude = ContentGrantScope::new(
            ExtensionId::parse("harness.claude").unwrap(),
            ContentKind::ClaudeEditDetail,
            [2; 32],
        );
        let mut store = Store::open(&db.0).unwrap();
        let grant = store.grant_content(&codex).unwrap();
        // Rebuild the exact v22 grant table so a pre-upgrade database again
        // rejects claude.edit_detail under its narrower CHECK rule.
        store
            .connection
            .execute_batch(
                "ALTER TABLE agenturm_content_grants RENAME TO agenturm_prior_content_grants;
                 CREATE TABLE agenturm_content_grants (
                    source_id TEXT NOT NULL,
                    kind TEXT NOT NULL CHECK (kind IN ('pi.user_text', 'codex.user_text', 'claude.user_text', 'kiro.user_text', 'opencode.user_text', 'codex.mcp_payload', 'codex.edit_detail', 'codex.command_detail', 'telemetry.span_payload', 'eval.result_detail', 'telemetry.log_text')),
                    source_fingerprint BLOB NOT NULL CHECK (length(source_fingerprint) = 32),
                    epoch INTEGER NOT NULL CHECK (typeof(epoch) = 'integer' AND epoch > 0),
                    nonce BLOB NOT NULL CHECK (length(nonce) = 16),
                    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
                    PRIMARY KEY (source_id, kind)
                 );
                 INSERT INTO agenturm_content_grants
                    SELECT * FROM agenturm_prior_content_grants;
                 DROP TABLE agenturm_prior_content_grants;
                 UPDATE agenturm_store_meta SET value='agenturm.observation-store.v22' WHERE key='format';
                 PRAGMA user_version=22;",
            )
            .unwrap();
        assert!(store.grant_content(&claude).is_err());
        drop(store);

        let mut upgraded = Store::open(&db.0).unwrap();
        validate_marked_shape(&upgraded.connection, SCHEMA_VERSION).unwrap();
        upgraded.verify_content_grant(&codex, &grant).unwrap();
        let renewed = upgraded.grant_content(&claude).unwrap();
        upgraded.verify_content_grant(&claude, &renewed).unwrap();
        assert_eq!(
            upgraded
                .current_content_grant(&codex)
                .unwrap()
                .unwrap()
                .epoch(),
            grant.epoch()
        );
    }

    #[test]
    fn opencode_edit_grants_upgrade_from_v23_and_keep_prior_approvals() {
        let db = TempDb::new();
        let claude = ContentGrantScope::new(
            ExtensionId::parse("harness.claude").unwrap(),
            ContentKind::ClaudeEditDetail,
            [2; 32],
        );
        let opencode = ContentGrantScope::new(
            ExtensionId::parse("harness.opencode").unwrap(),
            ContentKind::OpenCodeEditDetail,
            [3; 32],
        );
        let mut store = Store::open(&db.0).unwrap();
        let grant = store.grant_content(&claude).unwrap();
        // Rebuild the exact v23 grant table so a pre-upgrade database again
        // rejects opencode.edit_detail under its narrower CHECK rule.
        store
            .connection
            .execute_batch(
                "ALTER TABLE agenturm_content_grants RENAME TO agenturm_prior_content_grants;
                 CREATE TABLE agenturm_content_grants (
                    source_id TEXT NOT NULL,
                    kind TEXT NOT NULL CHECK (kind IN ('pi.user_text', 'codex.user_text', 'claude.user_text', 'kiro.user_text', 'opencode.user_text', 'codex.mcp_payload', 'codex.edit_detail', 'claude.edit_detail', 'codex.command_detail', 'telemetry.span_payload', 'eval.result_detail', 'telemetry.log_text')),
                    source_fingerprint BLOB NOT NULL CHECK (length(source_fingerprint) = 32),
                    epoch INTEGER NOT NULL CHECK (typeof(epoch) = 'integer' AND epoch > 0),
                    nonce BLOB NOT NULL CHECK (length(nonce) = 16),
                    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
                    PRIMARY KEY (source_id, kind)
                 );
                 INSERT INTO agenturm_content_grants
                    SELECT * FROM agenturm_prior_content_grants;
                 DROP TABLE agenturm_prior_content_grants;
                 UPDATE agenturm_store_meta SET value='agenturm.observation-store.v23' WHERE key='format';
                 PRAGMA user_version=23;",
            )
            .unwrap();
        assert!(store.grant_content(&opencode).is_err());
        drop(store);

        let mut upgraded = Store::open(&db.0).unwrap();
        validate_marked_shape(&upgraded.connection, SCHEMA_VERSION).unwrap();
        upgraded.verify_content_grant(&claude, &grant).unwrap();
        let renewed = upgraded.grant_content(&opencode).unwrap();
        upgraded.verify_content_grant(&opencode, &renewed).unwrap();
        assert_eq!(
            upgraded
                .current_content_grant(&claude)
                .unwrap()
                .unwrap()
                .epoch(),
            grant.epoch()
        );
    }

    #[test]
    fn opencode_file_change_evidence_requires_its_own_grant_kind_and_proofs() {
        use agenturm_kernel::{
            FileChange, Observation, ObservationIdentity, ObservationKind, ObservationStore,
        };
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let source = ExtensionId::parse("harness.opencode").unwrap();
        let user_text =
            ContentGrantScope::new(source.clone(), ContentKind::OpenCodeUserText, [1; 32]);
        let edit_detail =
            ContentGrantScope::new(source.clone(), ContentKind::OpenCodeEditDetail, [1; 32]);
        let grant = store.grant_content(&edit_detail).unwrap();
        let cursor = format!(
            "oc5:{}:{}:{}:0:m:0:1:0:1:1:0:1:{}",
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64),
            "d".repeat(64)
        );
        let mut identity = ObservationIdentity::session("session-one");
        identity.turn_id = Some("turn-one".into());
        identity.operation_id = Some("operation-one".into());
        let change = Observation {
            harness_id: source.clone(),
            event_id: "patch-event".into(),
            revision: 0,
            identity,
            observed_at_unix_ns: 30,
            kind: ObservationKind::FileChange(FileChange {
                files: 2,
                digest: "a".repeat(64),
            }),
        };
        store
            .commit(
                source.as_str(),
                None,
                &cursor,
                std::slice::from_ref(&change),
            )
            .unwrap();
        let selection = CodexOperationSelection {
            session_id: "session-one",
            event_id: "patch-event",
            revision: 0,
            operation_id: "operation-one",
            turn_id: Some("turn-one"),
        };
        let evidence = store
            .with_user_content_fence(|store| {
                store.current_codex_operation_evidence(&edit_detail, &grant, &selection)
            })
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(evidence.cursor, cursor);
        assert_eq!(evidence.digest.as_deref(), Some("a".repeat(64).as_str()));
        assert_eq!(evidence.files, Some(2));
        // A user-text approval grants no edit-detail access.
        let text_grant = store.grant_content(&user_text).unwrap();
        assert!(
            store
                .with_user_content_fence(|store| {
                    store.current_codex_operation_evidence(&user_text, &text_grant, &selection)
                })
                .unwrap()
                .is_err()
        );
        // Any newer revision retires the selected occurrence.
        let mut superseded = change.clone();
        superseded.revision = 1;
        store
            .commit(source.as_str(), Some(&cursor), "newer", &[superseded])
            .unwrap();
        assert!(
            store
                .with_user_content_fence(|store| {
                    store.current_codex_operation_evidence(&edit_detail, &grant, &selection)
                })
                .unwrap()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn v24_to_v26_preserves_legacy_grants_and_separates_pi_edit_from_claude_mcp() {
        use agenturm_kernel::{
            FileChange, McpMethod, Observation, ObservationIdentity, ObservationKind,
        };

        let db = TempDb::new();
        let source = ExtensionId::parse("harness.provider").unwrap();
        let codex_mcp =
            ContentGrantScope::new(source.clone(), ContentKind::CodexMcpPayload, [1; 32]);
        let claude_edit =
            ContentGrantScope::new(source.clone(), ContentKind::ClaudeEditDetail, [2; 32]);
        let pi_edit = ContentGrantScope::new(source.clone(), ContentKind::PiEditDetail, [3; 32]);
        let claude_mcp =
            ContentGrantScope::new(source.clone(), ContentKind::ClaudeMcpPayload, [4; 32]);
        let mut store = Store::open(&db.0).unwrap();
        let prior_codex_mcp = store.grant_content(&codex_mcp).unwrap();
        let prior_claude_edit = store.grant_content(&claude_edit).unwrap();

        // Recreate the v24 grant constraint while retaining exactly the rows
        // that v24 could represent, then exercise Store::open's real two-step
        // v24 -> v25 -> v26 upgrade path.
        let transaction = store.connection.transaction().unwrap();
        content_grant::migrate_grant_schema(&transaction, 24).unwrap();
        transaction
            .execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V24],
            )
            .unwrap();
        transaction.pragma_update(None, "user_version", 24).unwrap();
        transaction.commit().unwrap();
        drop(store);

        let mut store = Store::open(&db.0).unwrap();
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            26
        );
        store
            .verify_content_grant(&codex_mcp, &prior_codex_mcp)
            .unwrap();
        store
            .verify_content_grant(&claude_edit, &prior_claude_edit)
            .unwrap();
        assert!(store.current_content_grant(&pi_edit).unwrap().is_none());
        assert!(store.current_content_grant(&claude_mcp).unwrap().is_none());

        let pi_grant = store.grant_content(&pi_edit).unwrap();
        let claude_mcp_grant = store.grant_content(&claude_mcp).unwrap();
        store.verify_content_grant(&pi_edit, &pi_grant).unwrap();
        store
            .verify_content_grant(&claude_mcp, &claude_mcp_grant)
            .unwrap();
        assert!(
            store
                .verify_content_grant(&pi_edit, &claude_mcp_grant)
                .is_err()
        );
        assert!(store.verify_content_grant(&claude_mcp, &pi_grant).is_err());

        let mut identity = ObservationIdentity::session("session-one");
        identity.turn_id = Some("turn-one".into());
        identity.operation_id = Some("pi-operation".into());
        let pi_change = Observation {
            harness_id: source.clone(),
            event_id: "pi-edit-event".into(),
            revision: 0,
            identity: identity.clone(),
            observed_at_unix_ns: 10,
            kind: ObservationKind::FileChange(FileChange {
                files: 1,
                digest: "a".repeat(64),
            }),
        };
        identity.operation_id = Some("claude-operation".into());
        let claude_call = Observation {
            harness_id: source.clone(),
            event_id: "claude-mcp-event".into(),
            revision: 0,
            identity,
            observed_at_unix_ns: 20,
            kind: ObservationKind::McpCall {
                server: Some("server".into()),
                tool: Some("lookup".into()),
                method: McpMethod::ToolsCall,
                duration_ns: None,
                result: "started".into(),
            },
        };
        store
            .commit(
                source.as_str(),
                None,
                "v2:fixture-receipt",
                &[pi_change, claude_call],
            )
            .unwrap();
        let pi_selection = CodexOperationSelection {
            session_id: "session-one",
            event_id: "pi-edit-event",
            revision: 0,
            operation_id: "pi-operation",
            turn_id: Some("turn-one"),
        };
        let claude_selection = CodexOperationSelection {
            session_id: "session-one",
            event_id: "claude-mcp-event",
            revision: 0,
            operation_id: "claude-operation",
            turn_id: Some("turn-one"),
        };
        assert!(
            store
                .with_user_content_fence(|store| store.current_codex_operation_evidence(
                    &pi_edit,
                    &pi_grant,
                    &pi_selection,
                ))
                .unwrap()
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .with_user_content_fence(|store| store.current_codex_operation_evidence(
                    &claude_mcp,
                    &claude_mcp_grant,
                    &claude_selection,
                ))
                .unwrap()
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .with_user_content_fence(|store| store.current_codex_operation_evidence(
                    &pi_edit,
                    &claude_mcp_grant,
                    &pi_selection,
                ))
                .unwrap()
                .is_err()
        );
        assert!(
            store
                .with_user_content_fence(|store| store.current_codex_operation_evidence(
                    &claude_mcp,
                    &pi_grant,
                    &claude_selection,
                ))
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn pi_edit_detail_grant_is_independent_default_denied_and_revocable() {
        let mut store = Store::open(":memory:").unwrap();
        let source = ExtensionId::parse("harness.pi.review-fixture").unwrap();
        let pi_edit = ContentGrantScope::new(source.clone(), ContentKind::PiEditDetail, [7; 32]);
        let codex_edit =
            ContentGrantScope::new(source.clone(), ContentKind::CodexEditDetail, [8; 32]);
        assert!(store.current_content_grant(&pi_edit).unwrap().is_none());

        let grant = store.grant_content(&pi_edit).unwrap();
        store.verify_content_grant(&pi_edit, &grant).unwrap();
        assert!(store.verify_content_grant(&codex_edit, &grant).is_err());
        assert!(
            store
                .revoke_content(&source, ContentKind::PiEditDetail)
                .unwrap()
        );
        assert!(store.current_content_grant(&pi_edit).unwrap().is_none());
        assert!(store.verify_content_grant(&pi_edit, &grant).is_err());
        let renewed = store.grant_content(&pi_edit).unwrap();
        assert_ne!(renewed, grant);
        store.verify_content_grant(&pi_edit, &renewed).unwrap();
    }

    #[test]
    fn native_cost_requires_the_exact_assistant_in_its_admitted_batch() {
        use agenturm_kernel::{NativeCostBasis, ReportedCost};
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let mut message = event("assistant-message", "session-1", 10);
        message.kind = ObservationKind::Message {
            role: "assistant".into(),
        };
        message.identity.operation_id = None;
        let mut cost = message.clone();
        cost.event_id = "assistant-message:cost".into();
        cost.identity.operation_id = Some(message.event_id.clone());
        cost.kind = ObservationKind::ReportedCost(
            ReportedCost::native_amount(
                "USD",
                NativeCostBasis::Estimate,
                serde_json::from_str("0").unwrap(),
            )
            .unwrap(),
        );
        let mut sibling = message.clone();
        sibling.identity.turn_id = Some("sibling-turn".into());
        let mut another_revision = message.clone();
        another_revision.revision += 1;
        let mut user = message.clone();
        user.kind = ObservationKind::Message {
            role: "user".into(),
        };
        for (request, rows) in [
            ("missing", vec![cost.clone()]),
            ("wrong-turn", vec![cost.clone(), sibling]),
            ("wrong-revision", vec![cost.clone(), another_revision]),
            ("wrong-role", vec![cost.clone(), user]),
        ] {
            assert!(matches!(
                store.commit_batch(&batch(request, rows)),
                Err(StoreError::Invalid(
                    "native cost has no exact assistant message"
                ))
            ));
        }
        let committed = batch("correctly-anchored", vec![cost, message]);
        assert_eq!(
            store.commit_batch(&committed).unwrap(),
            CommitOutcome::Committed { inserted: 2 }
        );
        assert_eq!(
            store.commit_batch(&committed).unwrap(),
            CommitOutcome::Replay
        );
    }

    #[test]
    fn v16_read_only_preserves_cursor_and_existing_grant_through_v17_upgrade() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let scope = TelemetryBinding {
            source_id: ExtensionId::parse("otlp.fixture").unwrap(),
            native_source_id: None,
        }
        .grant_scope()
        .unwrap();
        let grant = store.grant_content(&scope).unwrap();
        store
            .commit_batch(&batch("original", vec![event("original", "session", 10)]))
            .unwrap();
        let identity = store.history_identity().unwrap();
        let query = ObservationQuery::latest(1);
        let snapshot = store.query_observations(&query).unwrap().snapshot_id;
        let tx = store.connection.transaction().unwrap();
        tx.execute_batch(
            "DROP TABLE agenturm_native_attention_events; DROP TABLE agenturm_native_attention_senders;
             DROP INDEX agenturm_metric_descriptions_by_series;
             ALTER TABLE agenturm_history_batches DROP COLUMN admitted_at_unix_ns;
             DROP INDEX agenturm_metric_series_points;
             DROP INDEX agenturm_usage_history_by_time;
             DROP TABLE agenturm_eval_details; DROP TABLE agenturm_eval_detail_admissions;",
        )
        .unwrap();
        content_grant::migrate_grant_schema(&tx, 16).unwrap();
        tx.execute(
            "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
            [STORE_MARKER_V16],
        )
        .unwrap();
        tx.pragma_update(None, "user_version", 16).unwrap();
        tx.commit().unwrap();
        drop(store);

        let reader = Store::open_read_only(&db.0).unwrap();
        reader.verify_content_grant(&scope, &grant).unwrap();
        assert_eq!(
            reader.query_observations(&query).unwrap().snapshot_id,
            snapshot
        );
        assert_eq!(
            reader
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            16
        );
        drop(reader);
        let upgraded = Store::open(&db.0).unwrap();
        upgraded.verify_content_grant(&scope, &grant).unwrap();
        assert_eq!(upgraded.history_identity().unwrap(), identity);
        assert_eq!(
            upgraded.query_observations(&query).unwrap().snapshot_id,
            snapshot
        );
        assert_eq!(
            upgraded
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        eval_detail::validate_schema(&upgraded.connection).unwrap();
    }

    #[test]
    fn v17_upgrade_preserves_existing_eval_tables_and_pinned_history() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        store
            .commit_batch(&batch("prior", vec![event("prior", "session", 10)]))
            .unwrap();
        let query = ObservationQuery::latest(1);
        let snapshot = store.query_observations(&query).unwrap().snapshot_id;
        let identity = store.history_identity().unwrap();
        store.connection.execute_batch("DROP TABLE agenturm_native_attention_events; DROP TABLE agenturm_native_attention_senders;
             DROP INDEX agenturm_metric_descriptions_by_series;
             ALTER TABLE agenturm_history_batches DROP COLUMN admitted_at_unix_ns;
             DROP INDEX agenturm_metric_series_points; DROP INDEX agenturm_usage_history_by_time;").unwrap();
        store
            .connection
            .execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V17],
            )
            .unwrap();
        store
            .connection
            .pragma_update(None, "user_version", 17)
            .unwrap();
        drop(store);
        let reader = Store::open_read_only(&db.0).unwrap();
        assert_eq!(
            reader.query_observations(&query).unwrap().snapshot_id,
            snapshot
        );
        drop(reader);
        let upgraded = Store::open(&db.0).unwrap();
        assert_eq!(upgraded.history_identity().unwrap(), identity);
        assert_eq!(
            upgraded.query_observations(&query).unwrap().snapshot_id,
            snapshot
        );
        validate_marked_shape(&upgraded.connection, SCHEMA_VERSION).unwrap();
    }

    #[test]
    fn watermark_allocation_rejects_an_overflowing_retention_offset_atomically() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        store
            .connection
            .execute(
                "UPDATE agenturm_retention_state SET snapshot_offset=?1, minimum_snapshot_id=?1",
                [i64::MAX],
            )
            .unwrap();
        let prior = store
            .query_observations(&ObservationQuery::latest(1))
            .unwrap();
        assert_eq!(prior.snapshot_id.watermark(), i64::MAX as u64);
        assert!(matches!(
            store.commit_batch(&batch("overflow", vec![event("overflow", "session", 10)])),
            Err(StoreError::Invalid("history snapshot ID is exhausted"))
        ));
        assert_eq!(
            store
                .query_observations(&ObservationQuery::latest(1))
                .unwrap()
                .snapshot_id,
            prior.snapshot_id
        );
        assert!(store.observations().unwrap().is_empty());
        assert!(store.source_cursor("harness.codex").unwrap().is_none());
    }

    #[test]
    fn mismatched_span_usage_scope_rejects_the_entire_batch() {
        use agenturm_kernel::{
            ObservationIdentity, ReportedCost, SpanTokenUsage, TraceSpan, TraceSpanIdentity,
            TraceStatus,
        };

        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let source = ExtensionId::parse("otlp.usage").unwrap();
        let trace = TraceSpanIdentity {
            trace_id: "11".repeat(16),
            span_id: "22".repeat(8),
        };
        let mut identity = ObservationIdentity::session("session");
        identity.turn_id = Some("turn".into());
        identity.worker_id = Some("worker".into());
        let row = |prefix: &str, kind| Observation {
            harness_id: source.clone(),
            event_id: format!("{prefix}:{}:{}", trace.trace_id, trace.span_id),
            revision: 0,
            identity: identity.clone(),
            observed_at_unix_ns: 2,
            kind,
        };
        let structural = row(
            "otlp",
            KernelObservationKind::TraceSpan(TraceSpan {
                trace_id: trace.trace_id.clone(),
                span_id: trace.span_id.clone(),
                parent_span_id: None,
                target_source_id: Some("harness.pi".into()),
                target_operation_id: None,
                started_at_unix_ns: 1,
                status: TraceStatus::Ok,
            }),
        );
        let cost = row(
            "otlp-cost",
            KernelObservationKind::ReportedCost(ReportedCost {
                currency: "USD".into(),
                nanounits: 0,
                span: Some(trace.clone()),
                native_origin: None,
            }),
        );
        let mut usage = row(
            "otlp-usage",
            KernelObservationKind::SpanTokenUsage(SpanTokenUsage {
                span: trace.clone(),
                target_source_id: Some("harness.pi".into()),
                input_tokens: Some(0),
                output_tokens: None,
                cache_read_input_tokens: None,
                cache_write_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: None,
            }),
        );
        usage.identity.worker_id = Some("sibling".into());
        let result = ObservationStore::commit(
            &mut store,
            source.as_str(),
            None,
            "next",
            &[structural, cost, usage],
        );
        assert!(result.unwrap_err().contains("has no exact structural span"));
        assert!(store.source_cursor(source.as_str()).unwrap().is_none());
        assert!(store.observation_page(10).unwrap().observations.is_empty());
    }

    fn downgrade_fixture_to_v9(connection: &Connection) {
        downgrade_fixture_to_v14(connection);
        connection
            .execute_batch(
                "DROP TABLE agenturm_content_cursor_proofs;
                 DROP TABLE agenturm_retired_observations;
                 DROP TABLE agenturm_retention_state;
                 UPDATE agenturm_store_meta SET value='agenturm.observation-store.v9'
                  WHERE key='format';
                 PRAGMA user_version=9;",
            )
            .unwrap();
    }

    #[test]
    fn selected_evaluations_use_current_metadata_for_the_exact_trace_branch() {
        use agenturm_kernel::{TraceSpan, TraceStatus};

        let db = TempDb::new();
        let backup = TempDb::new();
        let restored_path = TempDb::new();
        let left = ExtensionId::parse("otlp.left").unwrap();
        let right = ExtensionId::parse("otlp.right").unwrap();
        let trace_id = "11111111111111111111111111111111";
        let span_id = "2222222222222222";
        let append = |store: &mut Store, source: &ExtensionId, timestamp| {
            store
                .commit(
                    source.as_str(),
                    None,
                    "trace",
                    &[Observation {
                        harness_id: source.clone(),
                        event_id: format!("otlp:{trace_id}:{span_id}"),
                        revision: 0,
                        identity: agenturm_kernel::ObservationIdentity::session("session"),
                        observed_at_unix_ns: timestamp,
                        kind: KernelObservationKind::TraceSpan(TraceSpan {
                            trace_id: trace_id.into(),
                            span_id: span_id.into(),
                            parent_span_id: None,
                            target_source_id: None,
                            target_operation_id: None,
                            started_at_unix_ns: 1,
                            status: TraceStatus::Error,
                        }),
                    }],
                )
                .unwrap();
        };
        let outcome = |task: &str, passed| EvalOutcome {
            trace_id: trace_id.into(),
            passed,
            task_id: Some(task.into()),
            provider_model: None,
            scores: Vec::new(),
        };
        let mut store = Store::open(&db.0).unwrap();
        append(&mut store, &left, 2);
        store.backup_to(&backup.0).unwrap();
        let first = store.observation_page(1).unwrap().snapshot_id;
        let query = |evidence_snapshot, before_revision| SelectedEvalQuery {
            trace_id: trace_id.into(),
            evidence_snapshot,
            view: EvalQuery::History {
                trace_id: trace_id.into(),
                before_revision,
                limit: 1,
            },
        };
        let empty = store
            .query_selected_eval_outcomes(&left, &query(first, None))
            .unwrap()
            .unwrap();
        assert!(empty.page.outcomes.is_empty());
        assert_eq!(empty.current_revision, None);
        append(&mut store, &right, 30);
        let second = store.observation_page(1).unwrap().snapshot_id;
        assert_eq!(
            store
                .query_selected_eval_outcomes(&right, &query(first, None))
                .unwrap(),
            None
        );
        store
            .import_eval_outcomes(&left, &[outcome("left-first", false)])
            .unwrap();
        store
            .import_eval_outcomes(&left, &[outcome("left-current", true)])
            .unwrap();
        store
            .import_eval_outcomes(&right, &[outcome("right", false)])
            .unwrap();
        let page = store
            .query_selected_eval_outcomes(&left, &query(first, None))
            .unwrap()
            .unwrap();
        assert_eq!(page.current_revision, Some(2));
        assert_eq!(page.page.next_before_revision, Some(2));
        assert_eq!(page.page.outcomes[0].outcome, outcome("left-current", true));
        let older = store
            .query_selected_eval_outcomes(&left, &query(first, Some(2)))
            .unwrap()
            .unwrap();
        assert_eq!(older.current_revision, Some(2));
        assert_eq!(older.page.outcomes[0].outcome, outcome("left-first", false));
        let summary = SelectedEvalQuery {
            view: EvalQuery::Latest {
                after_trace_id: None,
                limit: 8,
            },
            ..query(second, None)
        };
        assert_eq!(
            store
                .query_selected_eval_outcomes(&right, &summary)
                .unwrap()
                .unwrap()
                .page
                .outcomes[0]
                .outcome,
            outcome("right", false)
        );

        store.prune_observations_before(3).unwrap();
        assert!(matches!(
            store.query_selected_eval_outcomes(&left, &query(second, None)),
            Err(StoreError::ExpiredHistorySnapshot { .. })
        ));
        Store::open_read_only(&backup.0)
            .unwrap()
            .backup_to(&restored_path.0)
            .unwrap();
        let mut restored = Store::open(&restored_path.0).unwrap();
        append(&mut restored, &right, 30);
        let regrown = restored.observation_page(1).unwrap().snapshot_id;
        assert_eq!(regrown.watermark(), second.watermark());
        assert_ne!(regrown.incarnation(), second.incarnation());
        restored
            .import_eval_outcomes(&left, &[outcome("restored", false)])
            .unwrap();
        assert!(matches!(
            restored.query_selected_eval_outcomes(&left, &query(second, None)),
            Err(StoreError::SnapshotReplaced)
        ));
        let page = restored
            .query_selected_eval_outcomes(&left, &query(regrown, None))
            .unwrap()
            .unwrap();
        assert_eq!(page.page.outcomes[0].outcome, outcome("restored", false));
    }

    #[test]
    fn v14_migration_retains_trace_membership_and_command_approval() {
        use agenturm_kernel::{ExtensionId, ObservationIdentity, TraceSpan, TraceStatus};

        let db = TempDb::new();
        let source = ExtensionId::parse("otlp.evaluation").unwrap();
        let trace_id = "11111111111111111111111111111111";
        let span_id = "2222222222222222";
        let command = ContentGrantScope::new(
            ExtensionId::parse("harness.codex").unwrap(),
            ContentKind::CodexCommandDetail,
            [1; 32],
        );
        let mut store = Store::open(&db.0).unwrap();
        let grant = store.grant_content(&command).unwrap();
        store
            .commit(
                source.as_str(),
                None,
                "trace",
                &[Observation {
                    harness_id: source.clone(),
                    event_id: format!("otlp:{trace_id}:{span_id}"),
                    revision: 0,
                    identity: ObservationIdentity::session("session-1"),
                    observed_at_unix_ns: 2,
                    kind: agenturm_kernel::ObservationKind::TraceSpan(TraceSpan {
                        trace_id: trace_id.into(),
                        span_id: span_id.into(),
                        parent_span_id: None,
                        target_source_id: None,
                        target_operation_id: None,
                        started_at_unix_ns: 1,
                        status: TraceStatus::Ok,
                    }),
                }],
            )
            .unwrap();
        downgrade_fixture_to_v14(&store.connection);
        drop(store);

        let reader = Store::open_read_only(&db.0).unwrap();
        assert!(reader.eval_outcome(&source, trace_id).unwrap().is_none());
        reader.verify_content_grant(&command, &grant).unwrap();
        drop(reader);

        let outcome = EvalOutcome {
            trace_id: trace_id.into(),
            passed: false,
            task_id: None,
            provider_model: None,
            scores: Vec::new(),
        };
        let mut store = Store::open_collector(&db.0).unwrap();
        store.verify_content_grant(&command, &grant).unwrap();
        assert_eq!(
            store
                .import_eval_outcomes(&source, std::slice::from_ref(&outcome))
                .unwrap()
                .inserted,
            1
        );
        let stored = store.eval_outcome(&source, trace_id).unwrap().unwrap();
        assert_eq!(stored.outcome, outcome);
        assert_eq!(stored.revision, 1);
        store.prune_observations_before(3).unwrap();
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT count(*), count(outcome_json) FROM agenturm_eval_outcomes",
                    [],
                    |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
                )
                .unwrap(),
            (1, 0)
        );
    }

    #[test]
    fn v11_approval_survives_upgrade_without_authorizing_mcp_payloads() {
        let db = TempDb::new();
        let source = agenturm_kernel::ExtensionId::parse("harness.codex").unwrap();
        let prompt = ContentGrantScope::new(source.clone(), ContentKind::CodexUserText, [1; 32]);
        let mcp = ContentGrantScope::new(source, ContentKind::CodexMcpPayload, [2; 32]);

        let mut store = Store::open(&db.0).unwrap();
        let existing = store.grant_content(&prompt).unwrap();
        let transaction = store.connection.transaction().unwrap();
        downgrade_fixture_to_v14(&transaction);
        content_grant::migrate_grant_schema(&transaction, 11).unwrap();
        transaction
            .execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V11],
            )
            .unwrap();
        transaction.pragma_update(None, "user_version", 11).unwrap();
        transaction.commit().unwrap();
        drop(store);

        let mut reopened = Store::open(&db.0).unwrap();
        reopened.verify_content_grant(&prompt, &existing).unwrap();
        assert!(reopened.current_content_grant(&mcp).unwrap().is_none());
        let mcp_grant = reopened.grant_content(&mcp).unwrap();
        reopened.verify_content_grant(&mcp, &mcp_grant).unwrap();
    }

    #[test]
    fn v12_mcp_approval_survives_upgrade_without_authorizing_edit_detail() {
        let db = TempDb::new();
        let source = agenturm_kernel::ExtensionId::parse("harness.codex").unwrap();
        let mcp = ContentGrantScope::new(source.clone(), ContentKind::CodexMcpPayload, [1; 32]);
        let edit = ContentGrantScope::new(source, ContentKind::CodexEditDetail, [2; 32]);
        let mut store = Store::open(&db.0).unwrap();
        let existing = store.grant_content(&mcp).unwrap();
        let transaction = store.connection.transaction().unwrap();
        downgrade_fixture_to_v14(&transaction);
        content_grant::migrate_grant_schema(&transaction, 12).unwrap();
        transaction
            .execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V12],
            )
            .unwrap();
        transaction.pragma_update(None, "user_version", 12).unwrap();
        transaction.commit().unwrap();
        drop(store);
        let mut reopened = Store::open(&db.0).unwrap();
        reopened.verify_content_grant(&mcp, &existing).unwrap();
        assert!(reopened.current_content_grant(&edit).unwrap().is_none());
        let edit_grant = reopened.grant_content(&edit).unwrap();
        reopened.verify_content_grant(&edit, &edit_grant).unwrap();
    }

    #[test]
    fn v13_edit_approval_survives_upgrade_without_authorizing_commands() {
        let db = TempDb::new();
        let source = agenturm_kernel::ExtensionId::parse("harness.codex").unwrap();
        let edit = ContentGrantScope::new(source.clone(), ContentKind::CodexEditDetail, [1; 32]);
        let command = ContentGrantScope::new(source, ContentKind::CodexCommandDetail, [2; 32]);
        let mut store = Store::open(&db.0).unwrap();
        let existing = store.grant_content(&edit).unwrap();
        let transaction = store.connection.transaction().unwrap();
        downgrade_fixture_to_v14(&transaction);
        content_grant::migrate_grant_schema(&transaction, 13).unwrap();
        transaction
            .execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [STORE_MARKER_V13],
            )
            .unwrap();
        transaction.pragma_update(None, "user_version", 13).unwrap();
        transaction.commit().unwrap();
        drop(store);
        let read_only = Store::open_read_only(&db.0).unwrap();
        assert!(read_only.current_content_grant(&command).unwrap().is_none());
        drop(read_only);
        let mut reopened = Store::open(&db.0).unwrap();
        reopened.verify_content_grant(&edit, &existing).unwrap();
        assert!(reopened.current_content_grant(&command).unwrap().is_none());
        let grant = reopened.grant_content(&command).unwrap();
        assert_ne!(grant.binding_fingerprint(), existing.binding_fingerprint());
        drop(reopened);
        Store::open_read_only(&db.0)
            .unwrap()
            .verify_content_grant(&command, &grant)
            .unwrap();
    }

    fn downgrade_fixture_to_v7(connection: &Connection) {
        downgrade_fixture_to_v9(connection);
        connection
            .execute_batch(
                "DROP TABLE agenturm_content_grants;
                 DROP INDEX agenturm_observations_by_session;
                 CREATE TABLE agenturm_observations_v7 (
                    source_id TEXT NOT NULL,
                    event_id TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision >= 0),
                    request_id TEXT NOT NULL,
                    event_digest TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    worker_id TEXT,
                    turn_id TEXT,
                    operation_id TEXT,
                    observed_at_unix_ns INTEGER NOT NULL CHECK (observed_at_unix_ns >= 0),
                    kind_json TEXT NOT NULL,
                    PRIMARY KEY (source_id, event_id, revision),
                    FOREIGN KEY (request_id) REFERENCES agenturm_ingest_receipts(request_id)
                        DEFERRABLE INITIALLY DEFERRED
                 );
                 INSERT INTO agenturm_observations_v7
                    (source_id, event_id, revision, request_id, event_digest, session_id,
                     worker_id, turn_id, operation_id, observed_at_unix_ns, kind_json)
                 SELECT source_id, event_id, revision, request_id, event_digest, session_id,
                        worker_id, turn_id, operation_id, observed_at_unix_ns, kind_json
                   FROM agenturm_observations;
                 DROP TABLE agenturm_observations;
                 ALTER TABLE agenturm_observations_v7 RENAME TO agenturm_observations;
                 CREATE INDEX agenturm_observations_by_session
                    ON agenturm_observations (session_id, observed_at_unix_ns, event_id, revision);
                 UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v7' WHERE key='format';
                 PRAGMA user_version=7;",
            )
            .unwrap();
    }

    fn event(event_id: &str, session_id: &str, timestamp: u64) -> StoredObservation {
        StoredObservation {
            event_id: event_id.into(),
            revision: 0,
            identity: ObservationIdentity {
                scope: StoredScope::Session,
                session_id: Some(session_id.into()),
                worker_id: Some("worker-1".into()),
                turn_id: Some("turn-1".into()),
                operation_id: Some("operation-1".into()),
            },
            observed_at_unix_ns: timestamp,
            kind: ObservationKind::Activity {
                category: "tool".into(),
            },
        }
    }

    fn tool_event(event_id: &str, session_id: &str, timestamp: u64) -> StoredObservation {
        let mut observation = event(event_id, session_id, timestamp);
        observation.kind = ObservationKind::Tool {
            category: "shell".into(),
            result: "succeeded".into(),
        };
        observation
    }

    fn batch(request_id: &str, observations: Vec<StoredObservation>) -> ObservationBatch {
        ObservationBatch {
            request_id: request_id.into(),
            source_id: "harness.codex".into(),
            expected_cursor: None,
            next_cursor: "cursor-1".into(),
            observations,
        }
    }

    #[test]
    fn telemetry_summaries_and_logs_require_exact_structural_scope() {
        use agenturm_kernel::telemetry_summary::{OtlpActivityLog, TraceSpanSummary};
        let mut store = Store::open(":memory:").unwrap();
        let mut log = event(&format!("otlp-log:{}", "a".repeat(64)), "session", 10);
        log.identity.worker_id = None;
        log.identity.turn_id = None;
        log.identity.operation_id = None;
        log.kind = ObservationKind::OtlpActivityLog(OtlpActivityLog {
            reported_error: true,
            target_source_id: None,
            usage: None,
            cost: None,
        });
        let mut logged = batch("log", vec![log.clone()]);
        assert!(store.commit_batch(&logged).is_err());
        logged.source_id = "otlp.fixture".into();
        assert!(store.commit_batch(&logged).is_ok());
        let mut malformed = logged.clone();
        malformed.request_id = "malformed".into();
        malformed.expected_cursor = Some(logged.next_cursor.clone());
        malformed.observations[0].identity.turn_id = Some("turn-without-target".into());
        assert!(store.commit_batch(&malformed).is_err());
        let trace_id = "11".repeat(16);
        let span_id = "22".repeat(8);
        let mut summary = event(&format!("otlp:{trace_id}:{span_id}:summary"), "session", 10);
        summary.identity = log.identity.clone();
        summary.kind = ObservationKind::TraceSpanSummary(TraceSpanSummary {
            trace_id: trace_id.clone(),
            span_id: span_id.clone(),
            agent_run: false,
            reported_error: false,
        });
        let mut missing = batch("missing", vec![summary.clone()]);
        missing.source_id = "otlp.other".into();
        assert!(store.commit_batch(&missing).is_err());
        let mut anchor = summary.clone();
        anchor.event_id = format!("otlp:{trace_id}:{span_id}");
        anchor.kind = ObservationKind::TraceSpan(agenturm_kernel::TraceSpan {
            trace_id,
            span_id,
            parent_span_id: None,
            started_at_unix_ns: 1,
            status: agenturm_kernel::TraceStatus::Ok,
            target_source_id: None,
            target_operation_id: None,
        });
        missing.observations.push(anchor);
        assert!(store.commit_batch(&missing).is_ok());
        missing.observations[0].identity.session_id = Some("sibling".into());
        assert!(store.commit_batch(&missing).is_err());
    }

    #[test]
    fn host_history_enforces_type_scope_and_pins_pages() {
        let mut store = Store::open(":memory:").unwrap();
        let host = "host.fixture";
        let make_batch = |source: &str, event_id: &str, timestamp, previous: Option<&str>| {
            let mut row = event(event_id, "unused", timestamp);
            row.identity = ObservationIdentity {
                scope: StoredScope::Source,
                session_id: None,
                worker_id: None,
                turn_id: None,
                operation_id: None,
            };
            row.kind = ObservationKind::HostSample(agenturm_kernel::HostSample {
                hostname: None,
                platform: None,
                boot_id: "12345678-1234-1234-1234-123456789abc".into(),
                clock_ticks_per_second: Some(100),
                cpu: None,
                load_1m_milli: Some(0),
                load_5m_milli: None,
                load_15m_milli: None,
                memory_total_bytes: Some(1024),
                memory_available_bytes: None,
                processes: Vec::new(),
                processes_partial: false,
            });
            ObservationBatch {
                source_id: source.into(),
                request_id: event_id.into(),
                expected_cursor: previous.map(str::to_owned),
                next_cursor: event_id.into(),
                observations: vec![row],
            }
        };
        let mut invalid = make_batch("harness.codex", "invalid", 1, None);
        assert!(matches!(
            store.commit_batch(&invalid),
            Err(StoreError::Invalid(_))
        ));
        invalid.source_id = host.into();
        invalid.observations[0].identity = event("unused", "session", 1).identity;
        assert!(matches!(
            store.commit_batch(&invalid),
            Err(StoreError::Invalid(_))
        ));

        store
            .commit_batch(&make_batch(host, "first", 1, None))
            .unwrap();
        store
            .commit_batch(&make_batch(host, "second", 2, Some("first")))
            .unwrap();
        assert!(store.observation_page(20).unwrap().observations.is_empty());
        let page = store
            .query_host_history(&ObservationQuery::latest(1), None)
            .unwrap();
        assert_eq!(page.observations[0].event_id, "second");
        let next = page.older_position().unwrap();
        store
            .commit_batch(&make_batch(host, "third", 3, Some("second")))
            .unwrap();
        let query = ObservationQuery {
            before: Some(next),
            snapshot_id: Some(page.snapshot_id),
            ..ObservationQuery::latest(1)
        };
        let page = store.query_host_history(&query, None).unwrap();
        assert_eq!(page.observations[0].event_id, "first");
        assert!(
            store
                .query_host_history(&query, Some(&BTreeSet::new()))
                .unwrap()
                .observations
                .is_empty()
        );
    }

    fn batch_after(
        request_id: &str,
        expected_cursor: &str,
        next_cursor: &str,
        observations: Vec<StoredObservation>,
    ) -> ObservationBatch {
        ObservationBatch {
            request_id: request_id.into(),
            source_id: "harness.codex".into(),
            expected_cursor: Some(expected_cursor.into()),
            next_cursor: next_cursor.into(),
            observations,
        }
    }

    fn discovery(event_id: &str, timestamp: u64) -> StoredObservation {
        let mut observation = event(event_id, "session-1", timestamp);
        observation.identity.turn_id = None;
        observation.identity.operation_id = None;
        observation.kind = ObservationKind::Activity {
            category: "session_discovered".into(),
        };
        observation
    }

    #[test]
    fn session_discovery_retention_uses_original_admission_across_replay_and_backup() {
        let db = TempDb::new();
        let archive = TempDb::new();
        let mut store = Store::open_collector(&db.0).unwrap();
        let mut noncanonical = discovery("turn-bound", 0);
        noncanonical.identity.turn_id = Some("turn-1".into());
        let ordinary = batch(
            "ordinary",
            vec![
                event("unrelated-unknown", "session-1", 0),
                noncanonical,
                discovery("native-timestamp", 1),
            ],
        );
        store.commit_batch(&ordinary).unwrap();
        assert_eq!(
            store
                .prune_observations_before(2)
                .unwrap()
                .removed_observations,
            3
        );

        let unknown = discovery("unknown-timestamp", 0);
        let next = batch_after(
            "discovery",
            "cursor-1",
            "cursor-2",
            vec![
                unknown.clone(),
                event("new-unrelated-unknown", "session-1", 0),
            ],
        );
        assert_eq!(
            store.commit_batch(&next).unwrap(),
            CommitOutcome::Committed { inserted: 1 }
        );
        assert_eq!(
            store.observations().unwrap(),
            [("harness.codex".into(), unknown.clone())]
        );
        let age =
            |store: &Store, request: &str| -> i64 {
                store.connection.query_row(
                "SELECT admitted_at_unix_ns FROM agenturm_history_batches WHERE request_id=?1",
                [request], |row| row.get(0),
            ).unwrap()
            };
        let first_age = age(&store, "discovery");
        assert!(first_age >= 2);
        assert_eq!(store.commit_batch(&next).unwrap(), CommitOutcome::Replay);
        assert_eq!(age(&store, "discovery"), first_age);
        let duplicate = batch_after("duplicate", "cursor-2", "cursor-3", vec![unknown]);
        assert_eq!(
            store.commit_batch(&duplicate).unwrap(),
            CommitOutcome::Committed { inserted: 0 }
        );
        store.connection.execute(
            "UPDATE agenturm_history_batches SET admitted_at_unix_ns=?1 WHERE request_id='duplicate'",
            [first_age + 2],
        ).unwrap();
        assert_eq!(
            store
                .prune_observations_before(first_age as u64)
                .unwrap()
                .removed_observations,
            0
        );
        store.backup_to(&archive.0).unwrap();
        let mut restored = Store::open_collector(&archive.0).unwrap();
        assert_eq!(age(&restored, "discovery"), first_age);
        assert_eq!(restored.observations().unwrap()[0].1.observed_at_unix_ns, 0);
        assert_eq!(
            restored
                .prune_observations_before(first_age as u64 + 1)
                .unwrap()
                .removed_observations,
            1
        );
        assert_eq!(restored.commit_batch(&next).unwrap(), CommitOutcome::Replay);
        assert_eq!(
            restored.commit_batch(&duplicate).unwrap(),
            CommitOutcome::Replay
        );
        let third = batch_after(
            "retired-duplicate",
            "cursor-3",
            "cursor-4",
            duplicate.observations,
        );
        assert_eq!(
            restored.commit_batch(&third).unwrap(),
            CommitOutcome::Committed { inserted: 0 }
        );
        assert!(restored.observations().unwrap().is_empty());
    }

    #[test]
    fn v21_history_admission_upgrade_preserves_unknown_age_and_receipts() {
        let db = TempDb::new();
        let mut store = Store::open_collector(&db.0).unwrap();
        let original = batch("original", vec![discovery("unknown", 0)]);
        store.commit_batch(&original).unwrap();
        let page = store.observation_page(10).unwrap();
        store.connection.execute_batch(
            "ALTER TABLE agenturm_history_batches DROP COLUMN admitted_at_unix_ns;
             UPDATE agenturm_store_meta SET value='agenturm.observation-store.v21' WHERE key='format';
             PRAGMA user_version=21;",
        ).unwrap();
        drop(store);

        let reader = Store::open_read_only(&db.0).unwrap();
        assert_eq!(reader.observation_page(10).unwrap(), page);
        let version = |store: &Store| -> i64 {
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap()
        };
        assert_eq!(version(&reader), 21);
        drop(reader);
        let mut writer = Store::open_collector(&db.0).unwrap();
        assert_eq!(version(&writer), SCHEMA_VERSION);
        assert_eq!(writer.observation_page(10).unwrap(), page);
        assert_eq!(
            writer.commit_batch(&original).unwrap(),
            CommitOutcome::Replay
        );
        assert_eq!(writer.connection.query_row(
            "SELECT admitted_at_unix_ns FROM agenturm_history_batches WHERE request_id='original'",
            [], |row| row.get::<_, i64>(0),
        ).unwrap(), 0);
        assert_eq!(
            writer
                .prune_observations_before(1)
                .unwrap()
                .removed_observations,
            1
        );
    }

    #[test]
    fn retention_preserves_replay_and_cursor_without_readmitting_expired_events() {
        let db = TempDb::new();
        let backup = TempDb::new();
        let expired = event("expired", "session-1", 10);
        let kept = event("kept", "session-1", 20);
        let initial = batch("initial", vec![expired.clone(), kept.clone()]);
        let mut store = Store::open_collector(&db.0).unwrap();
        store.commit_batch(&initial).unwrap();
        let prune = store.prune_observations_before(20).unwrap();
        assert_eq!(prune.removed_observations, 1);
        assert_eq!(
            store.observations().unwrap(),
            [("harness.codex".into(), kept)]
        );
        assert_eq!(
            store.source_cursor("harness.codex").unwrap().as_deref(),
            Some("cursor-1")
        );
        assert_eq!(
            store
                .prune_observations_before(20)
                .unwrap()
                .removed_observations,
            0
        );
        assert!(matches!(
            store.prune_observations_before(19),
            Err(StoreError::Invalid("retention cutoff cannot decrease"))
        ));
        store.backup_to(&backup.0).unwrap();

        let mut restored = Store::open_collector(&backup.0).unwrap();
        assert_eq!(
            restored.observations().unwrap(),
            store.observations().unwrap()
        );
        assert_eq!(
            restored.commit_batch(&initial).unwrap(),
            CommitOutcome::Replay
        );
        let late = event("late", "session-1", 11);
        let repeated = batch_after("next", "cursor-1", "cursor-2", vec![expired, late.clone()]);
        assert_eq!(
            restored.commit_batch(&repeated).unwrap(),
            CommitOutcome::Committed { inserted: 0 }
        );
        assert_eq!(
            restored.source_cursor("harness.codex").unwrap().as_deref(),
            Some("cursor-2")
        );
        assert_eq!(
            restored.commit_batch(&initial).unwrap(),
            CommitOutcome::Replay
        );

        let mut conflicting = late;
        conflicting.observed_at_unix_ns = 21;
        let conflict = batch_after("conflict", "cursor-2", "cursor-3", vec![conflicting]);
        assert!(
            matches!(restored.commit_batch(&conflict), Err(StoreError::EventConflict { event_id }) if event_id == "late")
        );
        assert_eq!(
            restored.source_cursor("harness.codex").unwrap().as_deref(),
            Some("cursor-2")
        );
        assert_eq!(restored.observations().unwrap().len(), 1);
    }

    #[test]
    fn retention_expires_old_snapshots_and_preserves_fresh_keyset_pages() {
        let db = TempDb::new();
        let mut writer = Store::open_collector(&db.0).unwrap();
        writer
            .commit_batch(&batch(
                "initial",
                vec![
                    event("expired", "session-1", 10),
                    event("middle", "session-1", 20),
                    event("newest", "session-1", 30),
                ],
            ))
            .unwrap();
        let reader = Store::open_read_only(&db.0).unwrap();
        let continued = |page: ObservationPage| {
            assert!(page.has_more);
            let first = &page.observations[0];
            ObservationQuery {
                before: Some(ObservationPosition {
                    observed_at_unix_ns: first.observed_at_unix_ns,
                    harness_id: first.harness_id.as_str().into(),
                    event_id: first.event_id.clone(),
                    revision: first.revision,
                    snapshot_id: page.snapshot_id,
                }),
                snapshot_id: Some(page.snapshot_id),
                ..ObservationQuery::latest(1)
            }
        };
        let old = continued(reader.observation_page(1).unwrap());
        let prune = writer.prune_observations_before(20).unwrap();
        assert!(
            matches!(reader.query_observations(&old), Err(StoreError::ExpiredHistorySnapshot { minimum_snapshot_id })
            if minimum_snapshot_id == prune.minimum_snapshot_id)
        );

        let page = reader.observation_page(1).unwrap();
        assert_eq!(page.snapshot_id, prune.minimum_snapshot_id);
        assert_eq!(page.observations[0].event_id, "newest");
        let next = continued(page);
        writer
            .commit_batch(&batch_after(
                "new",
                "cursor-1",
                "cursor-2",
                vec![event("added-later", "session-1", 25)],
            ))
            .unwrap();
        let older = reader.query_observations(&next).unwrap();
        assert_eq!(older.observations[0].event_id, "middle");
        assert!(!older.has_more);

        let second = writer.prune_observations_before(25).unwrap();
        assert!(second.minimum_snapshot_id.watermark() > prune.minimum_snapshot_id.watermark());
        assert!(matches!(
            reader.query_observations(&next),
            Err(StoreError::ExpiredHistorySnapshot { .. })
        ));
        assert_eq!(
            reader.observation_page(1).unwrap().snapshot_id,
            second.minimum_snapshot_id
        );
    }

    #[test]
    fn exact_snapshot_membership_accepts_maximum_revision_and_enforces_receipt_watermark() {
        let db = TempDb::new();
        let mut store = Store::open_collector(&db.0).unwrap();
        let source = agenturm_kernel::ExtensionId::parse("harness.codex").unwrap();
        let before = store.observation_page(1).unwrap().snapshot_id;
        let mut selected = event("maximum-revision", "session-1", 10);
        selected.revision = u32::MAX;
        store
            .commit_batch(&batch("maximum", vec![selected]))
            .unwrap();
        let visible = store.observation_page(1).unwrap().snapshot_id;
        let contains = |store: &Store, revision, snapshot| {
            store
                .with_user_content_fence(|store| {
                    store.contains_observation_at_snapshot(
                        &source,
                        "maximum-revision",
                        revision,
                        snapshot,
                    )
                })
                .unwrap()
        };
        assert!(!contains(&store, u32::MAX, before).unwrap());
        assert!(!contains(&store, u32::MAX - 1, visible).unwrap());
        assert!(contains(&store, u32::MAX, visible).unwrap());
        let retained = store.prune_observations_before(1).unwrap();
        assert_eq!(retained.removed_observations, 0);
        assert!(matches!(
            contains(&store, u32::MAX, visible),
            Err(StoreError::ExpiredHistorySnapshot { .. })
        ));
        assert!(contains(&store, u32::MAX, retained.minimum_snapshot_id).unwrap());
    }

    #[test]
    fn v9_readers_and_leased_migration_preserve_existing_snapshot_ids() {
        let db = TempDb::new();
        let initial = batch("initial", vec![event("event-1", "session-1", 10)]);
        let mut writer = Store::open_collector(&db.0).unwrap();
        writer.commit_batch(&initial).unwrap();
        downgrade_fixture_to_v9(&writer.connection);
        drop(writer);

        let reader = Store::open_read_only(&db.0).unwrap();
        let previous = reader.observation_page(1).unwrap();
        let mut writer = Store::open_collector(&db.0).unwrap();
        let migrated = reader.observation_page(1).unwrap();
        assert_eq!(migrated.snapshot_id, previous.snapshot_id);
        assert_eq!(migrated.observations, previous.observations);
        assert_eq!(
            writer.commit_batch(&initial).unwrap(),
            CommitOutcome::Replay
        );
        writer.prune_observations_before(11).unwrap();
        let pinned = ObservationQuery {
            snapshot_id: Some(previous.snapshot_id),
            ..ObservationQuery::latest(1)
        };
        assert!(matches!(
            reader.query_observations(&pinned),
            Err(StoreError::ExpiredHistorySnapshot { .. })
        ));
        assert!(reader.observations().unwrap().is_empty());
    }

    #[test]
    fn commits_once_and_exact_request_replay_is_idempotent() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let input = batch("request-1", vec![event("event-1", "session-1", 10)]);
        assert_eq!(
            store.commit_batch(&input).unwrap(),
            CommitOutcome::Committed { inserted: 1 }
        );
        assert_eq!(store.commit_batch(&input).unwrap(), CommitOutcome::Replay);
        assert_eq!(store.observations().unwrap().len(), 1);
    }

    #[test]
    fn typed_tool_lifecycle_check_and_message_observations_survive_store_reopen() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            let mut observation = tool_event("tool-event-1", "session-1", 10);
            observation.identity.operation_id = Some("opaque-operation-1".into());
            let mut lifecycle = event("lifecycle-event-1", "session-1", 11);
            lifecycle.kind = ObservationKind::Lifecycle {
                state: "interrupted".into(),
                reason_code: Some("codex_task_aborted".into()),
            };
            let mut check = event("check-event-1", "session-1", 12);
            check.kind = ObservationKind::Check {
                category: "lint".into(),
                result: "passed".into(),
            };
            let mut message = event("message-event-1", "session-1", 13);
            message.kind = ObservationKind::Message {
                role: "user".into(),
            };
            store
                .commit_batch(&batch(
                    "tool-request-1",
                    vec![observation, lifecycle, check, message],
                ))
                .unwrap();
        }
        let store = Store::open_read_only(&db.0).unwrap();
        let page = store.observation_page(10).unwrap();
        assert_eq!(page.observations.len(), 4);
        let tool = page
            .observations
            .iter()
            .find(|row| matches!(row.kind, KernelObservationKind::Tool { .. }))
            .unwrap();
        assert_eq!(
            tool.kind,
            KernelObservationKind::Tool {
                category: "shell".into(),
                result: "succeeded".into(),
            }
        );
        assert_eq!(
            tool.identity.operation_id.as_deref(),
            Some("opaque-operation-1")
        );
        assert!(page.observations.iter().any(|row| {
            row.kind
                == KernelObservationKind::Lifecycle {
                    state: "interrupted".into(),
                    reason_code: Some("codex_task_aborted".into()),
                }
        }));
        assert!(page.observations.iter().any(|row| {
            row.kind
                == KernelObservationKind::Message {
                    role: "user".into(),
                }
        }));
        assert!(page.observations.iter().any(|row| {
            row.kind
                == KernelObservationKind::Check {
                    category: "lint".into(),
                    result: "passed".into(),
                }
        }));
    }

    #[test]
    fn bounded_observation_page_survives_reopen_and_reports_older_rows() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch(
                    "request-page-1",
                    vec![event("event-1", "session-1", 10)],
                ))
                .unwrap();
            store
                .commit_batch(&batch_after(
                    "request-page-2",
                    "cursor-1",
                    "cursor-2",
                    vec![event("event-2", "session-2", 20)],
                ))
                .unwrap();
        }

        let store = Store::open(&db.0).unwrap();
        let newest = store.observation_page(1).unwrap();
        assert!(newest.has_more);
        assert_eq!(newest.observations.len(), 1);
        assert_eq!(newest.observations[0].event_id, "event-2");
        assert_eq!(newest.observations[0].harness_id.as_str(), "harness.codex");

        let complete = store.observation_page(2).unwrap();
        assert!(!complete.has_more);
        assert_eq!(
            complete
                .observations
                .iter()
                .map(|row| row.event_id.as_str())
                .collect::<Vec<_>>(),
            ["event-1", "event-2"]
        );
        assert!(store.observation_page(0).is_err());
    }

    #[test]
    fn v2_store_upgrades_atomically_to_explicit_history_sequences() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch(
                    "upgrade-request",
                    vec![event("event-1", "session-1", 1)],
                ))
                .unwrap();
            downgrade_fixture_to_v7(&store.connection);
            store
                .connection
                .execute_batch(
                    "DROP TABLE agenturm_history_batches;
                 UPDATE agenturm_store_meta SET value='agenturm.observation-store.v2'
                   WHERE key='format';
                 PRAGMA user_version=2;",
                )
                .unwrap();
        }

        let store = Store::open(&db.0).unwrap();
        let page = store.observation_page(10).unwrap();
        assert_eq!(page.snapshot_id, 1.into());
        assert_eq!(page.observations.len(), 1);
        let version: i64 = store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn v3_store_upgrades_to_current_schema_without_changing_history() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch("v3-request", vec![event("v3-event", "s1", 1)]))
                .unwrap();
            downgrade_fixture_to_v7(&store.connection);
            store
                .connection
                .execute_batch(
                    "UPDATE agenturm_store_meta SET value='agenturm.observation-store.v3'
                       WHERE key='format';
                     PRAGMA user_version=3;",
                )
                .unwrap();
        }
        let store = Store::open(&db.0).unwrap();
        let page = store.observation_page(10).unwrap();
        assert_eq!(page.snapshot_id, 1.into());
        assert_eq!(page.observations.len(), 1);
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
    }

    #[test]
    fn v4_store_upgrades_to_current_schema_without_changing_history() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch("v4-request", vec![event("v4-event", "s1", 1)]))
                .unwrap();
            downgrade_fixture_to_v7(&store.connection);
            store
                .connection
                .execute_batch(
                    "UPDATE agenturm_store_meta SET value='agenturm.observation-store.v4'
                       WHERE key='format';
                     PRAGMA user_version=4;",
                )
                .unwrap();
        }
        let store = Store::open(&db.0).unwrap();
        let page = store.observation_page(10).unwrap();
        assert_eq!(page.snapshot_id, 1.into());
        assert_eq!(page.observations.len(), 1);
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
    }

    #[test]
    fn v5_store_upgrades_to_current_schema_without_changing_history() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch("v5-request", vec![event("v5-event", "s1", 1)]))
                .unwrap();
            downgrade_fixture_to_v7(&store.connection);
            store
                .connection
                .execute_batch(
                    "UPDATE agenturm_store_meta SET value='agenturm.observation-store.v5'
                       WHERE key='format';
                     PRAGMA user_version=5;",
                )
                .unwrap();
        }
        let store = Store::open(&db.0).unwrap();
        let page = store.observation_page(10).unwrap();
        assert_eq!(page.snapshot_id, 1.into());
        assert_eq!(page.observations.len(), 1);
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
    }

    #[test]
    fn v6_store_upgrades_to_current_schema_without_changing_history() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch("v6-request", vec![event("v6-event", "s1", 1)]))
                .unwrap();
            downgrade_fixture_to_v7(&store.connection);
            store
                .connection
                .execute_batch(
                    "UPDATE agenturm_store_meta SET value='agenturm.observation-store.v6'
                       WHERE key='format';
                     PRAGMA user_version=6;",
                )
                .unwrap();
        }
        let store = Store::open(&db.0).unwrap();
        let page = store.observation_page(10).unwrap();
        assert_eq!(page.snapshot_id, 1.into());
        assert_eq!(page.observations.len(), 1);
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
    }

    #[test]
    fn v7_store_rebuilds_observation_table_and_preserves_session_scope() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch("v7-request", vec![event("v7-event", "s1", 1)]))
                .unwrap();
            downgrade_fixture_to_v7(&store.connection);
        }
        let store = Store::open(&db.0).unwrap();
        let page = store.observation_page(10).unwrap();
        assert_eq!(page.snapshot_id, 1.into());
        assert_eq!(page.observations.len(), 1);
        assert_eq!(
            page.observations[0].identity.scope,
            KernelObservationScope::Session
        );
        assert_eq!(
            page.observations[0].identity.session_id.as_deref(),
            Some("s1")
        );
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
    }

    #[test]
    fn read_only_store_can_query_live_wal_without_admitting_writes() {
        let db = TempDb::new();
        let mut writer = Store::open(&db.0).unwrap();
        writer
            .commit_batch(&batch(
                "readonly-request",
                vec![event("event-1", "session-1", 1)],
            ))
            .unwrap();

        let reader = Store::open_read_only(&db.0).unwrap();
        let page = reader.observation_page(10).unwrap();
        assert_eq!(page.observations.len(), 1);
        assert!(matches!(
            reader.connection.execute(
                "DELETE FROM agenturm_observations WHERE event_id='event-1'",
                [],
            ),
            Err(rusqlite::Error::SqliteFailure(_, _))
        ));
        writer
            .commit_batch(&batch_after(
                "readonly-request-2",
                "cursor-1",
                "cursor-2",
                vec![event("event-2", "session-1", 2)],
            ))
            .unwrap();
        let refreshed = reader.observation_page(10).unwrap();
        assert_eq!(refreshed.observations.len(), 2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn writable_store_open_requires_collector_lease_across_processes() {
        const CHILD_DATABASE: &str = "AGENTURM_TEST_WRITER_LEASE_DATABASE";
        const CHILD_EXPECT_DENIED: &str = "AGENTURM_TEST_WRITER_LEASE_DENIED";

        if let Some(database) = std::env::var_os(CHILD_DATABASE) {
            let path = Path::new(&database);
            let reader = Store::open_read_only(path).unwrap();
            assert_eq!(reader.observations().unwrap().len(), 1);
            if std::env::var_os(CHILD_EXPECT_DENIED).is_some() {
                assert!(matches!(Store::open(path), Err(StoreError::WriterLease(_))));
                assert!(matches!(
                    Store::open_collector(path),
                    Err(StoreError::WriterLease(_))
                ));
                let version: i64 = reader
                    .connection
                    .pragma_query_value(None, "user_version", |row| row.get(0))
                    .unwrap();
                assert_eq!(version, 9);
            } else {
                let store = Store::open(path).unwrap();
                let version: i64 = store
                    .connection
                    .pragma_query_value(None, "user_version", |row| row.get(0))
                    .unwrap();
                assert_eq!(version, SCHEMA_VERSION);
            }
            return;
        }

        let db = TempDb::new();
        let mut collector = Store::open_collector(&db.0).unwrap();
        collector
            .commit_batch(&batch("lease-test", vec![event("event-1", "session-1", 1)]))
            .unwrap();
        downgrade_fixture_to_v9(&collector.connection);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "tests::writable_store_open_requires_collector_lease_across_processes",
            ])
            .env(CHILD_DATABASE, &db.0)
            .env(CHILD_EXPECT_DENIED, "1");
        let blocked = child.output().unwrap();
        assert!(
            blocked.status.success(),
            "{}{}",
            String::from_utf8_lossy(&blocked.stdout),
            String::from_utf8_lossy(&blocked.stderr),
        );

        drop(collector);
        let reopened = child.env_remove(CHILD_EXPECT_DENIED).output().unwrap();
        assert!(
            reopened.status.success(),
            "{}{}",
            String::from_utf8_lossy(&reopened.stdout),
            String::from_utf8_lossy(&reopened.stderr),
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn leased_open_resolves_aliases_and_rejects_a_missing_or_replaced_target() {
        use agenturm_storage_lease::WriterLease;
        use std::os::unix::fs::symlink;

        let db = TempDb::new();
        let alias = TempDb::new();
        let moved = TempDb::new();
        let unrelated = TempDb::new();
        let mut seed = Store::open(&db.0).unwrap();
        seed.commit_batch(&batch("leased-open", vec![event("original", "session", 1)]))
            .unwrap();
        drop(seed);
        symlink(&db.0, &alias.0).unwrap();
        let store = Store::open_collector(&alias.0).unwrap();
        assert_eq!(
            Path::new(store.connection.path().unwrap()),
            db.0.canonicalize().unwrap()
        );
        assert_eq!(store.observations().unwrap().len(), 1);
        drop(store);
        let lease = WriterLease::acquire(&alias.0).unwrap();
        fs::rename(&db.0, &moved.0).unwrap();
        assert!(matches!(
            Store::open_leased_connection(&lease),
            Err(StoreError::WriterLease(_))
        ));
        assert!(!db.0.exists());
        fs::write(&unrelated.0, b"keep this file").unwrap();
        symlink(&unrelated.0, &db.0).unwrap();
        assert!(matches!(
            Store::open_leased_connection(&lease),
            Err(StoreError::WriterLease(_))
        ));
        assert_eq!(fs::read(&unrelated.0).unwrap(), b"keep this file");
        assert_eq!(
            Store::open_read_only(&moved.0)
                .unwrap()
                .observations()
                .unwrap()
                .len(),
            1
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn retargeted_writable_store_rejects_observations_retention_and_grants() {
        use std::os::unix::fs::symlink;

        let db = TempDb::new();
        let alias = TempDb::new();
        let replacement = TempDb::new();
        drop(Store::open(&db.0).unwrap());
        drop(Store::open(&replacement.0).unwrap());
        symlink(&db.0, &alias.0).unwrap();

        let mut writer = Store::open(&alias.0).unwrap();
        let reader = Store::open_read_only(&db.0).unwrap();
        let initial = batch("initial", vec![event("event-1", "session-1", 1)]);
        writer.commit_batch(&initial).unwrap();
        let source = agenturm_kernel::ExtensionId::parse("harness.pi").unwrap();
        let scope = ContentGrantScope::new(source.clone(), ContentKind::PiUserText, [1; 32]);
        let grant = writer.grant_content(&scope).unwrap();

        fs::remove_file(&alias.0).unwrap();
        symlink(&replacement.0, &alias.0).unwrap();
        let next = batch_after(
            "subsequent",
            &initial.next_cursor,
            "cursor-2",
            vec![event("event-2", "session-1", 2)],
        );
        assert!(matches!(
            writer.commit_batch(&next),
            Err(StoreError::WriterLease(_))
        ));
        assert!(matches!(
            writer.prune_observations_before(2),
            Err(StoreError::WriterLease(_))
        ));
        assert!(matches!(
            writer.grant_content(&scope),
            Err(StoreError::WriterLease(_))
        ));
        assert!(matches!(
            writer.revoke_content(&source, ContentKind::PiUserText),
            Err(StoreError::WriterLease(_))
        ));
        assert_eq!(
            reader.source_cursor(&initial.source_id).unwrap().as_deref(),
            Some(initial.next_cursor.as_str())
        );
        assert_eq!(reader.observations().unwrap().len(), 1);
        reader.verify_content_grant(&scope, &grant).unwrap();
        assert!(
            Store::open_read_only(&replacement.0)
                .unwrap()
                .observations()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn source_diagnostic_persists_without_fake_session_and_session_filters_exclude_it() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            let mut diagnostic = event("source-gap-1", "unused", 10);
            diagnostic.identity = ObservationIdentity {
                scope: StoredScope::Source,
                session_id: None,
                worker_id: None,
                turn_id: None,
                operation_id: None,
            };
            diagnostic.kind = ObservationKind::Diagnostic {
                code: "possible_source_gap".into(),
                severity: "warning".into(),
            };
            store
                .commit_batch(&batch("source-diagnostic-request", vec![diagnostic]))
                .unwrap();
        }
        let store = Store::open_read_only(&db.0).unwrap();
        let page = store.observation_page(10).unwrap();
        assert_eq!(page.observations.len(), 1);
        assert_eq!(
            page.observations[0].identity.scope,
            KernelObservationScope::Source
        );
        assert_eq!(page.observations[0].identity.session_id, None);
        assert_eq!(
            page.observations[0].kind,
            KernelObservationKind::Diagnostic {
                code: "possible_source_gap".into(),
                severity: "warning".into(),
            }
        );
        let session_page = store
            .query_observations(&ObservationQuery {
                harness_id: Some("harness.codex".into()),
                session_id: Some("session-1".into()),
                before: None,
                snapshot_id: Some(page.snapshot_id),
                limit: 10,
            })
            .unwrap();
        assert!(session_page.observations.is_empty());
    }

    #[test]
    fn session_catalog_pages_source_scoped_sessions_with_pinned_lifecycle() {
        let db = TempDb::new();
        let mut writer = Store::open_collector(&db.0).unwrap();
        let lifecycle = |id, timestamp, state: &str| {
            let mut row = event(id, "shared-session", timestamp);
            row.identity.operation_id = None;
            row.kind = ObservationKind::Lifecycle {
                state: state.into(),
                reason_code: Some("example_reason".into()),
            };
            row
        };
        let relationship = |id, timestamp, parent: &str, kind| {
            let mut row = event(id, "shared-session", timestamp);
            row.identity.turn_id = None;
            row.identity.operation_id = None;
            row.kind = ObservationKind::Relationship {
                relationship_kind: kind,
                parent_session_id: parent.into(),
            };
            row
        };
        let mut source_gap = event("source-gap", "unused", 100);
        source_gap.identity = ObservationIdentity {
            scope: StoredScope::Source,
            session_id: None,
            worker_id: None,
            turn_id: None,
            operation_id: None,
        };
        source_gap.kind = ObservationKind::Diagnostic {
            code: "possible_source_gap".into(),
            severity: "warning".into(),
        };
        writer
            .commit_batch(&batch(
                "session-catalog",
                vec![
                    event("recent", "recent-session", 30),
                    event("activity", "shared-session", 20),
                    lifecycle("lifecycle-initial", 10, "interrupted"),
                    relationship(
                        "relationship-initial",
                        15,
                        "parent-a",
                        agenturm_kernel::RelationshipKind::Delegated,
                    ),
                    source_gap,
                ],
            ))
            .unwrap();
        let mut pi = batch(
            "pi-session-catalog",
            vec![event("pi-event", "shared-session", 20)],
        );
        pi.source_id = "harness.pi".into();
        writer.commit_batch(&pi).unwrap();
        let reader = Store::open_read_only(&db.0).unwrap();
        let first = reader.query_sessions(&SessionQuery::latest(1)).unwrap();
        assert_eq!(first.sessions[0].session_id, "recent-session");
        assert_eq!(first.sessions[0].observed_at_unix_ns, 30);

        writer
            .commit_batch(&batch_after(
                "catalog-new-data",
                "cursor-1",
                "cursor-2",
                vec![
                    lifecycle("lifecycle-new", 60, "completed"),
                    relationship(
                        "relationship-new",
                        65,
                        "parent-b",
                        agenturm_kernel::RelationshipKind::Forked,
                    ),
                    event("new-session", "new-session", 70),
                ],
            ))
            .unwrap();

        let second = reader
            .query_sessions(&SessionQuery {
                before: first.next_before,
                snapshot_id: Some(first.snapshot_id),
                ..SessionQuery::latest(1)
            })
            .unwrap();
        let third = reader
            .query_sessions(&SessionQuery {
                before: second.next_before,
                snapshot_id: Some(first.snapshot_id),
                ..SessionQuery::latest(1)
            })
            .unwrap();
        assert!(third.next_before.is_none());
        let pi = &second.sessions[0];
        let codex = &third.sessions[0];
        assert_eq!(pi.harness_id.as_str(), "harness.pi");
        assert_eq!(codex.harness_id.as_str(), "harness.codex");
        assert_eq!(pi.session_id, codex.session_id);
        assert!(pi.parent_session_id.is_none());
        assert!(pi.relationship_kind.is_none());
        assert_eq!(codex.parent_session_id.as_deref(), Some("parent-a"));
        assert_eq!(
            codex.relationship_kind,
            Some(agenturm_kernel::RelationshipKind::Delegated)
        );
        assert_eq!(codex.observed_at_unix_ns, 20);
        assert_eq!(
            codex.lifecycle,
            Some(SessionLifecycle {
                observed_at_unix_ns: 10,
                state: "interrupted".into(),
                reason_code: Some("example_reason".into()),
                worker_id: Some("worker-1".into()),
                turn_id: Some("turn-1".into()),
            })
        );
        let fresh = reader
            .query_sessions(&SessionQuery {
                harness_id: Some("harness.codex".into()),
                ..SessionQuery::latest(10)
            })
            .unwrap();
        assert_eq!(fresh.sessions.len(), 3);
        assert_eq!(fresh.sessions[0].session_id, "new-session");
        assert_eq!(
            fresh.sessions[1].parent_session_id.as_deref(),
            Some("parent-b")
        );
        assert_eq!(
            fresh.sessions[1].relationship_kind,
            Some(agenturm_kernel::RelationshipKind::Forked)
        );
        assert_eq!(
            fresh.sessions[1].lifecycle.as_ref().unwrap().state,
            "completed"
        );
    }

    #[test]
    fn session_catalog_rejects_expired_snapshots_and_discards_fully_pruned_sessions() {
        let db = TempDb::new();
        let mut writer = Store::open_collector(&db.0).unwrap();
        let reader = Store::open_read_only(&db.0).unwrap();
        assert_eq!(
            reader.query_sessions(&SessionQuery::latest(1)).unwrap(),
            SessionPage::default()
        );
        writer
            .commit_batch(&batch(
                "session-retention",
                vec![event("keep", "keep", 20), event("prune", "prune", 10)],
            ))
            .unwrap();
        let page = reader.query_sessions(&SessionQuery::latest(1)).unwrap();
        writer.prune_observations_before(20).unwrap();
        let continuation = SessionQuery {
            snapshot_id: Some(page.snapshot_id),
            before: page.next_before,
            ..SessionQuery::latest(1)
        };
        assert!(matches!(
            reader.query_sessions(&continuation),
            Err(StoreError::ExpiredHistorySnapshot { .. })
        ));
        let fresh = reader.query_sessions(&SessionQuery::latest(2)).unwrap();
        assert_eq!(fresh.sessions.len(), 1);
        assert_eq!(fresh.sessions[0].session_id, "keep");
        assert!(fresh.next_before.is_none());
    }

    #[test]
    fn session_review_version_tracks_changed_evidence_even_when_event_time_does_not_move() {
        let db = TempDb::new();
        let mut writer = Store::open_collector(&db.0).unwrap();
        let version = |store: &Store| {
            let page = store.query_sessions(&SessionQuery::latest(10)).unwrap();
            let target = page
                .sessions
                .iter()
                .find(|row| row.session_id == "target")
                .unwrap();
            assert_eq!(target.observed_at_unix_ns, 100);
            target.review_version
        };
        writer
            .commit_batch(&batch(
                "initial-review",
                vec![event("target-1", "target", 100)],
            ))
            .unwrap();
        let initial = version(&writer);
        writer
            .commit_batch(&batch_after(
                "unrelated-review",
                "cursor-1",
                "cursor-2",
                vec![event("different", "different", 200)],
            ))
            .unwrap();
        assert_eq!(version(&writer), initial);

        writer
            .commit_batch(&batch_after(
                "late-review",
                "cursor-2",
                "cursor-3",
                vec![event("target-2", "target", 10)],
            ))
            .unwrap();
        let changed = version(&writer);
        assert!(changed.receipt_sequence > initial.receipt_sequence);
        assert_eq!(changed.retained_observations, 2);
        writer.prune_observations_before(50).unwrap();
        assert_eq!(version(&writer), initial);
    }

    #[test]
    fn worker_turn_history_filters_reused_turn_ids_before_pagination() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let occurrence = |event_id, timestamp, worker: Option<&str>| {
            let mut row = event(event_id, "session", timestamp);
            row.identity.worker_id = worker.map(str::to_owned);
            row
        };
        store
            .commit_batch(&batch(
                "worker-turn",
                vec![
                    occurrence("worker-a-older", 10, Some("worker-a")),
                    occurrence("worker-b-older", 20, Some("worker-b")),
                    occurrence("worker-a-newer", 30, Some("worker-a")),
                    occurrence("root", 40, None),
                    occurrence("worker-b-newer", 50, Some("worker-b")),
                ],
            ))
            .unwrap();
        let query = ObservationQuery {
            harness_id: Some("harness.codex".into()),
            session_id: Some("session".into()),
            ..ObservationQuery::latest(1)
        };
        let mut kernel = agenturm_kernel::Kernel::new();
        let first = kernel.refresh_worker_turn_with_store(
            &store,
            query.clone(),
            "turn-1",
            Some("worker-a"),
        );
        assert!(first.failures.is_empty());
        assert!(first.observations_truncated);
        assert_eq!(first.observations[0].event_id, "worker-a-newer");
        let cursor = ObservationPosition {
            observed_at_unix_ns: first.observations[0].observed_at_unix_ns,
            harness_id: "harness.codex".into(),
            event_id: first.observations[0].event_id.clone(),
            revision: first.observations[0].revision,
            snapshot_id: first.history_snapshot_id,
        };
        let older = kernel.refresh_worker_turn_with_store(
            &store,
            ObservationQuery {
                before: Some(cursor),
                snapshot_id: Some(first.history_snapshot_id),
                ..query.clone()
            },
            "turn-1",
            Some("worker-a"),
        );
        assert!(older.failures.is_empty());
        assert!(!older.observations_truncated);
        assert_eq!(older.observations[0].event_id, "worker-a-older");
        let root = store
            .query_worker_turn_observations(&query, "turn-1", None)
            .unwrap();
        assert_eq!(root.observations[0].event_id, "root");
        assert!(!root.has_more);
        let legacy = store.query_turn_observations(&query, "turn-1").unwrap();
        assert_eq!(legacy.observations[0].event_id, "worker-b-newer");
        assert!(legacy.has_more);
    }

    #[test]
    fn filtered_keyset_query_pages_equal_timestamps_without_skips_or_duplicates() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch(
                    "query-codex",
                    vec![
                        event("event-1", "session-a", 10),
                        event("event-2", "session-a", 20),
                        event("event-3", "session-a", 20),
                        event("event-4", "session-b", 30),
                    ],
                ))
                .unwrap();
            let mut pi = batch("query-pi", vec![event("event-5", "session-a", 20)]);
            pi.source_id = "harness.pi".into();
            store.commit_batch(&pi).unwrap();
        }

        let store = Store::open(&db.0).unwrap();
        let first = store
            .query_observations(&ObservationQuery {
                harness_id: None,
                session_id: Some("session-a".into()),
                before: None,
                snapshot_id: None,
                limit: 2,
            })
            .unwrap();
        assert!(first.has_more);
        assert_eq!(
            first
                .observations
                .iter()
                .map(|row| (row.harness_id.as_str(), row.event_id.as_str()))
                .collect::<Vec<_>>(),
            [("harness.codex", "event-3"), ("harness.pi", "event-5")]
        );

        let mut late = batch_after(
            "query-late-write",
            "cursor-1",
            "cursor-2",
            vec![event("event-0", "session-a", 5)],
        );
        late.source_id = "harness.codex".into();
        let mut store = store;
        store.commit_batch(&late).unwrap();

        let oldest = first.observations.first().unwrap();
        let second = store
            .query_observations(&ObservationQuery {
                harness_id: None,
                session_id: Some("session-a".into()),
                before: Some(ObservationPosition {
                    observed_at_unix_ns: oldest.observed_at_unix_ns,
                    harness_id: oldest.harness_id.as_str().into(),
                    event_id: oldest.event_id.clone(),
                    revision: oldest.revision,
                    snapshot_id: first.snapshot_id,
                }),
                snapshot_id: Some(first.snapshot_id),
                limit: 2,
            })
            .unwrap();
        assert!(!second.has_more);
        assert_eq!(
            second
                .observations
                .iter()
                .map(|row| row.event_id.as_str())
                .collect::<Vec<_>>(),
            ["event-1", "event-2"]
        );
        let latest = store
            .query_observations(&ObservationQuery {
                harness_id: None,
                session_id: Some("session-a".into()),
                before: None,
                snapshot_id: None,
                limit: 8,
            })
            .unwrap();
        assert!(
            latest
                .observations
                .iter()
                .any(|observation| observation.event_id == "event-0")
        );
    }

    #[test]
    fn keyset_query_validates_filters_and_cursor_before_sql() {
        let db = TempDb::new();
        let store = Store::open(&db.0).unwrap();
        let invalid = [
            ObservationQuery {
                harness_id: Some("Invalid ID".into()),
                session_id: None,
                before: None,
                snapshot_id: None,
                limit: 1,
            },
            ObservationQuery {
                harness_id: None,
                session_id: Some("bad session".into()),
                before: None,
                snapshot_id: None,
                limit: 1,
            },
            ObservationQuery {
                harness_id: None,
                session_id: None,
                before: Some(ObservationPosition {
                    observed_at_unix_ns: 1,
                    harness_id: "harness.codex".into(),
                    event_id: "bad event".into(),
                    revision: 0,
                    snapshot_id: 1.into(),
                }),
                snapshot_id: Some(1.into()),
                limit: 1,
            },
        ];
        for query in invalid {
            assert!(matches!(
                store.query_observations(&query),
                Err(StoreError::Invalid(_))
            ));
        }
    }

    #[test]
    fn deterministic_request_id_is_stable_across_event_input_order() {
        let first = event("event-1", "session-1", 10);
        let second = event("event-2", "session-1", 11);
        let forward = ObservationBatch::new_deterministic(
            "harness.codex",
            Some("cursor-0".into()),
            "cursor-1",
            vec![first.clone(), second.clone()],
        )
        .unwrap();
        let reversed = ObservationBatch::new_deterministic(
            "harness.codex",
            Some("cursor-0".into()),
            "cursor-1",
            vec![second, first],
        )
        .unwrap();

        assert_eq!(forward.request_id, reversed.request_id);
        assert_eq!(
            forward.deterministic_request_id().unwrap(),
            forward.request_id
        );
    }

    #[test]
    fn deterministic_batch_constructor_rejects_bounds_before_digest() {
        let observations = (0..=MAX_BATCH_EVENTS)
            .map(|index| event(&format!("event-{index}"), "session-1", index as u64))
            .collect();
        assert!(matches!(
            ObservationBatch::new_deterministic("harness.codex", None, "cursor-1", observations),
            Err(StoreError::Invalid("batch exceeds event count limit"))
        ));
    }

    #[test]
    fn exact_retry_after_later_cursor_advance_is_still_a_replay() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let first = batch("request-1", vec![event("event-1", "session-1", 10)]);
        store.commit_batch(&first).unwrap();
        let second = batch_after(
            "request-2",
            "cursor-1",
            "cursor-2",
            vec![event("event-2", "session-1", 11)],
        );
        store.commit_batch(&second).unwrap();

        assert_eq!(store.commit_batch(&first).unwrap(), CommitOutcome::Replay);
        assert_eq!(store.observations().unwrap().len(), 2);
        assert_eq!(
            store.source_cursor("harness.codex").unwrap().as_deref(),
            Some("cursor-2")
        );
    }

    #[test]
    fn request_id_reuse_with_different_batch_is_rejected() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        store
            .commit_batch(&batch("request-1", vec![event("event-1", "session-1", 10)]))
            .unwrap();
        let changed = batch("request-1", vec![event("event-2", "session-1", 10)]);
        assert!(matches!(
            store.commit_batch(&changed),
            Err(StoreError::RequestConflict)
        ));
        assert_eq!(store.observations().unwrap().len(), 1);
    }

    #[test]
    fn event_identity_cannot_be_reused_for_changed_content() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        store
            .commit_batch(&batch("request-1", vec![event("event-1", "session-1", 10)]))
            .unwrap();
        let changed = batch_after(
            "request-2",
            "cursor-1",
            "cursor-2",
            vec![event("event-1", "session-1", 11)],
        );
        assert!(matches!(
            store.commit_batch(&changed),
            Err(StoreError::EventConflict { .. })
        ));
        assert_eq!(store.observations().unwrap().len(), 1);
    }

    #[test]
    fn higher_revision_is_stored_as_a_correction_without_overwriting_history() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        store
            .commit_batch(&batch("request-1", vec![event("event-1", "session-1", 10)]))
            .unwrap();
        let mut correction = event("event-1", "session-1", 11);
        correction.revision = 1;
        store
            .commit_batch(&batch_after(
                "request-2",
                "cursor-1",
                "cursor-2",
                vec![correction],
            ))
            .unwrap();

        let observations = store.observations().unwrap();
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].1.revision, 0);
        assert_eq!(observations[1].1.revision, 1);
    }

    #[test]
    fn failed_batch_rolls_back_rows_and_receipt() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let input = batch(
            "request-1",
            vec![
                event("event-1", "session-1", 10),
                event("event-2", "session-1", 11),
            ],
        );
        assert!(matches!(
            store.commit_batch_inner(&input, None, Some(1), None),
            Err(StoreError::InjectedFailure)
        ));
        assert!(store.observations().unwrap().is_empty());
        assert_eq!(
            store.commit_batch(&input).unwrap(),
            CommitOutcome::Committed { inserted: 2 }
        );
    }

    #[cfg(unix)]
    #[test]
    fn sigkill_after_uncommitted_wal_writes_rolls_back_batch() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::{Command, Stdio};
        use std::thread;
        use std::time::{Duration, Instant};

        const CHILD_DATABASE: &str = "AGENTURM_TEST_UNCOMMITTED_DATABASE";
        const CHILD_MARKER: &str = "AGENTURM_TEST_UNCOMMITTED_MARKER";
        let input = batch_after(
            "interrupted-request",
            "cursor-1",
            "cursor-2",
            (0..128)
                .map(|index| {
                    event(
                        &format!("interrupted-{index:03}-{}", "x".repeat(200)),
                        "session-1",
                        100 + index,
                    )
                })
                .collect(),
        );

        if let Some(database) = std::env::var_os(CHILD_DATABASE) {
            let mut store = Store::open(Path::new(&database)).unwrap();
            // Force dirty pages out of the cache before SQLite writes its
            // commit marker, so recovery has actual WAL frames to ignore.
            store
                .connection
                .pragma_update(None, "cache_size", 4)
                .unwrap();
            store
                .commit_batch_inner(
                    &input,
                    None,
                    None,
                    Some(|| {
                        fs::write(std::env::var_os(CHILD_MARKER).unwrap(), b"ready").unwrap();
                        loop {
                            thread::park();
                        }
                    }),
                )
                .unwrap();
            panic!("transaction returned before SIGKILL");
        }

        let db = TempDb::new();
        let seed = event("committed-before-interruption", "session-1", 1);
        Store::open(&db.0)
            .unwrap()
            .commit_batch(&batch("seed-request", vec![seed.clone()]))
            .unwrap();
        let marker = db.0.with_extension("pre-commit");
        let _ = fs::remove_file(&marker);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::sigkill_after_uncommitted_wal_writes_rolls_back_batch",
                "--nocapture",
            ])
            .env(CHILD_DATABASE, &db.0)
            .env(CHILD_MARKER, &marker)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let ready = loop {
            if fs::read(&marker).is_ok_and(|contents| contents == b"ready") {
                break true;
            }
            if child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
                break false;
            }
            thread::sleep(Duration::from_millis(10));
        };
        let wal_bytes = fs::metadata(db.0.with_extension("sqlite-wal"))
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        let _ = child.kill();
        let status = child.wait().unwrap();
        let _ = fs::remove_file(&marker);
        assert!(
            ready,
            "child did not reach the pre-commit checkpoint: {status}"
        );
        assert_eq!(status.signal(), Some(9));
        assert!(
            wal_bytes > 32,
            "SQLite must spill pages into the WAL before the kill"
        );

        let mut store = Store::open(&db.0).unwrap();
        assert_eq!(
            store.source_cursor(&input.source_id).unwrap().as_deref(),
            Some("cursor-1")
        );
        assert_eq!(
            store.observations().unwrap(),
            vec![(input.source_id.clone(), seed)]
        );
        for table in ["agenturm_ingest_receipts", "agenturm_history_batches"] {
            let count = store
                .connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE request_id = ?1"),
                    [&input.request_id],
                    |row| row.get::<_, usize>(0),
                )
                .unwrap();
            assert_eq!(count, 0, "{table} retained uncommitted rows");
        }
        assert_eq!(
            store.commit_batch(&input).unwrap(),
            CommitOutcome::Committed {
                inserted: input.observations.len()
            }
        );
        assert_eq!(store.commit_batch(&input).unwrap(), CommitOutcome::Replay);
        assert_eq!(
            store.observations().unwrap().len(),
            input.observations.len() + 1
        );
        assert_eq!(
            store.source_cursor(&input.source_id).unwrap().as_deref(),
            Some("cursor-2")
        );
    }

    #[test]
    fn committed_observations_survive_close_and_reopen() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch("request-1", vec![event("event-1", "session-1", 10)]))
                .unwrap();
            assert_eq!(
                store.source_cursor("harness.codex").unwrap().as_deref(),
                Some("cursor-1")
            );
        }
        let store = Store::open(&db.0).unwrap();
        assert_eq!(
            store.source_cursor("harness.codex").unwrap().as_deref(),
            Some("cursor-1")
        );
        assert_eq!(
            store.observations().unwrap(),
            vec![("harness.codex".into(), event("event-1", "session-1", 10))]
        );
    }

    #[test]
    fn complete_native_identity_survives_close_and_reopen() {
        let db = TempDb::new();
        let expected = event("event-1", "session-1", 10);
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch("request-1", vec![expected.clone()]))
                .unwrap();
        }
        let store = Store::open(&db.0).unwrap();
        let actual = store.observations().unwrap();
        assert_eq!(actual[0].1.identity, expected.identity);
    }

    #[test]
    fn nanosecond_timestamps_above_javascript_integer_precision_survive_reopen() {
        let db = TempDb::new();
        let timestamp_ns = 1_700_000_000_123_456_789;
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch(
                    "request-1",
                    vec![event("event-1", "session-1", timestamp_ns)],
                ))
                .unwrap();
        }
        let store = Store::open(&db.0).unwrap();
        assert_eq!(
            store.observations().unwrap()[0].1.observed_at_unix_ns,
            timestamp_ns
        );
    }

    #[test]
    fn source_cursor_accepts_protocol_limit_and_rejects_over_limit() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let max_cursor = "c".repeat(MAX_CURSOR_BYTES);
        let first = ObservationBatch {
            request_id: "request-1".into(),
            source_id: "harness.codex".into(),
            expected_cursor: None,
            next_cursor: max_cursor.clone(),
            observations: vec![],
        };
        store.commit_batch(&first).unwrap();
        let over_limit = batch_after(
            "request-2",
            &max_cursor,
            &"n".repeat(MAX_CURSOR_BYTES + 1),
            vec![],
        );
        assert!(matches!(
            store.commit_batch(&over_limit),
            Err(StoreError::Invalid(_))
        ));
        assert_eq!(
            store.source_cursor("harness.codex").unwrap().as_deref(),
            Some(max_cursor.as_str())
        );
    }

    #[test]
    fn events_are_atomic_when_already_known_and_new_events_share_batch() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        store
            .commit_batch(&batch("request-1", vec![event("event-1", "session-1", 10)]))
            .unwrap();
        let mixed = batch_after(
            "request-2",
            "cursor-1",
            "cursor-2",
            vec![
                event("event-1", "session-1", 10),
                event("event-2", "session-1", 11),
            ],
        );
        assert_eq!(
            store.commit_batch(&mixed).unwrap(),
            CommitOutcome::Committed { inserted: 1 }
        );
        assert_eq!(store.observations().unwrap().len(), 2);
    }

    #[test]
    fn malformed_batch_is_rejected_before_database_writes() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let invalid = batch("request-1", vec![event("../bad", "session-1", 10)]);
        assert!(matches!(
            store.commit_batch(&invalid),
            Err(StoreError::Invalid(_))
        ));
        assert!(store.observations().unwrap().is_empty());
    }

    #[test]
    fn invalid_check_category_is_rejected_before_database_writes() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let mut invalid = event("event-1", "session-1", 10);
        invalid.kind = ObservationKind::Check {
            category: "coverage".into(),
            result: "passed".into(),
        };
        assert!(matches!(
            store.commit_batch(&batch("request-1", vec![invalid])),
            Err(StoreError::Invalid("check category or result is invalid"))
        ));
        assert!(store.observations().unwrap().is_empty());
    }

    #[test]
    fn invalid_message_role_is_rejected_before_database_writes() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let mut invalid = event("event-1", "session-1", 10);
        invalid.kind = ObservationKind::Message {
            role: "system".into(),
        };
        assert!(matches!(
            store.commit_batch(&batch("request-1", vec![invalid])),
            Err(StoreError::Invalid("message role is invalid"))
        ));
        assert!(store.observations().unwrap().is_empty());
    }

    #[test]
    fn source_scope_is_restricted_to_safe_diagnostics() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let mut invalid_kind = event("event-1", "session-1", 10);
        invalid_kind.identity.scope = StoredScope::Source;
        invalid_kind.identity.session_id = None;
        invalid_kind.identity.worker_id = None;
        invalid_kind.identity.turn_id = None;
        invalid_kind.identity.operation_id = None;
        assert!(matches!(
            store.commit_batch(&batch("request-source-usage", vec![invalid_kind])),
            Err(StoreError::Invalid(
                "source scope requires a source diagnostic without session identity"
            ))
        ));

        let mut invalid_code = event("event-2", "session-1", 11);
        invalid_code.kind = ObservationKind::Diagnostic {
            code: "private_text_encoded_as_identifier".into(),
            severity: "warning".into(),
        };
        assert!(matches!(
            store.commit_batch(&batch("request-diagnostic-code", vec![invalid_code])),
            Err(StoreError::Invalid(
                "diagnostic code or severity is invalid"
            ))
        ));
        assert!(store.observations().unwrap().is_empty());
    }

    #[test]
    fn invalid_optional_native_identity_is_rejected_before_database_writes() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let mut invalid = event("event-1", "session-1", 10);
        invalid.identity.operation_id = Some("operation id with spaces".into());
        assert!(matches!(
            store.commit_batch(&batch("request-1", vec![invalid])),
            Err(StoreError::Invalid("operation ID"))
        ));
        assert!(store.observations().unwrap().is_empty());
    }

    #[test]
    fn unrelated_existing_database_is_rejected_without_modification() {
        let db = TempDb::new();
        {
            let connection = Connection::open(&db.0).unwrap();
            connection
                .execute_batch("CREATE TABLE unrelated (value TEXT NOT NULL);")
                .unwrap();
            connection
                .execute("INSERT INTO unrelated VALUES ('keep me')", [])
                .unwrap();
        }
        let before = fs::read(&db.0).unwrap();
        assert!(matches!(
            Store::open(&db.0),
            Err(StoreError::UnrecognizedDatabase)
        ));
        let after = fs::read(&db.0).unwrap();
        assert_eq!(after, before);
    }

    #[test]
    fn v8_stores_migrate_to_default_denied_content_grants_without_changing_history() {
        let source = TempDb::new();
        let snapshot = TempDb::new();
        let first = batch("v8-source", vec![event("v8-event", "session-1", 1)]);
        {
            let mut store = Store::open(&source.0).unwrap();
            store.commit_batch(&first).unwrap();
            downgrade_fixture_to_v9(&store.connection);
            store
                .connection
                .execute_batch(
                    "DROP TABLE agenturm_content_grants;
                     UPDATE agenturm_store_meta SET value='agenturm.observation-store.v8'
                      WHERE key='format';
                     PRAGMA user_version=8;",
                )
                .unwrap();
        }

        let source_id = agenturm_kernel::ExtensionId::parse("harness.pi").unwrap();
        let scope = ContentGrantScope::new(source_id, ContentKind::PiUserText, [0x42; 32]);
        {
            let reader = Store::open_read_only(&source.0).unwrap();
            assert!(reader.current_content_grant(&scope).unwrap().is_none());
            assert_eq!(reader.observations().unwrap().len(), 1);
            reader.backup_to(&snapshot.0).unwrap();
        }
        let mut restored = Store::open(&snapshot.0).unwrap();
        assert!(restored.current_content_grant(&scope).unwrap().is_none());
        assert_eq!(
            restored.commit_batch(&first).unwrap(),
            CommitOutcome::Replay
        );

        let store = Store::open(&source.0).unwrap();
        assert!(store.current_content_grant(&scope).unwrap().is_none());
        let schema: i64 = store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(schema, SCHEMA_VERSION);
    }

    #[test]
    fn content_grant_revocation_and_rebinding_reject_stale_access() {
        let db = TempDb::new();
        let other_db = TempDb::new();
        let source = agenturm_kernel::ExtensionId::parse("harness.pi").unwrap();
        let kind = ContentKind::PiUserText;
        let initial_scope = ContentGrantScope::new(source.clone(), kind, [0x11; 32]);
        let changed_scope = ContentGrantScope::new(source.clone(), kind, [0x42; 32]);

        let mut store = Store::open(&db.0).unwrap();
        let reader = Store::open_read_only(&db.0).unwrap();
        assert!(
            reader
                .current_content_grant(&initial_scope)
                .unwrap()
                .is_none()
        );

        let first = store.grant_content(&initial_scope).unwrap();
        assert_eq!(store.grant_content(&initial_scope).unwrap(), first);
        reader.verify_content_grant(&initial_scope, &first).unwrap();
        assert!(matches!(
            reader.verify_content_grant(&changed_scope, &first),
            Err(StoreError::StaleContentGrant)
        ));
        let mut other_store = Store::open(&other_db.0).unwrap();
        let foreign = other_store.grant_content(&initial_scope).unwrap();
        assert_eq!(foreign.epoch(), first.epoch());
        assert!(matches!(
            reader.verify_content_grant(&initial_scope, &foreign),
            Err(StoreError::StaleContentGrant)
        ));

        assert!(store.revoke_content(&source, kind).unwrap());
        assert!(!store.revoke_content(&source, kind).unwrap());
        assert!(
            reader
                .current_content_grant(&initial_scope)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            reader.verify_content_grant(&initial_scope, &first),
            Err(StoreError::StaleContentGrant)
        ));
        drop(store);
        let mut store = Store::open(&db.0).unwrap();
        let second = store.grant_content(&initial_scope).unwrap();
        assert_eq!(second.epoch(), first.epoch() + 1);
        assert!(matches!(
            reader.verify_content_grant(&initial_scope, &first),
            Err(StoreError::StaleContentGrant)
        ));
        reader
            .verify_content_grant(&initial_scope, &second)
            .unwrap();

        let third = store.grant_content(&changed_scope).unwrap();
        assert_eq!(third.epoch(), second.epoch() + 1);
        assert!(
            reader
                .current_content_grant(&initial_scope)
                .unwrap()
                .is_none()
        );
        reader.verify_content_grant(&changed_scope, &third).unwrap();
    }

    #[test]
    fn restored_backup_requires_fresh_content_authorization() {
        let db = TempDb::new();
        let snapshot = TempDb::new();
        let source = agenturm_kernel::ExtensionId::parse("harness.pi").unwrap();
        let scope = ContentGrantScope::new(source, ContentKind::PiUserText, [0xff; 32]);
        let mut store = Store::open(&db.0).unwrap();
        let grant = store.grant_content(&scope).unwrap();
        Store::open_read_only(&db.0)
            .unwrap()
            .backup_to(&snapshot.0)
            .unwrap();
        store.verify_content_grant(&scope, &grant).unwrap();

        let mut restored = Store::open(&snapshot.0).unwrap();
        assert!(restored.current_content_grant(&scope).unwrap().is_none());
        assert!(matches!(
            restored.verify_content_grant(&scope, &grant),
            Err(StoreError::StaleContentGrant)
        ));
        let fresh = restored.grant_content(&scope).unwrap();
        assert_eq!(fresh.epoch(), grant.epoch() + 1);
        restored.verify_content_grant(&scope, &fresh).unwrap();
    }

    #[test]
    fn stale_cursor_rejects_batch_without_rows_receipt_or_cursor_advance() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let first = batch("request-1", vec![event("event-1", "session-1", 10)]);
        store.commit_batch(&first).unwrap();
        let stale = batch_after(
            "request-2",
            "old-cursor",
            "cursor-2",
            vec![event("event-2", "session-1", 11)],
        );
        assert!(matches!(
            store.commit_batch(&stale),
            Err(StoreError::StaleCursor)
        ));
        assert_eq!(store.observations().unwrap().len(), 1);
        assert_eq!(
            store.source_cursor("harness.codex").unwrap().as_deref(),
            Some("cursor-1")
        );
        let retry = batch_after(
            "request-2",
            "cursor-1",
            "cursor-2",
            vec![event("event-2", "session-1", 11)],
        );
        assert_eq!(
            store.commit_batch(&retry).unwrap(),
            CommitOutcome::Committed { inserted: 1 }
        );
    }
}
