//! Small composition kernel for Agenturm's Rust application.
//!
//! Harnesses contribute observations, capabilities turn those observations
//! into typed metric rows, and UI contributions provide data-only panes. The
//! kernel owns extension identity, execution order, attribution, and bounded
//! snapshot validation. These trait objects are trusted in-process extensions,
//! not a sandbox or resource-isolation boundary. Calls are synchronous,
//! validation happens after extensions allocate their results, and the default
//! panic hook can still report caught panics. Independently installed or
//! untrusted extensions must use the supervised process boundary rather than
//! implementing these traits in the application process. The kernel has no
//! terminal, filesystem, process, policy, or storage authority.

#![forbid(unsafe_code)]

mod read_snapshot;
pub use read_snapshot::{ReadIncarnation, ReadSnapshot};

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

const MAX_EXTENSIONS_PER_KIND: usize = 64;
const MAX_EXTENSION_PACKAGES: usize = 64;
const MAX_OBSERVATIONS_PER_HARNESS: usize = 2_048;
const MAX_METRIC_ROWS: usize = 256;
const MAX_SESSION_KEY_BYTES: usize = 512;
const MAX_EVENT_ID_BYTES: usize = 256;
const MAX_CATEGORY_BYTES: usize = 64;
const MAX_LABEL_BYTES: usize = 128;
const MAX_VALUE_BYTES: usize = 1_024;
const MAX_TITLE_BYTES: usize = 128;
const MAX_FAILURE_BYTES: usize = 256;
const MAX_CURSOR_BYTES: usize = 800;
pub const MAX_DURABLE_SNAPSHOT_OBSERVATIONS: usize = 16_384;

/// Stable package identity for one registered extension.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ExtensionId(String);

impl<'de> Deserialize<'de> for ExtensionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?)
            .map_err(|_| serde::de::Error::custom("invalid extension ID"))
    }
}

impl ExtensionId {
    pub fn parse(value: impl Into<String>) -> Result<Self, RegistrationError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 96
            && value.as_bytes()[0].is_ascii_lowercase()
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-_/".contains(&byte)
            });
        if !valid {
            return Err(RegistrationError::InvalidId(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistrationError {
    InvalidId(String),
    DuplicateId(String),
    ExtensionIdentityPanicked,
    EmptyExtensionPlugin(String),
    ExtensionInstallFailed {
        plugin_id: String,
        message: String,
    },
    ExtensionInstallPanicked {
        plugin_id: String,
    },
    SourceIdentityMismatch {
        registered: String,
        configured: String,
    },
    RegistryFull,
}

impl std::fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId(id) => write!(f, "invalid extension id: {id:?}"),
            Self::DuplicateId(id) => write!(f, "extension id already registered: {id}"),
            Self::ExtensionIdentityPanicked => {
                f.write_str("extension package panicked while reporting its ID")
            }
            Self::EmptyExtensionPlugin(id) => {
                write!(f, "extension package {id} registered no contributions")
            }
            Self::ExtensionInstallFailed { plugin_id, message } => {
                write!(
                    f,
                    "extension package {plugin_id} failed to register: {message}"
                )
            }
            Self::ExtensionInstallPanicked { plugin_id } => {
                write!(
                    f,
                    "extension package {plugin_id} panicked during registration"
                )
            }
            Self::SourceIdentityMismatch {
                registered,
                configured,
            } => write!(
                f,
                "source identity mismatch: registered {registered}, configured {configured}"
            ),
            Self::RegistryFull => write!(f, "extension registry limit reached"),
        }
    }
}

impl std::error::Error for RegistrationError {}

/// Harness facts are intentionally source-neutral. The host supplies the
/// harness identity so an adapter cannot attribute its output to a sibling.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub harness_id: ExtensionId,
    pub event_id: String,
    pub revision: u32,
    pub identity: ObservationIdentity,
    pub observed_at_unix_ns: u64,
    pub kind: ObservationKind,
}

/// Source-native identity components preserved across collection and storage.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationScope {
    Source,
    Session,
}

