//! Explicit observation retention. Receipts and source checkpoints outlive
//! their observation rows so exact replay and restart remain deterministic.

use crate::{
    ObservationKind, Result, SCHEMA_VERSION, Store, StoreError, StoredObservation, StoredScope,
};
use agenturm_kernel::{ReadIncarnation, ReadSnapshot};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Keep this predicate aligned with `observation_age_time`. `request_id`
/// identifies the batch that first admitted the observation revision.
const PRUNABLE_OBSERVATION: &str = "observed_at_unix_ns < ?1 AND NOT EXISTS (
    SELECT 1 FROM agenturm_history_batches AS batch
    WHERE batch.request_id = agenturm_observations.request_id
      AND batch.admitted_at_unix_ns >= ?1
      AND agenturm_observations.observed_at_unix_ns = 0
      AND agenturm_observations.revision = 0
      AND agenturm_observations.scope = 'session'
      AND agenturm_observations.turn_id IS NULL
      AND agenturm_observations.operation_id IS NULL
      AND json_extract(agenturm_observations.kind_json, '$.kind') = 'activity'
      AND json_extract(agenturm_observations.kind_json, '$.category') = 'session_discovered'
)";

pub(super) fn batch_admission_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|time| i64::try_from(time.as_nanos()).ok())
        .unwrap_or(0)
}

pub(super) fn observation_age_time(
    observation: &StoredObservation,
    observed_at: i64,
    admitted_at: i64,
) -> i64 {
    // Only neutral session discovery can age by admission time when its
    // native timestamp is unknown. Its reported timestamp stays unchanged.
    if observed_at == 0
        && observation.revision == 0
        && observation.identity.scope == StoredScope::Session
        && observation.identity.turn_id.is_none()
        && observation.identity.operation_id.is_none()
        && matches!(&observation.kind, ObservationKind::Activity { category } if category == "session_discovered")
    {
        admitted_at
    } else {
        observed_at
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionOutcome {
    pub cutoff_unix_ns: u64,
    pub removed_observations: usize,
    pub minimum_snapshot_id: ReadSnapshot,
}

#[derive(Default)]
pub(super) struct RetentionState {
    pub cutoff_unix_ns: i64,
    pub snapshot_offset: i64,
    pub minimum_snapshot_id: i64,
}

pub(super) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE agenturm_retention_state (
            singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
            cutoff_unix_ns INTEGER NOT NULL CHECK (cutoff_unix_ns >= 0),
            snapshot_offset INTEGER NOT NULL CHECK (snapshot_offset >= 0),
            minimum_snapshot_id INTEGER NOT NULL CHECK (minimum_snapshot_id >= snapshot_offset)
        );
        INSERT INTO agenturm_retention_state VALUES (1, 0, 0, 0);
        CREATE TABLE agenturm_retired_observations (
            source_id TEXT NOT NULL,
            event_id TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK (revision >= 0),
            event_digest TEXT NOT NULL,
            PRIMARY KEY (source_id, event_id, revision)
        );",
    )?;
    Ok(())
}

pub(super) fn read_state(connection: &Connection) -> Result<RetentionState> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        8 | 9 => Ok(RetentionState::default()),
        10..=SCHEMA_VERSION => connection
            .query_row(
                "SELECT cutoff_unix_ns, snapshot_offset, minimum_snapshot_id
                 FROM agenturm_retention_state WHERE singleton = 1",
                [],
                |row| {
                    Ok(RetentionState {
                        cutoff_unix_ns: row.get(0)?,
                        snapshot_offset: row.get(1)?,
                        minimum_snapshot_id: row.get(2)?,
                    })
                },
            )
            .map_err(Into::into),
        _ => Err(StoreError::UnsupportedSchema(version)),
    }
}

pub(super) fn receipt_head(connection: &Connection) -> Result<i64> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let sql = if version >= 17 {
        "SELECT max(
            COALESCE((SELECT MAX(sequence) FROM agenturm_history_batches), 0),
            COALESCE((SELECT MAX(sequence) FROM agenturm_eval_detail_admissions), 0)
        )"
    } else {
        "SELECT COALESCE(MAX(sequence), 0) FROM agenturm_history_batches"
    };
    connection
        .query_row(sql, [], |row| row.get(0))
        .map_err(Into::into)
}

