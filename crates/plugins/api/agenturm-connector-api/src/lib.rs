//! Stable, bounded contracts for integrations with external systems.
//!
//! Inbound and outbound connectors are different ports. The host owns policy,
//! grants, durable checkpoints and delivery receipts; implementing these traits
//! does not grant access to the store or authorize a network destination.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::{self, Write};

pub const CONNECTOR_API_VERSION: u16 = 1;
pub const MAX_CONNECTOR_ID_BYTES: usize = 200;
pub const MAX_CHECKPOINT_BYTES: usize = 800;
pub const MAX_INBOUND_RECORDS: usize = 256;
pub const MAX_INBOUND_BATCH_BYTES: usize = 1024 * 1024;
pub const MAX_DELIVERY_BYTES: usize = 1024 * 1024;
pub const MAX_DELIVERY_KEY_BYTES: usize = 200;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConnectorId(String);

impl ConnectorId {
    pub fn parse(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_CONNECTOR_ID_BYTES
            || !value.as_bytes()[0].is_ascii_alphanumeric()
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
            })
        {
            return Err(ContractError::InvalidIdentity);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorDirection {
    Inbound,
    Outbound,
}

/// Public description used by a host or package catalog before activation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorDescriptor {
    pub api_version: u16,
    pub id: ConnectorId,
    pub direction: ConnectorDirection,
    /// Narrow, namespaced features; these are declarations, not permissions.
    pub features: BTreeSet<String>,
}

impl ConnectorDescriptor {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.api_version != CONNECTOR_API_VERSION
            || self.features.len() > 32
            || ConnectorId::parse(self.id.as_str()).is_err()
        {
            return Err(ContractError::InvalidDescriptor);
        }
        for feature in &self.features {
            if feature.is_empty()
                || feature.len() > 100
                || !feature
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._/-".contains(&byte))
            {
                return Err(ContractError::InvalidDescriptor);
            }
        }
        Ok(())
    }
}

/// Resource limits assigned by the host for one inbound poll.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PollBudget {
    pub max_records: u16,
    pub max_bytes: u32,
}

impl PollBudget {
    pub fn validate(self) -> Result<(), ContractError> {
        if self.max_records == 0
            || usize::from(self.max_records) > MAX_INBOUND_RECORDS
            || self.max_bytes == 0
            || self.max_bytes as usize > MAX_INBOUND_BATCH_BYTES
        {
            return Err(ContractError::InvalidBudget);
        }
        Ok(())
    }
}

/// One bounded record from an external system. The host must still project
/// allowlisted fields and validate domain facts before durable admission.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundRecord {
    pub record_id: String,
    pub revision: u32,
    pub observed_at_unix_ns: u64,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundBatch {
    pub next_checkpoint: String,
    pub records: Vec<InboundRecord>,
}

impl InboundBatch {
    pub fn validate(
        &self,
        current_checkpoint: &str,
        budget: PollBudget,
    ) -> Result<(), ContractError> {
        budget.validate()?;
        if current_checkpoint.len() > MAX_CHECKPOINT_BYTES
            || current_checkpoint.chars().any(char::is_control)
            || self.next_checkpoint.len() > MAX_CHECKPOINT_BYTES
            || self.next_checkpoint.chars().any(char::is_control)
            || self.records.len() > usize::from(budget.max_records)
            || (!self.records.is_empty() && self.next_checkpoint == current_checkpoint)
        {
            return Err(ContractError::InvalidBatch);
        }
        let mut ids = BTreeSet::new();
        for record in &self.records {
            if !valid_atom(&record.record_id, MAX_CONNECTOR_ID_BYTES)
                || !ids.insert(&record.record_id)
            {
                return Err(ContractError::InvalidBatch);
            }
        }
        serialized_len(
            self,
            (budget.max_bytes as usize).min(MAX_INBOUND_BATCH_BYTES),
        )
        .map_err(|_| ContractError::InvalidBatch)?;
        Ok(())
    }
}

/// The adapter fetches and ACKs; it never advances the host's durable cursor.
/// The host must commit the batch and checkpoint atomically before calling ACK.
/// Repeating ACK for the same checkpoint is idempotent; an older checkpoint
/// ACK must not move source state backward. `reset` is host-invoked recovery: it
/// clears only transport/session state, never durable source history or cursor.
pub trait InboundConnector: Send {
    fn descriptor(&self) -> &ConnectorDescriptor;
    fn poll(
        &mut self,
        checkpoint: &str,
        budget: PollBudget,
    ) -> Result<InboundBatch, ConnectorError>;
    fn ack(&mut self, committed_checkpoint: &str) -> Result<(), ConnectorError>;
    fn reset(&mut self) -> Result<(), ConnectorError>;
}

