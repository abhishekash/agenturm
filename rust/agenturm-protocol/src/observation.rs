//! Shared `agenturm-observations/1` wire types for configured source adapters.
//!
//! An adapter converts native records into these values before sending a poll
//! reply. The host validates the schema, identities, payloads and poll budget
//! before committing observations with the cursor. The source ID in the batch
//! must echo the one assigned by the host.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::fmt;
use std::str::FromStr;

/// Observation encoding selected by the host's plugin catalog.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ObservationFormat {
    #[default]
    V1,
}

impl ObservationFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V1 => "agenturm-observations/1",
        }
    }
}

impl fmt::Display for ObservationFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnsupportedObservationFormat;

impl fmt::Display for UnsupportedObservationFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("unsupported observation format")
    }
}

impl std::error::Error for UnsupportedObservationFormat {}

impl FromStr for ObservationFormat {
    type Err = UnsupportedObservationFormat;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == Self::V1.as_str() {
            Ok(Self::V1)
        } else {
            Err(UnsupportedObservationFormat)
        }
    }
}

/// A version 1 poll response.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationBatch {
    #[serde(rename = "type")]
    kind: BatchType,
    protocol: u64,
    pub source_id: String,
    pub cursor: String,
    pub records: Vec<ObservationRecord>,
}

impl ObservationBatch {
    pub fn new(
        source_id: impl Into<String>,
        cursor: impl Into<String>,
        records: Vec<ObservationRecord>,
    ) -> Self {
        Self {
            kind: BatchType::Batch,
            protocol: 1,
            source_id: source_id.into(),
            cursor: cursor.into(),
            records,
        }
    }

    pub fn protocol(&self) -> u64 {
        self.protocol
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum BatchType {
    Batch,
}

/// One native event. `time_ns` is a canonical decimal Unix nanosecond timestamp.
/// `identity` is required for all facts other than source-scoped diagnostics.
#[derive(Debug, Serialize)]
pub struct ObservationRecord {
    pub event_id: String,
    pub revision: u32,
    pub time_ns: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<SessionIdentity>,
    #[serde(flatten)]
    pub fact: ObservationFact,
}

// Deserialize the record and its payload directly from JSON so duplicate keys
// cannot disappear into a generic map before field validation.
impl<'de> Deserialize<'de> for ObservationRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Record {
            event_id: String,
            revision: u32,
            time_ns: String,
            identity: Option<SessionIdentity>,
            kind: FactKind,
            payload: Box<RawValue>,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum FactKind {
            Usage,
            SpanTokenUsage,
            CreditUsage,
            ReportedCost,
            Tool,
            FileEdit,
            McpCall,
            TraceSpan,
            InternalReview,
            NativeRequest,
            Lifecycle,
            Check,
            PullRequestRaised,
            Relationship,
            Message,
            Diagnostic,
        }

        let record = Record::deserialize(deserializer)?;
        let payload = record.payload.get();
        let fact = match record.kind {
            FactKind::Usage => serde_json::from_str(payload).map(ObservationFact::Usage),
            FactKind::SpanTokenUsage => {
                serde_json::from_str(payload).map(ObservationFact::SpanTokenUsage)
            }
            FactKind::CreditUsage => {
                serde_json::from_str(payload).map(ObservationFact::CreditUsage)
            }
            FactKind::ReportedCost => {
                serde_json::from_str(payload).map(ObservationFact::ReportedCost)
            }
            FactKind::Tool => serde_json::from_str(payload).map(ObservationFact::Tool),
            FactKind::FileEdit => serde_json::from_str(payload).map(ObservationFact::FileEdit),
            FactKind::McpCall => serde_json::from_str(payload).map(ObservationFact::McpCall),
            FactKind::TraceSpan => serde_json::from_str(payload).map(ObservationFact::TraceSpan),
            FactKind::InternalReview => serde_json::from_str(payload).map(ObservationFact::InternalReview),
            FactKind::NativeRequest => serde_json::from_str(payload).map(ObservationFact::NativeRequest),
            FactKind::Lifecycle => serde_json::from_str(payload).map(ObservationFact::Lifecycle),
            FactKind::Check => serde_json::from_str(payload).map(ObservationFact::Check),
            FactKind::PullRequestRaised => {
                serde_json::from_str(payload).map(ObservationFact::PullRequestRaised)
            }
            FactKind::Relationship => {
                serde_json::from_str(payload).map(ObservationFact::Relationship)
            }
            FactKind::Message => serde_json::from_str(payload).map(ObservationFact::Message),
            FactKind::Diagnostic => serde_json::from_str(payload).map(ObservationFact::Diagnostic),
        }
        .map_err(|_| serde::de::Error::custom("invalid observation payload"))?;
        Ok(Self {
            event_id: record.event_id,
            revision: record.revision,
            time_ns: record.time_ns,
            identity: record.identity,
            fact,
        })
    }
}

/// Native session and optional worker, turn and operation identifiers.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionIdentity {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

impl SessionIdentity {
    pub fn session(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            worker_id: None,
            turn_id: None,
            operation_id: None,
        }
    }
}

