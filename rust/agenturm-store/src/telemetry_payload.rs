//! Private text captured by an OTLP producer and admitted with its exact span.

use crate::{
    ContentGrantScope, ContentKind, ObservationBatch, ObservationKind, Result, Store, StoreError,
    bounded_text_preview, retention,
};
use agenturm_kernel::{ExtensionId, ReadSnapshot};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt};

pub const MAX_TELEMETRY_PAYLOAD_TEXT_BYTES: usize = 2_048;
pub const MAX_TELEMETRY_PAYLOAD_BATCH_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryBinding {
    pub source_id: ExtensionId,
    pub native_source_id: Option<ExtensionId>,
}

impl TelemetryBinding {
    pub fn grant_scope(&self) -> Result<ContentGrantScope> {
        if !self.source_id.as_str().starts_with("otlp.")
            || self
                .native_source_id
                .as_ref()
                .is_some_and(|source| source.as_str().starts_with("otlp."))
        {
            return Err(StoreError::Invalid(
                "invalid telemetry payload source binding",
            ));
        }
        let mut hash = Sha256::new();
        hash.update(b"agenturm-telemetry-payload-binding/1\0");
        hash.update(self.source_id.as_str());
        hash.update(b"\0");
        if let Some(source) = &self.native_source_id {
            hash.update(source.as_str());
        }
        Ok(ContentGrantScope::new(
            self.source_id.clone(),
            ContentKind::TelemetrySpanPayload,
            hash.finalize().into(),
        ))
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

impl fmt::Debug for TelemetryPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TelemetryPayload")
            .finish_non_exhaustive()
    }
}

impl TelemetryPayload {
    pub fn validate(&self) -> Result<usize> {
        let mut bytes = 0;
        for text in [self.input.as_ref(), self.output.as_ref()]
            .into_iter()
            .flatten()
        {
            if text.len() > MAX_TELEMETRY_PAYLOAD_TEXT_BYTES
                || !bounded_text_preview(text.clone(), MAX_TELEMETRY_PAYLOAD_TEXT_BYTES)
                    .is_ok_and(|clean| clean == *text)
            {
                return Err(StoreError::Invalid("telemetry payload text was withheld"));
            }
            bytes += text.len();
        }
        if bytes == 0 {
            return Err(StoreError::Invalid("telemetry payload contains no text"));
        }
        Ok(bytes)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryPayloadRecord {
    pub event_id: String,
    pub payload: TelemetryPayload,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryPayloadSelection {
    pub history_identity: String,
    pub binding: TelemetryBinding,
    pub trace_id: String,
    pub span_id: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub worker_id: Option<String>,
    pub snapshot_id: ReadSnapshot,
}

pub(super) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE agenturm_telemetry_payloads (
            source_id TEXT NOT NULL,
            event_id TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK (revision = 0),
            request_id TEXT NOT NULL REFERENCES agenturm_ingest_receipts(request_id)
                DEFERRABLE INITIALLY DEFERRED,
            payload_digest BLOB NOT NULL CHECK (length(payload_digest) = 32),
            payload_json TEXT CHECK (length(CAST(payload_json AS BLOB)) <= 24576),
            PRIMARY KEY (source_id, event_id, revision)
        ) WITHOUT ROWID;",
    )?;
    Ok(())
}

pub(super) fn build_admission<'a>(
    batch: &ObservationBatch,
    binding: &TelemetryBinding,
    records: &'a [TelemetryPayloadRecord],
) -> Result<BTreeMap<&'a str, EncodedPayload>> {
    binding.grant_scope()?;
    if binding.source_id.as_str() != batch.source_id || records.len() > batch.observations.len() {
        return Err(StoreError::Invalid(
            "telemetry payload admission has an invalid source or count",
        ));
    }
    let spans = batch
        .observations
        .iter()
        .filter_map(|row| match &row.kind {
            ObservationKind::TraceSpan(span)
                if row.revision == 0
                    && row.identity.operation_id.is_none()
                    && row.event_id == format!("otlp:{}:{}", span.trace_id, span.span_id)
                    && span.target_source_id.as_deref()
                        == binding.native_source_id.as_ref().map(ExtensionId::as_str) =>
            {
                Some(row.event_id.as_str())
            }
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    let mut payloads = BTreeMap::new();
    let mut bytes = 0;
    for record in records {
        bytes += record.payload.validate()?;
        if bytes > MAX_TELEMETRY_PAYLOAD_BATCH_BYTES || !spans.contains(record.event_id.as_str()) {
            return Err(StoreError::Invalid(
                "telemetry payload exceeds the batch scope or size",
            ));
        }
        let payload_json = serde_json::to_string(&record.payload)?;
        let mut hash = Sha256::new();
        hash.update(b"agenturm-telemetry-payload/1\0");
        hash.update(payload_json.as_bytes());
        let payload = EncodedPayload {
            digest: hash.finalize().into(),
            json: payload_json,
        };
        if payloads.insert(record.event_id.as_str(), payload).is_some() {
            return Err(StoreError::Invalid(
                "telemetry payload span occurs more than once",
            ));
        }
    }
    Ok(payloads)
}

pub(super) struct EncodedPayload {
    digest: [u8; 32],
    json: String,
}

impl EncodedPayload {
    pub(super) fn verify_replay(
        &self,
        connection: &Connection,
        source: &str,
        event: &str,
    ) -> Result<()> {
        let matching = connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM agenturm_telemetry_payloads
                WHERE source_id = ?1 AND event_id = ?2 AND revision = 0 AND payload_digest = ?3
            )",
            params![source, event, self.digest],
            |row| row.get::<_, bool>(0),
        )?;
        if matching {
            Ok(())
        } else {
            Err(StoreError::RequestConflict)
        }
    }

    pub(super) fn insert(
        &self,
        connection: &Connection,
        batch: &ObservationBatch,
        event: &str,
        retained: bool,
    ) -> Result<()> {
        connection.execute(
            "INSERT INTO agenturm_telemetry_payloads (
                source_id, event_id, revision, request_id, payload_digest, payload_json
            ) VALUES (?1, ?2, 0, ?3, ?4, ?5)",
            params![
                batch.source_id,
                event,
                batch.request_id,
                self.digest,
                retained.then_some(&self.json),
            ],
        )?;
        Ok(())
    }
}

