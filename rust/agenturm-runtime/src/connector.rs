//! Trusted connector supervision, scoped grants, and a durable outbound queue.

use agenturm_connector_api::{
    CONNECTOR_API_VERSION, ConnectorDescriptor, ConnectorDirection, ConnectorError, ConnectorId,
    DeliveryLookup, DeliveryReceipt, InboundBatch, OutboundDelivery, PollBudget,
};
use agenturm_kernel::{
    ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationStore,
    is_lifecycle_state,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const APPLICATION_ID: i64 = 0x4147434e;
const MAX_ATTEMPTS: i64 = 4;
const MAX_LOOKUPS: i64 = 8;
const MAX_WATCH_BATCH: usize = 8;
const BUDGET: PollBudget = PollBudget {
    max_records: 128,
    max_bytes: 256 * 1024,
};

/// Selects the process launch policy included in the connector grant binding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorLaunch {
    #[default]
    Trusted,
    IsolatedLinux,
}

impl ConnectorLaunch {
    fn is_trusted(&self) -> bool {
        *self == Self::Trusted
    }
}

/// Explicit executable selection. No endpoint or credential is acquired from
/// the host's environment.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorSpec {
    pub descriptor: ConnectorDescriptor,
    pub executable: PathBuf,
    pub sha256: String,
    /// Inbound registered source ID or outbound destination ID.
    pub target: String,
    #[serde(default, skip_serializing_if = "ConnectorLaunch::is_trusted")]
    pub launch: ConnectorLaunch,
}

impl ConnectorSpec {
    fn binding(&self, direction: ConnectorDirection) -> Result<String, &'static str> {
        if self.descriptor.validate().is_err()
            || self.descriptor.direction != direction
            || ConnectorId::parse(&self.target).is_err()
            || !self.executable.is_absolute()
            || self.executable.as_os_str().len() > 4096
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("connector_invalid_config");
        }
        let bytes = serde_json::to_vec(self).map_err(|_| "connector_invalid_config")?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    fn key(&self) -> &str {
        self.descriptor.id.as_str()
    }
}

/// One correlation envelope for one process lifetime.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorRequest {
    pub protocol: u16,
    pub descriptor: ConnectorDescriptor,
    pub target: String,
    pub request_id: String,
    pub operation: Operation,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Poll {
        checkpoint: String,
        budget: PollBudget,
    },
    Ack {
        checkpoint: String,
    },
    Deliver {
        delivery: OutboundDelivery,
    },
    Lookup {
        delivery: OutboundDelivery,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorResponse {
    pub protocol: u16,
    pub descriptor: ConnectorDescriptor,
    pub target: String,
    pub request_id: String,
    pub outcome: Outcome,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    Batch { batch: InboundBatch },
    Ack { checkpoint: String },
    Delivered { receipt: DeliveryReceipt },
    Lookup { result: DeliveryLookup },
    Error { error: ConnectorError },
}

fn exchange(spec: &ConnectorSpec, operation: Operation) -> Result<Outcome, &'static str> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let request = ConnectorRequest {
        protocol: CONNECTOR_API_VERSION,
        descriptor: spec.descriptor.clone(),
        target: spec.target.clone(),
        request_id: format!(
            "{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ),
        operation,
    };
    let payload = serde_json::to_vec(&request).map_err(|_| "connector_protocol_error")?;
    let bytes = super::observation_extension::invoke_connector(
        &spec.executable,
        &spec.sha256,
        spec.launch,
        &payload,
    )?;
    let reply: ConnectorResponse =
        serde_json::from_slice(&bytes).map_err(|_| "connector_protocol_error")?;
    if reply.protocol != request.protocol
        || reply.descriptor != request.descriptor
        || reply.target != request.target
        || reply.request_id != request.request_id
    {
        return Err("connector_protocol_error");
    }
    if matches!(reply.outcome, Outcome::Error { .. }) {
        return Err("connector_reported_error");
    }
    Ok(reply.outcome)
}

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum DeliveryState {
    Pending,
    Waiting,
    Accepted { receipt_id: String },
    Unknown,
    DeadLetter,
}

