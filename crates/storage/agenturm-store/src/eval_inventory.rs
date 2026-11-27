//! One bounded inventory of current evaluation metadata and retained trace evidence.

use super::{EvalOutcome, Result, Store, StoreError, eval_outcome, retention};
use agenturm_kernel::{ExtensionId, ReadSnapshot};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeSet;

pub const MAX_EVAL_INVENTORY_SOURCES: usize = 16;
const MAX_TRACE_KEYS: usize = 1_024;
// Leave room for the scope, snapshot, and HTTP envelope under its 4 MiB cap.
const MAX_ROW_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvalInventoryQuery {
    #[serde(deserialize_with = "deserialize_sources")]
    pub source_ids: Vec<ExtensionId>,
}

impl EvalInventoryQuery {
    pub fn validate(&self) -> Result<()> {
        const INVALID: &str = "evaluation inventory requires 1 to 16 unique OTLP sources";
        if !(1..=MAX_EVAL_INVENTORY_SOURCES).contains(&self.source_ids.len())
            || self
                .source_ids
                .iter()
                .any(|source| !source.as_str().starts_with("otlp."))
            || self.source_ids.iter().collect::<BTreeSet<_>>().len() != self.source_ids.len()
        {
            return Err(StoreError::Invalid(INVALID));
        }
        Ok(())
    }
}

