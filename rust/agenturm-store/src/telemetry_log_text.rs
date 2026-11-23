//! Producer-captured scalar text for an exact retained OTLP error log.

use crate::telemetry_payload::{
    EncodedPayload, MAX_TELEMETRY_PAYLOAD_BATCH_BYTES, MAX_TELEMETRY_PAYLOAD_TEXT_BYTES,
    TelemetryBinding, TelemetryPayloadRecord, validate_text,
};
use crate::{
    ContentGrantScope, ContentKind, MAX_OTLP_BATCH_SPANS, ObservationBatch, ObservationKind,
    Result, Store, StoreError, StoredScope, retention,
};
use agenturm_kernel::{ExtensionId, ReadSnapshot};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Deserializer, Serialize, de};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryLogTextBinding {
    pub source_id: ExtensionId,
    pub native_source_id: Option<ExtensionId>,
}

impl From<&TelemetryBinding> for TelemetryLogTextBinding {
    fn from(binding: &TelemetryBinding) -> Self {
        Self {
            source_id: binding.source_id.clone(),
            native_source_id: binding.native_source_id.clone(),
        }
    }
}

impl TelemetryLogTextBinding {
    pub fn grant_scope(&self) -> Result<ContentGrantScope> {
        if !self.source_id.as_str().starts_with("otlp.")
            || self
                .native_source_id
                .as_ref()
                .is_some_and(|source| source.as_str().starts_with("otlp."))
        {
            return Err(StoreError::Invalid("invalid telemetry log source binding"));
        }
        let mut hash = Sha256::new();
        hash.update(b"agenturm-telemetry-log-text-binding/1\0");
        hash.update(self.source_id.as_str());
        hash.update(b"\0");
        if let Some(source) = &self.native_source_id {
            hash.update(source.as_str());
        }
        Ok(ContentGrantScope::new(
            self.source_id.clone(),
            ContentKind::TelemetryLogText,
            hash.finalize().into(),
        ))
    }

    pub(super) fn native_source(&self) -> Option<&str> {
        self.native_source_id.as_ref().map(ExtensionId::as_str)
    }
}

/// Producer-captured text. This reply is used only inside the disclosure fence.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryLogText {
    pub text: String,
}

impl fmt::Debug for TelemetryLogText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TelemetryLogText")
            .finish_non_exhaustive()
    }
}

impl TelemetryLogText {
    pub fn validate(&self) -> Result<usize> {
        let bytes = validate_text(&self.text)?;
        if bytes == 0 {
            return Err(StoreError::Invalid("telemetry log contains no text"));
        }
        Ok(bytes)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryLogTextSelection {
    pub history_identity: String,
    pub binding: TelemetryLogTextBinding,
    pub event_id: String,
    pub revision: u32,
    pub observed_at_unix_ns: u64,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub worker_id: Option<String>,
    pub snapshot_id: ReadSnapshot,
}

fn valid_log_event(event_id: &str) -> bool {
    event_id
        .strip_prefix("otlp-error-log:")
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
}

/// Bounds the new broker request before collecting its bodies into a vector.
pub fn deserialize_telemetry_log_records<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<TelemetryPayloadRecord>, D::Error> {
    struct LogRecords;

    impl<'de> de::Visitor<'de> for LogRecords {
        type Value = Vec<TelemetryPayloadRecord>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a bounded array of OTLP log text records")
        }

        fn visit_seq<A: de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut records = Vec::new();
            let mut bytes = 0;
            while records.len() < MAX_OTLP_BATCH_SPANS {
                let Some(record) = sequence.next_element::<TelemetryPayloadRecord>()? else {
                    return Ok(records);
                };
                let (Some(text), None) = (&record.payload.input, &record.payload.output) else {
                    return Err(de::Error::custom("telemetry log needs scalar input text"));
                };
                bytes += text.len();
                if !valid_log_event(&record.event_id)
                    || !(1..=MAX_TELEMETRY_PAYLOAD_TEXT_BYTES).contains(&text.len())
                    || bytes > MAX_TELEMETRY_PAYLOAD_BATCH_BYTES
                {
                    return Err(de::Error::custom("telemetry log text exceeds wire bounds"));
                }
                records.push(record);
            }
            if sequence.next_element::<de::IgnoredAny>()?.is_some() {
                return Err(de::Error::custom("too many telemetry log records"));
            }
            Ok(records)
        }
    }

