//! Private text admitted with one exact evaluation revision and trace receipt.

use crate::{
    ContentGrantScope, ContentKind, EvalOutcome, Result, Store, StoreError, bounded_text_preview,
    eval_outcome::{MAX_EVAL_SCORES, deserialize_eval_scores, valid_trace_id},
    retention,
};
use agenturm_kernel::{ExtensionId, ReadSnapshot};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt};

const MAX_TEXT_BYTES: usize = 2_048;
const MAX_RESULT_BYTES: usize = 4_096;
const MAX_BATCH_BYTES: usize = 64 * 1024;
const PRIVATE_HISTORY: &str = "evaluation detail requires a private leased history";

const ADMISSION_SCHEMA: &str = "CREATE TABLE agenturm_eval_detail_admissions (
    sequence INTEGER PRIMARY KEY NOT NULL CHECK(sequence > 0),
    trace_source_id TEXT NOT NULL,
    receipt_id TEXT NOT NULL UNIQUE CHECK(length(receipt_id) = 32),
    batch_digest BLOB NOT NULL CHECK(length(batch_digest) = 32)
)";
const DETAIL_SCHEMA: &str = "CREATE TABLE agenturm_eval_details (
    trace_source_id TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    admitted_sequence INTEGER NOT NULL REFERENCES agenturm_eval_detail_admissions(sequence),
    support_event_id TEXT NOT NULL,
    support_revision INTEGER NOT NULL,
    support_request_id TEXT NOT NULL REFERENCES agenturm_ingest_receipts(request_id),
    detail_digest BLOB NOT NULL CHECK(length(detail_digest) = 32),
    detail_json TEXT CHECK(length(CAST(detail_json AS BLOB)) <= 12288),
    PRIMARY KEY (trace_source_id, trace_id, revision),
    FOREIGN KEY (trace_source_id, trace_id, revision)
        REFERENCES agenturm_eval_outcomes(trace_source_id, trace_id, revision)
) WITHOUT ROWID";

pub(super) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute(ADMISSION_SCHEMA, [])?;
    connection.execute(DETAIL_SCHEMA, [])?;
    Ok(())
}

pub(super) fn validate_schema(connection: &Connection) -> Result<()> {
    for (name, schema) in [
        ("agenturm_eval_detail_admissions", ADMISSION_SCHEMA),
        ("agenturm_eval_details", DETAIL_SCHEMA),
    ] {
        let actual: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()?;
        if actual.as_deref() != Some(schema) {
            return Err(StoreError::UnrecognizedDatabase);
        }
    }
    Ok(())
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvalScoreDetail {
    pub score_index: usize,
    pub detail: String,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvalResultDetail {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_eval_scores"
    )]
    pub scores: Vec<EvalScoreDetail>,
}

impl fmt::Debug for EvalResultDetail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EvalResultDetail")
            .finish_non_exhaustive()
    }
}

impl EvalResultDetail {
    pub fn validate(&self) -> Result<()> {
        self.validated_bytes(MAX_EVAL_SCORES).map(|_| ())
    }