/// Diagnostic codes are a closed set of source-neutral, privacy-safe labels.
/// Unrecognized adapter codes remain count-only at the decoder boundary.
pub fn is_safe_diagnostic_code(code: &str) -> bool {
    matches!(
        code,
        "possible_source_gap"
            | "cursor_expired"
            | "unsupported_codex_event"
            | "unsupported_codex_item"
            | "unsupported_codex_response"
            | "unsupported_codex_role"
            | "unsupported_codex_row"
            | "codex_inherited_prefix"
            | "codex_historical_outcome"
            | "codex_historical_wait"
            | "codex_legacy_usage_unmapped"
            | "codex_tool_result_unknown"
            | "codex_usage_unavailable"
            | "unsupported_pi_role"
            | "unsupported_pi_row"
            | "unsupported_claude_block"
            | "unsupported_claude_row"
            | "unsupported_claude_stop_reason"
            | "unsupported_claude_user"
            | "kiro_credit_usage_unrepresented"
            | "kiro_display_error"
            | "kiro_error"
            | "kiro_non_message_assistant"
            | "kiro_permission_unrepresented"
            | "kiro_queued_prompt"
            | "kiro_subagent_relation_unresolved"
            | "kiro_unknown_stop_reason"
            | "unsupported_kiro_row"
            | "opencode_idle_outcome_unknown"
            | "opencode_latest_role_unknown"
            | "opencode_tool_result_unknown"
            | "oversized_opencode_identity"
            | "oversized_opencode_row"
            | "unsupported_opencode_message"
            | "unsupported_jsonl_row"
            | "source_root_unavailable"
            | "root_gap"
    )
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationIdentity {
    pub scope: ObservationScope,
    pub session_id: Option<String>,
    pub worker_id: Option<String>,
    pub turn_id: Option<String>,
    pub operation_id: Option<String>,
}

impl ObservationIdentity {
    pub fn session(session_id: impl Into<String>) -> Self {
        Self {
            scope: ObservationScope::Session,
            session_id: Some(session_id.into()),
            worker_id: None,
            turn_id: None,
            operation_id: None,
        }
    }

    pub fn source() -> Self {
        Self {
            scope: ObservationScope::Source,
            session_id: None,
            worker_id: None,
            turn_id: None,
            operation_id: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    FileEdit(FileEdit),
    McpCall {
        server: Option<String>,
        tool: Option<String>,
        method: McpMethod,
        duration_ns: Option<u64>,
        result: String,
    },
    TraceSpan(TraceSpan),
    TraceProgress(TraceProgress),
    HostSample(HostSample),
    MetricPoint(MetricPoint),
    TraceSpanSummary(telemetry_summary::TraceSpanSummary),
    OtlpActivityLog(telemetry_summary::OtlpActivityLog),
    InternalReview(agenturm_protocol::native_review::InternalReview),
    NativeRequest(agenturm_protocol::native_review::NativeRequest),
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
        relationship_kind: RelationshipKind,
        parent_session_id: String,
    },
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
    ReportedCost(ReportedCost),
    SpanTokenUsage(SpanTokenUsage),
}

/// Returns whether a source lifecycle state can be admitted and stored.
pub fn is_lifecycle_state(state: &str) -> bool {
    matches!(
        state,
        "running" | "completed" | "interrupted" | "failed" | "paused"
    )
}

/// Checks the typed reference and its native turn and operation anchors.
pub fn is_anchored_pull_request(
    repository: &str,
    number: u64,
    identity: &ObservationIdentity,
) -> bool {
    identity.scope == ObservationScope::Session
        && identity.session_id.is_some()
        && identity.turn_id.is_some()
        && identity.operation_id.is_some()
        && is_github_pull_request(repository, number)
}

pub use agenturm_protocol::host::{
    HostCpu, HostProcess, HostProcessState, HostRuntime, HostSample, MAX_HOST_PROCESSES,
    is_host_source,
};
pub use agenturm_protocol::metric;
pub use agenturm_protocol::metric::{MetricPoint, is_metric_source};
pub use agenturm_protocol::observation::{
    FileEdit, McpMethod, NANOCREDITS_PER_CREDIT, NANOUNITS_PER_CURRENCY_UNIT, NativeCostBasis,
    RelationshipKind, ReportedCost, SpanTokenUsage, TraceProgress, TraceSpan, TraceSpanIdentity,
    TraceStatus, is_github_pull_request, is_valid_mcp_metadata, parse_currency_nanounits,
};

pub fn is_mcp_operation(identity: &ObservationIdentity) -> bool {
    identity.scope == ObservationScope::Session
        && identity.session_id.is_some()
        && identity.operation_id.is_some()
}

pub fn is_file_edit_operation(identity: &ObservationIdentity) -> bool {
    identity.scope == ObservationScope::Session
        && identity.session_id.is_some()
        && identity.turn_id.is_some()
        && identity.operation_id.is_some()
}

/// Bound trace metadata is attributed to its OTLP source. The optional target
/// is an independent, exact harness identifier rather than source ownership.
pub fn is_trace_source_link_valid(import_source_id: &str, span: &TraceSpan) -> bool {
    span.target_source_id.as_deref().is_none_or(|target| {
        import_source_id.starts_with("otlp.") && is_native_trace_target(target)
    })
}

fn is_native_trace_target(target: &str) -> bool {
    !target.starts_with("otlp.") && ExtensionId::parse(target).is_ok()
}

pub fn is_trace_progress_scope_valid(
    progress: &TraceProgress,
    turn_id: Option<&str>,
    worker_id: Option<&str>,
    operation_id: Option<&str>,
) -> bool {
    progress
        .target_source_id
        .as_deref()
        .is_none_or(is_native_trace_target)
        && operation_id.is_none()
        && (turn_id.is_some() || worker_id.is_none())
        && (turn_id.is_none() || progress.target_source_id.is_some())
}

pub fn is_span_usage_anchor_valid(
    usage: &SpanTokenUsage,
    event_id: &str,
    turn_id: Option<&str>,
    worker_id: Option<&str>,
    operation_id: Option<&str>,
) -> bool {
    usage.is_valid()
        && usage.matches_event(event_id)
        && usage
            .target_source_id
            .as_deref()
            .is_none_or(is_native_trace_target)
        && operation_id.is_none()
        && (turn_id.is_some() || worker_id.is_none())
        && (turn_id.is_none() || usage.target_source_id.is_some())
}

/// A turn-scoped trace must explicitly identify the native harness. A worker
/// without its turn cannot identify a native execution anchor.
pub fn is_trace_turn_link_valid(
    span: &TraceSpan,
    event_id: &str,
    turn_id: Option<&str>,
    worker_id: Option<&str>,
    operation_id: Option<&str>,
) -> bool {
    if span.target_operation_id.is_some() && (turn_id.is_none() || span.target_source_id.is_none())
    {
        return false;
    }
    match turn_id {
        Some(_) => {
            span.target_source_id.is_some()
                && operation_id.is_none()
                && event_id
                    .strip_prefix("otlp:")
                    .and_then(|ids| ids.split_once(':'))
                    == Some((span.trace_id.as_str(), span.span_id.as_str()))
        }
        None => worker_id.is_none(),
    }
}

/// Formats native credits without introducing floating-point rounding.
pub fn format_credits(nanocredits: u64) -> String {
    format_nano_quantity(nanocredits, NANOCREDITS_PER_CREDIT)
}

pub fn format_reported_cost(cost: &ReportedCost) -> String {
    format!(
        "{} {}",
        format_nano_quantity(cost.nanounits, NANOUNITS_PER_CURRENCY_UNIT),
        cost.currency
    )
}

fn format_nano_quantity(value: u64, units: u64) -> String {
    let whole = value / units;
    let fraction = value % units;
    if fraction == 0 {
        return whole.to_string();
    }
    let fraction = format!("{fraction:09}");
    format!("{whole}.{}", fraction.trim_end_matches('0'))
}

/// Validates a parent ID in the same configured source as a distinct child session.
pub fn is_valid_relationship_parent(
    child_session_id: Option<&str>,
    parent_session_id: &str,
) -> bool {
    child_session_id.is_some_and(|child| child != parent_session_id)
        && valid_identity_part(parent_session_id, MAX_SESSION_KEY_BYTES)
}

/// The relation describes a session, so turn and operation anchors are absent.
pub fn is_session_relationship(parent_session_id: &str, identity: &ObservationIdentity) -> bool {
    identity.scope == ObservationScope::Session
        && identity.turn_id.is_none()
        && identity.operation_id.is_none()
        && is_valid_relationship_parent(identity.session_id.as_deref(), parent_session_id)
}

/// Adapter port. Implementations own harness-specific discovery and parsing;
/// the kernel stamps the registered identity on returned facts.
pub trait Harness: Send {
    fn poll(&mut self) -> Result<Vec<RawObservation>, String>;
}

/// Supervised adapter boundary. The kernel owns its cursor, validation,
/// durable admission, and ACK ordering; adapters only prepare facts and ACK.
pub trait SourceAdapter: Send {
    fn source_id(&self) -> &str;
    fn poll(&mut self, cursor: &str) -> Result<PreparedSourceBatch, String>;
    fn ack(&mut self, cursor: &str) -> Result<(), String>;
    /// Clears a rejected or failed protocol session so the next poll can retry
    /// from the still-durable cursor. Commit failures do not call this: the
    /// exact prepared batch remains cached for retry.
    fn reset(&mut self) -> Result<(), String>;
}

fn reset_source(source: &mut dyn SourceAdapter) -> Result<(), String> {
    match catch_unwind(AssertUnwindSafe(|| source.reset())) {
        Ok(result) => result,
        Err(_) => Err("source adapter panicked during reset".into()),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedSourceBatch {
    pub next_cursor: String,
    pub observations: Vec<RawObservation>,
    pub skipped_unprojected_facts: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreCommit {
    pub inserted: usize,
    pub replay: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationPage {
    pub observations: Vec<Observation>,
    pub has_more: bool,
    /// Highest durable batch receipt visible when this query began.
    pub snapshot_id: ReadSnapshot,
}

impl ObservationPage {
    pub fn older_position(&self) -> Option<ObservationPosition> {
        let row = self.observations.first().filter(|_| self.has_more)?;
        Some(ObservationPosition {
            observed_at_unix_ns: row.observed_at_unix_ns,
            harness_id: row.harness_id.as_str().to_owned(),
            event_id: row.event_id.clone(),
            revision: row.revision,
            snapshot_id: self.snapshot_id,
        })
    }
}

/// Exclusive keyset cursor for deterministic paging through durable history.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationPosition {
    pub observed_at_unix_ns: u64,
    pub harness_id: String,
    pub event_id: String,
    pub revision: u32,
    pub snapshot_id: ReadSnapshot,
}

/// Bounded durable-history query. `before` selects rows older than its exact
/// ordering key; filters are applied before the page limit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationQuery {
    pub harness_id: Option<String>,
    pub session_id: Option<String>,
    pub before: Option<ObservationPosition>,
    pub snapshot_id: Option<ReadSnapshot>,
    pub limit: usize,
}

impl ObservationQuery {
    pub fn latest(limit: usize) -> Self {
        Self {
            harness_id: None,
            session_id: None,
            before: None,
            snapshot_id: None,
            limit,
        }
    }
}

#[derive(Clone, Copy)]
struct TurnSelection<'a> {
    turn_id: &'a str,
    worker_filter: Option<Option<&'a str>>,
}

/// Atomic storage boundary used by kernel admission.
pub trait ObservationStore {
    fn source_cursor(&self, source_id: &str) -> Result<Option<String>, String>;
    /// Returns at most `limit` rows ordered oldest-to-newest within the
    /// selected newest page. `has_more` indicates older rows were omitted.
    fn load_observations(&self, limit: usize) -> Result<ObservationPage, String>;
    /// Queries durable history using stable keyset pagination. The fallback
    /// keeps small test/extension stores source-compatible for the default
    /// newest-page read; filtered or continued queries require implementation.
    fn query_observations(&self, query: &ObservationQuery) -> Result<ObservationPage, String> {
        if query.harness_id.is_some()
            || query.session_id.is_some()
            || query.before.is_some()
            || query.snapshot_id.is_some()
        {
            return Err("observation store does not support filtered history queries".into());
        }
        self.load_observations(query.limit)
    }
    /// Queries one native turn within an exact source and session.
    fn query_turn_observations(
        &self,
        _query: &ObservationQuery,
        _turn_id: &str,
    ) -> Result<ObservationPage, String> {
        Err("observation store does not support turn history queries".into())
    }
    /// Queries one exact worker anchor in a native turn. `None` selects
    /// workerless records. The default fails closed without an exact port.
    fn query_worker_turn_observations(
        &self,
        _query: &ObservationQuery,
        _turn_id: &str,
        _worker_id: Option<&str>,
    ) -> Result<ObservationPage, String> {
        Err("observation store does not support exact worker turn history queries".into())
    }
    fn commit(
        &mut self,
        source_id: &str,
        expected_cursor: Option<&str>,
        next_cursor: &str,
        observations: &[Observation],
    ) -> Result<StoreCommit, String>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdmissionStatus {
    pub inserted: usize,
    pub replay: bool,
    pub skipped_unprojected_facts: usize,
    pub source_ack_sent: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawObservation {
    pub event_id: String,
    pub revision: u32,
    pub identity: ObservationIdentity,
    pub observed_at_unix_ns: u64,
    pub kind: ObservationKind,
}

/// Capability extension port. A capability cannot mutate observations and
/// returns only bounded, render-neutral rows for the host to expose.
pub trait Capability: Send + Sync {
    fn evaluate(&self, observations: &[Observation]) -> Result<Vec<MetricRow>, String>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricRow {
    pub label: String,
    pub value: String,
}

/// UI contributions are data, not widgets. The application owns layout,
/// navigation, focus and terminal lifecycle and renders each pane generically.
pub trait UiContribution: Send + Sync {
    fn build_pane(&self, snapshot: &Snapshot) -> Result<PaneModel, String>;
}

/// Selection context for a durable observation page passed to an extension.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ObservationSelectionScope {
    #[default]
    Latest,
    Filtered,
    Continuation,
}

/// Minimal context for one atomic metrics-and-pane extension evaluation.
pub struct ObservationExtensionInput<'a> {
    pub observations: &'a [Observation],
    pub snapshot_id: u64,
    pub observations_truncated: bool,
    pub selection_scope: ObservationSelectionScope,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ObservationExtensionOutput {
    pub metrics: Vec<MetricRow>,
    pub pane: Option<PaneModel>,
}

/// Extension port for one evaluation that contributes metrics and optionally
/// one data-only pane under the same ID. The host validates both outputs before
/// publishing either of them.
pub trait ObservationExtension: Send + Sync {
    fn evaluate(
        &self,
        input: &ObservationExtensionInput<'_>,
    ) -> Result<ObservationExtensionOutput, String>;
}

/// Build-time extension package that installs one or more kernel contributions.
/// Registration runs against a staging kernel and is committed atomically only
/// after every contribution succeeds and all IDs/capacity checks pass.
pub trait ExtensionPlugin: Send + Sync {
    fn id(&self) -> ExtensionId;
    fn register(&self, kernel: &mut Kernel) -> Result<(), String>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaneModel {
    pub title: String,
    pub rows: Vec<MetricRow>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionFailure {
    pub extension_id: ExtensionId,
    pub stage: ExtensionStage,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionStage {
    Poll,
    AdmissionCursor,
    AdmissionPoll,
    AdmissionValidation,
    AdmissionCommit,
    AdmissionAck,
    AdmissionRead,
    Capability,
    ObservationExtension,
    UiContribution,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    pub observations: Vec<Observation>,
    pub metrics: BTreeMap<ExtensionId, Vec<MetricRow>>,
    pub panes: BTreeMap<ExtensionId, PaneModel>,
    pub admissions: BTreeMap<ExtensionId, AdmissionStatus>,
    pub observations_truncated: bool,
    pub history_snapshot_id: ReadSnapshot,
    pub failures: Vec<ExtensionFailure>,
}

#[derive(Default)]
pub struct Kernel {
    plugins: BTreeSet<ExtensionId>,
    harnesses: BTreeMap<ExtensionId, Box<dyn Harness>>,
    sources: BTreeMap<ExtensionId, Box<dyn SourceAdapter>>,
    capabilities: BTreeMap<ExtensionId, Box<dyn Capability>>,
    observation_extensions: BTreeMap<ExtensionId, Box<dyn ObservationExtension>>,
    ui_contributions: BTreeMap<ExtensionId, Box<dyn UiContribution>>,
}

impl Kernel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_harness(
        &mut self,
        id: ExtensionId,
        harness: impl Harness + 'static,
    ) -> Result<(), RegistrationError> {
        insert_unique(&mut self.harnesses, id, Box::new(harness))
    }

    /// Stages a package's contributions so a failed package cannot leave a
    /// partially registered capability, pane, harness, or source adapter.
    pub fn register_plugin(
        &mut self,
        plugin: impl ExtensionPlugin + 'static,
    ) -> Result<(), RegistrationError> {
        let id = catch_unwind(AssertUnwindSafe(|| plugin.id()))
            .map_err(|_| RegistrationError::ExtensionIdentityPanicked)?;
        if self.plugins.contains(&id) {
            return Err(RegistrationError::DuplicateId(id.0));
        }
        let mut staged = Kernel::new();
        match catch_unwind(AssertUnwindSafe(|| plugin.register(&mut staged))) {
            Ok(Ok(())) => {}
            Ok(Err(message)) => {
                return Err(RegistrationError::ExtensionInstallFailed {
                    plugin_id: id.0,
                    message: bounded(&message, MAX_FAILURE_BYTES),
                });
            }
            Err(_) => {
                return Err(RegistrationError::ExtensionInstallPanicked { plugin_id: id.0 });
            }
        }
        if let Some(duplicate) = first_duplicate_id(&self.harnesses, &staged.harnesses)
            .or_else(|| first_duplicate_id(&self.sources, &staged.sources))
            .or_else(|| first_duplicate_id(&self.capabilities, &staged.capabilities))
            .or_else(|| first_duplicate_id(&self.capabilities, &staged.observation_extensions))
            .or_else(|| first_duplicate_id(&self.observation_extensions, &staged.capabilities))
            .or_else(|| first_duplicate_id(&self.ui_contributions, &staged.observation_extensions))
            .or_else(|| first_duplicate_id(&self.observation_extensions, &staged.ui_contributions))
            .or_else(|| {
                first_duplicate_id(&self.observation_extensions, &staged.observation_extensions)
            })
            .or_else(|| first_duplicate_id(&self.ui_contributions, &staged.ui_contributions))
        {
            return Err(RegistrationError::DuplicateId(duplicate));
        }
        if let Some(duplicate) =
            first_duplicate_id(&staged.capabilities, &staged.observation_extensions)
                .or_else(|| {
                    first_duplicate_id(&staged.observation_extensions, &staged.capabilities)
                })
                .or_else(|| {
                    first_duplicate_id(&staged.ui_contributions, &staged.observation_extensions)
                })
                .or_else(|| {
                    first_duplicate_id(&staged.observation_extensions, &staged.ui_contributions)
                })
        {
            return Err(RegistrationError::DuplicateId(duplicate));
        }
        if staged.plugins.contains(&id) {
            return Err(RegistrationError::DuplicateId(id.0));
        }
        if let Some(duplicate) = staged.plugins.iter().find(|id| self.plugins.contains(*id)) {
            return Err(RegistrationError::DuplicateId(
                duplicate.as_str().to_owned(),
            ));
        }
        if staged.harnesses.is_empty()
            && staged.sources.is_empty()
            && staged.capabilities.is_empty()
            && staged.observation_extensions.is_empty()
            && staged.ui_contributions.is_empty()
            && staged.plugins.is_empty()
        {
            return Err(RegistrationError::EmptyExtensionPlugin(id.0));
        }
        if self.plugins.len() + staged.plugins.len() + 1 > MAX_EXTENSION_PACKAGES {
            return Err(RegistrationError::RegistryFull);
        }
        if self.harnesses.len() + staged.harnesses.len() > MAX_EXTENSIONS_PER_KIND
            || self.sources.len() + staged.sources.len() > MAX_EXTENSIONS_PER_KIND
            || self.capabilities.len() + staged.capabilities.len() > MAX_EXTENSIONS_PER_KIND
            || self.observation_extensions.len() + staged.observation_extensions.len()
                > MAX_EXTENSIONS_PER_KIND
            || self.ui_contributions.len() + staged.ui_contributions.len() > MAX_EXTENSIONS_PER_KIND
        {
            return Err(RegistrationError::RegistryFull);
        }
        self.plugins.insert(id);
        self.plugins.extend(staged.plugins);
        self.harnesses.extend(staged.harnesses);
        self.sources.extend(staged.sources);
        self.capabilities.extend(staged.capabilities);
        self.observation_extensions
            .extend(staged.observation_extensions);
        self.ui_contributions.extend(staged.ui_contributions);
        Ok(())
    }

    pub fn register_source_adapter(
        &mut self,
        id: ExtensionId,
        source: impl SourceAdapter + 'static,
    ) -> Result<(), RegistrationError> {
        if source.source_id() != id.as_str() {
            return Err(RegistrationError::SourceIdentityMismatch {
                registered: id.as_str().to_owned(),
                configured: source.source_id().to_owned(),
            });
        }
        insert_unique(&mut self.sources, id, Box::new(source))
    }

    /// Stops polling a configured source. Its durable cursor stays in the store.
    pub fn unregister_source_adapter(&mut self, id: &ExtensionId) {
        self.sources.remove(id);
    }

    pub fn register_capability(
        &mut self,
        id: ExtensionId,
        capability: impl Capability + 'static,
    ) -> Result<(), RegistrationError> {
        if self.observation_extensions.contains_key(&id) {
            return Err(RegistrationError::DuplicateId(id.0));
        }
        insert_unique(&mut self.capabilities, id, Box::new(capability))
    }

    pub fn register_observation_extension(
        &mut self,
        id: ExtensionId,
        extension: impl ObservationExtension + 'static,
    ) -> Result<(), RegistrationError> {
        if self.capabilities.contains_key(&id) || self.ui_contributions.contains_key(&id) {
            return Err(RegistrationError::DuplicateId(id.0));
        }
        insert_unique(&mut self.observation_extensions, id, Box::new(extension))
    }

    pub fn register_ui_contribution(
        &mut self,
        id: ExtensionId,
        contribution: impl UiContribution + 'static,
    ) -> Result<(), RegistrationError> {
        if self.observation_extensions.contains_key(&id) {
            return Err(RegistrationError::DuplicateId(id.0));
        }
        insert_unique(&mut self.ui_contributions, id, Box::new(contribution))
    }

    /// Polls trusted in-process extensions in stable ID order. Returned errors
    /// and unwind panics are recorded so healthy extensions still contribute.
    pub fn refresh(&mut self) -> Snapshot {
        let mut snapshot = Snapshot::default();
        self.poll_harnesses(&mut snapshot);
        self.evaluate_extensions(&mut snapshot, ObservationSelectionScope::Latest);
        snapshot
    }

    /// Admits supervised source batches. For each source the kernel validates
    /// facts, commits them atomically, and only then ACKs the source cursor.
    /// This durable snapshot excludes trusted in-process `Harness` output,
    /// whose port has no cursor/commit lifecycle. Use `refresh` for that
    /// explicitly in-memory view. Failed sources do not hide healthy results.
    pub fn refresh_with_store(&mut self, store: &mut impl ObservationStore) -> Snapshot {
        let mut snapshot = Snapshot::default();
        self.admit_sources(&mut snapshot, store);
        let selection_scope = self.load_observations(
            &mut snapshot,
            store,
            &ObservationQuery::latest(MAX_DURABLE_SNAPSHOT_OBSERVATIONS),
            None,
        );
        self.evaluate_extensions(&mut snapshot, selection_scope);
        snapshot
    }

    /// Rebuilds capability and pane state from one bounded query of durable
    /// observations. This path does not admit live sources.
    pub fn refresh_query_with_store(
        &mut self,
        store: &impl ObservationStore,
        query: ObservationQuery,
    ) -> Snapshot {
        let mut snapshot = Snapshot::default();
        let selection_scope = self.load_observations(&mut snapshot, store, &query, None);
        self.evaluate_extensions(&mut snapshot, selection_scope);
        snapshot
    }

    /// Rebuilds extension state from one exact, durable turn page.
    pub fn refresh_turn_with_store(
        &mut self,
        store: &impl ObservationStore,
        query: ObservationQuery,
        turn_id: &str,
    ) -> Snapshot {
        self.refresh_selected_turn(
            store,
            query,
            TurnSelection {
                turn_id,
                worker_filter: None,
            },
        )
    }

    /// Rebuilds extension state for a durable turn at one exact native
    /// worker anchor. Pass `None` to select workerless observations.
    pub fn refresh_worker_turn_with_store(
        &mut self,
        store: &impl ObservationStore,
        query: ObservationQuery,
        turn_id: &str,
        worker_id: Option<&str>,
    ) -> Snapshot {
        self.refresh_selected_turn(
            store,
            query,
            TurnSelection {
                turn_id,
                worker_filter: Some(worker_id),
            },
        )
    }

    fn refresh_selected_turn(
        &mut self,
        store: &impl ObservationStore,
        query: ObservationQuery,
        turn: TurnSelection<'_>,
    ) -> Snapshot {
        let mut snapshot = Snapshot::default();
        let selection_scope = self.load_observations(&mut snapshot, store, &query, Some(turn));
        self.evaluate_extensions(&mut snapshot, selection_scope);
        snapshot
    }

    fn load_observations(
        &self,
        snapshot: &mut Snapshot,
        store: &impl ObservationStore,
        query: &ObservationQuery,
        turn: Option<TurnSelection<'_>>,
    ) -> ObservationSelectionScope {
        let selection_scope = if query.before.is_some() {
            ObservationSelectionScope::Continuation
        } else if query.harness_id.is_some() || query.session_id.is_some() || turn.is_some() {
            ObservationSelectionScope::Filtered
        } else {
            ObservationSelectionScope::Latest
        };
        let store_id = ExtensionId("kernel.store".into());
        let page = match turn {
            Some(TurnSelection {
                turn_id,
                worker_filter: Some(worker_id),
            }) => store.query_worker_turn_observations(query, turn_id, worker_id),
            Some(TurnSelection { turn_id, .. }) => store.query_turn_observations(query, turn_id),
            None => store.query_observations(query),
        };
        match page {
            Ok(page) => {
                if page.observations.len() > MAX_DURABLE_SNAPSHOT_OBSERVATIONS {
                    snapshot.failures.push(failure(
                        &store_id,
                        ExtensionStage::AdmissionRead,
                        "observation store returned more rows than requested",
                    ));
                    return selection_scope;
                }
                if let Some(turn) = turn
                    && page.observations.iter().any(|row| {
                        query.harness_id.as_deref() != Some(row.harness_id.as_str())
                            || query.session_id != row.identity.session_id
                            || row.identity.turn_id.as_deref() != Some(turn.turn_id)
                            || turn
                                .worker_filter
                                .is_some_and(|worker| row.identity.worker_id.as_deref() != worker)
                    })
                {
                    snapshot.failures.push(failure(
                        &store_id,
                        ExtensionStage::AdmissionRead,
                        "observation store returned a row outside the requested turn",
                    ));
                    return selection_scope;
                }
                snapshot.observations_truncated = page.has_more;
                snapshot.history_snapshot_id = page.snapshot_id;
                let mut grouped = BTreeMap::<ExtensionId, Vec<RawObservation>>::new();
                let mut ordered = Vec::with_capacity(page.observations.len());
                let mut conversion_error = None;
                for observation in page.observations {
                    grouped
                        .entry(observation.harness_id.clone())
                        .or_default()
                        .push(RawObservation {
                            event_id: observation.event_id.clone(),
                            revision: observation.revision,
                            identity: observation.identity.clone(),
                            observed_at_unix_ns: observation.observed_at_unix_ns,
                            kind: observation.kind.clone(),
                        });
                    ordered.push(observation);
                }
                for (source, rows) in &grouped {
                    match validate_raw_observation_slice(
                        source,
                        rows,
                        MAX_DURABLE_SNAPSHOT_OBSERVATIONS,
                    ) {
                        Ok(_) => {}
                        Err(message) => {
                            conversion_error = Some(message);
                            break;
                        }
                    }
                }
                if let Some(message) = conversion_error {
                    snapshot.observations.clear();
                    snapshot.failures.push(failure(
                        &store_id,
                        ExtensionStage::AdmissionRead,
                        message,
                    ));
                } else {
                    snapshot.observations.extend(ordered);
                }
            }
            Err(message) => {
                snapshot
                    .failures
                    .push(failure(&store_id, ExtensionStage::AdmissionRead, message))
            }
        }
        selection_scope
    }

    fn poll_harnesses(&mut self, snapshot: &mut Snapshot) {
        for (id, harness) in &mut self.harnesses {
            let result = catch_unwind(AssertUnwindSafe(|| harness.poll()));
            match result {
                Ok(Ok(rows)) => match validate_raw_observations(id, rows) {
                    Ok(rows) => {
                        snapshot
                            .observations
                            .extend(rows.into_iter().map(|row| Observation {
                                harness_id: id.clone(),
                                event_id: row.event_id,
                                revision: row.revision,
                                identity: row.identity,
                                observed_at_unix_ns: row.observed_at_unix_ns,
                                kind: row.kind,
                            }))
                    }
                    Err(message) => {
                        snapshot
                            .failures
                            .push(failure(id, ExtensionStage::Poll, message))
                    }
                },
                Ok(Err(message)) => {
                    snapshot
                        .failures
                        .push(failure(id, ExtensionStage::Poll, message))
                }
                Err(_) => {
                    snapshot
                        .failures
                        .push(failure(id, ExtensionStage::Poll, "extension panicked"))
                }
            }
        }
    }

    fn admit_sources(&mut self, snapshot: &mut Snapshot, store: &mut impl ObservationStore) {
        for (id, source) in &mut self.sources {
            let expected_cursor = match store.source_cursor(id.as_str()) {
                Ok(cursor) => cursor,
                Err(message) => {
                    snapshot
                        .failures
                        .push(failure(id, ExtensionStage::AdmissionCursor, message));
                    continue;
                }
            };
            let cursor = expected_cursor.as_deref().unwrap_or_default();
            if !valid_cursor(cursor) {
                snapshot.failures.push(failure(
                    id,
                    ExtensionStage::AdmissionCursor,
                    "stored source cursor is invalid",
                ));
                continue;
            }
            let prepared = match catch_unwind(AssertUnwindSafe(|| source.poll(cursor))) {
                Ok(Ok(prepared)) => prepared,
                Ok(Err(message)) => {
                    let reset_failure = reset_source(source.as_mut()).err();
                    snapshot.failures.push(failure(
                        id,
                        ExtensionStage::AdmissionPoll,
                        match reset_failure {
                            Some(reset) => format!("{message}; source reset failed: {reset}"),
                            None => message,
                        },
                    ));
                    continue;
                }
                Err(_) => {
                    let reset_failure = reset_source(source.as_mut()).err();
                    snapshot.failures.push(failure(
                        id,
                        ExtensionStage::AdmissionPoll,
                        match reset_failure {
                            Some(reset) => {
                                format!("source adapter panicked; source reset failed: {reset}")
                            }
                            None => "source adapter panicked".into(),
                        },
                    ));
                    continue;
                }
            };
            if !valid_cursor(&prepared.next_cursor)
                || prepared
                    .observations
                    .len()
                    .saturating_add(prepared.skipped_unprojected_facts)
                    > MAX_OBSERVATIONS_PER_HARNESS
            {
                let reset_failure = reset_source(source.as_mut()).err();
                snapshot.failures.push(failure(
                    id,
                    ExtensionStage::AdmissionValidation,
                    match reset_failure {
                        Some(reset) => format!(
                            "source batch cursor or fact count exceeds limits; source reset failed: {reset}"
                        ),
                        None => "source batch cursor or fact count exceeds limits".into(),
                    },
                ));
                continue;
            }
            let rows = match validate_raw_observations(id, prepared.observations) {
                Ok(rows) => rows
                    .into_iter()
                    .map(|row| Observation {
                        harness_id: id.clone(),
                        event_id: row.event_id,
                        revision: row.revision,
                        identity: row.identity,
                        observed_at_unix_ns: row.observed_at_unix_ns,
                        kind: row.kind,
                    })
                    .collect::<Vec<_>>(),
                Err(message) => {
                    let reset_failure = reset_source(source.as_mut()).err();
                    snapshot.failures.push(failure(
                        id,
                        ExtensionStage::AdmissionValidation,
                        match reset_failure {
                            Some(reset) => format!("{message}; source reset failed: {reset}"),
                            None => message,
                        },
                    ));
                    continue;
                }
            };
            let committed = match store.commit(
                id.as_str(),
                expected_cursor.as_deref(),
                &prepared.next_cursor,
                &rows,
            ) {
                Ok(result) => result,
                Err(message) => {
                    snapshot
                        .failures
                        .push(failure(id, ExtensionStage::AdmissionCommit, message));
                    continue;
                }
            };
            let source_ack_sent =
                match catch_unwind(AssertUnwindSafe(|| source.ack(&prepared.next_cursor))) {
                    Ok(Ok(())) => true,
                    Ok(Err(message)) => {
                        let reset_failure = reset_source(source.as_mut()).err();
                        snapshot.failures.push(failure(
                            id,
                            ExtensionStage::AdmissionAck,
                            match reset_failure {
                                Some(reset) => format!("{message}; source reset failed: {reset}"),
                                None => message,
                            },
                        ));
                        false
                    }
                    Err(_) => {
                        let reset_failure = reset_source(source.as_mut()).err();
                        snapshot.failures.push(failure(
                            id,
                            ExtensionStage::AdmissionAck,
                            match reset_failure {
                                Some(reset) => {
                                    format!("source adapter panicked; source reset failed: {reset}")
                                }
                                None => "source adapter panicked".into(),
                            },
                        ));
                        false
                    }
                };
            snapshot.admissions.insert(
                id.clone(),
                AdmissionStatus {
                    inserted: committed.inserted,
                    replay: committed.replay,
                    skipped_unprojected_facts: prepared.skipped_unprojected_facts,
                    source_ack_sent,
                },
            );
        }
    }

    fn evaluate_extensions(
        &self,
        snapshot: &mut Snapshot,
        selection_scope: ObservationSelectionScope,
    ) {
        for (id, capability) in &self.capabilities {
            let result = catch_unwind(AssertUnwindSafe(|| {
                capability.evaluate(&snapshot.observations)
            }));
            match result {
                Ok(Ok(rows)) => match validate_metric_rows(rows) {
                    Ok(rows) => {
                        snapshot.metrics.insert(id.clone(), rows);
                    }
                    Err(message) => {
                        snapshot
                            .failures
                            .push(failure(id, ExtensionStage::Capability, message))
                    }
                },
                Ok(Err(message)) => {
                    snapshot
                        .failures
                        .push(failure(id, ExtensionStage::Capability, message))
                }
                Err(_) => snapshot.failures.push(failure(
                    id,
                    ExtensionStage::Capability,
                    "extension panicked",
                )),
            }
        }
        // Do not let a durable-history extension interpret a failed read as an
        // empty dataset. The store failure is already visible in the snapshot.
        let history_read_failed = snapshot
            .failures
            .iter()
            .any(|entry| entry.stage == ExtensionStage::AdmissionRead);
        if !history_read_failed {
            let input = ObservationExtensionInput {
                observations: &snapshot.observations,
                snapshot_id: snapshot.history_snapshot_id.watermark(),
                observations_truncated: snapshot.observations_truncated,
                selection_scope,
            };
            for (id, extension) in &self.observation_extensions {
                let result = catch_unwind(AssertUnwindSafe(|| extension.evaluate(&input)));
                match result {
                    Ok(Ok(output)) => {
                        let metrics = validate_metric_rows(output.metrics);
                        let pane = output.pane.map(validate_pane).transpose();
                        match (metrics, pane) {
                            (Ok(metrics), Ok(pane)) => {
                                snapshot.metrics.insert(id.clone(), metrics);
                                if let Some(pane) = pane {
                                    snapshot.panes.insert(id.clone(), pane);
                                }
                            }
                            (Err(message), _) | (_, Err(message)) => snapshot
                                .failures
                                .push(failure(id, ExtensionStage::ObservationExtension, message)),
                        }
                    }
                    Ok(Err(message)) => snapshot.failures.push(failure(
                        id,
                        ExtensionStage::ObservationExtension,
                        message,
                    )),
                    Err(_) => snapshot.failures.push(failure(
                        id,
                        ExtensionStage::ObservationExtension,
                        "extension panicked",
                    )),
                }
            }
        }
        for (id, contribution) in &self.ui_contributions {
            let result = catch_unwind(AssertUnwindSafe(|| contribution.build_pane(snapshot)));
            match result {
                Ok(Ok(pane)) => match validate_pane(pane) {
                    Ok(pane) => {
                        snapshot.panes.insert(id.clone(), pane);
                    }
                    Err(message) => {
                        snapshot
                            .failures
                            .push(failure(id, ExtensionStage::UiContribution, message))
                    }
                },
                Ok(Err(message)) => {
                    snapshot
                        .failures
                        .push(failure(id, ExtensionStage::UiContribution, message))
                }
                Err(_) => snapshot.failures.push(failure(
                    id,
                    ExtensionStage::UiContribution,
                    "extension panicked",
                )),
            }
        }
    }
}

fn valid_cursor(cursor: &str) -> bool {
    cursor.len() <= MAX_CURSOR_BYTES && !cursor.chars().any(char::is_control)
}

fn insert_unique<T>(
    registry: &mut BTreeMap<ExtensionId, T>,
    id: ExtensionId,
    extension: T,
) -> Result<(), RegistrationError> {
    if registry.contains_key(&id) {
        return Err(RegistrationError::DuplicateId(id.0));
    }
    if registry.len() >= MAX_EXTENSIONS_PER_KIND {
        return Err(RegistrationError::RegistryFull);
    }
    registry.insert(id, extension);
    Ok(())
}

fn first_duplicate_id<T, U>(
    existing: &BTreeMap<ExtensionId, T>,
    added: &BTreeMap<ExtensionId, U>,
) -> Option<String> {
    added
        .keys()
        .find(|id| existing.contains_key(*id))
        .map(|id| id.as_str().to_owned())
}

fn failure(id: &ExtensionId, stage: ExtensionStage, message: impl AsRef<str>) -> ExtensionFailure {
    ExtensionFailure {
        extension_id: id.clone(),
        stage,
        message: bounded(message.as_ref(), MAX_FAILURE_BYTES),
    }
}

fn bounded(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn validate_raw_observations(
    source: &ExtensionId,
    rows: Vec<RawObservation>,
) -> Result<Vec<RawObservation>, String> {
    validate_raw_observation_slice(source, &rows, MAX_OBSERVATIONS_PER_HARNESS)?;
    Ok(rows)
}

fn validate_raw_observation_slice(
    source: &ExtensionId,
    rows: &[RawObservation],
    max_observations: usize,
) -> Result<(), String> {
    if rows.len() > max_observations {
        return Err("observation count exceeds limit".into());
    }
    let mut event_revisions = BTreeSet::new();
    for row in rows {
        if row.event_id.is_empty()
            || row.event_id.len() > MAX_EVENT_ID_BYTES
            || !row.event_id.as_bytes()[0].is_ascii_alphanumeric()
            || !row
                .event_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
        {
            return Err("event identity is invalid or too long".into());
        }
        if !event_revisions.insert((row.event_id.as_str(), row.revision)) {
            return Err("duplicate event identity and revision".into());
        }
        let session_scope_valid = match row.identity.scope {
            ObservationScope::Session => row
                .identity
                .session_id
                .as_ref()
                .is_some_and(|value| valid_identity_part(value, MAX_SESSION_KEY_BYTES)),
            ObservationScope::Source => {
                matches!(
                    row.kind,
                    ObservationKind::Diagnostic { .. }
                        | ObservationKind::HostSample(_)
                        | ObservationKind::MetricPoint(_)
                        | ObservationKind::InternalReview(_)
                ) && row.identity.session_id.is_none()
                    && row.identity.worker_id.is_none()
                    && row.identity.turn_id.is_none()
                    && row.identity.operation_id.is_none()
            }
        };
        if !session_scope_valid
            || !row
                .identity
                .worker_id
                .as_ref()
                .is_none_or(|value| valid_identity_part(value, MAX_SESSION_KEY_BYTES))
            || !row
                .identity
                .turn_id
                .as_ref()
                .is_none_or(|value| valid_identity_part(value, MAX_SESSION_KEY_BYTES))
            || !row
                .identity
                .operation_id
                .as_ref()
                .is_none_or(|value| valid_identity_part(value, MAX_SESSION_KEY_BYTES))
        {
            return Err("native session, worker, turn, or operation identity is invalid".into());
        }
        if let ObservationKind::HostSample(sample) = &row.kind
            && (row.identity.scope != ObservationScope::Source
                || !is_host_source(source.as_str())
                || !sample.is_valid())
        {
            return Err(
                "host sample requires a valid host source and source-scoped metrics".into(),
            );
        }
        if let ObservationKind::MetricPoint(point) = &row.kind
            && (row.identity.scope != ObservationScope::Source
                || row.revision != 0
                || !is_metric_source(source.as_str())
                || !point.is_valid_at(row.observed_at_unix_ns)
                || !point.matches_event(&row.event_id, row.observed_at_unix_ns))
        {
            return Err(
                "metric point requires a valid OTLP source, event, and source scope".into(),
            );
        }
        if let ObservationKind::TraceSpanSummary(summary) = &row.kind
            && (row.identity.scope != ObservationScope::Session || !source.as_str().starts_with("otlp.") || row.revision != 0 || !summary.is_valid() || !summary.matches_event(&row.event_id) || !rows.iter().any(|anchor| {
                matches!(&anchor.kind, ObservationKind::TraceSpan(span) if span.trace_id == summary.trace_id && span.span_id == summary.span_id)
                && anchor.identity == row.identity && anchor.observed_at_unix_ns == row.observed_at_unix_ns && anchor.revision == row.revision
            })) {
            return Err("trace summary requires its exact structural span in the same batch".into());
        }
        if let ObservationKind::OtlpActivityLog(log) = &row.kind
            && (row.identity.scope != ObservationScope::Session
                || !source.as_str().starts_with("otlp.")
                || row.revision != 0
                || row.identity.operation_id.is_some()
                || !log.is_valid()
                || !log.matches_event(&row.event_id)
                || (row.identity.turn_id.is_some() && log.target_source_id.is_none())
                || (row.identity.worker_id.is_some() && row.identity.turn_id.is_none()))
        {
            return Err("activity log requires exact OTLP session scope and safe metadata".into());
        }
        let review_source =
            agenturm_protocol::native_review::is_codex_review_source(source.as_str());
        if review_source
            && !matches!(
                row.kind,
                ObservationKind::InternalReview(_) | ObservationKind::Diagnostic { .. }
            )
        {
            return Err("internal review sources may emit only review metadata".into());
        }
        if let ObservationKind::InternalReview(review) = &row.kind
            && (!review_source
                || row.identity.scope != ObservationScope::Source
                || !review.is_valid())
        {
            return Err("internal review metadata requires a valid reserved source".into());
        }
        if let ObservationKind::NativeRequest(request) = &row.kind
            && (row.identity.scope != ObservationScope::Session
                || row.identity.turn_id.is_none()
                || !request.is_valid())
        {
            return Err("native requests require a session, turn and opaque request ID".into());
        }
        if let ObservationKind::Activity { category } = &row.kind
            && (category.is_empty()
                || category.len() > MAX_CATEGORY_BYTES
                || category.chars().any(char::is_control))
        {
            return Err("activity category is invalid or exceeds limit".into());
        }
        if let ObservationKind::Tool { category, result } = &row.kind
            && (category.is_empty()
                || category.len() > MAX_CATEGORY_BYTES
                || category.chars().any(char::is_control)
                || !matches!(category.as_str(), "shell" | "file" | "mcp" | "other")
                || !matches!(
                    result.as_str(),
                    "started" | "succeeded" | "failed" | "denied"
                ))
        {
            return Err("tool observation is invalid or exceeds limit".into());
        }
        if let ObservationKind::FileEdit(edit) = row.kind
            && (!edit.is_valid() || !is_file_edit_operation(&row.identity))
        {
            return Err("file edit counts or operation identity are invalid".into());
        }
        if let ObservationKind::McpCall {
            server,
            tool,
            duration_ns,
            result,
            ..
        } = &row.kind
            && (!is_mcp_operation(&row.identity)
                || !is_valid_mcp_metadata(server.as_deref(), tool.as_deref(), *duration_ns, result))
        {
            return Err("MCP call metadata or operation identity is invalid".into());
        }
        if let ObservationKind::TraceSpan(span) = &row.kind
            && (!span.is_valid(row.observed_at_unix_ns)
                || !is_trace_turn_link_valid(
                    span,
                    &row.event_id,
                    row.identity.turn_id.as_deref(),
                    row.identity.worker_id.as_deref(),
                    row.identity.operation_id.as_deref(),
                ))
        {
            return Err("trace identity or timing is invalid".into());
        }
        if let ObservationKind::Lifecycle { state, reason_code } = &row.kind
            && (!is_lifecycle_state(state)
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
            return Err("lifecycle observation state or reason is invalid".into());
        }
        if let ObservationKind::Check { category, result } = &row.kind
            && (!matches!(category.as_str(), "test" | "lint" | "build")
                || !matches!(result.as_str(), "passed" | "failed" | "unknown"))
        {
            return Err("check observation category or result is invalid".into());
        }
        if let ObservationKind::PullRequestRaised { repository, number } = &row.kind
            && !is_anchored_pull_request(repository, *number, &row.identity)
        {
            return Err("pull request observation reference or identity is invalid".into());
        }
        if let ObservationKind::Relationship {
            parent_session_id, ..
        } = &row.kind
            && !is_session_relationship(parent_session_id, &row.identity)
        {
            return Err("relationship parent or identity is invalid".into());
        }
        if let ObservationKind::Message { role } = &row.kind
            && !matches!(role.as_str(), "user" | "assistant")
        {
            return Err("message observation role is invalid".into());
        }
        if let ObservationKind::CreditUsage { nanocredits } = &row.kind
            && *nanocredits > i64::MAX as u64
        {
            return Err("credit usage exceeds supported range".into());
        }
        if let ObservationKind::ReportedCost(cost) = &row.kind
            && (row.identity.scope != ObservationScope::Session
                || !cost.is_valid()
                || !cost.matches_evidence(&row.event_id, row.identity.operation_id.as_deref()))
        {
            return Err("reported cost metadata or event identity is invalid".into());
        }
        if let ObservationKind::TraceProgress(progress) = &row.kind
            && (row.identity.scope != ObservationScope::Session
                || row.revision != 0
                || !progress.is_valid(&row.event_id, row.observed_at_unix_ns)
                || !is_trace_progress_scope_valid(
                    progress,
                    row.identity.turn_id.as_deref(),
                    row.identity.worker_id.as_deref(),
                    row.identity.operation_id.as_deref(),
                ))
        {
            return Err("trace progress metadata or event identity is invalid".into());
        }
        if let ObservationKind::SpanTokenUsage(usage) = &row.kind
            && (row.identity.scope != ObservationScope::Session
                || !is_span_usage_anchor_valid(
                    usage,
                    &row.event_id,
                    row.identity.turn_id.as_deref(),
                    row.identity.worker_id.as_deref(),
                    row.identity.operation_id.as_deref(),
                ))
        {
            return Err("span token usage or event identity is invalid".into());
        }
        if let ObservationKind::Diagnostic { code, severity } = &row.kind
            && (!is_safe_diagnostic_code(code)
                || !matches!(severity.as_str(), "info" | "warning" | "error"))
        {
            return Err("diagnostic code or severity is invalid".into());
        }
    }
    Ok(())
}

fn valid_identity_part(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
}

fn validate_metric_rows(rows: Vec<MetricRow>) -> Result<Vec<MetricRow>, String> {
    if rows.len() > MAX_METRIC_ROWS {
        return Err("metric row count exceeds limit".into());
    }
    validate_metric_row_text(&rows)?;
    Ok(rows)
}

fn validate_metric_row_text(rows: &[MetricRow]) -> Result<(), String> {
    for row in rows {
        if row.label.is_empty()
            || row.label.len() > MAX_LABEL_BYTES
            || row.value.len() > MAX_VALUE_BYTES
        {
            return Err("metric row text is empty or exceeds limit".into());
        }
    }
    Ok(())
}

fn validate_pane(pane: PaneModel) -> Result<PaneModel, String> {
    if pane.title.is_empty() || pane.title.len() > MAX_TITLE_BYTES {
        return Err("pane title is empty or exceeds limit".into());
    }
    if pane.rows.len() > MAX_METRIC_ROWS {
        return Err("metric row count exceeds limit".into());
    }
    validate_metric_row_text(&pane.rows)?;
    Ok(pane)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct FixedHarness(Result<Vec<RawObservation>, &'static str>);

    impl Harness for FixedHarness {
        fn poll(&mut self) -> Result<Vec<RawObservation>, String> {
            self.0.clone().map_err(str::to_owned)
        }
    }

    struct ActivityCount;

    impl Capability for ActivityCount {
        fn evaluate(&self, observations: &[Observation]) -> Result<Vec<MetricRow>, String> {
            let count = observations
                .iter()
                .filter(|row| matches!(row.kind, ObservationKind::Activity { .. }))
                .count();
            Ok(vec![MetricRow {
                label: "activity events".into(),
                value: count.to_string(),
            }])
        }
    }

    struct ActivityPane;

    impl UiContribution for ActivityPane {
        fn build_pane(&self, snapshot: &Snapshot) -> Result<PaneModel, String> {
            let count = snapshot.observations.len();
            Ok(PaneModel {
                title: "Activity".into(),
                rows: vec![MetricRow {
                    label: "observations".into(),
                    value: count.to_string(),
                }],
            })
        }
    }

    struct FixedObservationExtension(ObservationExtensionOutput);

    impl ObservationExtension for FixedObservationExtension {
        fn evaluate(
            &self,
            _input: &ObservationExtensionInput<'_>,
        ) -> Result<ObservationExtensionOutput, String> {
            Ok(self.0.clone())
        }
    }

    type ObservationExtensionContext = (u64, bool, ObservationSelectionScope, usize);

    struct RecordingObservationExtension(Arc<Mutex<Vec<ObservationExtensionContext>>>);

    impl ObservationExtension for RecordingObservationExtension {
        fn evaluate(
            &self,
            input: &ObservationExtensionInput<'_>,
        ) -> Result<ObservationExtensionOutput, String> {
            self.0.lock().unwrap().push((
                input.snapshot_id,
                input.observations_truncated,
                input.selection_scope,
                input.observations.len(),
            ));
            Ok(ObservationExtensionOutput::default())
        }
    }

    fn id(value: &str) -> ExtensionId {
        ExtensionId::parse(value).unwrap()
    }

    fn activity(session_key: &str) -> RawObservation {
        RawObservation {
            event_id: format!("fixture:{session_key}"),
            revision: 0,
            identity: ObservationIdentity {
                scope: ObservationScope::Session,
                session_id: Some(session_key.into()),
                worker_id: Some("worker-1".into()),
                turn_id: Some("turn-1".into()),
                operation_id: Some("operation-1".into()),
            },
            observed_at_unix_ns: 123,
            kind: ObservationKind::Activity {
                category: "tool".into(),
            },
        }
    }

    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<String>>>);

    impl Log {
        fn push(&self, value: impl Into<String>) {
            self.0.lock().unwrap().push(value.into());
        }

        fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    struct FixedSource {
        name: &'static str,
        batch: PreparedSourceBatch,
        fail_ack: bool,
        panic_ack: bool,
        fail_poll: bool,
        fail_reset: bool,
        panic_reset: bool,
        log: Log,
    }

    impl SourceAdapter for FixedSource {
        fn source_id(&self) -> &str {
            if self.name == "codex" {
                "harness.codex"
            } else if self.name == "alpha" {
                "harness.alpha"
            } else {
                "harness.zeta"
            }
        }

        fn poll(&mut self, cursor: &str) -> Result<PreparedSourceBatch, String> {
            self.log.push(format!("poll:{}:{cursor}", self.name));
            if self.fail_poll {
                return Err("injected poll failure".into());
            }
            Ok(self.batch.clone())
        }

        fn ack(&mut self, cursor: &str) -> Result<(), String> {
            self.log.push(format!("ack:{}:{cursor}", self.name));
            assert!(!self.panic_ack, "injected ACK panic");
            if self.fail_ack {
                Err("injected ACK failure".into())
            } else {
                Ok(())
            }
        }

        fn reset(&mut self) -> Result<(), String> {
            self.log.push(format!("reset:{}", self.name));
            assert!(!self.panic_reset, "injected reset panic");
            if self.fail_reset {
                Err("injected reset failure".into())
            } else {
                Ok(())
            }
        }
    }

    struct FixedStore {
        cursor: Option<String>,
        fail_commit: bool,
        replay: bool,
        rows: Vec<Observation>,
        log: Log,
    }

    impl ObservationStore for FixedStore {
        fn source_cursor(&self, source_id: &str) -> Result<Option<String>, String> {
            self.log.push(format!("cursor:{source_id}"));
            Ok(self.cursor.clone())
        }

        fn load_observations(&self, limit: usize) -> Result<ObservationPage, String> {
            let has_more = self.rows.len() > limit;
            let start = self.rows.len().saturating_sub(limit);
            Ok(ObservationPage {
                observations: self.rows[start..].to_vec(),
                has_more,
                snapshot_id: 0.into(),
            })
        }

        fn query_observations(&self, query: &ObservationQuery) -> Result<ObservationPage, String> {
            self.load_observations(query.limit)
        }

        fn commit(
            &mut self,
            source_id: &str,
            _expected_cursor: Option<&str>,
            next_cursor: &str,
            observations: &[Observation],
        ) -> Result<StoreCommit, String> {
            self.log.push(format!("commit:{source_id}:{next_cursor}"));
            if self.fail_commit {
                Err("injected storage failure".into())
            } else {
                if !self.replay {
                    self.rows.extend_from_slice(observations);
                }
                Ok(StoreCommit {
                    inserted: if self.replay { 0 } else { observations.len() },
                    replay: self.replay,
                })
            }
        }
    }

    fn prepared(batch_event: &str) -> PreparedSourceBatch {
        PreparedSourceBatch {
            next_cursor: "cursor-1".into(),
            observations: vec![activity(batch_event)],
            skipped_unprojected_facts: 0,
        }
    }

    #[test]
    fn host_stamps_identity_and_builds_capability_and_pane_contributions() {
        let mut kernel = Kernel::new();
        kernel
            .register_harness(id("harness.codex"), FixedHarness(Ok(vec![activity("s1")])))
            .unwrap();
        kernel
            .register_capability(id("capability.activity-count"), ActivityCount)
            .unwrap();
        kernel
            .register_ui_contribution(id("pane.activity"), ActivityPane)
            .unwrap();

        let snapshot = kernel.refresh();
        assert_eq!(snapshot.observations.len(), 1);
        assert_eq!(snapshot.observations[0].event_id, "fixture:s1");
        assert_eq!(snapshot.observations[0].revision, 0);
        assert_eq!(
            snapshot.observations[0].identity.session_id.as_deref(),
            Some("s1")
        );
        assert_eq!(
            snapshot.observations[0].identity.worker_id.as_deref(),
            Some("worker-1")
        );
        assert_eq!(
            snapshot.observations[0].harness_id.as_str(),
            "harness.codex"
        );
        assert_eq!(
            snapshot.metrics[&id("capability.activity-count")][0].value,
            "1"
        );
        assert_eq!(snapshot.panes[&id("pane.activity")].title, "Activity");
        assert!(snapshot.failures.is_empty());
    }

    struct OnePageStore(ObservationPage);

    impl ObservationStore for OnePageStore {
        fn source_cursor(&self, _source_id: &str) -> Result<Option<String>, String> {
            Ok(None)
        }

        fn load_observations(&self, _limit: usize) -> Result<ObservationPage, String> {
            Ok(self.0.clone())
        }

        fn query_observations(&self, _query: &ObservationQuery) -> Result<ObservationPage, String> {
            Ok(self.0.clone())
        }

        fn commit(
            &mut self,
            _source_id: &str,
            _expected_cursor: Option<&str>,
            _next_cursor: &str,
            _observations: &[Observation],
        ) -> Result<StoreCommit, String> {
            Ok(StoreCommit {
                inserted: 0,
                replay: false,
            })
        }
    }

    struct FailingReadStore;

    impl ObservationStore for FailingReadStore {
        fn source_cursor(&self, _source_id: &str) -> Result<Option<String>, String> {
            Ok(None)
        }

        fn load_observations(&self, _limit: usize) -> Result<ObservationPage, String> {
            Err("store unavailable".into())
        }

        fn query_observations(&self, _query: &ObservationQuery) -> Result<ObservationPage, String> {
            Err("store unavailable".into())
        }

        fn commit(
            &mut self,
            _source_id: &str,
            _expected_cursor: Option<&str>,
            _next_cursor: &str,
            _observations: &[Observation],
        ) -> Result<StoreCommit, String> {
            Err("store unavailable".into())
        }
    }

    #[test]
    fn observation_extension_is_not_evaluated_when_durable_history_read_fails() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let mut kernel = Kernel::new();
        kernel
            .register_observation_extension(
                id("plugin.context"),
                RecordingObservationExtension(Arc::clone(&received)),
            )
            .unwrap();

        let snapshot =
            kernel.refresh_query_with_store(&FailingReadStore, ObservationQuery::latest(10));

        assert!(received.lock().unwrap().is_empty());
        assert!(snapshot.observations.is_empty());
        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(snapshot.failures[0].stage, ExtensionStage::AdmissionRead);
    }

    #[test]
    fn observation_extension_publishes_metrics_and_pane_atomically() {
        let mut kernel = Kernel::new();
        kernel
            .register_harness(id("harness.pi"), FixedHarness(Ok(vec![activity("s1")])))
            .unwrap();
        kernel
            .register_observation_extension(
                id("plugin.lifecycle"),
                FixedObservationExtension(ObservationExtensionOutput {
                    metrics: vec![MetricRow {
                        label: "interrupted".into(),
                        value: "1".into(),
                    }],
                    pane: Some(PaneModel {
                        title: "Lifecycle".into(),
                        rows: vec![MetricRow {
                            label: "harness.pi".into(),
                            value: "1 interrupted".into(),
                        }],
                    }),
                }),
            )
            .unwrap();

        let snapshot = kernel.refresh();
        assert_eq!(snapshot.metrics[&id("plugin.lifecycle")][0].value, "1");
        assert_eq!(snapshot.panes[&id("plugin.lifecycle")].title, "Lifecycle");
        assert!(snapshot.failures.is_empty());
    }

    #[test]
    fn invalid_observation_extension_pane_does_not_publish_its_metrics() {
        let mut kernel = Kernel::new();
        kernel
            .register_observation_extension(
                id("plugin.invalid"),
                FixedObservationExtension(ObservationExtensionOutput {
                    metrics: vec![MetricRow {
                        label: "valid".into(),
                        value: "1".into(),
                    }],
                    pane: Some(PaneModel {
                        title: "x".repeat(MAX_TITLE_BYTES + 1),
                        rows: vec![],
                    }),
                }),
            )
            .unwrap();
        let snapshot = kernel.refresh();
        assert!(!snapshot.metrics.contains_key(&id("plugin.invalid")));
        assert!(!snapshot.panes.contains_key(&id("plugin.invalid")));
        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(
            snapshot.failures[0].stage,
            ExtensionStage::ObservationExtension
        );
    }

    #[test]
    fn observation_extension_context_preserves_query_scope_and_truncation() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let mut kernel = Kernel::new();
        kernel
            .register_observation_extension(
                id("plugin.context"),
                RecordingObservationExtension(Arc::clone(&received)),
            )
            .unwrap();
        let store = OnePageStore(ObservationPage {
            observations: vec![],
            has_more: true,
            snapshot_id: 23.into(),
        });
        kernel.refresh_query_with_store(
            &store,
            ObservationQuery {
                harness_id: Some("harness.pi".into()),
                session_id: None,
                before: Some(ObservationPosition {
                    observed_at_unix_ns: 10,
                    harness_id: "harness.pi".into(),
                    event_id: "event-9".into(),
                    revision: 1,
                    snapshot_id: 23.into(),
                }),
                snapshot_id: Some(23.into()),
                limit: 10,
            },
        );
        assert_eq!(
            received.lock().unwrap().as_slice(),
            [(23, true, ObservationSelectionScope::Continuation, 0)]
        );
    }

    #[test]
    fn observation_extension_ids_cannot_overwrite_capability_or_pane_outputs() {
        let mut kernel = Kernel::new();
        kernel
            .register_capability(id("plugin.shared"), ActivityCount)
            .unwrap();
        assert!(matches!(
            kernel.register_observation_extension(
                id("plugin.shared"),
                FixedObservationExtension(ObservationExtensionOutput::default())
            ),
            Err(RegistrationError::DuplicateId(_))
        ));

        let mut other_kernel = Kernel::new();
        other_kernel
            .register_observation_extension(
                id("plugin.shared"),
                FixedObservationExtension(ObservationExtensionOutput::default()),
            )
            .unwrap();
        assert!(matches!(
            other_kernel.register_ui_contribution(id("plugin.shared"), ActivityPane),
            Err(RegistrationError::DuplicateId(_))
        ));
    }

    #[test]
    fn supervised_admission_commits_before_ack_and_stamps_registered_source() {
        let log = Log::default();
        let mut kernel = Kernel::new();
        kernel
            .register_source_adapter(
                id("harness.codex"),
                FixedSource {
                    name: "codex",
                    batch: prepared("fixture:event-1"),
                    fail_ack: false,
                    panic_ack: false,
                    fail_poll: false,
                    fail_reset: false,
                    panic_reset: false,
                    log: log.clone(),
                },
            )
            .unwrap();
        let mut store = FixedStore {
            cursor: None,
            fail_commit: false,
            replay: false,
            rows: Vec::new(),
            log: log.clone(),
        };

        let snapshot = kernel.refresh_with_store(&mut store);
        assert_eq!(
            log.take(),
            [
                "cursor:harness.codex",
                "poll:codex:",
                "commit:harness.codex:cursor-1",
                "ack:codex:cursor-1"
            ]
        );
        assert_eq!(snapshot.observations.len(), 1);
        assert_eq!(
            snapshot.observations[0].harness_id.as_str(),
            "harness.codex"
        );
        let status = snapshot.admissions[&id("harness.codex")];
        assert_eq!(status.inserted, 1);
        assert!(status.source_ack_sent);
        assert!(snapshot.failures.is_empty());
    }

    #[test]
    fn durable_snapshot_excludes_uncommitted_in_process_harness_facts() {
        let log = Log::default();
        let mut kernel = Kernel::new();
        kernel
            .register_harness(
                id("harness.trusted"),
                FixedHarness(Ok(vec![activity("uncommitted")])),
            )
            .unwrap();
        kernel
            .register_source_adapter(
                id("harness.codex"),
                FixedSource {
                    name: "codex",
                    batch: prepared("committed"),
                    fail_ack: false,
                    panic_ack: false,
                    fail_poll: false,
                    fail_reset: false,
                    panic_reset: false,
                    log: log.clone(),
                },
            )
            .unwrap();
        let mut store = FixedStore {
            cursor: None,
            fail_commit: false,
            replay: false,
            rows: Vec::new(),
            log,
        };

        let snapshot = kernel.refresh_with_store(&mut store);
        assert_eq!(snapshot.observations.len(), 1);
        assert_eq!(snapshot.observations[0].event_id, "fixture:committed");
    }

    #[test]
    fn read_only_refresh_rehydrates_committed_history_for_capabilities_and_panes() {
        let mut kernel = Kernel::new();
        kernel
            .register_capability(id("capability.activity-count"), ActivityCount)
            .unwrap();
        kernel
            .register_ui_contribution(id("pane.activity"), ActivityPane)
            .unwrap();
        let stored = |harness_id: &str, session_id: &str, timestamp| {
            let raw = activity(session_id);
            Observation {
                harness_id: id(harness_id),
                event_id: raw.event_id,
                revision: raw.revision,
                identity: raw.identity,
                observed_at_unix_ns: timestamp,
                kind: raw.kind,
            }
        };
        let mut store = FixedStore {
            cursor: None,
            fail_commit: false,
            replay: false,
            rows: vec![
                stored("harness.zeta", "stored-session-1", 10),
                stored("harness.alpha", "stored-session-2", 20),
            ],
            log: Log::default(),
        };

        let snapshot = kernel.refresh_with_store(&mut store);

        assert_eq!(snapshot.observations.len(), 2);
        assert_eq!(snapshot.observations[0].harness_id.as_str(), "harness.zeta");
        assert_eq!(
            snapshot.observations[1].harness_id.as_str(),
            "harness.alpha"
        );
        assert_eq!(
            snapshot.metrics[&id("capability.activity-count")][0].value,
            "2"
        );
        assert_eq!(snapshot.panes[&id("pane.activity")].rows[0].value, "2");
        assert!(!snapshot.observations_truncated);
        assert!(snapshot.failures.is_empty());
    }

    #[test]
    fn malformed_or_uncommitted_source_batch_is_never_acknowledged() {
        for (bad_event, fail_commit, expected_stage) in [
            ("bad event", false, ExtensionStage::AdmissionValidation),
            ("event-1", true, ExtensionStage::AdmissionCommit),
        ] {
            let log = Log::default();
            let mut batch = prepared(bad_event);
            if fail_commit {
                batch.observations[0] = activity("event-1");
            }
            let mut kernel = Kernel::new();
            kernel
                .register_source_adapter(
                    id("harness.codex"),
                    FixedSource {
                        name: "codex",
                        batch,
                        fail_ack: false,
                        panic_ack: false,
                        fail_poll: false,
                        fail_reset: false,
                        panic_reset: false,
                        log: log.clone(),
                    },
                )
                .unwrap();
            let mut store = FixedStore {
                cursor: None,
                fail_commit,
                replay: false,
                rows: Vec::new(),
                log: log.clone(),
            };

            let snapshot = kernel.refresh_with_store(&mut store);
            assert_eq!(snapshot.failures.len(), 1);
            assert_eq!(snapshot.failures[0].stage, expected_stage);
            assert!(snapshot.admissions.is_empty());
            assert!(snapshot.observations.is_empty());
            assert!(!log.take().iter().any(|entry| entry.starts_with("ack:")));
        }
    }

    #[test]
    fn invalid_check_category_fails_admission_without_commit_or_ack() {
        let log = Log::default();
        let mut batch = prepared("event-1");
        batch.observations[0].kind = ObservationKind::Check {
            category: "coverage".into(),
            result: "passed".into(),
        };
        let mut kernel = Kernel::new();
        kernel
            .register_source_adapter(
                id("harness.codex"),
                FixedSource {
                    name: "codex",
                    batch,
                    fail_ack: false,
                    panic_ack: false,
                    fail_poll: false,
                    fail_reset: false,
                    panic_reset: false,
                    log: log.clone(),
                },
            )
            .unwrap();
        let mut store = FixedStore {
            cursor: None,
            fail_commit: false,
            replay: false,
            rows: Vec::new(),
            log: log.clone(),
        };

        let snapshot = kernel.refresh_with_store(&mut store);
        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(
            snapshot.failures[0].stage,
            ExtensionStage::AdmissionValidation
        );
        assert!(snapshot.admissions.is_empty());
        assert!(snapshot.observations.is_empty());
        let entries = log.take();
        assert!(!entries.iter().any(|entry| entry.starts_with("commit:")));
        assert!(!entries.iter().any(|entry| entry.starts_with("ack:")));
    }

    #[test]
    fn invalid_message_role_fails_admission_without_commit_or_ack() {
        let log = Log::default();
        let mut batch = prepared("event-1");
        batch.observations[0].kind = ObservationKind::Message {
            role: "system".into(),
        };
        let mut kernel = Kernel::new();
        kernel
            .register_source_adapter(
                id("harness.codex"),
                FixedSource {
                    name: "codex",
                    batch,
                    fail_ack: false,
                    panic_ack: false,
                    fail_poll: false,
                    fail_reset: false,
                    panic_reset: false,
                    log: log.clone(),
                },
            )
            .unwrap();
        let mut store = FixedStore {
            cursor: None,
            fail_commit: false,
            replay: false,
            rows: Vec::new(),
            log: log.clone(),
        };

        let snapshot = kernel.refresh_with_store(&mut store);
        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(
            snapshot.failures[0].stage,
            ExtensionStage::AdmissionValidation
        );
        assert!(snapshot.admissions.is_empty());
        assert!(snapshot.observations.is_empty());
        let entries = log.take();
        assert!(!entries.iter().any(|entry| entry.starts_with("commit:")));
        assert!(!entries.iter().any(|entry| entry.starts_with("ack:")));
    }

    #[test]
    fn source_scoped_gap_diagnostic_is_admitted_without_fake_session() {
        assert!(is_safe_diagnostic_code("kiro_queued_prompt"));
        assert!(!is_safe_diagnostic_code("private_prompt_text"));
        let log = Log::default();
        let mut batch = prepared("gap-event");
        batch.observations[0].identity = ObservationIdentity::source();
        batch.observations[0].kind = ObservationKind::Diagnostic {
            code: "possible_source_gap".into(),
            severity: "warning".into(),
        };
        let mut kernel = Kernel::new();
        kernel
            .register_source_adapter(
                id("harness.zeta"),
                FixedSource {
                    name: "pi",
                    batch,
                    fail_ack: false,
                    panic_ack: false,
                    fail_poll: false,
                    fail_reset: false,
                    panic_reset: false,
                    log: log.clone(),
                },
            )
            .unwrap();
        let mut store = FixedStore {
            cursor: None,
            fail_commit: false,
            replay: false,
            rows: Vec::new(),
            log: log.clone(),
        };

        let snapshot = kernel.refresh_with_store(&mut store);
        assert!(snapshot.failures.is_empty());
        assert_eq!(snapshot.observations.len(), 1);
        assert_eq!(
            snapshot.observations[0].identity.scope,
            ObservationScope::Source
        );
        assert_eq!(snapshot.observations[0].identity.session_id, None);
        let entries = log.take();
        assert!(entries.iter().any(|entry| entry.starts_with("commit:")));
        assert!(entries.iter().any(|entry| entry.starts_with("ack:")));
    }

    #[test]
    fn invalid_cursor_batch_resets_source_without_commit_or_ack() {
        let log = Log::default();
        let mut batch = prepared("event-1");
        batch.next_cursor = "c".repeat(MAX_CURSOR_BYTES + 1);
        let mut kernel = Kernel::new();
        kernel
            .register_source_adapter(
                id("harness.codex"),
                FixedSource {
                    name: "codex",
                    batch,
                    fail_ack: false,
                    panic_ack: false,
                    fail_poll: false,
                    fail_reset: false,
                    panic_reset: false,
                    log: log.clone(),
                },
            )
            .unwrap();
        let mut store = FixedStore {
            cursor: None,
            fail_commit: false,
            replay: false,
            rows: Vec::new(),
            log: log.clone(),
        };

        let snapshot = kernel.refresh_with_store(&mut store);
        assert_eq!(
            snapshot.failures[0].stage,
            ExtensionStage::AdmissionValidation
        );
        let events = log.take();
        assert!(events.iter().any(|event| event == "reset:codex"));
        assert!(!events.iter().any(|event| event.starts_with("commit:")));
        assert!(!events.iter().any(|event| event.starts_with("ack:")));
    }

    #[test]
    fn exact_store_replay_is_acked_only_after_the_store_confirms_it() {
        let log = Log::default();
        let mut kernel = Kernel::new();
        kernel
            .register_source_adapter(
                id("harness.codex"),
                FixedSource {
                    name: "codex",
                    batch: prepared("fixture:event-1"),
                    fail_ack: false,
                    panic_ack: false,
                    fail_poll: false,
                    fail_reset: false,
                    panic_reset: false,
                    log: log.clone(),
                },
            )
            .unwrap();
        let mut store = FixedStore {
            cursor: Some("cursor-0".into()),
            fail_commit: false,
            replay: true,
            rows: Vec::new(),
            log: log.clone(),
        };

        let snapshot = kernel.refresh_with_store(&mut store);
        assert_eq!(
            log.take(),
            [
                "cursor:harness.codex",
                "poll:codex:cursor-0",
                "commit:harness.codex:cursor-1",
                "ack:codex:cursor-1"
            ]
        );
        assert!(snapshot.admissions[&id("harness.codex")].replay);
        assert!(snapshot.admissions[&id("harness.codex")].source_ack_sent);
    }

    #[test]
    fn source_registry_polls_in_stable_id_order_and_ack_failure_keeps_commit_visible() {
        let log = Log::default();
        let mut kernel = Kernel::new();
        for (id_text, name, fail_ack) in [
            ("harness.zeta", "zeta", true),
            ("harness.alpha", "alpha", false),
        ] {
            kernel
                .register_source_adapter(
                    id(id_text),
                    FixedSource {
                        name,
                        batch: prepared(&format!("fixture:{name}")),
                        fail_ack,
                        panic_ack: false,
                        fail_poll: false,
                        fail_reset: false,
                        panic_reset: false,
                        log: log.clone(),
                    },
                )
                .unwrap();
        }
        let mut store = FixedStore {
            cursor: None,
            fail_commit: false,
            replay: false,
            rows: Vec::new(),
            log: log.clone(),
        };
        let snapshot = kernel.refresh_with_store(&mut store);
        let events = log.take();
        let alpha_poll = events
            .iter()
            .position(|event| event == "poll:alpha:")
            .unwrap();
        let zeta_poll = events
            .iter()
            .position(|event| event == "poll:zeta:")
            .unwrap();
        assert!(alpha_poll < zeta_poll);
        assert_eq!(snapshot.observations.len(), 2);
        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(snapshot.failures[0].stage, ExtensionStage::AdmissionAck);
        assert!(!snapshot.admissions[&id("harness.zeta")].source_ack_sent);
        assert!(snapshot.admissions[&id("harness.alpha")].source_ack_sent);
    }

    #[test]
    fn ack_panic_resets_source_and_reports_reset_failure_after_commit() {
        let log = Log::default();
        let mut kernel = Kernel::new();
        kernel
            .register_source_adapter(
                id("harness.codex"),
                FixedSource {
                    name: "codex",
                    batch: prepared("fixture:event-1"),
                    fail_ack: false,
                    panic_ack: true,
                    fail_poll: false,
                    fail_reset: false,
                    panic_reset: true,
                    log: log.clone(),
                },
            )
            .unwrap();
        let mut store = FixedStore {
            cursor: None,
            fail_commit: false,
            replay: false,
            rows: Vec::new(),
            log: log.clone(),
        };

        let snapshot = kernel.refresh_with_store(&mut store);

        assert_eq!(snapshot.observations.len(), 1);
        assert!(!snapshot.admissions[&id("harness.codex")].source_ack_sent);
        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(snapshot.failures[0].stage, ExtensionStage::AdmissionAck);
        assert!(
            snapshot.failures[0]
                .message
                .contains("source adapter panicked")
        );
        assert!(
            snapshot.failures[0]
                .message
                .contains("source reset failed: source adapter panicked during reset")
        );
        let events = log.take();
        assert!(
            events
                .iter()
                .any(|event| event == "commit:harness.codex:cursor-1")
        );
        assert!(events.iter().any(|event| event == "reset:codex"));
    }

    #[test]
    fn poll_error_resets_source_before_the_next_refresh() {
        let log = Log::default();
        let mut kernel = Kernel::new();
        kernel
            .register_source_adapter(
                id("harness.codex"),
                FixedSource {
                    name: "codex",
                    batch: prepared("fixture:event-1"),
                    fail_ack: false,
                    panic_ack: false,
                    fail_poll: true,
                    fail_reset: false,
                    panic_reset: false,
                    log: log.clone(),
                },
            )
            .unwrap();
        let mut store = FixedStore {
            cursor: None,
            fail_commit: false,
            replay: false,
            rows: Vec::new(),
            log: log.clone(),
        };

        let snapshot = kernel.refresh_with_store(&mut store);

        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(snapshot.failures[0].stage, ExtensionStage::AdmissionPoll);
        assert_eq!(snapshot.failures[0].message, "injected poll failure");
        assert_eq!(log.take().last().unwrap(), "reset:codex");
    }

    #[test]
    fn failing_harness_does_not_hide_healthy_harness_output() {
        let mut kernel = Kernel::new();
        kernel
            .register_harness(id("harness.a-fails"), FixedHarness(Err("offline")))
            .unwrap();
        kernel
            .register_harness(
                id("harness.b-works"),
                FixedHarness(Ok(vec![activity("s2")])),
            )
            .unwrap();

        let snapshot = kernel.refresh();
        assert_eq!(snapshot.observations.len(), 1);
        assert_eq!(
            snapshot.observations[0].harness_id.as_str(),
            "harness.b-works"
        );
        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(snapshot.failures[0].stage, ExtensionStage::Poll);
    }

    #[test]
    fn invalid_and_duplicate_extension_ids_are_rejected() {
        assert!(matches!(
            ExtensionId::parse("Upper"),
            Err(RegistrationError::InvalidId(_))
        ));
        let mut kernel = Kernel::new();
        kernel
            .register_harness(id("harness.pi"), FixedHarness(Ok(vec![])))
            .unwrap();
        assert_eq!(
            kernel
                .register_harness(id("harness.pi"), FixedHarness(Ok(vec![])))
                .unwrap_err(),
            RegistrationError::DuplicateId("harness.pi".into()),
        );
    }

    #[test]
    fn source_adapter_cannot_be_registered_under_another_source_identity() {
        let mut kernel = Kernel::new();
        let error = kernel
            .register_source_adapter(
                id("harness.pi"),
                FixedSource {
                    name: "codex",
                    batch: prepared("fixture:event-1"),
                    fail_ack: false,
                    panic_ack: false,
                    fail_poll: false,
                    fail_reset: false,
                    panic_reset: false,
                    log: Log::default(),
                },
            )
            .unwrap_err();
        assert_eq!(
            error,
            RegistrationError::SourceIdentityMismatch {
                registered: "harness.pi".into(),
                configured: "harness.codex".into(),
            }
        );
    }

    #[test]
    fn duplicate_event_identity_in_one_harness_batch_is_rejected() {
        let mut kernel = Kernel::new();
        kernel
            .register_harness(
                id("harness.duplicate"),
                FixedHarness(Ok(vec![activity("same"), activity("same")])),
            )
            .unwrap();
        let snapshot = kernel.refresh();
        assert!(snapshot.observations.is_empty());
        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(snapshot.failures[0].stage, ExtensionStage::Poll);
    }

    #[test]
    fn invalid_optional_native_identity_rejects_the_harness_batch() {
        let mut row = activity("session");
        row.identity.turn_id = Some("turn with spaces".into());
        let mut kernel = Kernel::new();
        kernel
            .register_harness(id("harness.invalid-identity"), FixedHarness(Ok(vec![row])))
            .unwrap();

        let snapshot = kernel.refresh();
        assert!(snapshot.observations.is_empty());
        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(snapshot.failures[0].stage, ExtensionStage::Poll);
    }

    #[test]
    fn capability_failure_is_visible_and_other_extension_results_survive() {
        struct BrokenCapability;
        impl Capability for BrokenCapability {
            fn evaluate(&self, _: &[Observation]) -> Result<Vec<MetricRow>, String> {
                Err("broken extension".into())
            }
        }

        let mut kernel = Kernel::new();
        kernel
            .register_harness(id("harness.pi"), FixedHarness(Ok(vec![activity("s1")])))
            .unwrap();
        kernel
            .register_capability(id("capability.broken"), BrokenCapability)
            .unwrap();
        kernel
            .register_capability(id("capability.healthy"), ActivityCount)
            .unwrap();
        let snapshot = kernel.refresh();
        assert!(snapshot.metrics.contains_key(&id("capability.healthy")));
        assert!(!snapshot.metrics.contains_key(&id("capability.broken")));
        assert_eq!(snapshot.failures[0].stage, ExtensionStage::Capability);
    }

    #[test]
    fn harnesses_are_polled_in_stable_extension_id_order() {
        use std::sync::{Arc, Mutex};

        struct RecordingHarness(Arc<Mutex<Vec<String>>>, &'static str);
        impl Harness for RecordingHarness {
            fn poll(&mut self) -> Result<Vec<RawObservation>, String> {
                self.0.lock().unwrap().push(self.1.into());
                Ok(vec![])
            }
        }

        let order = Arc::new(Mutex::new(Vec::new()));
        let mut kernel = Kernel::new();
        kernel
            .register_harness(
                id("harness.z-last"),
                RecordingHarness(order.clone(), "z-last"),
            )
            .unwrap();
        kernel
            .register_harness(
                id("harness.a-first"),
                RecordingHarness(order.clone(), "a-first"),
            )
            .unwrap();
        kernel.refresh();
        assert_eq!(*order.lock().unwrap(), ["a-first", "z-last"]);
    }

    #[test]
    fn panics_and_unbounded_extension_output_become_visible_failures() {
        struct PanickingHarness;
        impl Harness for PanickingHarness {
            fn poll(&mut self) -> Result<Vec<RawObservation>, String> {
                panic!("broken adapter")
            }
        }

        struct OversizedCapability;
        impl Capability for OversizedCapability {
            fn evaluate(&self, _: &[Observation]) -> Result<Vec<MetricRow>, String> {
                Ok(vec![MetricRow {
                    label: "x".repeat(MAX_LABEL_BYTES + 1),
                    value: "1".into(),
                }])
            }
        }

        let mut kernel = Kernel::new();
        kernel
            .register_harness(id("harness.panic"), PanickingHarness)
            .unwrap();
        kernel
            .register_capability(id("capability.oversized"), OversizedCapability)
            .unwrap();
        let snapshot = kernel.refresh();
        assert_eq!(snapshot.failures.len(), 2);
        assert_eq!(snapshot.failures[0].message, "extension panicked");
        assert!(snapshot.failures[1].message.contains("metric row text"));
    }
}

pub mod retained_history;
pub mod workflow_alerts;
pub mod workflow_notifications;

pub use agenturm_protocol::native_review;

pub use agenturm_protocol::telemetry_summary;

pub use agenturm_protocol::native_attention;