    deserializer.deserialize_seq(LogRecords)
}

pub(super) fn build_admission<'a>(
    batch: &ObservationBatch,
    binding: &TelemetryLogTextBinding,
    records: &'a [TelemetryPayloadRecord],
) -> Result<BTreeMap<&'a str, EncodedPayload>> {
    binding.grant_scope()?;
    if binding.source_id.as_str() != batch.source_id
        || records.len() > batch.observations.len().min(MAX_OTLP_BATCH_SPANS)
        || batch.observations.iter().any(|row| {
            !matches!(
                row.kind,
                ObservationKind::OtlpErrorLog(_) | ObservationKind::OtlpActivityLog(_)
            )
        })
    {
        return Err(StoreError::Invalid(
            "telemetry log admission has an invalid source, count, or evidence kind",
        ));
    }
    let events = batch
        .observations
        .iter()
        .filter_map(|row| {
            if row.revision != 0 || row.identity.operation_id.is_some() {
                return None;
            }
            let target = match &row.kind {
                ObservationKind::OtlpErrorLog(log)
                    if log.is_valid_at(row.observed_at_unix_ns)
                        && log.matches_event(&row.event_id) =>
                {
                    log.target_source_id.as_deref()
                }
                _ => return None,
            };
            (target == binding.native_source()).then_some(row.event_id.as_str())
        })
        .collect::<std::collections::BTreeSet<_>>();
    let mut payloads = BTreeMap::new();
    let mut bytes = 0;
    for record in records {
        let (Some(text), None) = (&record.payload.input, &record.payload.output) else {
            return Err(StoreError::Invalid("telemetry log needs scalar input text"));
        };
        let payload = TelemetryLogText { text: text.clone() };
        bytes += payload.validate()?;
        if bytes > MAX_TELEMETRY_PAYLOAD_BATCH_BYTES || !events.contains(record.event_id.as_str()) {
            return Err(StoreError::Invalid(
                "telemetry log text exceeds the batch scope or size",
            ));
        }
        let encoded = EncodedPayload::new(b"agenturm-telemetry-log-text/1\0", &payload)?;
        if payloads.insert(record.event_id.as_str(), encoded).is_some() {
            return Err(StoreError::Invalid(
                "telemetry log text occurs more than once",
            ));
        }
    }
    Ok(payloads)
}