pub(super) fn purge_before(connection: &Connection, cutoff: i64) -> Result<()> {
    connection.execute(
        "UPDATE agenturm_telemetry_payloads AS payload SET payload_json = NULL
         WHERE (source_id, event_id, revision) IN (
             SELECT source_id, event_id, revision FROM agenturm_observations
             WHERE observed_at_unix_ns < ?1
         )",
        [cutoff],
    )?;
    Ok(())
}

impl Store {
    pub(super) fn verify_telemetry_storage(&self) -> Result<()> {
        self.verify_private_text_storage("telemetry text requires a private leased history")
    }

    pub(super) fn verify_private_text_storage(&self, required: &'static str) -> Result<()> {
        self.verify_write_admission()?;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;

            let lease = self
                .writer_lease
                .as_ref()
                .ok_or(StoreError::Invalid(required))?;
            let database = std::fs::symlink_metadata(lease.database_path())
                .map_err(StoreError::WriterLease)?;
            let lock =
                std::fs::symlink_metadata(lease.lock_path()).map_err(StoreError::WriterLease)?;
            if database.is_file()
                && database.mode() & 0o7777 == 0o600
                && database.nlink() == 1
                && database.uid() == lock.uid()
            {
                return Ok(());
            }
        }
        Err(StoreError::Invalid(required))
    }

    /// Consumes captured text inside the writer fence. No payload-bearing item
    /// exits Store unless the caller explicitly consumes it through this port.
    pub fn with_telemetry_payload<T>(
        &self,
        selected: &TelemetryPayloadSelection,
        consume: impl FnOnce(&TelemetryPayload) -> T,
    ) -> Result<Option<T>> {
        self.verify_telemetry_storage()?;
        let scope = selected.binding.grant_scope()?;
        if !valid_hex(&selected.trace_id, 32) || !valid_hex(&selected.span_id, 16) {
            return Err(StoreError::Invalid(
                "invalid telemetry payload trace or span",
            ));
        }
        self.with_user_content_fence(|store| {
            store.verify_telemetry_storage()?;
            if store.current_content_grant(&scope)?.is_none() {
                return Err(StoreError::StaleContentGrant);
            }
            if store.history_identity()? != selected.history_identity {
                return Err(StoreError::SnapshotReplaced);
            }
            let (_, watermark) =
                retention::resolve_read_snapshot(&store.connection, Some(selected.snapshot_id))?;
            let event_id = format!("otlp:{}:{}", selected.trace_id, selected.span_id);
            let json: Option<String> = store
                .connection
                .query_row(
                    "SELECT payload.payload_json
                 FROM agenturm_telemetry_payloads AS payload
                 JOIN agenturm_observations AS observation
                    USING (source_id, event_id, revision, request_id)
                 JOIN agenturm_history_batches AS batch USING (request_id)
                 WHERE payload.source_id = ?1 AND payload.event_id = ?2
                   AND payload.revision = 0 AND payload.payload_json IS NOT NULL
                   AND batch.sequence <= ?3
                   AND observation.session_id = ?4
                   AND observation.turn_id IS ?5 AND observation.worker_id IS ?6
                   AND json_extract(observation.kind_json, '$.target_source_id') IS ?7",
                    params![
                        selected.binding.source_id.as_str(),
                        event_id,
                        watermark,
                        selected.session_id,
                        selected.turn_id,
                        selected.worker_id,
                        selected
                            .binding
                            .native_source_id
                            .as_ref()
                            .map(ExtensionId::as_str),
                    ],
                    |row| row.get(0),
                )
                .optional()?;
            json.map(|json| {
                let payload: TelemetryPayload = serde_json::from_str(&json)
                    .map_err(|_| StoreError::Invalid("retained telemetry payload is invalid"))?;
                payload.validate()?;
                Ok(consume(&payload))
            })
            .transpose()
        })?
    }
}