pub struct ConnectorRuntime {
    db: Connection,
}

impl ConnectorRuntime {
    pub fn open(path: &Path) -> Result<Self, &'static str> {
        let mut db = Connection::open(path).map_err(store)?;
        let tx = begin(&mut db)?;
        let id: i64 = tx
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(store)?;
        let version: i64 = tx
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(store)?;
        if id == 0 && version == 0 {
            let occupied: bool = tx
                .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema)", [], |row| {
                    row.get(0)
                })
                .map_err(store)?;
            if occupied {
                return Err("connector_unrecognized_store");
            }
            tx.execute_batch(SCHEMA).map_err(store)?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)
                .map_err(store)?;
            tx.pragma_update(None, "user_version", 1).map_err(store)?;
        } else if id != APPLICATION_ID || version != 1 {
            return Err("connector_unrecognized_store");
        }
        tx.commit().map_err(store)?;
        Ok(Self { db })
    }

    pub fn grant(&mut self, spec: &ConnectorSpec) -> Result<(), &'static str> {
        let binding = spec.binding(spec.descriptor.direction)?;
        super::observation_extension::verify_connector_artifact(
            &spec.executable,
            &spec.sha256,
            spec.launch,
        )?;
        self.db.execute(
            "INSERT INTO connector_grants VALUES (?1, ?2, 1, 1)
             ON CONFLICT(id) DO UPDATE SET
                 epoch = CASE WHEN enabled = 1 AND binding = excluded.binding THEN epoch ELSE epoch + 1 END,
                 binding = excluded.binding, enabled = 1",
            params![spec.key(), binding],
        ).map_err(store)?;
        Ok(())
    }

    pub fn revoke(&mut self, spec: &ConnectorSpec) -> Result<bool, &'static str> {
        let binding = spec.binding(spec.descriptor.direction)?;
        let changed = self.db.execute(
            "UPDATE connector_grants SET enabled = 0 WHERE id = ?1 AND binding = ?2 AND enabled = 1",
            params![spec.key(), binding],
        ).map_err(store)?;
        Ok(changed != 0)
    }

    /// Commits projected observations with the cursor before ACK. Re-sends
    /// the previous durable ACK before polling to cover a crash after commit.
    pub fn collect(
        &mut self,
        spec: &ConnectorSpec,
        history: &mut dyn ObservationStore,
    ) -> Result<usize, &'static str> {
        let binding = spec.binding(ConnectorDirection::Inbound)?;
        let source = ExtensionId::parse(&spec.target).map_err(|_| "connector_invalid_config")?;
        let tx = begin(&mut self.db)?;
        epoch(&tx, spec, &binding)?;
        let cursor = history
            .source_cursor(source.as_str())
            .map_err(|_| "connector_history_error")?;
        if let Some(cursor) = &cursor {
            ack(spec, cursor)?;
        }
        let checkpoint = cursor.as_deref().unwrap_or_default();
        let Outcome::Batch { batch } = exchange(
            spec,
            Operation::Poll {
                checkpoint: checkpoint.to_owned(),
                budget: BUDGET,
            },
        )?
        else {
            return Err("connector_protocol_error");
        };
        batch
            .validate(checkpoint, BUDGET)
            .map_err(|_| "connector_protocol_error")?;
        let observations = batch
            .records
            .into_iter()
            .map(|record| {
                let payload: InboundLifecycle = serde_json::from_value(record.payload)
                    .map_err(|_| "connector_record_denied")?;
                ConnectorId::parse(&payload.session_id).map_err(|_| "connector_record_denied")?;
                if !is_lifecycle_state(&payload.state)
                    || record.observed_at_unix_ns > i64::MAX as u64
                {
                    return Err("connector_record_denied");
                }
                Ok(Observation {
                    harness_id: source.clone(),
                    event_id: record.record_id,
                    revision: record.revision,
                    observed_at_unix_ns: record.observed_at_unix_ns,
                    identity: ObservationIdentity::session(payload.session_id),
                    kind: ObservationKind::Lifecycle {
                        state: payload.state,
                        reason_code: None,
                    },
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if observations.is_empty() && batch.next_checkpoint == checkpoint {
            return Ok(0);
        }
        let admission = history
            .commit(
                source.as_str(),
                cursor.as_deref(),
                &batch.next_checkpoint,
                &observations,
            )
            .map_err(|_| "connector_history_error")?;
        ack(spec, &batch.next_checkpoint)?;
        Ok(admission.inserted)
    }

    pub fn enqueue(
        &mut self,
        spec: &ConnectorSpec,
        delivery: &OutboundDelivery,
    ) -> Result<(), &'static str> {
        let binding = spec.binding(ConnectorDirection::Outbound)?;
        validate_delivery(spec, delivery)?;
        let tx = begin(&mut self.db)?;
        let grant_epoch = epoch(&tx, spec, &binding)?;
        let data = serde_json::to_string(delivery).map_err(|_| "connector_invalid_delivery")?;
        let inserted = tx
            .execute(
                "INSERT INTO connector_deliveries
             (connector, id, destination, key, digest, epoch, data, state, attempts, lookups, due)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', 0, 0, 0)
             ON CONFLICT DO NOTHING",
                params![
                    spec.key(),
                    delivery.delivery_id,
                    delivery.destination_id,
                    delivery.idempotency_key,
                    delivery.request_sha256,
                    grant_epoch,
                    data
                ],
            )
            .map_err(store)?;
        if inserted == 0 {
            let existing: Option<(String, i64)> = tx
                .query_row(
                    "SELECT data, epoch FROM connector_deliveries WHERE connector = ?1 AND id = ?2",
                    params![spec.key(), delivery.delivery_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(store)?;
            if existing.as_ref() != Some(&(data, grant_epoch)) {
                return Err("connector_idempotency_conflict");
            }
        }
        tx.commit().map_err(store)
    }

    /// Checks a current grant before starting an unattended worker.
    pub fn check_grant(&self, spec: &ConnectorSpec) -> Result<(), &'static str> {
        let binding = spec.binding(spec.descriptor.direction)?;
        epoch(&self.db, spec, &binding).map(|_| ())
    }

    /// Returns a bounded next batch of actionable deliveries. Dispatch
    /// rechecks both grant epoch and the persisted due time for each ID.
    pub fn due_deliveries(
        &self,
        spec: &ConnectorSpec,
        now: i64,
    ) -> Result<Vec<String>, &'static str> {
        if !(0..i64::MAX - 60_000).contains(&now) {
            return Err("connector_clock_error");
        }
        let binding = spec.binding(ConnectorDirection::Outbound)?;
        let grant_epoch = epoch(&self.db, spec, &binding)?;
        let mut query = self
            .db
            .prepare(
                "SELECT id FROM connector_deliveries
             WHERE connector = ?1 AND epoch = ?2 AND state IN ('pending', 'unknown')
               AND due IS NOT NULL AND due <= ?3
             ORDER BY due, id LIMIT ?4",
            )
            .map_err(store)?;
        query
            .query_map(
                params![spec.key(), grant_epoch, now, MAX_WATCH_BATCH],
                |row| row.get(0),
            )
            .map_err(store)?
            .collect::<Result<_, _>>()
            .map_err(store)
    }

    /// Advances one persisted delivery. A caller can schedule subsequent ticks.
    /// The clock is time in Unix milliseconds, supplied by the host.
    pub fn drive(
        &mut self,
        spec: &ConnectorSpec,
        id: &str,
        now: i64,
    ) -> Result<DeliveryState, &'static str> {
        if !(0..i64::MAX - 60_000).contains(&now) || ConnectorId::parse(id).is_err() {
            return Err("connector_invalid_delivery");
        }
        let binding = spec.binding(ConnectorDirection::Outbound)?;
        let tx = begin(&mut self.db)?;
        let grant_epoch = epoch(&tx, spec, &binding)?;
        let queued = queued(&tx, spec, id, grant_epoch)?;
        validate_delivery(spec, &queued.delivery)?;
        match queued.state.as_str() {
            "accepted" => {
                return queued
                    .receipt
                    .map(|receipt_id| DeliveryState::Accepted { receipt_id })
                    .ok_or("connector_store_error");
            }
            "dead" => return Ok(DeliveryState::DeadLetter),
            "unknown" if queued.due.is_none() => return Ok(DeliveryState::Unknown),
            _ if queued.due.is_some_and(|due| now < due) => return Ok(DeliveryState::Waiting),
            "unknown" => {
                let lookup = exchange(
                    spec,
                    Operation::Lookup {
                        delivery: queued.delivery.clone(),
                    },
                );
                let state = match lookup {
                    Ok(Outcome::Lookup { result })
                        if result.validate_for(&queued.delivery).is_ok() =>
                    {
                        match result {
                            DeliveryLookup::Accepted { receipt_id, .. } => {
                                accept(&tx, spec, id, receipt_id)?
                            }
                            DeliveryLookup::ConfirmedMissing { .. }
                                if queued.attempts >= MAX_ATTEMPTS =>
                            {
                                persist(&tx, spec, id, "dead", None, None)?;
                                DeliveryState::DeadLetter
                            }
                            DeliveryLookup::ConfirmedMissing { .. } => {
                                persist(
                                    &tx,
                                    spec,
                                    id,
                                    "pending",
                                    Some(now + backoff(queued.attempts)),
                                    None,
                                )?;
                                DeliveryState::Pending
                            }
                            DeliveryLookup::Unknown { .. } => {
                                unresolved(&tx, spec, id, now, queued.lookups)?
                            }
                        }
                    }
                    _ => unresolved(&tx, spec, id, now, queued.lookups)?,
                };
                tx.commit().map_err(store)?;
                return Ok(state);
            }
            "pending" => {}
            _ => return Err("connector_store_error"),
        }

        // Persist the ambiguous state ahead of the external effect. A crash at
        // any point after this commit starts with lookup, never direct retry.
        tx.execute(
            "UPDATE connector_deliveries SET state = 'unknown', attempts = attempts + 1, lookups = 0, due = ?3
             WHERE connector = ?1 AND id = ?2",
            params![spec.key(), id, now + backoff(queued.attempts + 1)],
        ).map_err(store)?;
        tx.commit().map_err(store)?;
        let tx = begin(&mut self.db)?;
        if epoch(&tx, spec, &binding)? != grant_epoch {
            return Err("connector_permission_denied");
        }
        let still_claimed: bool = tx
            .query_row(
                "SELECT state = 'unknown' AND attempts = ?3 AND lookups = 0 AND due = ?4
             FROM connector_deliveries WHERE connector = ?1 AND id = ?2",
                params![
                    spec.key(),
                    id,
                    queued.attempts + 1,
                    now + backoff(queued.attempts + 1)
                ],
                |row| row.get(0),
            )
            .map_err(store)?;
        if !still_claimed {
            return Ok(DeliveryState::Waiting);
        }
        let sent = exchange(
            spec,
            Operation::Deliver {
                delivery: queued.delivery.clone(),
            },
        );
        let state = match sent {
            Ok(Outcome::Delivered { receipt })
                if receipt.validate_for(&queued.delivery).is_ok() =>
            {
                accept(&tx, spec, id, receipt.receipt_id)?
            }
            _ => DeliveryState::Unknown,
        };
        tx.commit().map_err(store)?;
        Ok(state)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundLifecycle {
    session_id: String,
    state: String,
}

fn validate_delivery(
    spec: &ConnectorSpec,
    delivery: &OutboundDelivery,
) -> Result<(), &'static str> {
    if delivery.destination_id != spec.target
        || delivery.action != "write_status"
        || delivery.validate().is_err()
        || !delivery.payload.as_str().is_some_and(is_lifecycle_state)
    {
        return Err("connector_delivery_denied");
    }
    Ok(())
}

fn ack(spec: &ConnectorSpec, committed: &str) -> Result<(), &'static str> {
    match exchange(
        spec,
        Operation::Ack {
            checkpoint: committed.to_owned(),
        },
    )? {
        Outcome::Ack { checkpoint } if checkpoint == committed => Ok(()),
        _ => Err("connector_protocol_error"),
    }
}