/// Source-neutral observation payload. The host validates each payload's values.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum ObservationFact {
    Usage(Usage),
    SpanTokenUsage(SpanTokenUsage),
    CreditUsage(CreditUsage),
    ReportedCost(ReportedCost),
    Tool(Tool),
    FileEdit(FileEdit),
    McpCall(McpCall),
    TraceSpan(TraceSpan),
    InternalReview(crate::native_review::InternalReview),
    NativeRequest(crate::native_review::NativeRequest),
    Lifecycle(Lifecycle),
    Check(Check),
    PullRequestRaised(PullRequestRaised),
    Relationship(Relationship),
    Message(Message),
    Diagnostic(Diagnostic),
}

/// Counts from one explicitly successful file edit. Native paths and diffs
/// stay in the source transcript and are outside this observation contract.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileEdit {
    pub added: u16,
    pub updated: u16,
    pub deleted: u16,
    pub moved: u16,
}

impl FileEdit {
    pub const MAX_CHANGES: usize = 256;

    pub fn is_valid(self) -> bool {
        let total = self.added as usize
            + self.updated as usize
            + self.deleted as usize
            + self.moved as usize;
        (1..=Self::MAX_CHANGES).contains(&total)
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}

/// Counters reported on an exact producer span. Breakdown fields are
/// recorded independently of the producer's input and output totals.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpanTokenUsage {
    pub span: TraceSpanIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}

impl SpanTokenUsage {
    pub fn is_valid(&self) -> bool {
        let counters = [
            self.input_tokens,
            self.output_tokens,
            self.cache_read_input_tokens,
            self.cache_write_input_tokens,
            self.reasoning_output_tokens,
            self.total_tokens,
        ];
        self.span.is_valid()
            && counters.iter().any(Option::is_some)
            && counters
                .into_iter()
                .flatten()
                .all(|count| count <= i64::MAX as u64)
    }

    pub fn matches_event(&self, event_id: &str) -> bool {
        self.span.matches_event("otlp-usage:", event_id)
    }
}

/// Native provider credit usage at a fixed precision of nine decimal places.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreditUsage {
    pub nanocredits: u64,
}

pub const NANOCREDITS_PER_CREDIT: u64 = 1_000_000_000;
pub const NANOUNITS_PER_CURRENCY_UNIT: u64 = 1_000_000_000;

/// A monetary amount supplied by a producer, with its original currency.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReportedCost {
    pub currency: String,
    pub nanounits: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<TraceSpanIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_origin: Option<NativeCostOrigin>,
}

/// Provenance of an amount taken from a retained native assistant record.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeCostOrigin {
    pub basis: NativeCostBasis,
    pub rounded: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeCostBasis {
    Estimate,
    ProviderReported,
    Unknown,
}

/// The producer's immutable OTLP trace and span IDs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceSpanIdentity {
    pub trace_id: String,
    pub span_id: String,
}

