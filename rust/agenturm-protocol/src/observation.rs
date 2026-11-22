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
            CreditUsage,
            Tool,
            McpCall,
            TraceSpan,
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
            FactKind::CreditUsage => {
                serde_json::from_str(payload).map(ObservationFact::CreditUsage)
            }
            FactKind::Tool => serde_json::from_str(payload).map(ObservationFact::Tool),
            FactKind::McpCall => serde_json::from_str(payload).map(ObservationFact::McpCall),
            FactKind::TraceSpan => serde_json::from_str(payload).map(ObservationFact::TraceSpan),
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
    CreditUsage(CreditUsage),
    Tool(Tool),
    McpCall(McpCall),
    TraceSpan(TraceSpan),
    Lifecycle(Lifecycle),
    Check(Check),
    PullRequestRaised(PullRequestRaised),
    Relationship(Relationship),
    Message(Message),
    Diagnostic(Diagnostic),
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

/// Native provider credit usage at a fixed precision of nine decimal places.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreditUsage {
    pub nanocredits: u64,
}

pub const NANOCREDITS_PER_CREDIT: u64 = 1_000_000_000;

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
    pub started_at_unix_ns: u64,
    pub status: TraceStatus,
}

impl TraceSpan {
    pub fn is_valid(&self, ended_at_unix_ns: u64) -> bool {
        fn id(value: &str, bytes: usize) -> bool {
            value.len() == bytes * 2
                && value
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
                && value.bytes().any(|byte| byte != b'0')
        }
        id(&self.trace_id, 16)
            && id(&self.span_id, 8)
            && self
                .parent_span_id
                .as_deref()
                .is_none_or(|parent| id(parent, 8) && parent != self.span_id)
            && (1..=ended_at_unix_ns).contains(&self.started_at_unix_ns)
            && ended_at_unix_ns <= i64::MAX as u64
    }
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