    fn validated_bytes(&self, score_count: usize) -> Result<usize> {
        if self.scores.iter().any(|row| row.score_index >= score_count)
            || self
                .scores
                .windows(2)
                .any(|rows| rows[0].score_index >= rows[1].score_index)
        {
            return Err(StoreError::Invalid(
                "evaluation detail has invalid score positions",
            ));
        }
        let mut bytes = 0;
        for text in self
            .error
            .iter()
            .chain(self.scores.iter().map(|row| &row.detail))
        {
            if text.len() > MAX_TEXT_BYTES
                || !bounded_text_preview(text.clone(), MAX_TEXT_BYTES)
                    .is_ok_and(|safe| safe == *text)
            {
                return Err(StoreError::Invalid("evaluation detail text was withheld"));
            }
            bytes += text.len();
        }
        if !(1..=MAX_RESULT_BYTES).contains(&bytes) {
            return Err(StoreError::Invalid(
                "evaluation detail exceeds its result bound or is empty",
            ));
        }
        Ok(bytes)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvalDetailRecord {
    pub trace_id: String,
    pub detail: EvalResultDetail,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvalDetailSelection {
    pub trace_source_id: ExtensionId,
    pub trace_id: String,
    pub revision: u64,
    pub receipt_id: String,
    pub snapshot_id: ReadSnapshot,
}

pub fn eval_detail_grant_scope(source: &ExtensionId) -> Result<ContentGrantScope> {
    if !source.as_str().starts_with("otlp.") {
        return Err(StoreError::Invalid(
            "evaluation detail requires an OTLP source",
        ));
    }
    let mut hash = Sha256::new();
    hash.update(b"agenturm-eval-detail-grant/1\0");
    hash.update(source.as_str());
    Ok(ContentGrantScope::new(
        source.clone(),
        ContentKind::EvalResultDetail,
        hash.finalize().into(),
    ))
}

pub(super) struct EncodedDetail {
    pub(super) digest: [u8; 32],
    pub(super) json: String,
}

fn detail_digest(json: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"agenturm-eval-detail-text/1\0");
    hash.update(json.as_bytes());
    hash.finalize().into()
}

pub(super) fn encode_batch<'a>(
    outcomes: &[EvalOutcome],
    details: &'a [EvalDetailRecord],
) -> Result<BTreeMap<&'a str, EncodedDetail>> {
    if details.len() > outcomes.len() {
        return Err(StoreError::Invalid(
            "evaluation detail exceeds its batch scope",
        ));
    }
    let scores = outcomes
        .iter()
        .map(|outcome| (outcome.trace_id.as_str(), outcome.scores.len()))
        .collect::<BTreeMap<_, _>>();
    let mut encoded = BTreeMap::new();
    let mut bytes = 0;
    for item in details {
        let count = scores
            .get(item.trace_id.as_str())
            .copied()
            .ok_or(StoreError::Invalid(
                "evaluation detail has no matching outcome",
            ))?;
        bytes += item.detail.validated_bytes(count)?;
        if bytes > MAX_BATCH_BYTES {
            return Err(StoreError::Invalid(
                "evaluation detail exceeds 64 KiB per import",
            ));
        }
        let json = serde_json::to_string(&item.detail)?;
        if encoded
            .insert(
                item.trace_id.as_str(),
                EncodedDetail {
                    digest: detail_digest(&json),
                    json,
                },
            )
            .is_some()
        {
            return Err(StoreError::Invalid(
                "duplicate private detail in evaluation import",
            ));
        }
    }
    Ok(encoded)
}

pub(super) fn admit_marker(
    connection: &Connection,
    source: &ExtensionId,
    details: &BTreeMap<&str, EncodedDetail>,
) -> Result<i64> {
    let sequence = retention::next_receipt_sequence(connection)?;
    let mut hash = Sha256::new();
    hash.update(b"agenturm-eval-detail-admission/1\0");
    hash.update(source.as_str());
    hash.update(b"\0");
    for (trace, detail) in details {
        hash.update(trace);
        hash.update(detail.digest);
    }
    let digest: [u8; 32] = hash.finalize().into();
    connection.execute(
        "INSERT INTO agenturm_eval_detail_admissions(sequence,trace_source_id,batch_digest,receipt_id)
         VALUES (?1,?2,?3,lower(hex(randomblob(16))))",
        params![sequence, source.as_str(), digest],
    )?;
    Ok(sequence)
}

pub(super) struct TraceSupport {
    event_id: String,
    revision: u32,
    request_id: String,
}

pub(super) fn trace_support(
    connection: &Connection,
    source: &ExtensionId,
    trace: &str,
) -> Result<TraceSupport> {
    connection
        .query_row(
            "SELECT o.event_id,o.revision,o.request_id FROM agenturm_observations o
         JOIN agenturm_history_batches b USING (request_id)
         WHERE o.source_id=?1 AND o.scope='session'
           AND json_extract(o.kind_json,'$.kind')='trace_span'
           AND json_extract(o.kind_json,'$.trace_id')=?2
         ORDER BY b.sequence DESC,o.event_id,o.revision DESC LIMIT 1",
            params![source.as_str(), trace],
            |row| {
                Ok(TraceSupport {
                    event_id: row.get(0)?,
                    revision: row.get(1)?,
                    request_id: row.get(2)?,
                })
            },
        )
        .optional()?
        .ok_or(StoreError::EvalTraceNotRetained)
}