impl TraceSpanIdentity {
    pub fn is_valid(&self) -> bool {
        valid_otlp_id(&self.trace_id, 16) && valid_otlp_id(&self.span_id, 8)
    }

    pub fn matches_event(&self, prefix: &str, event_id: &str) -> bool {
        event_id
            .strip_prefix(prefix)
            .and_then(|ids| ids.split_once(':'))
            == Some((self.trace_id.as_str(), self.span_id.as_str()))
    }
}

impl ReportedCost {
    /// Native harnesses store JSON number amounts calculated or obtained by the
    /// producer. Round the recorded decimal to the nearest nano-unit.
    pub fn native_amount(
        currency: &str,
        basis: NativeCostBasis,
        value: &serde_json::value::RawValue,
    ) -> Option<Self> {
        let (nanounits, rounded) = parse_currency_amount(value.get(), true)?;
        let cost = Self {
            currency: currency.into(),
            nanounits,
            span: None,
            native_origin: Some(NativeCostOrigin { basis, rounded }),
        };
        cost.is_valid().then_some(cost)
    }

    pub fn is_valid(&self) -> bool {
        self.currency.len() == 3
            && self.currency.bytes().all(|byte| byte.is_ascii_uppercase())
            && self.nanounits <= i64::MAX as u64
            && self.span.as_ref().is_none_or(TraceSpanIdentity::is_valid)
            && self
                .native_origin
                .is_none_or(|origin| self.span.is_none() && (!origin.rounded || self.nanounits > 0))
    }

    pub fn matches_event(&self, event_id: &str) -> bool {
        self.span
            .as_ref()
            .is_none_or(|span| span.matches_event("otlp-cost:", event_id))
    }

    pub fn matches_evidence(&self, event_id: &str, operation_id: Option<&str>) -> bool {
        self.matches_event(event_id)
            && (self.native_origin.is_none()
                || operation_id
                    .is_some_and(|message| event_id.strip_suffix(":cost") == Some(message)))
    }
}

/// Parses an exactly representable nonnegative decimal currency amount.
pub fn parse_currency_nanounits(value: &str) -> Option<u64> {
    parse_currency_amount(value, false).map(|(nanounits, _)| nanounits)
}

fn parse_currency_amount(value: &str, allow_rounding: bool) -> Option<(u64, bool)> {
    if value.is_empty() || value.len() > 48 {
        return None;
    }
    let (mantissa, exponent) = match value.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
        None => (value, 0),
    };
    if !(-64..=64).contains(&exponent) {
        return None;
    }
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let digits = format!("{whole}{fraction}");
    let significant = digits.trim_end_matches('0').trim_start_matches('0');
    if significant.is_empty() {
        return Some((0, false));
    }
    let trailing = digits.len() - digits.trim_end_matches('0').len();
    let integer_scale = (NANOUNITS_PER_CURRENCY_UNIT.ilog10() as i32)
        .checked_add(exponent)?
        .checked_add(trailing as i32)?
        .checked_sub(fraction.len() as i32)?;
    if let Ok(exponent) = u32::try_from(integer_scale) {
        let scale = 10u64.checked_pow(exponent)?;
        let result = significant.parse::<u64>().ok()?.checked_mul(scale)?;
        return (result <= i64::MAX as u64).then_some((result, false));
    }
    if !allow_rounding {
        return None;
    }
    let integer_digits = significant.len().checked_sub((-integer_scale) as usize)?;
    let (integer, fraction) = significant.split_at(integer_digits);
    let whole = if integer.is_empty() {
        0
    } else {
        integer.parse::<u64>().ok()?
    };
    if whole >= i64::MAX as u64 {
        return None;
    }
    let rounded = whole + u64::from(fraction.as_bytes()[0] >= b'5');
    (rounded > 0).then_some((rounded, true))
}

#[cfg(test)]
mod reported_cost_tests {
    use super::*;