fn valid_hex(value: &str, width: usize) -> bool {
    value.len() == width
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        && value.bytes().any(|byte| byte != b'0')
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::CommitOutcome;
    use agenturm_kernel::{
        Observation, ObservationIdentity, ObservationKind, TraceSpan, TraceStatus,
    };
    use std::path::PathBuf;

    const SOURCE: &str = "otlp.privacy";
    const TRACE: &str = "1234567890abcdef1234567890abcdef";
    const SPAN: &str = "1234567890abcdef";
    const OTHER: &str = "abcdef1234567890";

    #[test]
    fn exact_replay_never_attaches_or_replaces_private_text() {
        let root = scratch("replay");
        let mut store = Store::open_collector(root.join("history.sqlite")).unwrap();
        let binding = binding();
        let first = span(SPAN, 10);
        let first_batch = batch(None, "checkpoint-first", std::slice::from_ref(&first));
        let payload = record(&first, "fixture message");
        assert_eq!(
            store
                .commit_telemetry_batch(&first_batch, &binding, std::slice::from_ref(&payload))
                .unwrap(),
            CommitOutcome::Committed { inserted: 1 }
        );
        assert_eq!(
            store
                .commit_telemetry_batch(&first_batch, &binding, std::slice::from_ref(&payload))
                .unwrap(),
            CommitOutcome::Replay
        );
        let current = batch(
            Some("checkpoint-first"),
            "checkpoint-first",
            std::slice::from_ref(&first),
        );
        assert_eq!(
            store
                .commit_telemetry_batch(&current, &binding, &[payload])
                .unwrap(),
            CommitOutcome::Replay
        );

        let second = span(OTHER, 20);
        let atomic = batch(
            Some("checkpoint-first"),
            "checkpoint-second",
            &[second.clone(), first.clone()],
        );
        assert!(matches!(
            store.commit_telemetry_batch(
                &atomic,
                &binding,
                &[record(&second, "new text"), record(&first, "changed text")]
            ),
            Err(StoreError::RequestConflict)
        ));
        assert_eq!(
            store.source_cursor(SOURCE).unwrap().as_deref(),
            Some("checkpoint-first")
        );
        assert_eq!(store.observations().unwrap().len(), 1);

        let metadata = batch(
            Some("checkpoint-first"),
            "checkpoint-second",
            std::slice::from_ref(&second),
        );
        assert_eq!(
            store.commit_batch(&metadata).unwrap(),
            CommitOutcome::Committed { inserted: 1 }
        );
        assert!(matches!(
            store.commit_telemetry_batch(&metadata, &binding, &[record(&second, "attach later")]),
            Err(StoreError::RequestConflict)
        ));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn grant_scope_retention_backup_and_snapshot_gate_the_exact_occurrence() {
        let root = scratch("fence");
        let db = root.join("history.sqlite");
        let backup = root.join("backup.sqlite");
        let mut store = Store::open_collector(&db).unwrap();
        let binding = binding();
        let first = span(SPAN, 10);
        let batch = batch(None, "checkpoint", std::slice::from_ref(&first));
        let payload = record(&first, "retained fixture message");
        assert!(
            store
                .commit_telemetry_batch(
                    &batch,
                    &binding,
                    &[record(&first, "Authorization: Bearer redacted")]
                )
                .is_err()
        );
        assert!(store.observations().unwrap().is_empty());
        store
            .commit_telemetry_batch(&batch, &binding, std::slice::from_ref(&payload))
            .unwrap();
        let scope = binding.grant_scope().unwrap();
        let mut selected = TelemetryPayloadSelection {
            history_identity: store.history_identity().unwrap(),
            binding,
            trace_id: TRACE.into(),
            span_id: SPAN.into(),
            session_id: "fixture-session".into(),
            turn_id: None,
            worker_id: None,
            snapshot_id: store.observation_page(1).unwrap().snapshot_id,
        };
        let read = |store: &Store, selection: &TelemetryPayloadSelection| {
            store.with_telemetry_payload(selection, |payload| payload.input.clone())
        };
        let native_grant = ContentGrantScope::new(
            selected.binding.source_id.clone(),
            ContentKind::CodexUserText,
            scope.source_fingerprint(),
        );
        store.grant_content(&native_grant).unwrap();
        assert!(matches!(
            read(&store, &selected),
            Err(StoreError::StaleContentGrant)
        ));
        store.grant_content(&scope).unwrap();
        assert_eq!(
            read(&store, &selected).unwrap(),
            Some(payload.payload.input.clone())
        );
        let wrong_session = TelemetryPayloadSelection {
            session_id: "another-session".into(),
            ..selected.clone()
        };
        assert_eq!(read(&store, &wrong_session).unwrap(), None);
        let wrong_turn = TelemetryPayloadSelection {
            turn_id: Some("another-turn".into()),
            ..selected.clone()
        };
        assert_eq!(read(&store, &wrong_turn).unwrap(), None);
        let before_receipt = TelemetryPayloadSelection {
            snapshot_id: 0.into(),
            ..selected.clone()
        };
        assert_eq!(read(&store, &before_receipt).unwrap(), None);
        store.revoke_bound_content(&scope).unwrap();
        assert!(matches!(
            read(&store, &selected),
            Err(StoreError::StaleContentGrant)
        ));
        store.grant_content(&scope).unwrap();

        store.backup_to(&backup).unwrap();
        let mut restored = Store::open_collector(&backup).unwrap();
        assert!(matches!(
            read(&restored, &selected),
            Err(StoreError::StaleContentGrant)
        ));
        restored.grant_content(&scope).unwrap();
        assert!(matches!(
            read(&restored, &selected),
            Err(StoreError::SnapshotReplaced)
        ));
        selected.snapshot_id = restored.observation_page(1).unwrap().snapshot_id;
        assert_eq!(
            read(&restored, &selected).unwrap(),
            Some(payload.payload.input.clone())
        );
        restored.prune_observations_before(11).unwrap();
        assert!(matches!(
            read(&restored, &selected),
            Err(StoreError::ExpiredHistorySnapshot { .. })
        ));
        selected.snapshot_id = restored.observation_page(1).unwrap().snapshot_id;
        assert_eq!(read(&restored, &selected).unwrap(), None);
        let persisted: Option<String> = restored
            .connection
            .query_row(
                "SELECT payload_json FROM agenturm_telemetry_payloads",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(persisted.is_none());
        assert_eq!(
            restored
                .commit_telemetry_batch(&batch, &selected.binding, &[payload])
                .unwrap(),
            CommitOutcome::Replay
        );
        assert_eq!(read(&restored, &selected).unwrap(), None);
        drop((store, restored));
        std::fs::remove_dir_all(root).unwrap();
    }

    fn binding() -> TelemetryBinding {
        TelemetryBinding {
            source_id: ExtensionId::parse(SOURCE).unwrap(),
            native_source_id: None,
        }
    }

    fn span(span_id: &str, timestamp: u64) -> Observation {
        Observation {
            harness_id: binding().source_id,
            event_id: format!("otlp:{TRACE}:{span_id}"),
            revision: 0,
            identity: ObservationIdentity::session("fixture-session"),
            observed_at_unix_ns: timestamp,
            kind: ObservationKind::TraceSpan(TraceSpan {
                trace_id: TRACE.into(),
                span_id: span_id.into(),
                parent_span_id: None,
                target_source_id: None,
                target_operation_id: None,
                started_at_unix_ns: 1,
                status: TraceStatus::Ok,
            }),
        }
    }

    fn record(span: &Observation, text: &str) -> TelemetryPayloadRecord {
        TelemetryPayloadRecord {
            event_id: span.event_id.clone(),
            payload: TelemetryPayload {
                input: Some(text.into()),
                output: None,
            },
        }
    }

    fn batch(prior: Option<&str>, next: &str, spans: &[Observation]) -> ObservationBatch {
        ObservationBatch::from_observations(SOURCE, prior, next, spans).unwrap()
    }

    fn scratch(case: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agenturm-telemetry-payload-{}-{case}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        root
    }
}
