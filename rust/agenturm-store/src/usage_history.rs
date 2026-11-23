//! Seven UTC calendar buckets from a single retained history snapshot.

use crate::{
    MAX_SESSION_KEY_BYTES, ObservationKind, Result, Store, StoreError, retention,
    validate_identifier,
};
use agenturm_kernel::ExtensionId;
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet};

mod trace_graph;
mod trace_history;

pub use agenturm_kernel::retained_history::{
    ActivityLogTotals, MAX_USAGE_COST_CURRENCIES, TraceUsageTotals, USAGE_DAYS, UsageCurrencyTotal,
    UsageDay, UsageHistoryPage, UsageHistoryQuery, UsageTotals,
};

const DAY_NS: u64 = 86_400_000_000_000;
const MAX_USAGE_ROWS: usize = 16_384;

#[derive(Default)]
struct Counters {
    reports: usize,
    sums: [Option<u128>; 6],
}

impl Counters {
    fn add(&mut self, values: [Option<u64>; 6]) {
        self.reports += 1;
        for (sum, value) in self.sums.iter_mut().zip(values) {
            if let Some(value) = value {
                *sum.get_or_insert(0) += u128::from(value);
            }
        }
    }

    fn totals(self) -> UsageTotals {
        let [
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            reasoning_output_tokens,
            total_tokens,
        ] = self.sums.map(|value| value.map(|value| value.to_string()));
        UsageTotals {
            reports: self.reports,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            reasoning_output_tokens,
            total_tokens,
        }
    }
}

#[derive(Default)]
struct CostCounters {
    amounts: BTreeMap<String, (u128, usize)>,
}

impl CostCounters {
    fn add(&mut self, currency: &str, nanounits: u64) {
        let totals = self.amounts.entry(currency.to_owned()).or_default();
        totals.0 += u128::from(nanounits);
        totals.1 += 1;
    }

    fn totals(self) -> (Vec<UsageCurrencyTotal>, bool) {
        let truncated = self.amounts.len() > MAX_USAGE_COST_CURRENCIES;
        let costs = self
            .amounts
            .into_iter()
            .take(MAX_USAGE_COST_CURRENCIES)
            .map(|(currency, (amount, reports))| UsageCurrencyTotal {
                currency,
                nanounits: amount.to_string(),
                reports,
            })
            .collect();
        (costs, truncated)
    }
}

impl trace_graph::DayTraceReports {
    fn totals(self) -> TraceUsageTotals {
        let (costs, cost_currencies_truncated) = self.costs.totals();
        TraceUsageTotals {
            traces: self.traces,
            reported_error_traces: self.reported_error_traces,
            partial_traces: self.partial_traces,
            traces_missing_exact_cost: self.traces_missing_exact_cost,
            tokens: self.tokens.totals(),
            costs,
            cost_currencies_truncated,
        }
    }
}

#[derive(Default)]
struct DayCounters {
    native: Counters,
    span: Counters,
}

const INDEX_SQL: &str = "CREATE INDEX agenturm_usage_history_by_time ON agenturm_observations(
           observed_at_unix_ns DESC, source_id DESC, event_id DESC, revision DESC)
         WHERE json_extract(kind_json, '$.kind') IN ('usage', 'span_token_usage');";

pub(super) fn create_index(connection: &Connection) -> Result<()> {
    connection.execute_batch(INDEX_SQL)?;
    Ok(())
}

pub(super) fn validate_index(connection: &Connection) -> Result<()> {
    let sql: String = connection.query_row("SELECT sql FROM sqlite_schema WHERE type='index' AND name='agenturm_usage_history_by_time'", [], |row| row.get(0))?;
    if sql != INDEX_SQL.trim_end_matches(';') {
        return Err(StoreError::UnrecognizedDatabase);
    }
    Ok(())
}