    #[test]
    fn native_amount_distinguishes_explicit_zero_from_missing_precision() {
        for (literal, expected) in [
            ("0.0", Some((0, false))),
            ("0.00123", Some((1_230_000, false))),
            ("0.001230000000000001", Some((1_230_000, true))),
            ("0.0000000005", Some((1, true))),
            ("0.00000000049999999999999999", None),
            ("0.0000000001", None),
            ("9223372036.8547758069", Some((i64::MAX as u64, true))),
            ("9223372036.8547758071", None),
            ("1e-400", None),
            ("1e400", None),
            ("-0.00123", None),
            ("null", None),
            (r#""0""#, None),
            (r#"{"amount":0}"#, None),
        ] {
            let raw = serde_json::from_str(literal).unwrap();
            let output = ReportedCost::native_amount("USD", NativeCostBasis::Estimate, raw)
                .map(|cost| (cost.nanounits, cost.native_origin.unwrap().rounded));
            assert_eq!(output, expected, "{literal}");
        }
    }

    #[test]
    fn existing_span_cost_keeps_its_exact_serialized_shape() {
        let previous = concat!(
            r#"{"currency":"USD","nanounits":0,"span":{"trace_id":"11111111111111111111111111111111","#,
            r#""span_id":"2222222222222222"}}"#,
        );
        let cost: ReportedCost = serde_json::from_str(previous).unwrap();
        assert!(cost.native_origin.is_none());
        assert_eq!(serde_json::to_string(&cost).unwrap(), previous);
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    /// `shell`, `file`, `mcp`, or `other`.
    pub category: String,
    /// `started`, `succeeded`, `failed`, or `denied`.
    pub result: String,
}

/// Privacy-bounded details of a native MCP call, linked by the record's
/// session and operation identity.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub method: McpMethod,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ns: Option<u64>,
    pub result: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum McpMethod {
    #[serde(rename = "tools/call")]
    ToolsCall,
}

impl McpMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ToolsCall => "tools/call",
        }
    }
}

/// MCP labels are identifiers, so paths, arguments, and free text cannot
/// cross the metadata boundary through these fields.
pub fn is_mcp_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// Splits the exact native qualified tool name, rejecting ambiguous separators
/// and labels that cannot cross the metadata boundary.
pub fn mcp_tool_labels(name: &str) -> Option<(&str, &str)> {
    let (server, tool) = name.strip_prefix("mcp__")?.split_once("__")?;
    (!tool.contains("__") && is_mcp_identifier(server) && is_mcp_identifier(tool))
        .then_some((server, tool))
}

pub fn is_valid_mcp_metadata(
    server: Option<&str>,
    tool: Option<&str>,
    duration_ns: Option<u64>,
    result: &str,
) -> bool {
    (server.is_some() || tool.is_some() || duration_ns.is_some())
        && server.is_none_or(is_mcp_identifier)
        && tool.is_none_or(is_mcp_identifier)
        && duration_ns.is_none_or(|duration| duration <= i64::MAX as u64)
        && matches!(result, "started" | "succeeded" | "failed" | "denied")
}

/// Structural OTLP trace evidence. The observation time is the native span
/// end time. Span names, attributes, events, and links are outside this DTO.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceSpan {
    pub trace_id: String,
    pub span_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    /// Explicit harness source for the observation's native session identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_source_id: Option<String>,
    /// Explicit native operation anchor; never inferred from a span name or time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_operation_id: Option<String>,
    pub started_at_unix_ns: u64,
    pub status: TraceStatus,
}

impl TraceSpan {
    pub fn is_valid(&self, ended_at_unix_ns: u64) -> bool {
        valid_otlp_id(&self.trace_id, 16)
            && valid_otlp_id(&self.span_id, 8)
            && self
                .parent_span_id
                .as_deref()
                .is_none_or(|parent| valid_otlp_id(parent, 8) && parent != self.span_id)
            && self.target_operation_id.as_deref().is_none_or(|operation| {
                self.target_source_id.is_some()
                    && !operation.is_empty()
                    && operation.len() <= 200
                    && operation
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
            })
            && (1..=ended_at_unix_ns).contains(&self.started_at_unix_ns)
            && ended_at_unix_ns <= i64::MAX as u64
    }
}