impl Store {
    /// Consumes one exact retained log while its current grant stays fenced.
    pub fn with_telemetry_log_text<T>(
        &self,
        selected: &TelemetryLogTextSelection,
        consume: impl FnOnce(&TelemetryLogText) -> T,
    ) -> Result<Option<T>> {
        self.verify_telemetry_storage()?;
        let scope = selected.binding.grant_scope()?;
        let observed_at = i64::try_from(selected.observed_at_unix_ns)
            .map_err(|_| StoreError::Invalid("invalid telemetry log time"))?;
        if selected.revision != 0
            || !valid_log_event(&selected.event_id)
            || (selected.session_id.is_none()
                && (selected.turn_id.is_some()
                    || selected.worker_id.is_some()
                    || selected.binding.native_source_id.is_some()))
            || (selected.worker_id.is_some() && selected.turn_id.is_none())
        {
            return Err(StoreError::Invalid("invalid telemetry log selection"));
        }
        let stored_scope = if selected.session_id.is_some() {
            StoredScope::Session
        } else {
            StoredScope::Source
        };

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
                       AND batch.sequence <= ?3 AND observation.scope = ?4
                       AND observation.observed_at_unix_ns = ?5
                       AND observation.session_id IS ?6 AND observation.turn_id IS ?7
                       AND observation.worker_id IS ?8 AND observation.operation_id IS NULL
                       AND json_extract(observation.kind_json, '$.target_source_id') IS ?9
                       AND json_extract(observation.kind_json, '$.kind') = 'otlp_error_log'",
                    params![
                        selected.binding.source_id.as_str(),
                        selected.event_id,
                        watermark,
                        stored_scope.as_str(),
                        observed_at,
                        selected.session_id,
                        selected.turn_id,
                        selected.worker_id,
                        selected.binding.native_source(),
                    ],
                    |row| row.get(0),
                )
                .optional()?;
            json.map(|json| {
                let payload: TelemetryLogText = serde_json::from_str(&json)
                    .map_err(|_| StoreError::Invalid("retained telemetry log text is invalid"))?;
                payload.validate()?;
                Ok(consume(&payload))
            })
            .transpose()
        })?
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::{CommitOutcome, TelemetryPayload, TelemetryPayloadSelection};
    use agenturm_kernel::telemetry_summary::{OtlpActivityLog, OtlpErrorLog};
    use agenturm_kernel::{Observation, ObservationIdentity, TraceSpan, TraceStatus};
    use std::path::PathBuf;

    const SOURCE: &str = "otlp.error-log";
    const TARGET: &str = "harness.codex.selected";
    const TEXT: &str = "private scalar fixture";
    const TRACE: &str = "1234567890abcdef1234567890abcdef";
    const SPAN: &str = "1234567890abcdef";

    #[test]
    fn exact_log_grant_snapshot_and_scope_fence_private_text() {
        let root = scratch("exact");
        let mut store = Store::open_collector(root.join("history.sqlite")).unwrap();
        let binding = binding(Some(TARGET));
        let log = log(&binding, 'a', 10);
        let batch = batch(None, "error-1", std::slice::from_ref(&log));
        let record = record(&log, TEXT);
        store
            .commit_telemetry_log_batch(&batch, &binding, std::slice::from_ref(&record))
            .unwrap();
        let selected = selection(&store, &binding, &log);
        let scope = binding.grant_scope().unwrap();
        let span_binding = TelemetryBinding {
            source_id: binding.source_id.clone(),
            native_source_id: binding.native_source_id.clone(),
        };
        let span_grant = span_binding.grant_scope().unwrap();
        assert_ne!(span_grant.source_fingerprint(), scope.source_fingerprint());
        store.grant_content(&span_grant).unwrap();
        assert!(matches!(
            read(&store, &selected),
            Err(StoreError::StaleContentGrant)
        ));
        store.grant_content(&scope).unwrap();
        assert_eq!(read(&store, &selected).unwrap().as_deref(), Some(TEXT));
        assert!(
            !serde_json::to_string(&store.observations().unwrap())
                .unwrap()
                .contains(TEXT)
        );
        assert!(!format!("{record:?}").contains(TEXT));
        assert!(!format!("{:?}", TelemetryLogText { text: TEXT.into() }).contains(TEXT));

        let before = TelemetryLogTextSelection {
            snapshot_id: 0.into(),
            ..selected.clone()
        };
        let wrong_scope = TelemetryLogTextSelection {
            worker_id: None,
            ..selected.clone()
        };
        let wrong_time = TelemetryLogTextSelection {
            observed_at_unix_ns: 11,
            ..selected.clone()
        };
        let wrong_event = TelemetryLogTextSelection {
            event_id: format!("otlp-error-log:{}", "b".repeat(64)),
            ..selected.clone()
        };
        let wrong_session = TelemetryLogTextSelection {
            session_id: Some("sibling-session".into()),
            ..selected.clone()
        };
        for wrong in [before, wrong_scope, wrong_time, wrong_event, wrong_session] {
            assert_eq!(read(&store, &wrong).unwrap(), None);
        }
        let wrong_revision = TelemetryLogTextSelection {
            revision: 1,
            ..selected.clone()
        };
        assert!(matches!(
            read(&store, &wrong_revision),
            Err(StoreError::Invalid(_))
        ));
        let wrong_source = TelemetryLogTextSelection {
            history_identity: "different-history".into(),
            ..selected.clone()
        };
        assert!(matches!(
            read(&store, &wrong_source),
            Err(StoreError::SnapshotReplaced)
        ));
        assert_eq!(
            store
                .with_telemetry_payload(
                    &TelemetryPayloadSelection {
                        history_identity: selected.history_identity.clone(),
                        binding: span_binding,
                        trace_id: TRACE.into(),
                        span_id: SPAN.into(),
                        session_id: log.identity.session_id.clone().unwrap(),
                        turn_id: log.identity.turn_id.clone(),
                        worker_id: log.identity.worker_id.clone(),
                        snapshot_id: selected.snapshot_id,
                    },
                    |_| ()
                )
                .unwrap(),
            None
        );
        store.revoke_bound_content(&scope).unwrap();
        assert!(matches!(
            read(&store, &selected),
            Err(StoreError::StaleContentGrant)
        ));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_regrant_and_prune_require_fresh_selection_and_never_restore_deleted_text() {
        let root = scratch("restore");
        let mut store = Store::open_collector(root.join("history.sqlite")).unwrap();
        let backup = root.join("backup.sqlite");
        let binding = binding(None);
        let log = log(&binding, 'a', 10);
        let batch = batch(None, "error-1", std::slice::from_ref(&log));
        let record = record(&log, TEXT);
        store
            .commit_telemetry_log_batch(&batch, &binding, std::slice::from_ref(&record))
            .unwrap();
        let scope = binding.grant_scope().unwrap();
        let mut selected = selection(&store, &binding, &log);
        store.grant_content(&scope).unwrap();
        assert_eq!(read(&store, &selected).unwrap().as_deref(), Some(TEXT));

        let mut sibling = Store::open_collector(root.join("independent.sqlite")).unwrap();
        sibling
            .commit_telemetry_log_batch(&batch, &binding, &[self::record(&log, "different text")])
            .unwrap();
        sibling.grant_content(&scope).unwrap();
        assert_eq!(
            selected.snapshot_id,
            selection(&sibling, &binding, &log).snapshot_id
        );
        assert!(matches!(
            read(&sibling, &selected),
            Err(StoreError::SnapshotReplaced)
        ));

        store.backup_to(&backup).unwrap();
        let mut restored = Store::open_collector(&backup).unwrap();
        assert_eq!(
            restored.history_identity().unwrap(),
            selected.history_identity
        );
        assert!(matches!(
            read(&restored, &selected),
            Err(StoreError::StaleContentGrant)
        ));
        restored.grant_content(&scope).unwrap();
        assert!(matches!(
            read(&restored, &selected),
            Err(StoreError::SnapshotReplaced)
        ));
        selected = selection(&restored, &binding, &log);
        assert_eq!(read(&restored, &selected).unwrap().as_deref(), Some(TEXT));
        restored.prune_observations_before(11).unwrap();
        assert!(matches!(
            read(&restored, &selected),
            Err(StoreError::ExpiredHistorySnapshot { .. })
        ));
        selected = selection(&restored, &binding, &log);
        assert_eq!(read(&restored, &selected).unwrap(), None);
        assert_eq!(
            restored
                .commit_telemetry_log_batch(&batch, &binding, &[record])
                .unwrap(),
            CommitOutcome::Replay
        );
        let tombstone: Option<String> = restored
            .connection
            .query_row(
                "SELECT payload_json FROM agenturm_telemetry_payloads",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(tombstone.is_none());
        assert_eq!(read(&restored, &selected).unwrap(), None);
        drop((store, restored, sibling));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn log_payload_admission_is_atomic_immutable_and_never_treats_activity_as_text() {
        let root = scratch("replay");
        let mut store = Store::open_collector(root.join("history.sqlite")).unwrap();
        let binding = binding(None);
        let error = log(&binding, 'a', 10);
        let mut activity = error.clone();
        activity.event_id = format!("otlp-log:{}", "b".repeat(64));
        activity.identity = ObservationIdentity::session("standalone-session");
        activity.kind = agenturm_kernel::ObservationKind::OtlpActivityLog(OtlpActivityLog {
            reported_error: true,
            target_source_id: None,
            usage: None,
            cost: None,
        });
        let first = batch(None, "error-1", &[error.clone(), activity.clone()]);
        let mut output_only = record(&error, TEXT);
        output_only.payload.output = output_only.payload.input.take();
        for rejected in [
            output_only,
            record(&activity, TEXT),
            record(&error, "Authorization: Bearer secret"),
            record(&error, &"x".repeat(MAX_TELEMETRY_PAYLOAD_TEXT_BYTES + 1)),
        ] {
            assert!(matches!(
                store.commit_telemetry_log_batch(&first, &binding, &[rejected]),
                Err(StoreError::Invalid(_))
            ));
            assert!(store.observations().unwrap().is_empty());
            assert_eq!(store.source_cursor(SOURCE).unwrap(), None);
        }
        let original = record(&error, TEXT);
        assert_eq!(
            store
                .commit_telemetry_log_batch(&first, &binding, std::slice::from_ref(&original))
                .unwrap(),
            CommitOutcome::Committed { inserted: 2 }
        );
        assert_eq!(
            store
                .commit_telemetry_log_batch(&first, &binding, &[original])
                .unwrap(),
            CommitOutcome::Replay
        );
        assert!(matches!(
            store.commit_telemetry_log_batch(&first, &binding, &[record(&error, "different text")]),
            Err(StoreError::RequestConflict)
        ));

        let metadata_only = log(&binding, 'c', 12);
        let second = batch(
            Some("error-1"),
            "error-2",
            std::slice::from_ref(&metadata_only),
        );
        store.commit_batch(&second).unwrap();
        assert!(matches!(
            store.commit_telemetry_log_batch(&second, &binding, &[record(&metadata_only, TEXT)]),
            Err(StoreError::RequestConflict)
        ));
        store
            .grant_content(&binding.grant_scope().unwrap())
            .unwrap();
        assert_eq!(
            read(&store, &selection(&store, &binding, &metadata_only)).unwrap(),
            None
        );
        assert_eq!(
            read(&store, &selection(&store, &binding, &error))
                .unwrap()
                .as_deref(),
            Some(TEXT)
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn v19_read_only_then_writer_upgrade_preserves_span_grant_and_receipt() {
        let root = scratch("v19");
        let path = root.join("history.sqlite");
        let mut store = Store::open_collector(&path).unwrap();
        let binding = TelemetryBinding {
            source_id: ExtensionId::parse(SOURCE).unwrap(),
            native_source_id: None,
        };
        let span = span(&binding);
        let prior = batch(None, "span-1", std::slice::from_ref(&span));
        let record = record(&span, TEXT);
        store
            .commit_telemetry_batch(&prior, &binding, std::slice::from_ref(&record))
            .unwrap();
        let grant_scope = binding.grant_scope().unwrap();
        let grant = store
            .grant_content(&grant_scope)
            .unwrap()
            .binding_fingerprint();
        let selected = TelemetryPayloadSelection {
            history_identity: store.history_identity().unwrap(),
            binding: binding.clone(),
            trace_id: TRACE.into(),
            span_id: SPAN.into(),
            session_id: "standalone-session".into(),
            turn_id: None,
            worker_id: None,
            snapshot_id: store.observation_page(1).unwrap().snapshot_id,
        };
        let tx = store.connection.transaction().unwrap();
        crate::content_grant::migrate_grant_schema(&tx, 19).unwrap();
        tx.execute(
            "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
            [crate::STORE_MARKER_V19],
        )
        .unwrap();
        tx.execute_batch("DROP INDEX agenturm_metric_descriptions_by_series;")
            .unwrap();
        tx.pragma_update(None, "user_version", 19).unwrap();
        tx.commit().unwrap();
        drop(store);

        let read_only = Store::open_read_only(&path).unwrap();
        let version = |s: &Store| {
            s.connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap()
        };
        assert_eq!(version(&read_only), 19);
        assert_eq!(
            read_only.observation_page(1).unwrap().snapshot_id,
            selected.snapshot_id
        );
        drop(read_only);
        let mut upgraded = Store::open_collector(&path).unwrap();
        assert_eq!(version(&upgraded), crate::SCHEMA_VERSION);
        assert_eq!(
            upgraded
                .current_content_grant(&grant_scope)
                .unwrap()
                .unwrap()
                .binding_fingerprint(),
            grant
        );
        assert_eq!(
            upgraded
                .with_telemetry_payload(&selected, |p| p.input.clone())
                .unwrap(),
            Some(Some(TEXT.into()))
        );
        assert_eq!(
            upgraded
                .commit_telemetry_batch(&prior, &binding, &[record])
                .unwrap(),
            CommitOutcome::Replay
        );
        let log_scope = TelemetryLogTextBinding::from(&binding)
            .grant_scope()
            .unwrap();
        upgraded.grant_content(&log_scope).unwrap();
        assert!(
            upgraded
                .current_content_grant(&grant_scope)
                .unwrap()
                .is_some()
        );
        drop(upgraded);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn scratch(case: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agenturm-telemetry-log-text-{}-{case}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        root
    }

    fn binding(target: Option<&str>) -> TelemetryLogTextBinding {
        TelemetryLogTextBinding {
            source_id: ExtensionId::parse(SOURCE).unwrap(),
            native_source_id: target.map(|id| ExtensionId::parse(id).unwrap()),
        }
    }

    fn log(binding: &TelemetryLogTextBinding, digest: char, at: u64) -> Observation {
        let identity = if binding.native_source_id.is_some() {
            ObservationIdentity {
                turn_id: Some("recorded-turn".into()),
                worker_id: Some("recorded-worker".into()),
                ..ObservationIdentity::session("recorded-session")
            }
        } else {
            ObservationIdentity::source()
        };
        Observation {
            harness_id: binding.source_id.clone(),
            event_id: format!("otlp-error-log:{}", digest.to_string().repeat(64)),
            revision: 0,
            identity,
            observed_at_unix_ns: at,
            kind: agenturm_kernel::ObservationKind::OtlpErrorLog(OtlpErrorLog {
                severity_number: 17,
                source_time_unix_ns: Some(at),
                observed_time_unix_ns: None,
                trace_id: Some(TRACE.into()),
                span_id: Some(SPAN.into()),
                target_source_id: binding.native_source().map(str::to_owned),
            }),
        }
    }

    fn span(binding: &TelemetryBinding) -> Observation {
        Observation {
            harness_id: binding.source_id.clone(),
            event_id: format!("otlp:{TRACE}:{SPAN}"),
            revision: 0,
            identity: ObservationIdentity::session("standalone-session"),
            observed_at_unix_ns: 10,
            kind: agenturm_kernel::ObservationKind::TraceSpan(TraceSpan {
                trace_id: TRACE.into(),
                span_id: SPAN.into(),
                parent_span_id: None,
                target_source_id: None,
                target_operation_id: None,
                started_at_unix_ns: 1,
                status: TraceStatus::Ok,
            }),
        }
    }

    fn record(row: &Observation, text: &str) -> TelemetryPayloadRecord {
        TelemetryPayloadRecord {
            event_id: row.event_id.clone(),
            payload: TelemetryPayload {
                input: Some(text.into()),
                output: None,
            },
        }
    }

    fn batch(prior: Option<&str>, next: &str, rows: &[Observation]) -> ObservationBatch {
        ObservationBatch::from_observations(SOURCE, prior, next, rows).unwrap()
    }

    fn selection(
        store: &Store,
        binding: &TelemetryLogTextBinding,
        row: &Observation,
    ) -> TelemetryLogTextSelection {
        TelemetryLogTextSelection {
            history_identity: store.history_identity().unwrap(),
            snapshot_id: store.observation_page(1).unwrap().snapshot_id,
            binding: binding.clone(),
            event_id: row.event_id.clone(),
            revision: row.revision,
            observed_at_unix_ns: row.observed_at_unix_ns,
            session_id: row.identity.session_id.clone(),
            turn_id: row.identity.turn_id.clone(),
            worker_id: row.identity.worker_id.clone(),
        }
    }

    fn read(store: &Store, selection: &TelemetryLogTextSelection) -> Result<Option<String>> {
        store.with_telemetry_log_text(selection, |payload| payload.text.clone())
    }
}
