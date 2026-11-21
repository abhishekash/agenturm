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
