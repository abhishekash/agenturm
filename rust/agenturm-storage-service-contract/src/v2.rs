//! Data-only v2 contract for one host-prepared, metadata-only lifecycle batch.
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
    AdmitLifecycle {
        lease: SourceLease,
        expected_cursor: String,
        cursor: String,
        batch_digest: String,
        records: Vec<PreparedLifecycle>,
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
    pub result: AdmissionResult,
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
    value.len() <= 512 && !value.chars().any(char::is_control)
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
        let Operation::AdmitLifecycle {
            expected_cursor,
            cursor,
            batch_digest,
            records,
            ..
        } = &request.operation;
        if self.version != VERSION
            || self.request_id != request.request_id
            || self.receiver_id != request.receiver_id
            || self.request_digest != replay.digest
            || self.result.cursor != *cursor
            || self.result.batch_digest != *batch_digest
            || self.result.dirty_seq < 0
            || (self.result.disposition == Disposition::Idle
                && (!records.is_empty() || cursor != expected_cursor))
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
        let Operation::AdmitLifecycle { cursor, .. } = &mut changed.operation;
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
        value["operation"]["cursor"] = json!("x".repeat(513));
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
        } = &request.operation;
        let reply = Reply {
            version: VERSION,
            request_id: request.request_id.clone(),
            receiver_id: request.receiver_id.clone(),
            request_digest: digest,
            result: AdmissionResult {
                disposition: Disposition::Committed,
                cursor: cursor.clone(),
                batch_digest: batch_digest.clone(),
                dirty_seq: 1,
            },
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
        changed.result.batch_digest = "f".repeat(64);
        assert!(changed.to_json(&request).is_err());
        changed = reply;
        changed.request_digest = "f".repeat(64);
        assert!(changed.to_json(&request).is_err());
        changed.request_digest = request.replay_identity().unwrap().digest;
        changed.result.disposition = Disposition::Idle;
        assert!(changed.to_json(&request).is_err());
    }
}