/// Progress reported by a producer independently of the trace's span exports.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceProgress {
    pub trace_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_source_id: Option<String>,
    pub stage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent_basis_points: Option<u16>,
}

impl TraceProgress {
    pub const EVENT_PREFIX: &str = "otlp-progress:";

    pub fn is_valid(&self, event_id: &str, observed_at_unix_ns: u64) -> bool {
        event_id
            .strip_prefix(Self::EVENT_PREFIX)
            .is_some_and(|id| valid_otlp_id(id, 16))
            && valid_otlp_id(&self.trace_id, 16)
            && (1..=i64::MAX as u64).contains(&observed_at_unix_ns)
            && self
                .percent_basis_points
                .is_none_or(|percent| percent <= 10_000)
            && valid_progress_stage(&self.stage)
    }
}

fn valid_progress_stage(stage: &str) -> bool {
    const CREDENTIAL_PREFIXES: &[&str] = &[
        "sk-",
        "sk_live_",
        "sk_test_",
        "rk_live_",
        "rk_test_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "akia",
        "aiza",
        "xoxb-",
        "xoxa-",
        "xoxp-",
        "xoxr-",
        "xoxs-",
        "bearer-",
        "bearer_",
        "authorization-",
        "authorization_",
        "password-",
        "password_",
        "api_key-",
        "api_key_",
        "secret-",
        "secret_",
    ];
    if stage.is_empty()
        || stage.len() > 64
        || !stage.as_bytes()[0].is_ascii_alphabetic()
        || !stage
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        || (stage.len() >= 32 && stage.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return false;
    }
    let lower = stage.to_ascii_lowercase();
    std::iter::once(lower.as_str())
        .chain(
            lower
                .match_indices(['.', '_', '-'])
                .map(|(index, _)| &lower[index + 1..]),
        )
        .all(|segment| {
            !CREDENTIAL_PREFIXES
                .iter()
                .any(|prefix| segment.starts_with(prefix))
        })
}

fn valid_otlp_id(value: &str, bytes: usize) -> bool {
    value.len() == bytes * 2
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        && value.bytes().any(|byte| byte != b'0')
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceStatus {
    Unset,
    Ok,
    Error,
}

impl TraceStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unset => "unset",
            Self::Ok => "ok",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lifecycle {
    /// `running`, `completed`, `interrupted`, `failed`, or `paused`.
    pub state: String,
    /// Optional bounded code. Omit the raw message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    /// `test`, `lint`, or `build`.
    pub category: String,
    /// `passed`, `failed`, or `unknown`.
    pub result: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PullRequestRaised {
    /// GitHub owner and repository, separated by one slash.
    pub repository: String,
    /// GitHub pull request number.
    pub number: u64,
}

/// An explicit parent relationship for the session named in the record's identity.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Relationship {
    pub relationship_kind: RelationshipKind,
    pub parent_session_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    Delegated,
    Forked,
}

impl RelationshipKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Delegated => "delegated",
            Self::Forked => "forked",
        }
    }
}

/// Validates the bounded GitHub reference retained as work-result evidence.
pub fn is_github_pull_request(repository: &str, number: u64) -> bool {
    let Some((owner, name)) = repository.split_once('/') else {
        return false;
    };
    (1..=i64::MAX as u64).contains(&number)
        && (1..=39).contains(&owner.len())
        && owner.as_bytes()[0].is_ascii_alphanumeric()
        && owner.as_bytes()[owner.len() - 1].is_ascii_alphanumeric()
        && owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && (1..=100).contains(&name.len())
        && !matches!(name, "." | "..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    /// `user` or `assistant`.
    pub role: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    /// Bounded code. The host retains only codes from its closed catalog.
    pub code: String,
    /// `info`, `warning`, or `error`.
    pub severity: String,
}
