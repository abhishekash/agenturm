//! Data-only v2 contract for host-prepared, content-free plugin facts.
//!
//! This module does not read sockets or write SQLite. Digests in a request are
//! untrusted assertions: only a future independent service writer can check
//! source state, policy, evidence provenance, and durable replay receipts.

use crate::ContractError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub const VERSION: u16 = 2;
pub const MAX_FRAME_BYTES: usize = crate::MAX_FRAME_BYTES;
pub const MAX_RECORDS: usize = 48;
/// Matches the bounded shared Rust JSONL cursor and Python source host.
pub const MAX_CURSOR_CHARS: usize = 800;

type Result<T> = std::result::Result<T, ContractError>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u16,
    pub request_id: String,
    pub receiver_id: String,
    pub operation: Operation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    ActivateSource {
        plugin_id: String,
        source_id: String,
        generation: String,
    },
    AdmitLifecycle {
        lease: SourceLease,
        expected_cursor: String,
        cursor: String,
        batch_digest: String,
        records: Vec<PreparedLifecycle>,
    },
    AdmitFacts {
        lease: SourceLease,
        expected_cursor: String,
        cursor: String,
        batch_digest: String,
        records: Vec<PreparedFact>,
    },
    /// Atomically admit lifecycle evidence and supported typed facts in one cursor advance.
    AdmitBatch {
        lease: SourceLease,
        expected_cursor: String,
        cursor: String,
        batch_digest: String,
        records: Vec<PreparedBatchRecord>,
    },
    RevokeSource {
        lease: SourceLease,
        cursor: String,
        expected_dirty_seq: i64,
    },
    SetSourceCapture {
        lease: SourceLease,
        enabled: bool,
        expected_cursor: String,
        expected_dirty_seq: i64,
        expected_capture_enabled: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLease {
    pub receiver_id: String,
    pub plugin_id: String,
    pub source_id: String,
    pub generation: String,
    pub epoch: i64,
    pub policy_epoch: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedLifecycle {
    pub event_id_hash: String,
    pub revision: i64,
    pub kind: LifecycleKind,
    pub time_ns: i64,
    /// Canonical, metadata-only JSON from host validation; no raw event body.
    pub payload_json: String,
    pub record_digest: String,
    pub native_session_id: String,
    pub native_worker_id: String,
    pub native_turn_id: String,
    pub native_operation_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedFact {
    pub event_id_hash: String,
    pub revision: i64,
    pub kind: FactKind,
    pub time_ns: i64,
    /// Canonical, content-free JSON validated again by the storage service.
    pub payload_json: String,
    pub record_digest: String,
    pub native_session_id: String,
    pub native_worker_id: String,
    pub native_turn_id: String,
    pub native_operation_id: String,
}

/// A record in an atomic mixed lifecycle/fact admission batch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedBatchRecord {
    pub event_id_hash: String,
    pub revision: i64,
    pub kind: BatchRecordKind,
    pub time_ns: i64,
    /// Canonical, content-free JSON validated again by the storage service.
    pub payload_json: String,
    pub record_digest: String,
    pub native_session_id: String,
    pub native_worker_id: String,
    pub native_turn_id: String,
    pub native_operation_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchRecordKind {
    Lifecycle,
    Message,
    Tool,
    Check,
    Usage,
}

impl BatchRecordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lifecycle => "lifecycle",
            Self::Message => "message",
            Self::Tool => "tool",
            Self::Check => "check",
            Self::Usage => "usage",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactKind {
    Message,
    Tool,
    Check,
    Usage,
}

impl FactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Tool => "tool",
            Self::Check => "check",
            Self::Usage => "usage",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleKind {
    Lifecycle,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LifecyclePayload {
    state: LifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_code: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LifecycleState {
    Started,
    Running,
    Paused,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub version: u16,
    pub request_id: String,
    pub receiver_id: String,
    /// SHA-256 of canonical request JSON, binding an ack to the entire batch.
    pub request_digest: String,
    pub result: OperationResult,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationResult {
    SourceActivated(SourceActivatedResult),
    LifecycleAdmission(AdmissionResult),
    FactAdmission(AdmissionResult),
    BatchAdmission(AdmissionResult),
    SourceRevoked(SourceRevokedResult),
    SourceCaptureSet(SourceCaptureSetResult),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceActivatedResult {
    pub disposition: Disposition,
    pub lease: SourceLease,
    pub cursor: String,
    pub dirty_seq: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionResult {
    pub disposition: Disposition,
    pub cursor: String,
    pub batch_digest: String,
    pub dirty_seq: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRevokedResult {
    pub disposition: Disposition,
    pub cursor: String,
    pub dirty_seq: i64,
    pub lease_epoch: i64,
    pub policy_epoch: i64,
    pub capture_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCaptureSetResult {
    pub disposition: Disposition,
    pub lease: SourceLease,
    pub enabled: bool,
    pub cursor: String,
    pub dirty_seq: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Committed,
    Replay,
    Idle,
}

fn identity(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn digest(value: &str, bytes: usize) -> bool {
    value.len() == bytes * 2
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn cursor(value: &str) -> bool {
    value.len() <= MAX_CURSOR_CHARS && !value.chars().any(char::is_control)
}

fn reason_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.bytes().skip(1).all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'.' | b'/' | b'-')
        })
}

fn payload(raw: &str) -> bool {
    if raw.len() > 4096 {
        return false;
    }
    // Comparing exact canonical bytes also rejects duplicate keys and
    // alternate encodings that serde_json::Value would otherwise normalize.
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.len() > 2
        || object
            .keys()
            .any(|key| key != "state" && key != "reason_code")
        || object
            .get("reason_code")
            .is_some_and(|reason| !reason.as_str().is_some_and(reason_code))
    {
        return false;
    }
    let Ok(typed) = serde_json::from_value::<LifecyclePayload>(value.clone()) else {
        return false;
    };
    sorted_json(&value).is_ok_and(|canonical| canonical == raw.as_bytes())
        && serde_json::to_value(typed).is_ok_and(|roundtrip| roundtrip == value)
}

pub fn fact_payload_is_valid(kind: FactKind, raw: &str) -> bool {
    if raw.len() > 4096 {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    let valid = match kind {
        FactKind::Message => {
            object.len() == 1
                && object
                    .get("role")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|role| matches!(role, "user" | "assistant"))
        }
        FactKind::Tool | FactKind::Check => {
            let category = object.get("category").and_then(serde_json::Value::as_str);
            let result = object.get("result").and_then(serde_json::Value::as_str);
            object.len() == 2
                && match kind {
                    FactKind::Tool => {
                        category.is_some_and(|value| {
                            matches!(value, "shell" | "file" | "mcp" | "other")
                        }) && result.is_some_and(|value| {
                            matches!(value, "started" | "succeeded" | "failed" | "denied")
                        })
                    }
                    FactKind::Check => {
                        category.is_some_and(|value| {
                            matches!(value, "test" | "lint" | "build" | "review" | "other")
                        }) && result.is_some_and(|value| {
                            matches!(value, "passed" | "failed" | "skipped" | "unknown")
                        })
                    }
                    _ => false,
                }
        }
        FactKind::Usage => {
            !object.is_empty()
                && object.len() <= 5
                && object.iter().all(|(key, value)| {
                    matches!(
                        key.as_str(),
                        "input_tokens"
                            | "output_tokens"
                            | "cache_read_tokens"
                            | "cache_write_tokens"
                            | "total_tokens"
                    ) && value.as_i64().is_some_and(|number| number >= 0)
                })
        }
    };
    valid && sorted_json(&value).is_ok_and(|canonical| canonical == raw.as_bytes())
}

fn sorted_json(value: &serde_json::Value) -> Result<Vec<u8>> {
    fn append(value: &serde_json::Value, out: &mut Vec<u8>) -> Result<()> {
        match value {
            serde_json::Value::Object(map) => {
                out.push(b'{');
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort_unstable();
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        out.push(b',');
                    }
                    out.extend_from_slice(
                        &serde_json::to_vec(key)
                            .map_err(|_| ContractError::Invalid("v2 serialization failed"))?,
                    );
                    out.push(b':');
                    append(&map[key], out)?;
                }
                out.push(b'}');
            }
            serde_json::Value::Array(items) => {
                out.push(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        out.push(b',');
                    }
                    append(item, out)?;
                }
                out.push(b']');
            }
            scalar => out.extend_from_slice(
                &serde_json::to_vec(scalar)
                    .map_err(|_| ContractError::Invalid("v2 serialization failed"))?,
            ),
        }
        Ok(())
    }

    let mut out = Vec::new();
    append(value, &mut out)?;
    Ok(out)
}

fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let value = serde_json::to_value(value)
        .map_err(|_| ContractError::Invalid("v2 serialization failed"))?;
    let bytes = sorted_json(&value)?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(ContractError::Invalid("v2 frame length exceeds contract"));
    }
    Ok(bytes)
}

impl Request {
    pub fn from_json(raw: &[u8]) -> Result<Self> {
        if raw.is_empty() || raw.len() > MAX_FRAME_BYTES {
            return Err(ContractError::Invalid("v2 frame length exceeds contract"));
        }
        let request: Self = serde_json::from_slice(raw)
            .map_err(|_| ContractError::Invalid("malformed or unsupported v2 request"))?;
        request.validate()?;
        Ok(request)
    }

    pub fn to_json(&self) -> Result<Vec<u8>> {
        self.validate()?;
        canonical(self)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != VERSION
            || !digest(&self.request_id, 16)
            || !identity(&self.receiver_id, 128)
        {
            return Err(ContractError::Invalid("invalid v2 request identity"));
        }
        match &self.operation {
            Operation::ActivateSource {
                plugin_id,
                source_id,
                generation,
            } => {
                if !identity(plugin_id, 256) || !identity(source_id, 256) || !digest(generation, 32)
                {
                    return Err(ContractError::Invalid("invalid v2 source activation"));
                }
            }
            Operation::AdmitLifecycle {
                lease,
                expected_cursor,
                cursor: next_cursor,
                batch_digest,
                records,
            } => {
                if lease.receiver_id != self.receiver_id
                    || !identity(&lease.plugin_id, 256)
                    || !identity(&lease.source_id, 256)
                    || !digest(&lease.generation, 32)
                    || lease.epoch <= 0
                    || lease.policy_epoch <= 0
                    || !cursor(expected_cursor)
                    || !cursor(next_cursor)
                    || !digest(batch_digest, 32)
                    || records.len() > MAX_RECORDS
                    || (next_cursor.is_empty() && !records.is_empty())
                {
                    return Err(ContractError::Invalid("invalid v2 source lease or batch"));
                }
                let mut seen = HashSet::new();
                for row in records {
                    if !digest(&row.event_id_hash, 32)
                        || !seen.insert(row.event_id_hash.as_str())
                        || row.revision != 0
                        || row.time_ns < 0
                        || !digest(&row.record_digest, 32)
                        || !payload(&row.payload_json)
                        || ![
                            &row.native_session_id,
                            &row.native_worker_id,
                            &row.native_turn_id,
                            &row.native_operation_id,
                        ]
                        .into_iter()
                        .all(|field| field.is_empty() || identity(field, 256))
                    {
                        return Err(ContractError::Invalid(
                            "unsupported v2 prepared lifecycle evidence",
                        ));
                    }
                }
            }
            Operation::AdmitFacts {
                lease,
                expected_cursor,
                cursor: next_cursor,
                batch_digest,
                records,
            } => {
                if lease.receiver_id != self.receiver_id
                    || !identity(&lease.plugin_id, 256)
                    || !identity(&lease.source_id, 256)
                    || !digest(&lease.generation, 32)
                    || lease.epoch <= 0
                    || lease.policy_epoch <= 0
                    || !cursor(expected_cursor)
                    || !cursor(next_cursor)
                    || !digest(batch_digest, 32)
                    || records.is_empty()
                    || records.len() > MAX_RECORDS
                    || next_cursor.is_empty()
                {
                    return Err(ContractError::Invalid(
                        "invalid v2 source lease or fact batch",
                    ));
                }
                let mut seen = HashSet::new();
                for row in records {
                    let identity_fields = [
                        &row.native_session_id,
                        &row.native_worker_id,
                        &row.native_turn_id,
                        &row.native_operation_id,
                    ];
                    let identity_valid = identity_fields
                        .into_iter()
                        .all(|field| field.is_empty() || identity(field, 256));
                    let has_identity = identity_fields.iter().any(|field| !field.is_empty());
                    if !digest(&row.event_id_hash, 32)
                        || !seen.insert(row.event_id_hash.as_str())
                        || row.revision != 0
                        || row.time_ns < 0
                        || !digest(&row.record_digest, 32)
                        || row.payload_json.len() > 4096
                        || !identity_valid
                        || !fact_payload_is_valid(row.kind, &row.payload_json)
                        || (row.kind != FactKind::Usage && row.native_session_id.is_empty())
                        || (row.kind == FactKind::Usage
                            && has_identity
                            && row.native_session_id.is_empty())
                        || (row.kind != FactKind::Tool && !row.native_operation_id.is_empty())
                    {
                        return Err(ContractError::Invalid(
                            "unsupported v2 prepared plugin fact",
                        ));
                    }
                }
            }
            Operation::AdmitBatch {
                lease,
                expected_cursor,
                cursor: next_cursor,
                batch_digest,
                records,
            } => {
                if lease.receiver_id != self.receiver_id
                    || !identity(&lease.plugin_id, 256)
                    || !identity(&lease.source_id, 256)
                    || !digest(&lease.generation, 32)
                    || lease.epoch <= 0
                    || lease.policy_epoch <= 0
                    || !cursor(expected_cursor)
                    || !cursor(next_cursor)
                    || !digest(batch_digest, 32)
                    || records.len() > MAX_RECORDS
                    || (next_cursor.is_empty() && !records.is_empty())
                {
                    return Err(ContractError::Invalid(
                        "invalid v2 source lease or mixed admission batch",
                    ));
                }
                let mut seen = HashSet::new();
                for row in records {
                    let identity_fields = [
                        &row.native_session_id,
                        &row.native_worker_id,
                        &row.native_turn_id,
                        &row.native_operation_id,
                    ];
                    let identity_valid = identity_fields
                        .iter()
                        .all(|field| field.is_empty() || identity(field, 256));
                    let has_identity = identity_fields.iter().any(|field| !field.is_empty());
                    let payload_valid = match row.kind {
                        BatchRecordKind::Lifecycle => payload(&row.payload_json),
                        BatchRecordKind::Message => {
                            fact_payload_is_valid(FactKind::Message, &row.payload_json)
                        }
                        BatchRecordKind::Tool => {
                            fact_payload_is_valid(FactKind::Tool, &row.payload_json)
                        }
                        BatchRecordKind::Check => {
                            fact_payload_is_valid(FactKind::Check, &row.payload_json)
                        }
                        BatchRecordKind::Usage => {
                            fact_payload_is_valid(FactKind::Usage, &row.payload_json)
                        }
                    };
                    let fact_identity_valid = match row.kind {
                        BatchRecordKind::Lifecycle => true,
                        BatchRecordKind::Message
                        | BatchRecordKind::Tool
                        | BatchRecordKind::Check => !row.native_session_id.is_empty(),
                        BatchRecordKind::Usage => {
                            !has_identity || !row.native_session_id.is_empty()
                        }
                    } && (row.kind == BatchRecordKind::Tool
                        || row.native_operation_id.is_empty());
                    if !digest(&row.event_id_hash, 32)
                        || !seen.insert(row.event_id_hash.as_str())
                        || row.revision != 0
                        || row.time_ns < 0
                        || !digest(&row.record_digest, 32)
                        || row.payload_json.len() > 4096
                        || !identity_valid
                        || !payload_valid
                        || !fact_identity_valid
                    {
                        return Err(ContractError::Invalid(
                            "unsupported v2 prepared mixed admission evidence",
                        ));
                    }
                }
            }
            Operation::RevokeSource {
                lease,
                cursor: current_cursor,
                expected_dirty_seq,
            } => {
                if lease.receiver_id != self.receiver_id
                    || !identity(&lease.plugin_id, 256)
                    || !identity(&lease.source_id, 256)
                    || !digest(&lease.generation, 32)
                    || lease.epoch <= 0
                    || lease.policy_epoch <= 0
                    || !cursor(current_cursor)
                    || *expected_dirty_seq < 0
                    || lease.epoch.checked_add(1).is_none()
                    || lease.policy_epoch.checked_add(1).is_none()
                    || expected_dirty_seq.checked_add(1).is_none()
                {
                    return Err(ContractError::Invalid("invalid v2 capture revocation"));
                }
            }
            Operation::SetSourceCapture {
                lease,
                enabled,
                expected_cursor,
                expected_dirty_seq,
                expected_capture_enabled,
            } => {
                if lease.receiver_id != self.receiver_id
                    || !identity(&lease.plugin_id, 256)
                    || !identity(&lease.source_id, 256)
                    || !digest(&lease.generation, 32)
                    || lease.epoch <= 0
                    || lease.policy_epoch <= 0
                    || !cursor(expected_cursor)
                    || *expected_dirty_seq < 0
                    || enabled != expected_capture_enabled
                        && (lease.policy_epoch.checked_add(1).is_none()
                            || expected_dirty_seq.checked_add(1).is_none())
                {
                    return Err(ContractError::Invalid("invalid v2 source capture update"));
                }
            }
        }
        canonical(self)?;
        Ok(())
    }

    /// Deterministic comparison material, not a durable replay receipt.
    pub fn replay_identity(&self) -> Result<ReplayIdentity> {
        self.validate()?;
        let bytes = canonical(self)?;
        Ok(ReplayIdentity {
            request_id: self.request_id.clone(),
            receiver_id: self.receiver_id.clone(),
            digest: format!("{:x}", Sha256::digest(bytes)),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayIdentity {
    pub request_id: String,
    pub receiver_id: String,
    pub digest: String,
}

impl ReplayIdentity {
    pub fn compare(&self, next: &Self) -> crate::ReplayRelation {
        if self.request_id != next.request_id {
            crate::ReplayRelation::DifferentRequest
        } else if self.receiver_id == next.receiver_id && self.digest == next.digest {
            crate::ReplayRelation::Exact
        } else {
            crate::ReplayRelation::Conflict
        }
    }
}

impl Reply {
    pub fn from_json(raw: &[u8], request: &Request) -> Result<Self> {
        if raw.is_empty() || raw.len() > MAX_FRAME_BYTES {
            return Err(ContractError::Invalid("v2 frame length exceeds contract"));
        }
        let reply: Self = serde_json::from_slice(raw)
            .map_err(|_| ContractError::Invalid("malformed or unsupported v2 reply"))?;
        reply.validate_for(request)?;
        Ok(reply)
    }

    pub fn to_json(&self, request: &Request) -> Result<Vec<u8>> {
        self.validate_for(request)?;
        canonical(self)
    }

    pub fn validate_for(&self, request: &Request) -> Result<()> {
        let replay = request.replay_identity()?;
        if self.version != VERSION
            || self.request_id != request.request_id
            || self.receiver_id != request.receiver_id
            || self.request_digest != replay.digest
            || match (&request.operation, &self.result) {
                (
                    Operation::ActivateSource {
                        plugin_id,
                        source_id,
                        generation,
                    },
                    OperationResult::SourceActivated(result),
                ) => {
                    result.lease.receiver_id != request.receiver_id
                        || result.lease.plugin_id != *plugin_id
                        || result.lease.source_id != *source_id
                        || result.lease.generation != *generation
                        || result.lease.epoch <= 0
                        || result.lease.policy_epoch <= 0
                        || !cursor(&result.cursor)
                        || result.dirty_seq < 0
                        || result.disposition == Disposition::Idle
                }
                (
                    Operation::AdmitLifecycle {
                        cursor,
                        batch_digest,
                        expected_cursor,
                        records,
                        ..
                    },
                    OperationResult::LifecycleAdmission(result),
                ) => {
                    result.cursor != *cursor
                        || result.batch_digest != *batch_digest
                        || result.dirty_seq < 0
                        || result.disposition == Disposition::Idle
                            && (!records.is_empty() || cursor != expected_cursor)
                }
                (
                    Operation::AdmitFacts {
                        cursor,
                        batch_digest,
                        ..
                    },
                    OperationResult::FactAdmission(result),
                ) => {
                    result.cursor != *cursor
                        || result.batch_digest != *batch_digest
                        || result.dirty_seq < 0
                        || result.disposition == Disposition::Idle
                }
                (
                    Operation::AdmitBatch {
                        cursor,
                        batch_digest,
                        expected_cursor,
                        records,
                        ..
                    },
                    OperationResult::BatchAdmission(result),
                ) => {
                    result.cursor != *cursor
                        || result.batch_digest != *batch_digest
                        || result.dirty_seq < 0
                        || result.disposition == Disposition::Idle
                            && (!records.is_empty() || cursor != expected_cursor)
                }
                (
                    Operation::RevokeSource {
                        cursor,
                        lease,
                        expected_dirty_seq,
                        ..
                    },
                    OperationResult::SourceRevoked(result),
                ) => {
                    result.cursor != *cursor
                        || result.dirty_seq != expected_dirty_seq.checked_add(1).unwrap_or_default()
                        || result.lease_epoch != lease.epoch.checked_add(1).unwrap_or_default()
                        || result.policy_epoch
                            != lease.policy_epoch.checked_add(1).unwrap_or_default()
                        || result.capture_enabled
                        || result.dirty_seq < 0
                        || result.lease_epoch <= 0
                        || result.policy_epoch <= 0
                        || result.disposition == Disposition::Idle
                }
                (
                    Operation::SetSourceCapture {
                        lease,
                        enabled,
                        expected_cursor,
                        expected_dirty_seq,
                        expected_capture_enabled,
                    },
                    OperationResult::SourceCaptureSet(result),
                ) => {
                    let identity_mismatch = result.lease.receiver_id != request.receiver_id
                        || result.lease.plugin_id != lease.plugin_id
                        || result.lease.source_id != lease.source_id
                        || result.lease.generation != lease.generation
                        || result.lease.epoch <= 0
                        || result.lease.policy_epoch <= 0
                        || result.enabled != *enabled
                        || result.cursor != *expected_cursor
                        || result.dirty_seq < 0;
                    if identity_mismatch {
                        true
                    } else if enabled == expected_capture_enabled {
                        (result.disposition != Disposition::Idle
                            && result.disposition != Disposition::Replay)
                            || result.lease != *lease
                            || result.dirty_seq != *expected_dirty_seq
                    } else {
                        result.disposition != Disposition::Committed
                            && result.disposition != Disposition::Replay
                            || result.lease.epoch == lease.epoch
                            || result.lease.policy_epoch
                                != lease.policy_epoch.checked_add(1).unwrap_or_default()
                            || result.dirty_seq
                                != expected_dirty_seq.checked_add(1).unwrap_or_default()
                    }
                }
                _ => true,
            }
        {
            return Err(ContractError::Invalid("v2 reply does not match request"));
        }
        canonical(self)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn fixture() -> (Request, String) {
        let value: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/rust_storage_mutation_v2.json"
        ))
        .unwrap();
        let request = Request::from_json(&serde_json::to_vec(&value["request"]).unwrap()).unwrap();
        let digest = value["request_digest"].as_str().unwrap().to_owned();
        (request, digest)
    }

    fn facts_fixture() -> Request {
        let lease = SourceLease {
            receiver_id: "receiver-one".into(),
            plugin_id: "fixture.plugin".into(),
            source_id: "source-one".into(),
            generation: "a".repeat(64),
            epoch: 7,
            policy_epoch: 1,
        };
        let records = [
            (
                FactKind::Message,
                "{\"role\":\"user\"}",
                "session-one",
                "",
                "",
            ),
            (
                FactKind::Tool,
                "{\"category\":\"mcp\",\"result\":\"succeeded\"}",
                "session-one",
                "turn-one",
                "operation-one",
            ),
            (
                FactKind::Check,
                "{\"category\":\"test\",\"result\":\"passed\"}",
                "session-one",
                "",
                "",
            ),
            (
                FactKind::Usage,
                "{\"input_tokens\":12,\"total_tokens\":12}",
                "",
                "",
                "",
            ),
        ]
        .into_iter()
        .enumerate()
        .map(
            |(index, (kind, payload_json, session, turn, operation))| PreparedFact {
                event_id_hash: format!("{:x}", index + 1).repeat(64),
                revision: 0,
                kind,
                time_ns: 100 + index as i64,
                payload_json: payload_json.to_owned(),
                record_digest: format!("{:x}", index + 5).repeat(64),
                native_session_id: session.to_owned(),
                native_worker_id: String::new(),
                native_turn_id: turn.to_owned(),
                native_operation_id: operation.to_owned(),
            },
        )
        .collect();
        Request {
            version: VERSION,
            request_id: "b".repeat(32),
            receiver_id: lease.receiver_id.clone(),
            operation: Operation::AdmitFacts {
                lease,
                expected_cursor: String::new(),
                cursor: "facts-one".into(),
                batch_digest: "c".repeat(64),
                records,
            },
        }
    }

    fn mixed_fixture() -> Request {
        let (lifecycle, _) = fixture();
        let Operation::AdmitLifecycle { lease, records, .. } = lifecycle.operation else {
            panic!("fixture is lifecycle admission")
        };
        let lifecycle_record = &records[0];
        let fact = PreparedBatchRecord {
            event_id_hash: "9".repeat(64),
            revision: 0,
            kind: BatchRecordKind::Message,
            time_ns: 101,
            payload_json: "{\"role\":\"assistant\"}".to_owned(),
            record_digest: "8".repeat(64),
            native_session_id: "native-one".to_owned(),
            native_worker_id: String::new(),
            native_turn_id: String::new(),
            native_operation_id: String::new(),
        };
        let lifecycle_record = PreparedBatchRecord {
            event_id_hash: lifecycle_record.event_id_hash.clone(),
            revision: lifecycle_record.revision,
            kind: BatchRecordKind::Lifecycle,
            time_ns: lifecycle_record.time_ns,
            payload_json: lifecycle_record.payload_json.clone(),
            record_digest: lifecycle_record.record_digest.clone(),
            native_session_id: lifecycle_record.native_session_id.clone(),
            native_worker_id: lifecycle_record.native_worker_id.clone(),
            native_turn_id: lifecycle_record.native_turn_id.clone(),
            native_operation_id: lifecycle_record.native_operation_id.clone(),
        };
        Request {
            version: VERSION,
            request_id: "e".repeat(32),
            receiver_id: lease.receiver_id.clone(),
            operation: Operation::AdmitBatch {
                lease,
                expected_cursor: String::new(),
                cursor: "mixed-one".to_owned(),
                batch_digest: "f".repeat(64),
                records: vec![lifecycle_record, fact],
            },
        }
    }

    #[test]
    fn python_prepared_fixture_has_same_canonical_replay_digest() {
        let (request, expected) = fixture();
        assert_eq!(request.replay_identity().unwrap().digest, expected);
        assert_eq!(
            Request::from_json(&request.to_json().unwrap()).unwrap(),
            request
        );
    }

    #[test]
    fn mixed_lifecycle_and_fact_batch_round_trips_and_rejects_invalid_rows() {
        let request = mixed_fixture();
        let bytes = request.to_json().unwrap();
        assert_eq!(Request::from_json(&bytes).unwrap(), request);
        let Operation::AdmitBatch { records, .. } = &request.operation else {
            panic!("fixture is mixed admission")
        };
        assert_eq!(records[0].kind, BatchRecordKind::Lifecycle);
        assert_eq!(records[1].kind, BatchRecordKind::Message);

        let mut duplicate = request.clone();
        let Operation::AdmitBatch { records, .. } = &mut duplicate.operation else {
            unreachable!()
        };
        records[1].event_id_hash = records[0].event_id_hash.clone();
        assert!(duplicate.validate().is_err());

        let mut operation_identity = request.clone();
        let Operation::AdmitBatch { records, .. } = &mut operation_identity.operation else {
            unreachable!()
        };
        records[0].native_operation_id = "operation-one".to_owned();
        assert!(operation_identity.validate().is_err());

        let mut private = request;
        let Operation::AdmitBatch { records, .. } = &mut private.operation else {
            unreachable!()
        };
        records[1].payload_json = "{\"role\":\"assistant\",\"content\":\"private\"}".to_owned();
        assert!(private.validate().is_err());
    }

    #[test]
    fn mixed_empty_batch_allows_initial_idle_and_cursor_only_advance() {
        let mut request = mixed_fixture();
        let Operation::AdmitBatch { records, .. } = &mut request.operation else {
            unreachable!()
        };
        records.clear();
        assert!(request.validate().is_ok());

        let Operation::AdmitBatch { cursor, .. } = &mut request.operation else {
            unreachable!()
        };
        cursor.clear();
        assert!(request.validate().is_ok());

        let Operation::AdmitBatch { records, .. } = &mut request.operation else {
            unreachable!()
        };
        records.push(PreparedBatchRecord {
            event_id_hash: "9".repeat(64),
            revision: 0,
            kind: BatchRecordKind::Message,
            time_ns: 101,
            payload_json: "{\"role\":\"assistant\"}".to_owned(),
            record_digest: "8".repeat(64),
            native_session_id: "native-one".to_owned(),
            native_worker_id: String::new(),
            native_turn_id: String::new(),
            native_operation_id: String::new(),
        });
        assert!(request.validate().is_err());
    }

    #[test]
    fn mixed_batch_reply_is_bound_to_the_single_transaction() {
        let request = mixed_fixture();
        let Operation::AdmitBatch {
            cursor,
            batch_digest,
            ..
        } = &request.operation
        else {
            panic!("fixture is mixed admission")
        };
        let reply = Reply {
            version: VERSION,
            request_id: request.request_id.clone(),
            receiver_id: request.receiver_id.clone(),
            request_digest: request.replay_identity().unwrap().digest,
            result: OperationResult::BatchAdmission(AdmissionResult {
                disposition: Disposition::Committed,
                cursor: cursor.clone(),
                batch_digest: batch_digest.clone(),
                dirty_seq: 1,
            }),
        };
        let encoded = reply.to_json(&request).unwrap();
        assert_eq!(Reply::from_json(&encoded, &request).unwrap(), reply);

        let mut wrong_kind = reply;
        wrong_kind.result = OperationResult::FactAdmission(AdmissionResult {
            disposition: Disposition::Committed,
            cursor: cursor.clone(),
            batch_digest: batch_digest.clone(),
            dirty_seq: 1,
        });
        assert!(wrong_kind.to_json(&request).is_err());
    }

    #[test]
    fn identity_replay_is_exact_and_detects_conflict() {
        let (request, _) = fixture();
        let original = request.replay_identity().unwrap();
        let reordered = serde_json::from_str::<Value>(include_str!(
            "../../../tests/fixtures/rust_storage_mutation_v2.json"
        ))
        .unwrap();
        let equivalent = Request::from_json(
            serde_json::to_string_pretty(&reordered["request"])
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(
            original.compare(&equivalent.replay_identity().unwrap()),
            crate::ReplayRelation::Exact
        );
        let mut changed = request.clone();
        let Operation::AdmitLifecycle { cursor, .. } = &mut changed.operation else {
            panic!("fixture is lifecycle admission")
        };
        *cursor = "different".into();
        assert_eq!(
            original.compare(&changed.replay_identity().unwrap()),
            crate::ReplayRelation::Conflict
        );
        changed.request_id = "f".repeat(32);
        assert_eq!(
            original.compare(&changed.replay_identity().unwrap()),
            crate::ReplayRelation::DifferentRequest
        );
    }

    #[test]
    fn rejects_unknown_duplicate_and_oversized_envelopes() {
        let (request, _) = fixture();
        let mut value: Value = serde_json::from_slice(&request.to_json().unwrap()).unwrap();
        value["operation"]["content_refs"] = json!([]);
        assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
        let duplicate = request.to_json().unwrap();
        let raw = String::from_utf8(duplicate).unwrap();
        let raw = raw.replacen("\"version\":2", "\"version\":2,\"version\":2", 1);
        assert!(Request::from_json(raw.as_bytes()).is_err());
        let nested = String::from_utf8(request.to_json().unwrap())
            .unwrap()
            .replacen("\"epoch\":7", "\"epoch\":7,\"epoch\":7", 1);
        assert!(Request::from_json(nested.as_bytes()).is_err());
        assert!(Request::from_json(&vec![b' '; MAX_FRAME_BYTES + 1]).is_err());
    }

    #[test]
    fn lifecycle_cursor_accepts_exact_shared_limit_and_rejects_overflow() {
        let (mut request, _) = fixture();
        if let Operation::AdmitLifecycle { cursor, .. } = &mut request.operation {
            *cursor = "c".repeat(MAX_CURSOR_CHARS);
        } else {
            panic!("fixture is lifecycle admission");
        }
        assert!(request.validate().is_ok());
        if let Operation::AdmitLifecycle { cursor, .. } = &mut request.operation {
            *cursor = "c".repeat(MAX_CURSOR_CHARS + 1);
        }
        assert!(request.validate().is_err());
    }

    #[test]
    fn rejects_unsupported_evidence_and_duplicate_identity() {
        let (request, _) = fixture();
        let mut value: Value = serde_json::from_slice(&request.to_json().unwrap()).unwrap();
        let row = value["operation"]["records"][0].clone();
        value["operation"]["records"]
            .as_array_mut()
            .unwrap()
            .push(row);
        assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
        value["operation"]["records"].as_array_mut().unwrap().pop();
        value["operation"]["records"][0]["revision"] = json!(1);
        assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
        value["operation"]["records"][0]["revision"] = json!(0);
        value["operation"]["records"][0]["kind"] = json!("usage");
        assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
        value["operation"]["records"][0]["kind"] = json!("lifecycle");
        value["operation"]["records"][0]["payload_json"] =
            json!("{\"state\":\"completed\",\"state\":\"failed\"}");
        assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
        value["operation"]["records"][0]["payload_json"] =
            json!("{\"state\":\"completed\",\"output\":\"secret\"}");
        assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
        value["operation"]["records"][0]["payload_json"] = json!("{\"state\":\"completed\"}");
        value["operation"]["lease"]["receiver_id"] = json!("another-receiver");
        assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
        value["operation"]["lease"]["receiver_id"] = json!("receiver-one");
        value["operation"]["lease"]["generation"] = json!("A".repeat(64));
        assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn fact_admission_matches_content_free_python_metadata_rules() {
        let request = facts_fixture();
        assert!(request.validate().is_ok());
        let json = request.to_json().unwrap();
        assert_eq!(Request::from_json(&json).unwrap(), request);
        let reply = Reply {
            version: VERSION,
            request_id: request.request_id.clone(),
            receiver_id: request.receiver_id.clone(),
            request_digest: request.replay_identity().unwrap().digest,
            result: OperationResult::FactAdmission(AdmissionResult {
                disposition: Disposition::Committed,
                cursor: "facts-one".into(),
                batch_digest: "c".repeat(64),
                dirty_seq: 1,
            }),
        };
        assert!(Reply::from_json(&reply.to_json(&request).unwrap(), &request).is_ok());

        let mut invalid = request.clone();
        let Operation::AdmitFacts { records, .. } = &mut invalid.operation else {
            unreachable!()
        };
        records[0].payload_json = "{\"role\":\"system\"}".into();
        assert!(invalid.validate().is_err());

        let mut invalid = request.clone();
        let Operation::AdmitFacts { records, .. } = &mut invalid.operation else {
            unreachable!()
        };
        records[1].native_operation_id.clear();
        records[1].native_session_id.clear();
        assert!(invalid.validate().is_err());

        let mut invalid = request.clone();
        let Operation::AdmitFacts { records, .. } = &mut invalid.operation else {
            unreachable!()
        };
        records[3].payload_json = "{\"input_tokens\":true}".into();
        assert!(invalid.validate().is_err());

        let mut valid_usage_identity = request;
        let Operation::AdmitFacts { records, .. } = &mut valid_usage_identity.operation else {
            unreachable!()
        };
        records[3].native_session_id = "session-one".into();
        records[3].native_worker_id = "worker-one".into();
        assert!(valid_usage_identity.validate().is_ok());
    }

    #[test]
    fn fact_admission_enforces_exact_cursor_record_and_frame_bounds() {
        let mut request = facts_fixture();
        if let Operation::AdmitFacts {
            cursor, records, ..
        } = &mut request.operation
        {
            *cursor = "c".repeat(MAX_CURSOR_CHARS);
            records.resize(48, records[0].clone());
            for (index, row) in records.iter_mut().enumerate() {
                row.event_id_hash = format!("{:064x}", index + 1);
                row.record_digest = format!("{:064x}", index + 100);
            }
        }
        assert!(request.validate().is_ok());
        if let Operation::AdmitFacts { cursor, .. } = &mut request.operation {
            *cursor = "c".repeat(MAX_CURSOR_CHARS + 1);
        }
        assert!(request.validate().is_err());
        if let Operation::AdmitFacts {
            cursor, records, ..
        } = &mut request.operation
        {
            *cursor = "c".repeat(MAX_CURSOR_CHARS);
            records.push(records[0].clone());
        }
        assert!(request.validate().is_err());
        let mut oversized = facts_fixture();
        if let Operation::AdmitFacts { cursor, .. } = &mut oversized.operation {
            *cursor = "c".repeat(MAX_FRAME_BYTES);
        }
        assert!(oversized.to_json().is_err());
    }

    #[test]
    fn limits_rows_cursors_and_payload_bytes() {
        let (request, _) = fixture();
        let mut value: Value = serde_json::from_slice(&request.to_json().unwrap()).unwrap();
        let original = value["operation"]["records"][0].clone();
        let rows = value["operation"]["records"].as_array_mut().unwrap();
        rows.clear();
        for index in 0..=MAX_RECORDS {
            let mut row = original.clone();
            row["event_id_hash"] = json!(format!("{index:064x}"));
            rows.push(row);
        }
        assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
        value["operation"]["records"]
            .as_array_mut()
            .unwrap()
            .truncate(1);
        value["operation"]["cursor"] = json!("x".repeat(MAX_CURSOR_CHARS + 1));
        assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
        value["operation"]["cursor"] = json!("cursor-one");
        value["operation"]["records"][0]["payload_json"] = json!(format!(
            "{{\"state\":\"completed\",\"reason_code\":\"{}\"}}",
            "x".repeat(4096)
        ));
        assert!(Request::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn reply_must_bind_request_digest_cursor_and_batch() {
        let (request, digest) = fixture();
        let Operation::AdmitLifecycle {
            cursor,
            batch_digest,
            ..
        } = &request.operation
        else {
            panic!("fixture is lifecycle admission")
        };
        let reply = Reply {
            version: VERSION,
            request_id: request.request_id.clone(),
            receiver_id: request.receiver_id.clone(),
            request_digest: digest,
            result: OperationResult::LifecycleAdmission(AdmissionResult {
                disposition: Disposition::Committed,
                cursor: cursor.clone(),
                batch_digest: batch_digest.clone(),
                dirty_seq: 1,
            }),
        };
        assert_eq!(
            Reply::from_json(&reply.to_json(&request).unwrap(), &request).unwrap(),
            reply
        );
        let raw = String::from_utf8(reply.to_json(&request).unwrap()).unwrap();
        let duplicate = raw.replacen("\"version\":2", "\"version\":2,\"version\":2", 1);
        assert!(Reply::from_json(duplicate.as_bytes(), &request).is_err());
        let mut unknown: Value = serde_json::from_str(&raw).unwrap();
        unknown["result"]["content"] = json!("forbidden");
        assert!(Reply::from_json(&serde_json::to_vec(&unknown).unwrap(), &request).is_err());
        assert!(Reply::from_json(&vec![b' '; MAX_FRAME_BYTES + 1], &request).is_err());
        let mut changed = reply.clone();
        if let OperationResult::LifecycleAdmission(result) = &mut changed.result {
            result.batch_digest = "f".repeat(64);
        }
        assert!(changed.to_json(&request).is_err());
        changed = reply;
        changed.request_digest = "f".repeat(64);
        assert!(changed.to_json(&request).is_err());
        changed.request_digest = request.replay_identity().unwrap().digest;
        if let OperationResult::LifecycleAdmission(result) = &mut changed.result {
            result.disposition = Disposition::Idle;
        }
        assert!(changed.to_json(&request).is_err());
    }

    fn revoke_fixture() -> Request {
        let (admission, _) = fixture();
        let Operation::AdmitLifecycle { lease, cursor, .. } = admission.operation else {
            panic!("fixture is lifecycle admission")
        };
        Request {
            version: VERSION,
            request_id: "1123456789abcdef0123456789abcdef".into(),
            receiver_id: admission.receiver_id,
            operation: Operation::RevokeSource {
                lease,
                cursor,
                expected_dirty_seq: 1,
            },
        }
    }

    fn activation_fixture() -> Request {
        Request {
            version: VERSION,
            request_id: "3123456789abcdef0123456789abcdef".into(),
            receiver_id: "receiver-one".into(),
            operation: Operation::ActivateSource {
                plugin_id: "fixture.plugin".into(),
                source_id: "source-one".into(),
                generation: "a".repeat(64),
            },
        }
    }

    fn set_capture_fixture(request_id: &str, current: bool, target: bool) -> Request {
        Request {
            version: VERSION,
            request_id: request_id.into(),
            receiver_id: "receiver-one".into(),
            operation: Operation::SetSourceCapture {
                lease: SourceLease {
                    receiver_id: "receiver-one".into(),
                    plugin_id: "fixture.plugin".into(),
                    source_id: "source-one".into(),
                    generation: "a".repeat(64),
                    epoch: 11,
                    policy_epoch: 3,
                },
                enabled: target,
                expected_cursor: "cursor-one".into(),
                expected_dirty_seq: 4,
                expected_capture_enabled: current,
            },
        }
    }

    #[test]
    fn source_capture_contract_binds_transition_and_stable_noop() {
        let no_op = set_capture_fixture("4123456789abcdef0123456789abcdef", true, true);
        let Operation::SetSourceCapture { lease, .. } = &no_op.operation else {
            unreachable!()
        };
        let reply = Reply {
            version: VERSION,
            request_id: no_op.request_id.clone(),
            receiver_id: no_op.receiver_id.clone(),
            request_digest: no_op.replay_identity().unwrap().digest,
            result: OperationResult::SourceCaptureSet(SourceCaptureSetResult {
                disposition: Disposition::Idle,
                lease: lease.clone(),
                enabled: true,
                cursor: "cursor-one".into(),
                dirty_seq: 4,
            }),
        };
        assert_eq!(
            Reply::from_json(&reply.to_json(&no_op).unwrap(), &no_op).unwrap(),
            reply
        );
        let mut replay = reply.clone();
        if let OperationResult::SourceCaptureSet(result) = &mut replay.result {
            result.disposition = Disposition::Replay;
        }
        assert!(replay.to_json(&no_op).is_ok());
        let mut invalid_noop = reply.clone();
        let OperationResult::SourceCaptureSet(result) = &mut invalid_noop.result else {
            unreachable!()
        };
        result.lease.epoch += 1;
        assert!(invalid_noop.to_json(&no_op).is_err());

        let transition = set_capture_fixture("5123456789abcdef0123456789abcdef", true, false);
        let Operation::SetSourceCapture { lease, .. } = &transition.operation else {
            unreachable!()
        };
        let reply = Reply {
            version: VERSION,
            request_id: transition.request_id.clone(),
            receiver_id: transition.receiver_id.clone(),
            request_digest: transition.replay_identity().unwrap().digest,
            result: OperationResult::SourceCaptureSet(SourceCaptureSetResult {
                disposition: Disposition::Committed,
                lease: SourceLease {
                    epoch: 12,
                    policy_epoch: 4,
                    ..lease.clone()
                },
                enabled: false,
                cursor: "cursor-one".into(),
                dirty_seq: 5,
            }),
        };
        assert_eq!(
            Reply::from_json(&reply.to_json(&transition).unwrap(), &transition).unwrap(),
            reply
        );
        let mut replay = reply.clone();
        if let OperationResult::SourceCaptureSet(result) = &mut replay.result {
            result.disposition = Disposition::Replay;
        }
        assert!(replay.to_json(&transition).is_ok());
        let mut wrong_result = reply.clone();
        let OperationResult::SourceCaptureSet(result) = &mut wrong_result.result else {
            unreachable!()
        };
        result.dirty_seq = 7;
        assert!(wrong_result.to_json(&transition).is_err());
        let mut wrong_kind = reply;
        let OperationResult::SourceCaptureSet(result) = wrong_kind.result else {
            unreachable!()
        };
        wrong_kind.result = OperationResult::SourceRevoked(SourceRevokedResult {
            disposition: Disposition::Committed,
            cursor: result.cursor,
            dirty_seq: 5,
            lease_epoch: 12,
            policy_epoch: 4,
            capture_enabled: false,
        });
        assert!(wrong_kind.to_json(&transition).is_err());
    }

    #[test]
    fn activation_contract_returns_request_bound_server_lease() {
        let request = activation_fixture();
        let Operation::ActivateSource {
            plugin_id,
            source_id,
            generation,
        } = &request.operation
        else {
            panic!("expected activation request")
        };
        let reply = Reply {
            version: VERSION,
            request_id: request.request_id.clone(),
            receiver_id: request.receiver_id.clone(),
            request_digest: request.replay_identity().unwrap().digest,
            result: OperationResult::SourceActivated(SourceActivatedResult {
                disposition: Disposition::Committed,
                lease: SourceLease {
                    receiver_id: request.receiver_id.clone(),
                    plugin_id: plugin_id.clone(),
                    source_id: source_id.clone(),
                    generation: generation.clone(),
                    epoch: 23,
                    policy_epoch: 1,
                },
                cursor: String::new(),
                dirty_seq: 0,
            }),
        };
        assert_eq!(
            Reply::from_json(&reply.to_json(&request).unwrap(), &request).unwrap(),
            reply
        );
        let mut mismatched = reply.clone();
        let OperationResult::SourceActivated(result) = &mut mismatched.result else {
            unreachable!()
        };
        result.lease.epoch = 0;
        assert!(mismatched.to_json(&request).is_err());
        let OperationResult::SourceActivated(result) = &mut mismatched.result else {
            unreachable!()
        };
        result.lease.epoch = 23;
        result.lease.plugin_id = "other.plugin".into();
        assert!(mismatched.to_json(&request).is_err());
        let mut wrong_kind = reply.clone();
        let OperationResult::SourceActivated(result) = wrong_kind.result else {
            unreachable!()
        };
        wrong_kind.result = OperationResult::SourceRevoked(SourceRevokedResult {
            disposition: Disposition::Committed,
            cursor: result.cursor,
            dirty_seq: 1,
            lease_epoch: 24,
            policy_epoch: 2,
            capture_enabled: false,
        });
        assert!(wrong_kind.to_json(&request).is_err());

        let mut wrong_operation = request.clone();
        wrong_operation.operation = Operation::AdmitLifecycle {
            lease: SourceLease {
                receiver_id: request.receiver_id.clone(),
                plugin_id: plugin_id.clone(),
                source_id: source_id.clone(),
                generation: generation.clone(),
                epoch: 23,
                policy_epoch: 1,
            },
            expected_cursor: String::new(),
            cursor: "next".into(),
            batch_digest: "d".repeat(64),
            records: Vec::new(),
        };
        assert!(reply.to_json(&wrong_operation).is_err());
    }

    #[test]
    fn revoke_contract_uses_typed_policy_result_and_binds_fence() {
        let request = revoke_fixture();
        let Operation::RevokeSource {
            lease,
            cursor,
            expected_dirty_seq,
        } = &request.operation
        else {
            panic!("expected revoke request")
        };
        let reply = Reply {
            version: VERSION,
            request_id: request.request_id.clone(),
            receiver_id: request.receiver_id.clone(),
            request_digest: request.replay_identity().unwrap().digest,
            result: OperationResult::SourceRevoked(SourceRevokedResult {
                disposition: Disposition::Committed,
                cursor: cursor.clone(),
                dirty_seq: expected_dirty_seq + 1,
                lease_epoch: lease.epoch + 1,
                policy_epoch: lease.policy_epoch + 1,
                capture_enabled: false,
            }),
        };
        let bytes = reply.to_json(&request).unwrap();
        assert_eq!(Reply::from_json(&bytes, &request).unwrap(), reply);

        let mut wrong = reply.clone();
        let OperationResult::SourceRevoked(result) = &mut wrong.result else {
            unreachable!()
        };
        result.capture_enabled = true;
        let _ = result;
        assert!(wrong.to_json(&request).is_err());
        let OperationResult::SourceRevoked(result) = &mut wrong.result else {
            unreachable!()
        };
        result.capture_enabled = false;
        result.policy_epoch += 1;
        assert!(wrong.to_json(&request).is_err());
        let mut wrong_kind = reply;
        let OperationResult::SourceRevoked(result) = wrong_kind.result else {
            unreachable!()
        };
        wrong_kind.result = OperationResult::LifecycleAdmission(AdmissionResult {
            disposition: Disposition::Committed,
            cursor: result.cursor,
            batch_digest: "d".repeat(64),
            dirty_seq: result.dirty_seq,
        });
        assert!(wrong_kind.to_json(&request).is_err());
    }
}
