//! Positive current execution clocks from scope-bound receiver start receipts.

use super::*;
use agenturm_kernel::workflow_alerts::WorkflowAlertScope;

pub const MAX_EXECUTION_CLOCK_SCOPES: usize = 200;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionClockQuery {
    pub snapshot_id: ReadSnapshot,
    pub scopes: Vec<WorkflowAlertScope>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionClockStart {
    pub scope: WorkflowAlertScope,
    pub event_id: String,
    pub receipt_time_unix_ns: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionClockPage {
    pub snapshot_id: ReadSnapshot,
    pub receiver_time_unix_ns: Option<u64>,
    pub active: Vec<ExecutionClockStart>,
}

#[derive(Serialize)]
struct Requested<'a> {
    scope: &'a WorkflowAlertScope,
    digest: String,
}

impl Store {
    pub fn query_execution_clocks(
        &self,
        query: &ExecutionClockQuery,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
    ) -> Result<ExecutionClockPage> {
        let mut requested = Vec::with_capacity(query.scopes.len().min(MAX_EXECUTION_CLOCK_SCOPES));
        let mut distinct = BTreeSet::new();
        if !(1..=MAX_EXECUTION_CLOCK_SCOPES).contains(&query.scopes.len()) {
            return Err(StoreError::Invalid("invalid execution clock scope count"));
        }
        for scope in &query.scopes {
            let binding = NativeAttentionScope {
                source_id: scope.source_id.as_str().to_owned(),
                session_id: scope.session_id.clone(),
                turn_id: scope.turn_id.clone(),
                worker_id: scope.worker_id.clone(),
            };
            if !binding.is_valid()
                || allowed_sources.is_some_and(|sources| !sources.contains(&scope.source_id))
                || !distinct.insert(scope)
            {
                return Err(StoreError::Invalid("invalid execution clock scope"));
            }
            requested.push(Requested {
                scope,
                digest: format!("{:X}", Sha256::digest(serde_json::to_vec(&binding)?)),
            });
        }
        let transaction = self.connection.unchecked_transaction()?;
        let (snapshot_id, watermark) = retention::resolve_read_snapshot(&transaction, None)?;
        if snapshot_id != query.snapshot_id {
            return Err(StoreError::Invalid(
                "execution clock snapshot is no longer current",
            ));
        }
        let mut page = ExecutionClockPage {
            snapshot_id,
            receiver_time_unix_ns: None,
            active: Vec::new(),
        };
        if !table_exists(&transaction)? {
            return Ok(page);
        }
        let Some(floor) = retention::execution_clock_floor(&transaction)? else {
            return Ok(page);
        };
        if floor > watermark {
            return Err(StoreError::Invalid("execution clock floor exceeds history"));
        }
        let Some(now) = receiver_time_unix_ns() else {
            return Ok(page);
        };
        let latest_clock: Option<i64> = transaction.query_row(
            "SELECT max(receipt_time_unix_ns) FROM agenturm_native_attention_events
             WHERE history_sequence>?1",
            [floor],
            |row| row.get(0),
        )?;
        if latest_clock.is_some_and(|previous| previous > now) {
            return Ok(page);
        }
        page.receiver_time_unix_ns = Some(now as u64);
        let workflow_filter = crate::history_projection::WORKFLOW_ALERTS_FILTER;
        let sql = format!(
            "WITH requested AS MATERIALIZED (
               SELECT CAST(key AS INTEGER) slot, json_extract(value,'$.digest') digest,
                      json_extract(value,'$.scope.source_id') source_id,
                      json_extract(value,'$.scope.session_id') session_id,
                      json_extract(value,'$.scope.turn_id') turn_id,
                      json_extract(value,'$.scope.worker_id') worker_id
               FROM json_each(?2)
             ), sender_tail AS (
               SELECT requested.slot, max(event.history_sequence) tail_sequence
               FROM requested
               JOIN agenturm_native_attention_senders sender
                 ON hex(sender.scope_digest)=requested.digest
               JOIN agenturm_native_attention_events event
                 ON event.collector_id=sender.collector_id AND event.sender_id=sender.sender_id
               WHERE event.kind IN ('turn_started','turn_completed','turn_interrupted')
                 AND event.history_sequence<=?1 AND event.history_sequence>?5
               GROUP BY requested.slot, sender.collector_id, sender.sender_id
             ), unique_tail AS (
               SELECT slot, max(tail_sequence) tail_sequence FROM sender_tail
               GROUP BY slot HAVING count(*)=1
             ), native_state AS MATERIALIZED (
               SELECT requested.slot, json_extract(observation.kind_json,'$.state') state,
                      history_batch.sequence sequence, observation.observed_at_unix_ns at
               FROM requested
               JOIN agenturm_observations observation
                 ON observation.source_id=requested.source_id AND observation.session_id=requested.session_id
                 AND observation.turn_id=requested.turn_id AND observation.worker_id IS requested.worker_id
               JOIN agenturm_history_batches history_batch ON history_batch.request_id=observation.request_id
               WHERE history_batch.sequence<=?1 AND ({workflow_filter})
                 AND json_extract(observation.kind_json,'$.kind')='lifecycle'
             )
             SELECT chosen.slot, start.event_id, start.receipt_time_unix_ns
             FROM unique_tail chosen
             JOIN agenturm_native_attention_events start ON start.history_sequence=chosen.tail_sequence
             WHERE start.kind='turn_started' AND start.source_id IS NOT NULL
               AND start.receipt_time_unix_ns<=?3
               AND (?4 IS NULL OR start.collector_id IN (SELECT value FROM json_each(?4)))
               AND EXISTS (
                 SELECT 1 FROM native_state running
                 WHERE running.slot=chosen.slot AND running.state='running'
                 AND NOT EXISTS (
                   SELECT 1 FROM native_state advanced
                   WHERE advanced.slot=running.slot AND (
                     advanced.state IN ('failed','interrupted','completed')
                     OR (advanced.state='paused'
                         AND (advanced.sequence>=running.sequence OR advanced.at>=running.at))
                   )
                 )
               )
             ORDER BY chosen.slot",
        );
        let mut statement = transaction.prepare(&sql)?;
        let mut rows = statement.query(params![
            watermark,
            serde_json::to_string(&requested)?,
            now,
            allowed_sources.map(serde_json::to_string).transpose()?,
            floor,
        ])?;
        while let Some(row) = rows.next()? {
            let scope = query
                .scopes
                .get(row.get::<_, usize>(0)?)
                .ok_or(StoreError::Invalid(
                    "execution clock result has invalid scope",
                ))?;
            page.active.push(ExecutionClockStart {
                scope: scope.clone(),
                event_id: row.get(1)?,
                receipt_time_unix_ns: row.get(2)?,
            });
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{
        Observation, ObservationIdentity, ObservationKind, ObservationQuery, ObservationStore,
    };

    fn setup() -> (
        Store,
        ExtensionId,
        WorkflowAlertScope,
        NativeAttentionCapture,
    ) {
        let mut store = Store::open(":memory:").unwrap();
        let source = ExtensionId::parse("harness.clock").unwrap();
        let collector = ExtensionId::parse("otlp.clock").unwrap();
        let scope = WorkflowAlertScope {
            source_id: source.clone(),
            session_id: "session-fixture".into(),
            turn_id: "turn-fixture".into(),
            worker_id: None,
        };
        let mut identity = ObservationIdentity::session(&scope.session_id);
        identity.turn_id = Some(scope.turn_id.clone());
        let row = Observation {
            harness_id: source.clone(),
            event_id: "native-running".into(),
            revision: 0,
            identity,
            observed_at_unix_ns: 1,
            kind: ObservationKind::Lifecycle {
                state: "running".into(),
                reason_code: None,
            },
        };
        store
            .commit(source.as_str(), None, "first", &[row])
            .unwrap();
        let capture = NativeAttentionCapture {
            history_binding: binding(&store.connection).unwrap(),
            scope: NativeAttentionScope {
                source_id: source.as_str().into(),
                session_id: scope.session_id.clone(),
                turn_id: scope.turn_id.clone(),
                worker_id: None,
            },
            sender_id: "1".repeat(32),
            event_id: "begin".into(),
            sequence: 1,
            kind: NativeAttentionKind::TurnStarted,
            request_id: None,
            source_time_unix_ns: Some(1),
        };
        (store, collector, scope, capture)
    }

    fn current(store: &Store, scopes: Vec<WorkflowAlertScope>) -> ExecutionClockQuery {
        let head = store
            .query_observations(&ObservationQuery::latest(1))
            .unwrap();
        ExecutionClockQuery {
            snapshot_id: head.snapshot_id,
            scopes,
        }
    }

    #[test]
    fn current_bridge_start_uses_receipt_clock_and_is_denied_when_scope_or_clock_changes() {
        let (mut store, collector, scope, mut capture) = setup();
        let initial = current(&store, vec![scope.clone()]);
        assert!(
            store
                .query_execution_clocks(&initial, None)
                .unwrap()
                .active
                .is_empty()
        );
        let receipt = store
            .admit_native_attention(&collector, &capture, None)
            .unwrap();
        assert!(!receipt.replayed);
        assert!(store.query_execution_clocks(&initial, None).is_err());
        let mut sibling = scope.clone();
        sibling.worker_id = Some("sibling".into());
        let running = current(&store, vec![scope.clone(), sibling]);
        let allowed = BTreeSet::from([collector.clone(), scope.source_id.clone()]);
        let page = store
            .query_execution_clocks(&running, Some(&allowed))
            .unwrap();
        assert_eq!(page.snapshot_id, running.snapshot_id);
        let [start] = page.active.as_slice() else {
            panic!("expected exact main scope");
        };
        assert_eq!(start.scope, scope);
        assert_eq!(start.event_id, receipt.event_id);
        assert_ne!(
            start.receipt_time_unix_ns,
            capture.source_time_unix_ns.unwrap()
        );
        assert!(page.receiver_time_unix_ns.unwrap() >= start.receipt_time_unix_ns);
        let replay = store
            .admit_native_attention(&collector, &capture, None)
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(
            store
                .query_execution_clocks(&running, Some(&allowed))
                .unwrap()
                .active,
            page.active
        );
        assert!(
            store
                .query_execution_clocks(&running, Some(&BTreeSet::from([collector.clone()])))
                .is_err()
        );
        let target_only = BTreeSet::from([scope.source_id.clone()]);
        assert!(
            store
                .query_execution_clocks(&running, Some(&target_only))
                .unwrap()
                .active
                .is_empty()
        );

        capture.sequence = 2;
        capture.event_id = "end".into();
        capture.kind = NativeAttentionKind::TurnCompleted;
        store
            .admit_native_attention(&collector, &capture, None)
            .unwrap();
        assert!(store.query_execution_clocks(&running, None).is_err());
        let ended = current(&store, vec![scope]);
        assert!(
            store
                .query_execution_clocks(&ended, Some(&allowed))
                .unwrap()
                .active
                .is_empty()
        );
    }

    #[test]
    fn hidden_competing_sender_and_native_terminal_each_invalidate_elapsed_authority() {
        let (mut store, collector, scope, mut capture) = setup();
        store
            .admit_native_attention(&collector, &capture, None)
            .unwrap();
        let hidden = ExtensionId::parse("otlp.hidden").unwrap();
        capture.sender_id = "2".repeat(32);
        capture.kind = NativeAttentionKind::TurnInterrupted;
        store
            .admit_native_attention(&hidden, &capture, None)
            .unwrap();
        let ambiguous = current(&store, vec![scope.clone()]);
        let one_sender = BTreeSet::from([collector, scope.source_id]);
        assert!(
            store
                .query_execution_clocks(&ambiguous, Some(&one_sender))
                .unwrap()
                .active
                .is_empty()
        );

        let (mut store, collector, scope, capture) = setup();
        store
            .admit_native_attention(&collector, &capture, None)
            .unwrap();
        let before = current(&store, vec![scope.clone()]);
        assert_eq!(
            store
                .query_execution_clocks(&before, None)
                .unwrap()
                .active
                .len(),
            1
        );
        let mut identity = ObservationIdentity::session(&scope.session_id);
        identity.turn_id = Some(scope.turn_id.clone());
        let stopped = Observation {
            harness_id: scope.source_id.clone(),
            event_id: "native-running".into(),
            revision: 1,
            identity,
            observed_at_unix_ns: 1,
            kind: ObservationKind::Lifecycle {
                state: "interrupted".into(),
                reason_code: None,
            },
        };
        store
            .commit(
                scope.source_id.as_str(),
                Some("first"),
                "stopped",
                &[stopped],
            )
            .unwrap();
        let after = current(&store, vec![scope]);
        assert!(
            store
                .query_execution_clocks(&after, None)
                .unwrap()
                .active
                .is_empty()
        );
        assert!(store.query_execution_clocks(&before, None).is_err());
    }

    #[test]
    fn restoring_the_same_watermark_requires_a_fresh_start_in_the_new_incarnation() {
        let (mut store, collector, scope, mut capture) = setup();
        store
            .admit_native_attention(&collector, &capture, None)
            .unwrap();
        let live = current(&store, vec![scope.clone()]);
        assert_eq!(
            store
                .query_execution_clocks(&live, None)
                .unwrap()
                .active
                .len(),
            1
        );
        let staged = store.connection.transaction().unwrap();
        retention::renew_read_incarnation(&staged).unwrap();
        staged.commit().unwrap();
        let restored = current(&store, vec![scope.clone()]);
        assert_eq!(
            restored.snapshot_id.watermark(),
            live.snapshot_id.watermark()
        );
        assert_ne!(
            restored.snapshot_id.incarnation(),
            live.snapshot_id.incarnation()
        );
        assert!(store.query_execution_clocks(&live, None).is_err());
        assert!(
            store
                .query_execution_clocks(&restored, None)
                .unwrap()
                .active
                .is_empty()
        );
        assert!(
            store
                .admit_native_attention(&collector, &capture, None)
                .is_err()
        );

        // Model an archive produced before the clock floor was preserved.
        store
            .connection
            .execute(
                "DELETE FROM agenturm_store_meta WHERE key='execution_clock_floor'",
                [],
            )
            .unwrap();
        let unqualified = store.query_execution_clocks(&restored, None).unwrap();
        assert!(unqualified.active.is_empty() && unqualified.receiver_time_unix_ns.is_none());

        capture.history_binding = binding(&store.connection).unwrap();
        capture.sender_id = "2".repeat(32);
        let fresh = store
            .admit_native_attention(&collector, &capture, None)
            .unwrap();
        let current = current(&store, vec![scope]);
        let after = store.query_execution_clocks(&current, None).unwrap();
        let [start] = after.active.as_slice() else {
            panic!("fresh restored provider start");
        };
        assert_eq!(start.event_id, fresh.event_id);
        assert!(
            store
                .admit_native_attention(&collector, &capture, None)
                .unwrap()
                .replayed
        );
        assert_eq!(
            store.query_execution_clocks(&current, None).unwrap().active,
            after.active
        );
    }
}