fn begin(db: &mut Connection) -> Result<Transaction<'_>, &'static str> {
    db.transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store)
}

fn store(_: rusqlite::Error) -> &'static str {
    "connector_store_error"
}

fn epoch(db: &Connection, spec: &ConnectorSpec, binding: &str) -> Result<i64, &'static str> {
    db.query_row(
        "SELECT epoch FROM connector_grants WHERE id = ?1 AND binding = ?2 AND enabled = 1",
        params![spec.key(), binding],
        |row| row.get(0),
    )
    .optional()
    .map_err(store)?
    .ok_or("connector_permission_denied")
}

struct Queued {
    delivery: OutboundDelivery,
    state: String,
    attempts: i64,
    lookups: i64,
    due: Option<i64>,
    receipt: Option<String>,
}

fn queued(
    db: &Connection,
    spec: &ConnectorSpec,
    id: &str,
    epoch: i64,
) -> Result<Queued, &'static str> {
    let (data, state, attempts, lookups, due, receipt) = db
        .query_row(
            "SELECT data, state, attempts, lookups, due, receipt
         FROM connector_deliveries WHERE connector = ?1 AND id = ?2 AND epoch = ?3",
            params![spec.key(), id, epoch],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(store)?
        .ok_or("connector_delivery_unavailable")?;
    let delivery = serde_json::from_str(&data).map_err(|_| "connector_store_error")?;
    Ok(Queued {
        delivery,
        state,
        attempts,
        lookups,
        due,
        receipt,
    })
}