impl Store {
    pub fn query_usage_history(
        &self,
        query: &UsageHistoryQuery,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
    ) -> Result<UsageHistoryPage> {
        if let Some(session) = &query.session_id {
            if query.source_id.is_none() {
                return Err(StoreError::Invalid(
                    "usage session requires an exact source",
                ));
            }
            validate_identifier(session, MAX_SESSION_KEY_BYTES, "usage session ID")?;
        }
        if query
            .source_id
            .as_ref()
            .is_some_and(|source| allowed_sources.is_some_and(|allowed| !allowed.contains(source)))
        {
            return Err(StoreError::Invalid("usage source is unavailable"));
        }
        let end = query.end_unix_ns;
        if !(DAY_NS * (USAGE_DAYS as u64 - 1)..=i64::MAX as u64).contains(&end) {
            return Err(StoreError::Invalid("usage window end is invalid"));
        }
        let first = (end / DAY_NS - (USAGE_DAYS as u64 - 1)) * DAY_NS;
        let transaction = self.connection.unchecked_transaction()?;
        let (snapshot_id, watermark) =
            retention::resolve_read_snapshot(&transaction, query.snapshot_id)?;
        let cutoff = retention::read_state(&transaction)?.cutoff_unix_ns as u64;
        let mut statement = transaction.prepare(
            "SELECT o.observed_at_unix_ns, o.kind_json
             FROM agenturm_observations o JOIN agenturm_history_batches b USING (request_id)
             WHERE o.observed_at_unix_ns>=?1 AND o.observed_at_unix_ns<?2 AND b.sequence<=?3
               AND json_extract(o.kind_json, '$.kind') IN ('usage', 'span_token_usage')
               AND (?5 IS NULL OR o.session_id=?5)
               AND (?6 IS NULL OR o.source_id IN (SELECT value FROM json_each(?6)))
               AND (?4 IS NULL OR o.source_id=?4 OR (
                   ?5 IS NOT NULL AND json_extract(o.kind_json,'$.kind')='span_token_usage'
                   AND json_extract(o.kind_json, '$.target_source_id')=?4
                   AND EXISTS (
                       SELECT 1 FROM agenturm_observations anchor
                       JOIN agenturm_history_batches ab USING (request_id)
                       WHERE anchor.source_id=?4 AND anchor.session_id=?5 AND anchor.scope='session'
                         AND ab.sequence<=?3 AND json_extract(anchor.kind_json,'$.kind')<>'trace_span'
                         AND (o.turn_id IS NULL OR (anchor.turn_id=o.turn_id AND anchor.worker_id IS o.worker_id))
                   )
               ))
               AND NOT EXISTS (
                   SELECT 1 FROM agenturm_observations newer
                   JOIN agenturm_history_batches nb USING (request_id)
                   WHERE newer.source_id=o.source_id AND newer.event_id=o.event_id
                     AND newer.revision>o.revision AND nb.sequence<=?3
               )
             ORDER BY o.observed_at_unix_ns DESC, o.source_id DESC, o.event_id DESC, o.revision DESC LIMIT ?7",
        )?;
        let mut rows = statement.query(params![
            first.max(cutoff) as i64,
            end as i64,
            watermark,
            query.source_id.as_ref().map(ExtensionId::as_str),
            query.session_id,
            allowed_sources.map(serde_json::to_string).transpose()?,
            (MAX_USAGE_ROWS + 1) as i64,
        ])?;
        let mut counters: [DayCounters; USAGE_DAYS] =
            std::array::from_fn(|_| DayCounters::default());
        let mut truncated_day = None;
        for index in 0..=MAX_USAGE_ROWS {
            let Some(row) = rows.next()? else { break };
            let at: u64 = row.get(0)?;
            let day = usize::try_from((at - first) / DAY_NS)
                .map_err(|_| StoreError::Invalid("usage observation time is outside its bucket"))?;
            if index == MAX_USAGE_ROWS {
                truncated_day = Some(day);
                break;
            }
            let bucket = counters.get_mut(day).ok_or(StoreError::Invalid(
                "usage observation time is outside its bucket",
            ))?;
            add_report(bucket, serde_json::from_str(&row.get::<_, String>(1)?)?);
        }
        drop(rows);
        let telemetry = trace_history::Window {
            connection: &transaction,
            query,
            first,
            cutoff,
            watermark,
            allowed: allowed_sources,
        }
        .read()?;
        let partial_before_day = match (truncated_day, telemetry.partial_before_day) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (left, right) => left.or(right),
        };
        let mut days = counters.into_iter().enumerate();
        let mut trace_days = telemetry.traces.into_iter();
        let mut log_days = telemetry.logs.into_iter();
        Ok(UsageHistoryPage {
            snapshot_id,
            end_unix_ns: end,
            retention_cutoff_unix_ns: cutoff,
            scan_limited: partial_before_day.is_some() || telemetry.membership_limited,
            days: std::array::from_fn(|_| {
                let (index, counters) = days.next().expect("seven usage buckets");
                let start_unix_ns = first + index as u64 * DAY_NS;
                UsageDay {
                    start_unix_ns,
                    native: counters.native.totals(),
                    span: counters.span.totals(),
                    traces: trace_days.next().expect("seven trace buckets").totals(),
                    logs: log_days.next().expect("seven log buckets").totals(),
                    retention_partial: cutoff > start_unix_ns,
                    scan_partial: partial_before_day.is_some_and(|limit| index <= limit),
                }
            }),
        })
    }
}