fn deserialize_sources<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<ExtensionId>, D::Error> {
    eval_outcome::deserialize_capped::<D, ExtensionId, MAX_EVAL_INVENTORY_SOURCES>(deserializer)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationMetadata {
    Current,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvalInventory {
    pub source_ids: Vec<ExtensionId>,
    pub trace_evidence_snapshot: ReadSnapshot,
    pub evaluation_metadata: EvaluationMetadata,
    pub outcomes: Vec<EvalInventoryRow>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvalInventoryRow {
    pub trace_source_id: ExtensionId,
    pub revision: u64,
    pub outcome: EvalOutcome,
}

impl Store {
    /// Resolves the current trace evidence watermark and outcome revisions
    /// in one read transaction. The watermark applies to trace evidence.
    pub fn query_eval_inventory(&self, query: &EvalInventoryQuery) -> Result<EvalInventory> {
        query.validate()?;
        let transaction = self.connection.unchecked_transaction()?;
        let (snapshot, watermark) = retention::resolve_read_snapshot(&transaction, None)?;
        let mut inventory = EvalInventory {
            source_ids: query.source_ids.clone(),
            trace_evidence_snapshot: snapshot,
            evaluation_metadata: EvaluationMetadata::Current,
            outcomes: Vec::new(),
        };
        inventory.source_ids.sort_unstable();
        let schema: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if schema < 15 {
            return Ok(inventory);
        }

        // Seek past all revisions of the current trace on each iteration.
        // Retired trace keys count against the scan limit as well.
        let mut next_key = transaction.prepare(
            "SELECT trace_id FROM agenturm_eval_outcomes
             WHERE trace_source_id=?1 AND trace_id>?2 ORDER BY trace_id LIMIT 1",
        )?;
        let mut latest = transaction.prepare(
            "SELECT revision,outcome_json FROM agenturm_eval_outcomes
             WHERE trace_source_id=?1 AND trace_id=?2 ORDER BY revision DESC LIMIT 1",
        )?;
        let mut retained = transaction.prepare(
            "SELECT EXISTS(SELECT 1 FROM agenturm_observations AS o
                JOIN agenturm_history_batches AS b USING(request_id)
                WHERE o.source_id=?1 AND o.scope='session'
                  AND json_extract(o.kind_json,'$.kind')='trace_span'
                  AND json_extract(o.kind_json,'$.trace_id')=?2 AND b.sequence<=?3)",
        )?;
        let budget_error = || StoreError::EvalInventoryTooLarge {
            max_trace_keys: MAX_TRACE_KEYS,
            max_row_bytes: MAX_ROW_BYTES,
        };
        let mut trace_keys = 0;
        let mut row_bytes = 0;
        for source in &inventory.source_ids {
            let mut after = String::new();
            while let Some(trace_id) = next_key
                .query_row(params![source.as_str(), after], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
            {
                if trace_keys == MAX_TRACE_KEYS {
                    return Err(budget_error());
                }
                trace_keys += 1;
                let (revision, content): (u64, Option<String>) = latest
                    .query_row(params![source.as_str(), trace_id], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })?;
                after = trace_id;
                let Some(content) = content else { continue };
                if !retained.query_row(params![source.as_str(), after, watermark], |row| {
                    row.get::<_, bool>(0)
                })? {
                    continue;
                }
                let outcome: EvalOutcome = serde_json::from_str(&content)
                    .map_err(|_| StoreError::Invalid("stored evaluation metadata is invalid"))?;
                outcome.validate()?;
                if outcome.trace_id != after {
                    return Err(StoreError::Invalid(
                        "stored evaluation trace identity differs",
                    ));
                }
                let row = EvalInventoryRow {
                    trace_source_id: source.clone(),
                    revision,
                    outcome,
                };
                row_bytes +=
                    serde_json::to_vec(&row)?.len() + usize::from(!inventory.outcomes.is_empty());
                if row_bytes > MAX_ROW_BYTES {
                    return Err(budget_error());
                }
                inventory.outcomes.push(row);
            }
        }
        Ok(inventory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EvalScore;
    use agenturm_kernel::{
        Observation, ObservationIdentity, ObservationKind, ObservationStore, TraceSpan, TraceStatus,
    };

    #[test]
    fn inventory_counts_retired_trace_keys_and_never_returns_a_partial_list() {
        let mut store = Store::open(":memory:").unwrap();
        let source = ExtensionId::parse("otlp.fixture").unwrap();
        let query = EvalInventoryQuery {
            source_ids: vec![source.clone()],
        };
        let transaction = store.connection.transaction().unwrap();
        let mut insert = transaction
            .prepare(
                "INSERT INTO agenturm_eval_outcomes
             (trace_source_id,trace_id,revision,outcome_digest,outcome_json)
             VALUES (?1,?2,1,zeroblob(32),NULL)",
            )
            .unwrap();
        for index in 1..=MAX_TRACE_KEYS {
            insert
                .execute(params![source.as_str(), format!("{index:032x}")])
                .unwrap();
        }
        drop(insert);
        transaction.commit().unwrap();
        assert!(
            store
                .query_eval_inventory(&query)
                .unwrap()
                .outcomes
                .is_empty()
        );
        store
            .connection
            .execute(
                "INSERT INTO agenturm_eval_outcomes VALUES (?1,?2,1,zeroblob(32),NULL)",
                params![source.as_str(), format!("{:032x}", MAX_TRACE_KEYS + 1)],
            )
            .unwrap();
        assert!(matches!(
            store.query_eval_inventory(&query),
            Err(StoreError::EvalInventoryTooLarge {
                max_trace_keys: MAX_TRACE_KEYS,
                max_row_bytes: MAX_ROW_BYTES,
            })
        ));
    }

    #[test]
    fn inventory_enforces_serialized_row_budget_before_reaching_trace_key_limit() {
        let mut store = Store::open(":memory:").unwrap();
        let source = ExtensionId::parse("otlp.fixture").unwrap();
        let query = EvalInventoryQuery {
            source_ids: vec![source.clone()],
        };
        let scores: Vec<_> = (0..32)
            .map(|index| EvalScore {
                scorer: format!("{index:02}{}", "a".repeat(126)),
                passed: None,
                score: Some(0.into()),
            })
            .collect();
        let mut observations = Vec::new();
        let mut outcomes = Vec::new();
        for index in 1..=512 {
            let trace_id = format!("{index:032x}");
            let span_id = format!("{index:016x}");
            observations.push(Observation {
                harness_id: source.clone(),
                event_id: format!("otlp:{trace_id}:{span_id}"),
                revision: 0,
                identity: ObservationIdentity::session("fixture"),
                observed_at_unix_ns: 2,
                kind: ObservationKind::TraceSpan(TraceSpan {
                    trace_id: trace_id.clone(),
                    span_id,
                    parent_span_id: None,
                    target_source_id: None,
                    target_operation_id: None,
                    started_at_unix_ns: 1,
                    status: TraceStatus::Ok,
                }),
            });
            outcomes.push(EvalOutcome {
                trace_id,
                passed: true,
                task_id: None,
                provider_model: None,
                scores: scores.clone(),
            });
        }
        store
            .commit(source.as_str(), None, "fixture", &observations)
            .unwrap();
        store
            .import_eval_outcomes(&source, &outcomes[..128])
            .unwrap();
        assert_eq!(
            store.query_eval_inventory(&query).unwrap().outcomes.len(),
            128
        );
        store
            .import_eval_outcomes(&source, &outcomes[128..])
            .unwrap();
        assert!(matches!(
            store.query_eval_inventory(&query),
            Err(StoreError::EvalInventoryTooLarge {
                max_trace_keys: MAX_TRACE_KEYS,
                max_row_bytes: MAX_ROW_BYTES,
            })
        ));
    }
}
