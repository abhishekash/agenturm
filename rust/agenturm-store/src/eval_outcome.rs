//! Structural evaluation results bound to exact retained trace identities.

use super::*;
use serde_json::Number;

pub const MAX_EVAL_OUTCOMES: usize = 1_024;
const MAX_EVAL_SCORES: usize = 32;
const MAX_METADATA_LABEL_BYTES: usize = 128;

const TRACE_LOOKUP_INDEX: &str = "CREATE INDEX agenturm_observations_by_trace
    ON agenturm_observations (source_id, json_extract(kind_json,'$.trace_id'))
    WHERE scope='session' AND json_extract(kind_json,'$.kind')='trace_span'";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvalScore {
    pub scorer: String,
    pub passed: Option<bool>,
    pub score: Option<Number>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvalOutcome {
    pub trace_id: String,
    pub passed: bool,
    pub task_id: Option<String>,
    pub provider_model: Option<String>,
    pub scores: Vec<EvalScore>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvalImportReport {
    pub inserted: usize,
    pub corrected: usize,
    pub replayed: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedEvalOutcome {
    pub trace_source_id: ExtensionId,
    pub revision: u64,
    pub outcome: EvalOutcome,
}

impl EvalOutcome {
    pub fn validate(&self) -> Result<()> {
        if !valid_trace_id(&self.trace_id)
            || self.task_id.as_deref().is_some_and(|id| !valid_label(id))
            || self
                .provider_model
                .as_deref()
                .is_some_and(|id| !valid_label(id))
            || self.scores.len() > MAX_EVAL_SCORES
            || self.scores.iter().any(|score| {
                !valid_label(&score.scorer) || (score.passed.is_none() && score.score.is_none())
            })
        {
            return Err(StoreError::Invalid(
                "invalid evaluation trace ID, metadata label, or score",
            ));
        }
        Ok(())
    }
}

impl Store {
    /// Import one batch of safe evaluation metadata. Only exact retained traces
    /// in the selected source are eligible. Failed joins leave the batch unchanged.
    pub fn import_eval_outcomes(
        &mut self,
        source: &ExtensionId,
        outcomes: &[EvalOutcome],
    ) -> Result<EvalImportReport> {
        if !source.as_str().starts_with("otlp.") || outcomes.len() > MAX_EVAL_OUTCOMES {
            return Err(StoreError::Invalid(
                "evaluation import requires a bounded batch and an OTLP source",
            ));
        }
        let mut seen = BTreeSet::new();
        for outcome in outcomes {
            outcome.validate()?;
            if !seen.insert(outcome.trace_id.as_str()) {
                return Err(StoreError::Invalid(
                    "duplicate trace ID in evaluation import",
                ));
            }
        }
        self.verify_write_admission()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut report = EvalImportReport::default();
        for outcome in outcomes {
            if !retained_trace(&transaction, source, &outcome.trace_id)? {
                return Err(StoreError::EvalTraceNotRetained);
            }
            let content = serde_json::to_string(outcome)?;
            let digest: [u8; 32] = Sha256::digest(content.as_bytes()).into();
            let (prior, replay): (Option<i64>, bool) = transaction.query_row(
                "SELECT (SELECT max(revision) FROM agenturm_eval_outcomes
                           WHERE trace_source_id=?1 AND trace_id=?2),
                        EXISTS(SELECT 1 FROM agenturm_eval_outcomes
                           WHERE trace_source_id=?1 AND trace_id=?2 AND outcome_digest=?3)",
                params![source.as_str(), outcome.trace_id, digest],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if replay {
                report.replayed += 1;
                continue;
            }
            let revision = prior
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(StoreError::Invalid("evaluation revision is exhausted"))?;
            transaction.execute(
                "INSERT INTO agenturm_eval_outcomes
                    (trace_source_id,trace_id,revision,outcome_digest,outcome_json)
                 VALUES (?1,?2,?3,?4,?5)",
                params![source.as_str(), outcome.trace_id, revision, digest, content],
            )?;
            if prior.is_some() {
                report.corrected += 1;
            } else {
                report.inserted += 1;
            }
        }
        transaction.commit()?;
        Ok(report)
    }

    /// Read the latest retained evaluation and its trace membership from one
    /// SQLite snapshot. This point query always reads current state.
    pub fn eval_outcome(
        &self,
        source: &ExtensionId,
        trace_id: &str,
    ) -> Result<Option<RetainedEvalOutcome>> {
        if !source.as_str().starts_with("otlp.") || !valid_trace_id(trace_id) {
            return Err(StoreError::Invalid("invalid evaluation trace source or ID"));
        }
        let transaction = self.connection.unchecked_transaction()?;
        let schema: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if schema < 15 || !retained_trace(&transaction, source, trace_id)? {
            return Ok(None);
        }
        let row: Option<(u64, Option<String>)> = transaction
            .query_row(
                "SELECT revision,outcome_json FROM agenturm_eval_outcomes
                 WHERE trace_source_id=?1 AND trace_id=?2
                 ORDER BY revision DESC LIMIT 1",
                params![source.as_str(), trace_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((revision, Some(content))) = row else {
            return Ok(None);
        };
        let outcome: EvalOutcome = serde_json::from_str(&content)
            .map_err(|_| StoreError::Invalid("stored evaluation metadata is invalid"))?;
        outcome.validate()?;
        if outcome.trace_id != trace_id {
            return Err(StoreError::Invalid(
                "stored evaluation trace identity differs",
            ));
        }
        Ok(Some(RetainedEvalOutcome {
            trace_source_id: source.clone(),
            revision,
            outcome,
        }))
    }
}

fn valid_trace_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        && value.bytes().any(|byte| byte != b'0')
}

fn valid_label(value: &str) -> bool {
    if !value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        || value.len() > MAX_METADATA_LABEL_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/@+-".contains(&byte))
    {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    !lower.split([':', '/', '@']).any(|part| {
        [
            "sk-",
            "sk_live_",
            "sk_test_",
            "ghp_",
            "github_pat_",
            "akia",
            "xoxb-",
            "xoxp-",
        ]
        .iter()
        .any(|prefix| part.starts_with(prefix))
    })
}

fn retained_trace(connection: &Connection, source: &ExtensionId, trace_id: &str) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM agenturm_observations WHERE source_id=?1 AND scope='session'
            AND json_extract(kind_json,'$.kind')='trace_span'
            AND json_extract(kind_json,'$.trace_id')=?2)",
        params![source.as_str(), trace_id],
        |row| row.get(0),
    )?)
}