impl EncodedDetail {
    pub(super) fn verify_replay(
        &self,
        connection: &Connection,
        source: &ExtensionId,
        trace: &str,
        revision: i64,
    ) -> Result<()> {
        if !connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM agenturm_eval_details
                WHERE trace_source_id=?1 AND trace_id=?2 AND revision=?3 AND detail_digest=?4)",
            params![source.as_str(), trace, revision, self.digest],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(StoreError::Invalid(
                "evaluation detail differs from its original import",
            ));
        }
        Ok(())
    }

    pub(super) fn insert(
        &self,
        connection: &Connection,
        source: &ExtensionId,
        trace: &str,
        revision: i64,
        sequence: i64,
        support: TraceSupport,
    ) -> Result<()> {
        connection.execute(
            "INSERT INTO agenturm_eval_details (trace_source_id,trace_id,revision,admitted_sequence,
                  support_event_id,support_revision,support_request_id,detail_digest,detail_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![source.as_str(), trace, revision, sequence, support.event_id, support.revision,
                    support.request_id, self.digest, self.json],
        )?;
        Ok(())
    }
}

pub(super) fn purge_unretained(connection: &Connection) -> Result<()> {
    connection.execute(
        "UPDATE agenturm_eval_details AS d SET detail_json=NULL
         WHERE detail_json IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM agenturm_observations AS o
            WHERE o.source_id=d.trace_source_id AND o.event_id=d.support_event_id
              AND o.revision=d.support_revision AND o.request_id=d.support_request_id
         )",
        [],
    )?;
    Ok(())
}

impl Store {
    pub(super) fn require_eval_detail_storage(&self) -> Result<()> {
        self.verify_private_text_storage(PRIVATE_HISTORY)
    }

    /// Consumes captured evaluation text while approval, retention, and both
    /// the detail and trace receipts are fenced by the history writer.
    pub fn with_eval_result_detail<T>(
        &self,
        selected: &EvalDetailSelection,
        consume: impl FnOnce(&EvalResultDetail) -> T,
    ) -> Result<Option<T>> {
        self.require_eval_detail_storage()?;
        let scope = eval_detail_grant_scope(&selected.trace_source_id)?;
        if !valid_trace_id(&selected.trace_id)
            || !(1..=i64::MAX as u64).contains(&selected.revision)
            || selected.receipt_id.len() != 32
            || !selected
                .receipt_id
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(StoreError::Invalid("invalid evaluation detail occurrence"));
        }
        self.with_user_content_fence(|store| {
            store.require_eval_detail_storage()?;
            if store.current_content_grant(&scope)?.is_none() {
                return Err(StoreError::StaleContentGrant);
            }
            let (_, watermark) = retention::resolve_read_snapshot(&store.connection, Some(selected.snapshot_id))?;
            let row: Option<(String, [u8; 32], usize)> = store.connection.query_row(
                "SELECT d.detail_json,d.detail_digest,json_array_length(e.outcome_json,'$.scores')
                 FROM agenturm_eval_details AS d
                 JOIN agenturm_eval_detail_admissions AS admission
                    ON admission.sequence=d.admitted_sequence AND admission.trace_source_id=d.trace_source_id
                 JOIN agenturm_eval_outcomes AS e USING(trace_source_id,trace_id,revision)
                 JOIN agenturm_observations AS o
                    ON o.source_id=d.trace_source_id AND o.event_id=d.support_event_id
                   AND o.revision=d.support_revision AND o.request_id=d.support_request_id
                 JOIN agenturm_history_batches AS support ON support.request_id=o.request_id
                 WHERE d.trace_source_id=?1 AND d.trace_id=?2 AND d.revision=?3
                   AND d.detail_json IS NOT NULL AND e.outcome_json IS NOT NULL
                   AND admission.sequence<=?4 AND support.sequence<=?4 AND admission.receipt_id=?5
                   AND o.scope='session' AND json_extract(o.kind_json,'$.kind')='trace_span'
                   AND json_extract(o.kind_json,'$.trace_id')=d.trace_id",
                params![selected.trace_source_id.as_str(), selected.trace_id, selected.revision, watermark, selected.receipt_id],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            ).optional()?;
            row.map(|(json, digest, score_count)| {
                if detail_digest(&json) != digest {
                    return Err(StoreError::Invalid("retained evaluation detail is invalid"));
                }
                let detail: EvalResultDetail = serde_json::from_str(&json)
                    .map_err(|_| StoreError::Invalid("retained evaluation detail is invalid"))?;
                detail.validated_bytes(score_count)?;
                Ok(consume(&detail))
            }).transpose()
        })?
    }
}