/// Allocates a watermark for either domain while holding the history writer transaction.
pub(super) fn next_receipt_sequence(connection: &Connection) -> Result<i64> {
    let next = receipt_head(connection)?
        .checked_add(1)
        .ok_or(StoreError::Invalid("history snapshot ID is exhausted"))?;
    snapshot_id(next, read_state(connection)?.snapshot_offset)?;
    Ok(next)
}

pub(super) fn snapshot_id(receipt_sequence: i64, offset: i64) -> Result<i64> {
    receipt_sequence
        .checked_add(offset)
        .ok_or(StoreError::Invalid("history snapshot ID is exhausted"))
}

pub(super) fn read_incarnation(connection: &Connection) -> Result<Option<ReadIncarnation>> {
    connection
        .query_row(
            "SELECT CASE WHEN typeof(value) = 'text' AND length(CAST(value AS BLOB)) = 32
                      THEN value ELSE '' END
             FROM agenturm_store_meta WHERE key = 'read_incarnation'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|value| value.parse().map_err(StoreError::Invalid))
        .transpose()
}

pub(super) fn renew_read_incarnation(connection: &Connection) -> Result<()> {
    connection.execute(
        "INSERT INTO agenturm_store_meta(key, value)
         VALUES ('read_incarnation', lower(hex(randomblob(16))))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [],
    )?;
    set_execution_clock_floor(connection, receipt_head(connection)?)
}

pub(super) fn execution_clock_floor(connection: &Connection) -> Result<Option<i64>> {
    if read_incarnation(connection)?.is_none() {
        return Ok(Some(0));
    }
    connection
        .query_row(
            "SELECT value FROM agenturm_store_meta WHERE key='execution_clock_floor'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|value| {
            value
                .parse::<i64>()
                .ok()
                .filter(|floor| *floor >= 0)
                .ok_or(StoreError::Invalid("invalid execution clock floor"))
        })
        .transpose()
}

pub(super) fn set_execution_clock_floor(connection: &Connection, floor: i64) -> Result<()> {
    connection.execute(
        "INSERT INTO agenturm_store_meta(key,value) VALUES ('execution_clock_floor',?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [floor.to_string()],
    )?;
    Ok(())
}

/// Resolves a read snapshot inside the transaction that reads the result rows.
pub(super) fn resolve_read_snapshot(
    connection: &Connection,
    requested: Option<ReadSnapshot>,
) -> Result<(ReadSnapshot, i64)> {
    let incarnation = read_incarnation(connection)?;
    if requested.is_some_and(|id| id.incarnation() != incarnation) {
        return Err(StoreError::SnapshotReplaced);
    }
    let state = read_state(connection)?;
    let newest = snapshot_id(receipt_head(connection)?, state.snapshot_offset)?;
    let selected = requested
        .map(|snapshot| i64::try_from(snapshot.watermark()))
        .transpose()
        .map_err(|_| StoreError::Invalid("query snapshot ID is invalid"))?
        .unwrap_or(newest);
    if selected > newest {
        return Err(StoreError::Invalid("query snapshot ID is from the future"));
    }
    if selected < state.minimum_snapshot_id {
        return Err(StoreError::ExpiredHistorySnapshot {
            minimum_snapshot_id: ReadSnapshot::new(state.minimum_snapshot_id as u64, incarnation),
        });
    }
    Ok((
        ReadSnapshot::new(selected as u64, incarnation),
        selected - state.snapshot_offset,
    ))
}