pub(super) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE agenturm_eval_outcomes (
            trace_source_id TEXT NOT NULL,
            trace_id TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK(typeof(revision) = 'integer' AND revision > 0),
            outcome_digest BLOB NOT NULL CHECK(length(outcome_digest)=32),
            outcome_json TEXT,
            PRIMARY KEY (trace_source_id,trace_id,revision),
            UNIQUE (trace_source_id,trace_id,outcome_digest)
         ) WITHOUT ROWID;",
    )?;
    connection.execute_batch(TRACE_LOOKUP_INDEX)?;
    Ok(())
}

pub(super) fn validate_trace_index(connection: &Connection) -> Result<()> {
    let sql: String = connection.query_row(
        "SELECT sql FROM sqlite_schema WHERE type='index' AND name='agenturm_observations_by_trace'",
        [],
        |row| row.get(0),
    )?;
    if sql != TRACE_LOOKUP_INDEX {
        return Err(StoreError::UnrecognizedDatabase);
    }
    Ok(())
}

pub(super) fn purge_unretained(connection: &Connection) -> Result<()> {
    // Keep the digest and revision as a replay receipt after dropping the
    // payload. A newer span may arrive with the same trace identity.
    connection.execute(
        "UPDATE agenturm_eval_outcomes AS evaluation SET outcome_json=NULL
         WHERE outcome_json IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM agenturm_observations
            WHERE source_id=evaluation.trace_source_id AND scope='session'
              AND json_extract(kind_json,'$.kind')='trace_span'
              AND json_extract(kind_json,'$.trace_id')=evaluation.trace_id
         )",
        [],
    )?;
    Ok(())
}