fn add_report(bucket: &mut DayCounters, observation: ObservationKind) {
    match observation {
        ObservationKind::Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            total_tokens,
        } => bucket.native.add([
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            None,
            total_tokens,
        ]),
        ObservationKind::SpanTokenUsage(usage) => bucket.span.add([
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_input_tokens,
            usage.cache_write_input_tokens,
            usage.reasoning_output_tokens,
            usage.total_tokens,
        ]),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{
        Observation, ObservationIdentity, ObservationKind, ObservationStore, SpanTokenUsage,
        TraceSpan, TraceSpanIdentity, TraceStatus,
    };

    fn usage(source: &str, event: &str, at: u64, tokens: Option<u64>) -> Observation {
        let mut identity = ObservationIdentity::session("common-session");
        identity.turn_id = Some("matching-turn".into());
        identity.worker_id = Some("matching-worker".into());
        Observation {
            harness_id: ExtensionId::parse(source).unwrap(),
            event_id: event.into(),
            revision: 0,
            identity,
            observed_at_unix_ns: at,
            kind: ObservationKind::Usage {
                input_tokens: tokens,
                output_tokens: None,
                cache_read_tokens: None,
                cache_write_tokens: None,
                total_tokens: None,
            },
        }
    }

    fn trace_and_usage(at: u64, worker: &str, suffix: char) -> [Observation; 2] {
        let trace = TraceSpanIdentity {
            trace_id: "1".repeat(32),
            span_id: suffix.to_string().repeat(16),
        };
        let mut row = usage("otlp.fixture", "unused", at, None);
        row.identity.worker_id = Some(worker.into());
        row.event_id = format!("otlp:{}:{}", trace.trace_id, trace.span_id);
        row.kind = ObservationKind::TraceSpan(TraceSpan {
            trace_id: trace.trace_id.clone(),
            span_id: trace.span_id.clone(),
            parent_span_id: None,
            target_source_id: Some("harness.fixture".into()),
            target_operation_id: None,
            started_at_unix_ns: at - 1,
            status: TraceStatus::Ok,
        });
        let mut tokens = row.clone();
        tokens.event_id = format!("otlp-usage:{}:{}", trace.trace_id, trace.span_id);
        tokens.kind = ObservationKind::SpanTokenUsage(SpanTokenUsage {
            span: trace,
            target_source_id: Some("harness.fixture".into()),
            input_tokens: Some(0),
            output_tokens: None,
            cache_read_input_tokens: Some(5),
            cache_write_input_tokens: None,
            reasoning_output_tokens: None,
            total_tokens: None,
        });
        [row, tokens]
    }

    #[test]
    fn summaries_and_log_only_rows_use_the_same_pinned_scope_and_latest_trace_day() {
        use agenturm_kernel::telemetry_summary::{
            LogCost, LogTokenUsage, OtlpActivityLog, TraceSpanSummary,
        };
        let mut store = Store::open(":memory:").unwrap();
        let collector = ExtensionId::parse("otlp.fixture").unwrap();
        let at = 9 * DAY_NS + 3;
        let [mut span, mut tokens] = trace_and_usage(at, "matching-worker", '3');
        span.identity.worker_id = None;
        span.identity.turn_id = None;
        span.identity.session_id = Some("trace-backed".into());
        tokens.identity = span.identity.clone();
        let ObservationKind::TraceSpan(span_body) = &span.kind else {
            panic!("trace")
        };
        let mut summary = span.clone();
        summary.event_id.push_str(":summary");
        summary.kind = ObservationKind::TraceSpanSummary(TraceSpanSummary {
            trace_id: span_body.trace_id.clone(),
            span_id: span_body.span_id.clone(),
            agent_run: false,
            reported_error: true,
        });
        let log = |session: &str, id: char, source: &str, value| Observation {
            harness_id: ExtensionId::parse(source).unwrap(),
            identity: ObservationIdentity::session(session),
            event_id: format!("otlp-log:{}", id.to_string().repeat(64)),
            revision: 0,
            observed_at_unix_ns: at + 1,
            kind: ObservationKind::OtlpActivityLog(OtlpActivityLog {
                reported_error: true,
                target_source_id: Some("harness.fixture".into()),
                usage: Some(LogTokenUsage {
                    input_tokens: Some(value),
                    output_tokens: None,
                    cache_read_input_tokens: None,
                    cache_write_input_tokens: None,
                    reasoning_output_tokens: None,
                    total_tokens: None,
                }),
                cost: Some(LogCost {
                    currency: "USD".into(),
                    nanounits: 0,
                }),
            }),
        };
        ObservationStore::commit(
            &mut store,
            collector.as_str(),
            None,
            "traces",
            &[span.clone(), summary, tokens],
        )
        .unwrap();
        for (source, checkpoint, rows) in [
            (
                collector.as_str(),
                Some("traces"),
                vec![
                    log("trace-backed", 'a', collector.as_str(), 99),
                    log("log-only", 'b', collector.as_str(), 7),
                ],
            ),
            (
                "otlp.unrelated",
                None,
                vec![log("trace-backed", 'a', "otlp.unrelated", 13)],
            ),
        ] {
            ObservationStore::commit(&mut store, source, checkpoint, "logs", &rows).unwrap();
        }
        let allowed = [collector.clone()].into_iter().collect();
        let mut query = UsageHistoryQuery {
            source_id: None,
            session_id: None,
            snapshot_id: None,
            end_unix_ns: 10 * DAY_NS + 10,
        };
        let initial = store.query_usage_history(&query, Some(&allowed)).unwrap();
        assert_eq!(initial.days[5].traces.traces, 1);
        assert_eq!(initial.days[5].traces.reported_error_traces, 1);
        assert_eq!(
            initial.days[5].traces.tokens.input_tokens.as_deref(),
            Some("0")
        );
        assert_eq!(initial.days[5].traces.traces_missing_exact_cost, 1);
        let logs = initial.days[5].logs.as_ref().unwrap();
        assert_eq!(logs.logs, 1);
        assert_eq!(logs.reported_errors, 1);
        assert_eq!(logs.tokens.input_tokens.as_deref(), Some("7"));
        assert_eq!(logs.costs[0].nanounits, "0");
        assert_eq!(initial.days[5].native.reports, 0);
        let combined = store.query_usage_history(&query, None).unwrap();
        assert_eq!(combined.days[5].logs.as_ref().unwrap().logs, 2);
        let mut later = span.clone();
        let new_id = "4".repeat(16);
        later.observed_at_unix_ns = 10 * DAY_NS;
        let ObservationKind::TraceSpan(body) = &mut later.kind else {
            panic!("trace")
        };
        body.parent_span_id = Some(std::mem::replace(&mut body.span_id, new_id));
        later.event_id = format!("otlp:{}:{}", body.trace_id, body.span_id);
        ObservationStore::commit(
            &mut store,
            collector.as_str(),
            Some("logs"),
            "later",
            &[later],
        )
        .unwrap();
        query.snapshot_id = Some(initial.snapshot_id);
        assert_eq!(
            store.query_usage_history(&query, Some(&allowed)).unwrap(),
            initial
        );
        query.snapshot_id = None;
        let latest = store.query_usage_history(&query, Some(&allowed)).unwrap();
        assert_eq!(latest.days[5].traces.traces, 0);
        assert_eq!(latest.days[6].traces.traces, 1);
        assert_eq!(latest.days[6].traces.partial_traces, 1);
        assert_eq!(
            latest.days[6].traces.tokens.input_tokens.as_deref(),
            Some("0")
        );
    }

    #[test]
    fn seven_days_reports_absence_zero_revisions_and_exact_link_scope() {
        let mut store = Store::open(":memory:").unwrap();
        let source = ExtensionId::parse("harness.fixture").unwrap();
        let fixture = usage(source.as_str(), "fresh", 10 * DAY_NS + 1, Some(0));
        let early = usage(source.as_str(), "early", 4 * DAY_NS, Some(6));
        let omitted = usage(source.as_str(), "too-old", 4 * DAY_NS - 1, Some(100));
        let after_window = usage(source.as_str(), "future", 10 * DAY_NS + 8, Some(100));
        ObservationStore::commit(
            &mut store,
            source.as_str(),
            None,
            "base",
            &[fixture.clone(), early, omitted, after_window],
        )
        .unwrap();
        ObservationStore::commit(
            &mut store,
            "harness.other",
            None,
            "other",
            &[usage("harness.other", "other", 10 * DAY_NS + 1, Some(100))],
        )
        .unwrap();
        let linked = trace_and_usage(10 * DAY_NS + 3, "matching-worker", '3');
        let wrong_worker = trace_and_usage(10 * DAY_NS + 4, "sibling-worker", '4');
        let telemetry: Vec<_> = linked.into_iter().chain(wrong_worker).collect();
        ObservationStore::commit(&mut store, "otlp.fixture", None, "telemetry", &telemetry)
            .unwrap();
        let mut query = UsageHistoryQuery {
            source_id: Some(source.clone()),
            session_id: fixture.identity.session_id.clone(),
            snapshot_id: None,
            end_unix_ns: 10 * DAY_NS + 8,
        };
        let allowed = [source.clone(), ExtensionId::parse("otlp.fixture").unwrap()]
            .into_iter()
            .collect();
        let first = store.query_usage_history(&query, Some(&allowed)).unwrap();
        assert_eq!(first.days[0].native.input_tokens.as_deref(), Some("6"));
        assert_eq!(first.days[6].native.input_tokens.as_deref(), Some("0"));
        assert_eq!(first.days[6].native.reports, 1);
        assert_eq!(first.days[6].span.cache_read_tokens.as_deref(), Some("5"));
        assert_eq!(first.days[6].span.reports, 1);
        assert_eq!(first.days[6].span.total_tokens, None);
        assert_eq!(first.days[5].native.input_tokens, None);
        let only_native = [source.clone()].into_iter().collect();
        assert_eq!(
            store
                .query_usage_history(&query, Some(&only_native))
                .unwrap()
                .days[6]
                .span
                .reports,
            0,
        );
        let mut corrected = fixture;
        corrected.revision = 1;
        let ObservationKind::Usage { input_tokens, .. } = &mut corrected.kind else {
            panic!("usage fixture")
        };
        *input_tokens = Some(7);
        ObservationStore::commit(
            &mut store,
            source.as_str(),
            Some("base"),
            "correction",
            &[corrected],
        )
        .unwrap();
        query.snapshot_id = Some(first.snapshot_id);
        assert_eq!(
            store.query_usage_history(&query, Some(&allowed)).unwrap(),
            first
        );
        query.snapshot_id = None;
        let latest = store.query_usage_history(&query, Some(&allowed)).unwrap();
        assert_eq!(latest.days[6].native.input_tokens.as_deref(), Some("7"));
        assert_eq!(latest.days[6].native.reports, 1);
        let transaction = store
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        retention::prune_in(&transaction, 5 * DAY_NS).unwrap();
        transaction.commit().unwrap();
        let retained = store.query_usage_history(&query, Some(&allowed)).unwrap();
        assert!(retained.days[0].retention_partial);
        assert_eq!(retained.days[0].native.input_tokens, None);
        assert_eq!(retained.days[6].native.input_tokens.as_deref(), Some("7"));
    }
}