impl Store {
    /// Checks the exact retained revision against the snapshot's receipt
    /// watermark. Call within the same fence as the disclosure it guards.
    pub fn contains_observation_at_snapshot(
        &self,
        source: &agenturm_kernel::ExtensionId,
        event_id: &str,
        revision: u32,
        snapshot_id: ReadSnapshot,
    ) -> Result<bool> {
        crate::validate_identifier(event_id, crate::MAX_EVENT_ID_BYTES, "event ID")?;
        let (_, watermark) = resolve_read_snapshot(&self.connection, Some(snapshot_id))?;
        Ok(self.connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM agenturm_observations AS observation
                JOIN agenturm_history_batches AS batch USING (request_id)
                WHERE observation.source_id = ?1 AND observation.event_id = ?2
                  AND observation.revision = ?3 AND batch.sequence <= ?4
            )",
            params![source.as_str(), event_id, revision, watermark],
            |row| row.get(0),
        )?)
    }

    /// Atomically removes observations strictly older than the supplied cutoff.
    /// Unknown-time neutral session discovery ages from its first admission.
    /// Equal cutoffs are idempotent and lowering the cutoff is an error.
    /// Later batches still advance their checkpoint when an observation falls
    /// below the cutoff. The store retains each expired event's source, ID,
    /// revision and digest to preserve conflicts and replay semantics. Request
    /// receipts are retained in full. Raising the cutoff expires earlier
    /// snapshot IDs, including if no observations fall below the new cutoff.
    /// On Linux the caller must open a persistent writable store.
    /// This is logical cleanup of the live store. Existing backups and SQLite
    /// file space can still contain the prior data.
    pub fn prune_observations_before(&mut self, cutoff_unix_ns: u64) -> Result<RetentionOutcome> {
        #[cfg(target_os = "linux")]
        if self.writer_lease.is_none() {
            return Err(StoreError::WriterLease(std::io::Error::other(
                "retention requires a writer lease",
            )));
        }
        self.verify_write_admission()?;

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let outcome = prune_in(&transaction, cutoff_unix_ns)?;
        transaction.commit()?;
        Ok(outcome)
    }
}

/// Caller owns an immediate transaction and has verified the writer lease.
pub(super) fn prune_in(
    transaction: &Transaction<'_>,
    cutoff_unix_ns: u64,
) -> Result<RetentionOutcome> {
    let cutoff = i64::try_from(cutoff_unix_ns)
        .map_err(|_| StoreError::Invalid("retention cutoff exceeds SQLite INTEGER"))?;
    let state = read_state(transaction)?;
    let incarnation = read_incarnation(transaction)?;
    if cutoff < state.cutoff_unix_ns {
        return Err(StoreError::Invalid("retention cutoff cannot decrease"));
    }
    if cutoff == state.cutoff_unix_ns {
        return Ok(RetentionOutcome {
            cutoff_unix_ns,
            removed_observations: 0,
            minimum_snapshot_id: ReadSnapshot::new(state.minimum_snapshot_id as u64, incarnation),
        });
    }

    let offset = snapshot_id(state.snapshot_offset, 1)?;
    let barrier = snapshot_id(receipt_head(transaction)?, offset)?;
    transaction.execute(
        &format!("INSERT INTO agenturm_retired_observations (source_id, event_id, revision, event_digest)
         SELECT source_id, event_id, revision, event_digest FROM agenturm_observations
         WHERE {PRUNABLE_OBSERVATION}"),
        [cutoff],
    )?;
    crate::telemetry_payload::purge_before(transaction, cutoff)?;
    crate::native_attention::prune_before(transaction, cutoff)?;
    let removed_observations = transaction.execute(
        &format!("DELETE FROM agenturm_observations WHERE {PRUNABLE_OBSERVATION}"),
        [cutoff],
    )?;
    if removed_observations > 0 {
        crate::eval_outcome::purge_unretained(transaction)?;
        crate::eval_detail::purge_unretained(transaction)?;
        transaction.execute(
            "DELETE FROM agenturm_content_cursor_proofs AS proof
             WHERE proof.request_id NOT IN (
                SELECT request_id FROM agenturm_observations
                WHERE (json_extract(kind_json, '$.kind') = 'message'
                       AND json_extract(kind_json, '$.role') = 'user')
                   OR json_extract(kind_json, '$.kind') IN ('mcp_call', 'file_edit', 'file_change')
                   OR (json_extract(kind_json, '$.kind') = 'tool'
                       AND json_extract(kind_json, '$.category') = 'shell')
             )",
            [],
        )?;
    }
    transaction.execute(
        "UPDATE agenturm_retention_state SET cutoff_unix_ns = ?1,
                snapshot_offset = ?2, minimum_snapshot_id = ?3 WHERE singleton = 1",
        params![cutoff, offset, barrier],
    )?;
    Ok(RetentionOutcome {
        cutoff_unix_ns,
        removed_observations,
        minimum_snapshot_id: ReadSnapshot::new(barrier as u64, incarnation),
    })
}