fn backoff(attempts: i64) -> i64 {
    1000 << attempts.clamp(0, 5)
}

fn persist(
    db: &Connection,
    spec: &ConnectorSpec,
    id: &str,
    state: &str,
    due: Option<i64>,
    receipt: Option<&str>,
) -> Result<(), &'static str> {
    db.execute(
        "UPDATE connector_deliveries SET state = ?3, due = ?4, receipt = ?5 WHERE connector = ?1 AND id = ?2",
        params![spec.key(), id, state, due, receipt],
    ).map_err(store)?;
    Ok(())
}

fn accept(
    db: &Connection,
    spec: &ConnectorSpec,
    id: &str,
    receipt_id: String,
) -> Result<DeliveryState, &'static str> {
    persist(db, spec, id, "accepted", None, Some(&receipt_id))?;
    Ok(DeliveryState::Accepted { receipt_id })
}

fn unresolved(
    db: &Connection,
    spec: &ConnectorSpec,
    id: &str,
    now: i64,
    lookups: i64,
) -> Result<DeliveryState, &'static str> {
    let next = lookups + 1;
    let due = (next < MAX_LOOKUPS).then(|| now + backoff(next));
    db.execute(
        "UPDATE connector_deliveries SET lookups = ?3, due = ?4 WHERE connector = ?1 AND id = ?2",
        params![spec.key(), id, next, due],
    )
    .map_err(store)?;
    Ok(DeliveryState::Unknown)
}

const SCHEMA: &str = "
    CREATE TABLE connector_grants (
        id TEXT PRIMARY KEY, binding TEXT NOT NULL, epoch INTEGER NOT NULL CHECK(epoch > 0),
        enabled INTEGER NOT NULL CHECK(enabled IN (0, 1))
    );
    CREATE TABLE connector_deliveries (
        connector TEXT NOT NULL, id TEXT NOT NULL, destination TEXT NOT NULL, key TEXT NOT NULL,
        digest TEXT NOT NULL, epoch INTEGER NOT NULL, data TEXT NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('pending', 'unknown', 'accepted', 'dead')),
        attempts INTEGER NOT NULL, lookups INTEGER NOT NULL, due INTEGER, receipt TEXT,
        PRIMARY KEY(connector, id), UNIQUE(destination, key)
    );
    CREATE INDEX connector_deliveries_due
        ON connector_deliveries (connector, epoch, due, id)
        WHERE state IN ('pending', 'unknown') AND due IS NOT NULL;
";