/// Host-created outbound work item. The key and request digest must be stable
/// across every retry and reconciliation attempt for this delivery ID.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundDelivery {
    pub delivery_id: String,
    pub idempotency_key: String,
    pub destination_id: String,
    pub action: String,
    pub payload: Value,
    pub request_sha256: String,
}

impl OutboundDelivery {
    pub fn validate(&self) -> Result<(), ContractError> {
        if !valid_atom(&self.delivery_id, MAX_DELIVERY_KEY_BYTES)
            || !valid_atom(&self.idempotency_key, MAX_DELIVERY_KEY_BYTES)
            || !valid_atom(&self.destination_id, MAX_CONNECTOR_ID_BYTES)
            || !valid_atom(&self.action, 100)
            || !valid_digest(&self.request_sha256)
            || serialized_len(self, MAX_DELIVERY_BYTES).is_err()
        {
            return Err(ContractError::InvalidDelivery);
        }
        if request_sha256(&self.destination_id, &self.action, &self.payload)? != self.request_sha256
        {
            return Err(ContractError::InvalidDelivery);
        }
        Ok(())
    }
}

/// Stable digest over the destination, action and payload tuple used to reject
/// accidental reuse of an idempotency key for changed delivery work.
pub fn request_sha256(
    destination_id: &str,
    action: &str,
    payload: &Value,
) -> Result<String, ContractError> {
    serialized_len(payload, MAX_DELIVERY_BYTES).map_err(|_| ContractError::InvalidDelivery)?;
    let payload = canonical_value(payload);
    let request = (destination_id, action, payload);
    serialized_len(&request, MAX_DELIVERY_BYTES).map_err(|_| ContractError::InvalidDelivery)?;
    let bytes = serde_json::to_vec(&request).map_err(|_| ContractError::InvalidDelivery)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical_value).collect()),
        Value::Object(fields) => {
            let mut sorted = fields.iter().collect::<std::collections::BTreeMap<_, _>>();
            let mut canonical = serde_json::Map::new();
            for (key, value) in std::mem::take(&mut sorted) {
                canonical.insert(key.clone(), canonical_value(value));
            }
            Value::Object(canonical)
        }
        other => other.clone(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", deny_unknown_fields)]
pub enum DeliveryLookup {
    Accepted {
        destination_id: String,
        idempotency_key: String,
        request_sha256: String,
        receipt_id: String,
    },
    ConfirmedMissing {
        destination_id: String,
        idempotency_key: String,
        request_sha256: String,
    },
    Unknown {
        destination_id: String,
        idempotency_key: String,
        request_sha256: String,
    },
}

impl DeliveryLookup {
    pub fn validate(&self) -> Result<(), ContractError> {
        let (destination_id, idempotency_key, request_sha256) = match self {
            Self::Accepted {
                destination_id,
                idempotency_key,
                request_sha256,
                receipt_id,
            } => {
                if !valid_atom(receipt_id, MAX_DELIVERY_KEY_BYTES) {
                    return Err(ContractError::InvalidDelivery);
                }
                (destination_id, idempotency_key, request_sha256)
            }
            Self::ConfirmedMissing {
                destination_id,
                idempotency_key,
                request_sha256,
            }
            | Self::Unknown {
                destination_id,
                idempotency_key,
                request_sha256,
            } => (destination_id, idempotency_key, request_sha256),
        };
        if !valid_atom(destination_id, MAX_CONNECTOR_ID_BYTES)
            || !valid_atom(idempotency_key, MAX_DELIVERY_KEY_BYTES)
            || !valid_digest(request_sha256)
        {
            return Err(ContractError::InvalidDelivery);
        }
        Ok(())
    }

    pub fn validate_for(&self, delivery: &OutboundDelivery) -> Result<(), ContractError> {
        delivery.validate()?;
        self.validate()?;
        let (destination_id, idempotency_key, request_sha256) = match self {
            Self::Accepted {
                destination_id,
                idempotency_key,
                request_sha256,
                ..
            }
            | Self::ConfirmedMissing {
                destination_id,
                idempotency_key,
                request_sha256,
            }
            | Self::Unknown {
                destination_id,
                idempotency_key,
                request_sha256,
            } => (destination_id, idempotency_key, request_sha256),
        };
        if destination_id != &delivery.destination_id
            || idempotency_key != &delivery.idempotency_key
            || request_sha256 != &delivery.request_sha256
        {
            return Err(ContractError::InvalidDelivery);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryReceipt {
    pub destination_id: String,
    pub idempotency_key: String,
    pub request_sha256: String,
    pub receipt_id: String,
}

impl DeliveryReceipt {
    pub fn validate(&self) -> Result<(), ContractError> {
        if !valid_atom(&self.destination_id, MAX_CONNECTOR_ID_BYTES)
            || !valid_atom(&self.idempotency_key, MAX_DELIVERY_KEY_BYTES)
            || !valid_digest(&self.request_sha256)
            || !valid_atom(&self.receipt_id, MAX_DELIVERY_KEY_BYTES)
        {
            return Err(ContractError::InvalidDelivery);
        }
        Ok(())
    }

    pub fn validate_for(&self, delivery: &OutboundDelivery) -> Result<(), ContractError> {
        delivery.validate()?;
        self.validate()?;
        if self.destination_id != delivery.destination_id
            || self.idempotency_key != delivery.idempotency_key
            || self.request_sha256 != delivery.request_sha256
        {
            return Err(ContractError::InvalidDelivery);
        }
        Ok(())
    }
}

/// Outbound delivery is at-least-once. A timeout or ambiguous result must be
/// looked up by the same idempotency key before the host retries it.
pub trait OutboundConnector: Send {
    fn descriptor(&self) -> &ConnectorDescriptor;
    fn deliver(&mut self, delivery: &OutboundDelivery) -> Result<DeliveryReceipt, ConnectorError>;
    fn lookup(&mut self, delivery: &OutboundDelivery) -> Result<DeliveryLookup, ConnectorError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorErrorKind {
    Rejected,
    Retryable,
    OutcomeUnknown,
    Unauthorized,
    InvalidRequest,
}

/// Bounded, non-sensitive error classification using a closed code vocabulary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorError {
    pub kind: ConnectorErrorKind,
    pub code: ConnectorErrorCode,
}

/// Closed error vocabulary safe to retain in diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorErrorCode {
    Timeout,
    Unavailable,
    RateLimited,
    Unauthorized,
    RequestRejected,
    InvalidRequest,
    InvalidResponse,
    IdempotencyConflict,
    CheckpointMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractError {
    InvalidIdentity,
    InvalidDescriptor,
    InvalidBudget,
    InvalidBatch,
    InvalidDelivery,
}

fn valid_atom(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn serialized_len<T: Serialize>(value: &T, max_bytes: usize) -> io::Result<usize> {
    let mut counter = BoundedCounter {
        bytes: 0,
        max_bytes,
    };
    serde_json::to_writer(&mut counter, value).map_err(io::Error::other)?;
    Ok(counter.bytes)
}

struct BoundedCounter {
    bytes: usize,
    max_bytes: usize,
}

impl Write for BoundedCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|size| *size <= self.max_bytes)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "serialized value too large")
            })?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn budget() -> PollBudget {
        PollBudget {
            max_records: 2,
            max_bytes: 4096,
        }
    }

    fn descriptor(direction: ConnectorDirection) -> ConnectorDescriptor {
        ConnectorDescriptor {
            api_version: CONNECTOR_API_VERSION,
            id: ConnectorId::parse("example.test").unwrap(),
            direction,
            features: BTreeSet::from(["metadata/1".to_owned()]),
        }
    }

    #[test]
    fn descriptor_and_ids_are_versioned_and_bounded() {
        assert!(descriptor(ConnectorDirection::Inbound).validate().is_ok());
        assert!(descriptor(ConnectorDirection::Outbound).validate().is_ok());
        assert!(
            ConnectorDescriptor {
                api_version: CONNECTOR_API_VERSION + 1,
                ..descriptor(ConnectorDirection::Inbound)
            }
            .validate()
            .is_err()
        );
        assert!(ConnectorId::parse("../escape").is_err());
    }

    #[test]
    fn inbound_batch_rejects_duplicate_ids_and_budget_overruns() {
        let record = || InboundRecord {
            record_id: "event-1".into(),
            revision: 1,
            observed_at_unix_ns: 5,
            payload: json!({"kind":"usage","total_tokens":7}),
        };
        let batch = InboundBatch {
            next_checkpoint: "cursor-2".into(),
            records: vec![record()],
        };
        assert!(batch.validate("cursor-1", budget()).is_ok());
        let duplicate = InboundBatch {
            records: vec![record(), record()],
            ..batch.clone()
        };
        assert_eq!(
            duplicate.validate("cursor-1", budget()),
            Err(ContractError::InvalidBatch)
        );
        let too_many = InboundBatch {
            records: vec![
                record(),
                InboundRecord {
                    record_id: "event-2".into(),
                    ..record()
                },
                InboundRecord {
                    record_id: "event-3".into(),
                    ..record()
                },
            ],
            ..batch
        };
        assert_eq!(
            too_many.validate("cursor-1", budget()),
            Err(ContractError::InvalidBatch)
        );
    }

    #[test]
    fn inbound_cursor_does_not_accept_control_characters() {
        let batch = InboundBatch {
            next_checkpoint: "cursor\nforged".into(),
            records: Vec::new(),
        };
        assert_eq!(
            batch.validate("cursor-1", budget()),
            Err(ContractError::InvalidBatch)
        );
    }

    #[test]
    fn inbound_byte_budget_is_enforced_during_serialization() {
        let batch = InboundBatch {
            next_checkpoint: "cursor-2".into(),
            records: vec![InboundRecord {
                record_id: "event-1".into(),
                revision: 1,
                observed_at_unix_ns: 10,
                payload: Value::String("x".repeat(8192)),
            }],
        };
        assert_eq!(
            batch.validate("cursor-1", budget()),
            Err(ContractError::InvalidBatch)
        );
    }

    #[test]
    fn nonempty_inbound_batch_must_advance_checkpoint() {
        let batch = InboundBatch {
            next_checkpoint: "cursor-1".into(),
            records: vec![InboundRecord {
                record_id: "event-1".into(),
                revision: 1,
                observed_at_unix_ns: 1,
                payload: json!({"kind":"usage"}),
            }],
        };
        assert_eq!(
            batch.validate("cursor-1", budget()),
            Err(ContractError::InvalidBatch)
        );
        let idle = InboundBatch {
            records: Vec::new(),
            ..batch
        };
        assert!(idle.validate("cursor-1", budget()).is_ok());
    }

    #[test]
    fn outbound_delivery_requires_stable_identity_and_digest_shape() {
        let payload = json!({"text":"bounded by host policy"});
        let request_sha256 = request_sha256("workspace:main", "comment.create", &payload).unwrap();
        let delivery = OutboundDelivery {
            delivery_id: "delivery-1".into(),
            idempotency_key: "key-1".into(),
            destination_id: "workspace:main".into(),
            action: "comment.create".into(),
            payload,
            request_sha256,
        };
        assert!(delivery.validate().is_ok());
        assert!(
            OutboundDelivery {
                idempotency_key: "bad key".into(),
                ..delivery.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            OutboundDelivery {
                request_sha256: "not-a-digest".into(),
                ..delivery
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn changed_action_cannot_reuse_the_existing_request_digest() {
        let payload = json!({"text":"same payload"});
        let delivery = OutboundDelivery {
            delivery_id: "delivery-2".into(),
            idempotency_key: "key-2".into(),
            destination_id: "workspace:main".into(),
            action: "comment.create".into(),
            request_sha256: request_sha256("workspace:main", "comment.create", &payload).unwrap(),
            payload,
        };
        assert!(delivery.validate().is_ok());
        assert!(
            OutboundDelivery {
                action: "comment.delete".into(),
                ..delivery
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn oversized_outbound_payload_cannot_be_hashed_as_a_delivery() {
        assert_eq!(
            request_sha256(
                "workspace:main",
                "comment.create",
                &Value::String("x".repeat(2 * 1024 * 1024))
            ),
            Err(ContractError::InvalidDelivery)
        );
    }

    #[test]
    fn request_digest_is_independent_of_object_key_insertion_order() {
        let first: Value = serde_json::from_str(r#"{"z":1,"a":{"y":2,"b":3}}"#).unwrap();
        let second: Value = serde_json::from_str(r#"{"a":{"b":3,"y":2},"z":1}"#).unwrap();
        assert_eq!(
            request_sha256("workspace:main", "comment.create", &first).unwrap(),
            request_sha256("workspace:main", "comment.create", &second).unwrap()
        );
    }

    #[test]
    fn ambiguous_delivery_state_is_serializable_and_distinct() {
        let lookup_scope = || ("workspace:main".into(), "key-1".into(), "a".repeat(64));
        let (destination_id, idempotency_key, request_sha256) = lookup_scope();
        let unknown = DeliveryLookup::Unknown {
            destination_id,
            idempotency_key,
            request_sha256,
        };
        let (destination_id, idempotency_key, request_sha256) = lookup_scope();
        let missing = DeliveryLookup::ConfirmedMissing {
            destination_id,
            idempotency_key,
            request_sha256,
        };
        assert_ne!(unknown, missing);
        let (destination_id, idempotency_key, request_sha256) = lookup_scope();
        assert_ne!(
            unknown,
            DeliveryLookup::Accepted {
                destination_id,
                idempotency_key,
                request_sha256,
                receipt_id: "r1".into()
            }
        );
        let raw = serde_json::to_string(&unknown).unwrap();
        assert_eq!(
            serde_json::from_str::<DeliveryLookup>(&raw).unwrap(),
            unknown
        );
        let (destination_id, idempotency_key, request_sha256) = lookup_scope();
        assert!(
            DeliveryLookup::Accepted {
                destination_id,
                idempotency_key,
                request_sha256,
                receipt_id: "receipt-1".into()
            }
            .validate()
            .is_ok()
        );
        let (destination_id, idempotency_key, request_sha256) = lookup_scope();
        assert!(
            DeliveryLookup::Accepted {
                destination_id,
                idempotency_key,
                request_sha256,
                receipt_id: "bad receipt".into()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn outbound_responses_must_match_the_request_identity() {
        let payload = json!({"summary":"test"});
        let delivery = OutboundDelivery {
            delivery_id: "delivery-9".into(),
            idempotency_key: "key-9".into(),
            destination_id: "workspace:main".into(),
            action: "comment.create".into(),
            request_sha256: request_sha256("workspace:main", "comment.create", &payload).unwrap(),
            payload,
        };
        let accepted = DeliveryLookup::Accepted {
            destination_id: delivery.destination_id.clone(),
            idempotency_key: delivery.idempotency_key.clone(),
            request_sha256: delivery.request_sha256.clone(),
            receipt_id: "receipt-9".into(),
        };
        assert!(accepted.validate_for(&delivery).is_ok());
        let missing = DeliveryLookup::ConfirmedMissing {
            destination_id: delivery.destination_id.clone(),
            idempotency_key: delivery.idempotency_key.clone(),
            request_sha256: delivery.request_sha256.clone(),
        };
        assert!(missing.validate_for(&delivery).is_ok());
        let unknown = DeliveryLookup::Unknown {
            destination_id: delivery.destination_id.clone(),
            idempotency_key: delivery.idempotency_key.clone(),
            request_sha256: delivery.request_sha256.clone(),
        };
        assert!(unknown.validate_for(&delivery).is_ok());

        let mismatched_responses = [
            DeliveryLookup::Accepted {
                destination_id: "other:destination".into(),
                idempotency_key: delivery.idempotency_key.clone(),
                request_sha256: delivery.request_sha256.clone(),
                receipt_id: "receipt-9".into(),
            },
            DeliveryLookup::ConfirmedMissing {
                destination_id: delivery.destination_id.clone(),
                idempotency_key: "other-key".into(),
                request_sha256: delivery.request_sha256.clone(),
            },
            DeliveryLookup::Unknown {
                destination_id: delivery.destination_id.clone(),
                idempotency_key: delivery.idempotency_key.clone(),
                request_sha256: "b".repeat(64),
            },
        ];
        for response in mismatched_responses {
            assert_eq!(
                response.validate_for(&delivery),
                Err(ContractError::InvalidDelivery)
            );
        }
        let receipt = DeliveryReceipt {
            destination_id: delivery.destination_id.clone(),
            idempotency_key: "other-key".into(),
            request_sha256: delivery.request_sha256.clone(),
            receipt_id: "receipt-9".into(),
        };
        assert_eq!(
            receipt.validate_for(&delivery),
            Err(ContractError::InvalidDelivery)
        );
    }

    #[test]
    fn connector_errors_are_closed_and_bounded() {
        let error = ConnectorError {
            kind: ConnectorErrorKind::OutcomeUnknown,
            code: ConnectorErrorCode::Timeout,
        };
        assert_eq!(
            serde_json::from_str::<ConnectorError>(&serde_json::to_string(&error).unwrap())
                .unwrap(),
            error
        );
        let token_like = json!({
            "kind":"outcome_unknown",
            "code":"sk_live_12345678901234567890"
        });
        assert!(serde_json::from_value::<ConnectorError>(token_like).is_err());
    }

    struct FakeSource {
        descriptor: ConnectorDescriptor,
        batch: InboundBatch,
        committed_checkpoint: Option<String>,
        acked_checkpoint: Option<String>,
    }

    impl InboundConnector for FakeSource {
        fn descriptor(&self) -> &ConnectorDescriptor {
            &self.descriptor
        }

        fn poll(
            &mut self,
            checkpoint: &str,
            budget: PollBudget,
        ) -> Result<InboundBatch, ConnectorError> {
            self.batch
                .validate(checkpoint, budget)
                .map_err(|_| ConnectorError {
                    kind: ConnectorErrorKind::InvalidRequest,
                    code: ConnectorErrorCode::InvalidResponse,
                })?;
            Ok(self.batch.clone())
        }

        fn ack(&mut self, committed_checkpoint: &str) -> Result<(), ConnectorError> {
            if self.committed_checkpoint.as_deref() != Some(committed_checkpoint)
                || committed_checkpoint != self.batch.next_checkpoint
            {
                return Err(ConnectorError {
                    kind: ConnectorErrorKind::Rejected,
                    code: ConnectorErrorCode::CheckpointMismatch,
                });
            }
            self.acked_checkpoint = Some(committed_checkpoint.to_owned());
            Ok(())
        }

        fn reset(&mut self) -> Result<(), ConnectorError> {
            // Reset the transport; do not alter the fake source's durable state.
            Ok(())
        }
    }

    #[test]
    fn fake_source_models_commit_before_ack_and_idempotent_ack() {
        let mut source = FakeSource {
            descriptor: descriptor(ConnectorDirection::Inbound),
            batch: InboundBatch {
                next_checkpoint: "cursor-2".into(),
                records: vec![InboundRecord {
                    record_id: "remote-1".into(),
                    revision: 1,
                    observed_at_unix_ns: 10,
                    payload: json!({"kind":"usage","total_tokens":7}),
                }],
            },
            committed_checkpoint: None,
            acked_checkpoint: None,
        };
        assert_eq!(source.descriptor().direction, ConnectorDirection::Inbound);
        let batch = source.poll("cursor-1", budget()).unwrap();
        batch.validate("cursor-1", budget()).unwrap();

        // Model the host marking both rows and the checkpoint committed before ACK.
        let durable_checkpoint = batch.next_checkpoint.clone();
        let durable_records = batch.records.clone();
        assert_eq!(durable_checkpoint, "cursor-2");
        assert_eq!(durable_records.len(), 1);
        assert_eq!(
            source.ack(&durable_checkpoint).unwrap_err().code,
            ConnectorErrorCode::CheckpointMismatch
        );
        source.committed_checkpoint = Some(durable_checkpoint.clone());
        source.ack(&durable_checkpoint).unwrap();
        source.ack(&durable_checkpoint).unwrap();
        assert_eq!(
            source.ack("cursor-1").unwrap_err().code,
            ConnectorErrorCode::CheckpointMismatch
        );
        assert_eq!(source.acked_checkpoint.as_deref(), Some("cursor-2"));
    }

    struct FakeSink {
        descriptor: ConnectorDescriptor,
        accepted: HashMap<String, (String, String, String)>,
        lose_first_response: bool,
    }

    impl OutboundConnector for FakeSink {
        fn descriptor(&self) -> &ConnectorDescriptor {
            &self.descriptor
        }

        fn deliver(
            &mut self,
            delivery: &OutboundDelivery,
        ) -> Result<DeliveryReceipt, ConnectorError> {
            delivery.validate().map_err(|_| ConnectorError {
                kind: ConnectorErrorKind::InvalidRequest,
                code: ConnectorErrorCode::InvalidRequest,
            })?;
            if let Some((destination_id, digest, receipt_id)) =
                self.accepted.get(&delivery.idempotency_key)
            {
                if digest != &delivery.request_sha256 || destination_id != &delivery.destination_id
                {
                    return Err(ConnectorError {
                        kind: ConnectorErrorKind::Rejected,
                        code: ConnectorErrorCode::IdempotencyConflict,
                    });
                }
                return Ok(DeliveryReceipt {
                    destination_id: destination_id.clone(),
                    idempotency_key: delivery.idempotency_key.clone(),
                    request_sha256: digest.clone(),
                    receipt_id: receipt_id.clone(),
                });
            }
            let receipt_id = format!("receipt-{}", self.accepted.len() + 1);
            self.accepted.insert(
                delivery.idempotency_key.clone(),
                (
                    delivery.destination_id.clone(),
                    delivery.request_sha256.clone(),
                    receipt_id.clone(),
                ),
            );
            if self.lose_first_response {
                self.lose_first_response = false;
                return Err(ConnectorError {
                    kind: ConnectorErrorKind::OutcomeUnknown,
                    code: ConnectorErrorCode::Timeout,
                });
            }
            Ok(DeliveryReceipt {
                destination_id: delivery.destination_id.clone(),
                idempotency_key: delivery.idempotency_key.clone(),
                request_sha256: delivery.request_sha256.clone(),
                receipt_id,
            })
        }

        fn lookup(
            &mut self,
            delivery: &OutboundDelivery,
        ) -> Result<DeliveryLookup, ConnectorError> {
            let result = match self.accepted.get(&delivery.idempotency_key) {
                Some((destination_id, digest, receipt_id))
                    if destination_id == &delivery.destination_id
                        && digest == &delivery.request_sha256 =>
                {
                    DeliveryLookup::Accepted {
                        destination_id: destination_id.clone(),
                        idempotency_key: delivery.idempotency_key.clone(),
                        request_sha256: digest.clone(),
                        receipt_id: receipt_id.clone(),
                    }
                }
                Some(_) => {
                    return Err(ConnectorError {
                        kind: ConnectorErrorKind::Rejected,
                        code: ConnectorErrorCode::IdempotencyConflict,
                    });
                }
                None => DeliveryLookup::ConfirmedMissing {
                    destination_id: delivery.destination_id.clone(),
                    idempotency_key: delivery.idempotency_key.clone(),
                    request_sha256: delivery.request_sha256.clone(),
                },
            };
            result.validate_for(delivery).map_err(|_| ConnectorError {
                kind: ConnectorErrorKind::InvalidRequest,
                code: ConnectorErrorCode::InvalidResponse,
            })?;
            Ok(result)
        }
    }

    #[test]
    fn fake_sink_reconciles_accept_then_timeout_using_same_idempotency_key() {
        let payload = json!({"summary":"review requested"});
        let delivery = OutboundDelivery {
            delivery_id: "delivery-1".into(),
            idempotency_key: "agenturm:delivery-1".into(),
            destination_id: "team:product".into(),
            action: "review.request".into(),
            request_sha256: request_sha256("team:product", "review.request", &payload).unwrap(),
            payload,
        };
        let mut sink = FakeSink {
            descriptor: descriptor(ConnectorDirection::Outbound),
            accepted: HashMap::new(),
            lose_first_response: true,
        };
        assert_eq!(sink.descriptor().direction, ConnectorDirection::Outbound);
        let timeout = sink.deliver(&delivery).unwrap_err();
        assert_eq!(timeout.kind, ConnectorErrorKind::OutcomeUnknown);
        assert_eq!(
            sink.lookup(&delivery).unwrap(),
            DeliveryLookup::Accepted {
                destination_id: delivery.destination_id.clone(),
                idempotency_key: delivery.idempotency_key.clone(),
                request_sha256: delivery.request_sha256.clone(),
                receipt_id: "receipt-1".into()
            }
        );
        let receipt = sink.deliver(&delivery).unwrap();
        assert_eq!(receipt.receipt_id, "receipt-1");
        assert!(receipt.validate().is_ok());

        let changed = OutboundDelivery {
            action: "review.cancel".into(),
            request_sha256: request_sha256(
                &delivery.destination_id,
                "review.cancel",
                &delivery.payload,
            )
            .unwrap(),
            ..delivery
        };
        assert_eq!(
            sink.deliver(&changed).unwrap_err().code,
            ConnectorErrorCode::IdempotencyConflict
        );
    }
}
