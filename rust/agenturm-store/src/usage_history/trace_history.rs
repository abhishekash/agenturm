//! Read the bounded retained trace and activity-log inputs under one snapshot.

use super::trace_graph::{Classification, DayTraceReports, TraceMember, summarize};
use super::{ActivityLogTotals, CostCounters, Counters, DAY_NS, USAGE_DAYS, UsageHistoryQuery};
use crate::{ObservationKind, Result, StoreError};
use agenturm_kernel::ExtensionId;
use rusqlite::{Connection, params};
use std::collections::BTreeSet;

const MAX_TRACES: usize = 2_048;
const MAX_MEMBERS: usize = 16_384;
const MAX_PER_TRACE: usize = 1_024;
const MAX_LOGS: usize = 8_192;

// Linked telemetry must have the same retained native owner at this watermark.
// This is the same session/turn/worker proof as the raw usage query.
const STRUCTURAL: &str = "o.scope='session' AND json_extract(o.kind_json,'$.kind')='trace_span'
     AND o.revision=0 AND substr(o.source_id,1,5)='otlp.'
     AND o.event_id='otlp:' || json_extract(o.kind_json,'$.trace_id') || ':' ||
        json_extract(o.kind_json,'$.span_id')";

const SCOPE: &str = "
    AND (?4 IS NULL OR o.source_id=?4 OR (
        ?5 IS NOT NULL AND json_extract(o.kind_json,'$.target_source_id')=?4
        AND EXISTS (
            SELECT 1 FROM agenturm_observations anchor
            JOIN agenturm_history_batches ab USING (request_id)
            WHERE anchor.source_id=?4 AND anchor.session_id=?5 AND anchor.scope='session'
                AND ab.sequence<=?3 AND json_extract(anchor.kind_json,'$.kind')<>'trace_span'
                AND (o.turn_id IS NULL OR
                     (anchor.turn_id=o.turn_id AND anchor.worker_id IS o.worker_id))
        )
    ))
    AND (?5 IS NULL OR o.session_id=?5)
    AND (?6 IS NULL OR o.source_id IN (SELECT value FROM json_each(?6)))";

pub(super) struct Window<'a> {
    pub connection: &'a Connection,
    pub query: &'a UsageHistoryQuery,
    pub first: u64,
    pub cutoff: u64,
    pub watermark: i64,
    pub allowed: Option<&'a BTreeSet<ExtensionId>>,
}

#[derive(Default)]
pub(super) struct DayLogReports {
    count: usize,
    errors: usize,
    tokens: Counters,
    costs: CostCounters,
}

impl DayLogReports {
    pub(super) fn totals(self) -> Option<ActivityLogTotals> {
        if self.count == 0 {
            return None;
        }
        let (costs, cost_currencies_truncated) = self.costs.totals();
        Some(ActivityLogTotals {
            logs: self.count,
            reported_errors: self.errors,
            tokens: self.tokens.totals(),
            costs,
            cost_currencies_truncated,
        })
    }
}

pub(super) struct DailyTelemetry {
    pub traces: [DayTraceReports; USAGE_DAYS],
    pub logs: [DayLogReports; USAGE_DAYS],
    pub partial_before_day: Option<usize>,
    pub membership_limited: bool,
}

impl Window<'_> {
    pub(super) fn read(&self) -> Result<DailyTelemetry> {
        let allowed = self.allowed.map(serde_json::to_string).transpose()?;
        let mut totals = DailyTelemetry {
            traces: std::array::from_fn(|_| DayTraceReports::default()),
            logs: std::array::from_fn(|_| DayLogReports::default()),
            partial_before_day: None,
            membership_limited: false,
        };
        self.traces(&allowed, &mut totals)?;
        self.logs(&allowed, &mut totals)?;
        Ok(totals)
    }

    fn day(&self, timestamp: u64) -> Result<usize> {
        timestamp
            .checked_sub(self.first)
            .map(|delta| (delta / DAY_NS) as usize)
            .filter(|day| *day < USAGE_DAYS)
            .ok_or(StoreError::Invalid(
                "telemetry timestamp is outside its usage bucket",
            ))
    }

    fn traces(&self, allowed: &Option<String>, totals: &mut DailyTelemetry) -> Result<()> {
        let cohort_sql = format!(
            "WITH candidates AS MATERIALIZED (
                SELECT o.source_id, json_extract(o.kind_json,'$.trace_id') AS trace_id,
                       o.observed_at_unix_ns AS time
                FROM agenturm_observations o JOIN agenturm_history_batches b USING (request_id)
                WHERE {STRUCTURAL} AND o.observed_at_unix_ns>=?1 AND o.observed_at_unix_ns<?2
                    AND b.sequence<=?3 {SCOPE}
                ORDER BY o.observed_at_unix_ns DESC,o.source_id DESC,o.event_id DESC LIMIT ?7
             ), cohort AS (
                SELECT source_id, trace_id, max(time) AS last FROM candidates
                GROUP BY source_id, trace_id ORDER BY last DESC,source_id,trace_id LIMIT ?8
             )
             SELECT c.source_id, c.trace_id, c.last,
                EXISTS (
                    SELECT 1 FROM agenturm_observations o
                    JOIN agenturm_history_batches b USING (request_id)
                    WHERE {STRUCTURAL} AND o.source_id=c.source_id
                        AND json_extract(o.kind_json,'$.trace_id')=c.trace_id
                        AND o.observed_at_unix_ns>=?2 AND b.sequence<=?3 {SCOPE}
                ),
                CASE WHEN (SELECT count(*) FROM candidates)>=?7
                    THEN (SELECT min(time) FROM candidates) END
             FROM cohort c ORDER BY c.last DESC,c.source_id,c.trace_id"
        );
        let mut cohort = self.connection.prepare(&cohort_sql)?;
        let mut cohorts = cohort.query(params![
            self.first.max(self.cutoff) as i64,
            self.query.end_unix_ns as i64,
            self.watermark,
            self.query.source_id.as_ref().map(ExtensionId::as_str),
            self.query.session_id,
            allowed,
            (MAX_MEMBERS + 1) as i64,
            (MAX_TRACES + 1) as i64,
        ])?;
        let members_sql = format!(
            "SELECT o.kind_json, CASE WHEN sb.sequence<=?3 THEN s.kind_json END,
                    CASE WHEN ub.sequence<=?3 THEN u.kind_json END,
                    CASE WHEN cb.sequence<=?3 THEN c.kind_json END
             FROM agenturm_observations o JOIN agenturm_history_batches b ON b.request_id=o.request_id
             LEFT JOIN agenturm_observations s ON s.source_id=o.source_id AND s.revision=0
                 AND s.event_id=o.event_id || ':summary' AND s.observed_at_unix_ns=o.observed_at_unix_ns
                 AND s.session_id=o.session_id AND s.turn_id IS o.turn_id AND s.worker_id IS o.worker_id
             LEFT JOIN agenturm_history_batches sb ON sb.request_id=s.request_id
             LEFT JOIN agenturm_observations u ON u.source_id=o.source_id AND u.revision=0
                 AND u.event_id='otlp-usage:' || substr(o.event_id,6) AND u.observed_at_unix_ns=o.observed_at_unix_ns
                 AND u.session_id=o.session_id AND u.turn_id IS o.turn_id AND u.worker_id IS o.worker_id
             LEFT JOIN agenturm_history_batches ub ON ub.request_id=u.request_id
             LEFT JOIN agenturm_observations c ON c.source_id=o.source_id AND c.revision=0
                 AND c.event_id='otlp-cost:' || substr(o.event_id,6) AND c.observed_at_unix_ns=o.observed_at_unix_ns
                 AND c.session_id=o.session_id AND c.turn_id IS o.turn_id AND c.worker_id IS o.worker_id
             LEFT JOIN agenturm_history_batches cb ON cb.request_id=c.request_id
             WHERE o.scope='session' AND json_extract(o.kind_json,'$.kind')='trace_span' AND o.revision=0
               AND o.source_id=?1 AND json_extract(o.kind_json,'$.trace_id')=?2 AND b.sequence<=?3
               AND o.event_id='otlp:' || json_extract(o.kind_json,'$.trace_id') || ':' ||
                   json_extract(o.kind_json,'$.span_id') {SCOPE}
             ORDER BY o.observed_at_unix_ns DESC,o.event_id LIMIT ?7"
        );
        let mut members = self.connection.prepare(&members_sql)?;
        let mut remaining = MAX_MEMBERS;
        for index in 0..=MAX_TRACES {
            let Some(group) = cohorts.next()? else { break };
            let day = self.day(group.get(2)?)?;
            if index == 0
                && let Some(cutoff) = group.get::<_, Option<u64>>(4)?
            {
                totals.partial_before_day = Some(self.day(cutoff)?);
            }
            if index == MAX_TRACES {
                totals.partial_before_day = Some(
                    totals
                        .partial_before_day
                        .map_or(day, |other| other.max(day)),
                );
                break;
            }
            if group.get::<_, bool>(3)? {
                continue;
            }
            let capacity = remaining.min(MAX_PER_TRACE);
            let mut group_members = Vec::with_capacity(capacity.min(32));
            let complete = if capacity == 0 {
                false
            } else {
                let source: String = group.get(0)?;
                let trace: String = group.get(1)?;
                let mut rows = members.query(params![
                    source,
                    trace,
                    self.watermark,
                    self.query.source_id.as_ref().map(ExtensionId::as_str),
                    self.query.session_id,
                    allowed,
                    (capacity + 1) as i64,
                ])?;
                while group_members.len() < capacity {
                    let Some(row) = rows.next()? else { break };
                    group_members.push(member(row)?);
                }
                rows.next()?.is_none()
            };
            remaining -= group_members.len();
            totals.membership_limited |= !complete;
            summarize(&group_members, complete, &mut totals.traces[day]);
        }
        Ok(())
    }

    fn logs(&self, allowed: &Option<String>, totals: &mut DailyTelemetry) -> Result<()> {
        let sql = format!(
            "SELECT o.observed_at_unix_ns, o.kind_json
             FROM agenturm_observations o JOIN agenturm_history_batches b USING (request_id)
             WHERE o.scope='session' AND json_extract(o.kind_json,'$.kind')='otlp_activity_log'
               AND o.revision=0 AND substr(o.source_id,1,5)='otlp.'
               AND o.observed_at_unix_ns>=?1 AND o.observed_at_unix_ns<?2 AND b.sequence<=?3 {SCOPE}
               AND NOT EXISTS (
                   SELECT 1 FROM agenturm_observations t
                   JOIN agenturm_history_batches tb USING (request_id)
                   WHERE t.scope='session' AND t.source_id=o.source_id AND t.session_id=o.session_id
                     AND json_extract(t.kind_json,'$.kind')='trace_span' AND t.revision=0 AND tb.sequence<=?3
                     AND t.event_id='otlp:' || json_extract(t.kind_json,'$.trace_id') || ':' ||
                         json_extract(t.kind_json,'$.span_id')
                     AND json_extract(t.kind_json,'$.target_source_id') IS json_extract(o.kind_json,'$.target_source_id')
               )
             ORDER BY o.observed_at_unix_ns DESC,o.source_id,o.event_id LIMIT ?7"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut rows = statement.query(params![
            self.first.max(self.cutoff) as i64,
            self.query.end_unix_ns as i64,
            self.watermark,
            self.query.source_id.as_ref().map(ExtensionId::as_str),
            self.query.session_id,
            allowed,
            (MAX_LOGS + 1) as i64,
        ])?;
        for index in 0..=MAX_LOGS {
            let Some(row) = rows.next()? else { break };
            let day = self.day(row.get(0)?)?;
            if index == MAX_LOGS {
                totals.partial_before_day = Some(
                    totals
                        .partial_before_day
                        .map_or(day, |earlier| earlier.max(day)),
                );
                break;
            }
            let ObservationKind::OtlpActivityLog(log) =
                serde_json::from_str(&row.get::<_, String>(1)?)?
            else {
                return Err(StoreError::Invalid("unexpected retained activity log kind"));
            };
            let bucket = &mut totals.logs[day];
            bucket.count += 1;
            bucket.errors += usize::from(log.reported_error);
            if let Some(usage) = log.usage {
                bucket.tokens.add([
                    usage.input_tokens,
                    usage.output_tokens,
                    usage.cache_read_input_tokens,
                    usage.cache_write_input_tokens,
                    usage.reasoning_output_tokens,
                    usage.total_tokens,
                ]);
            }
            if let Some(cost) = log.cost {
                bucket.costs.add(&cost.currency, cost.nanounits);
            }
        }
        Ok(())
    }
}

fn member(row: &rusqlite::Row<'_>) -> Result<TraceMember> {
    fn optional(row: &rusqlite::Row<'_>, index: usize) -> Result<Option<ObservationKind>> {
        let value: Option<String> = row.get(index)?;
        Ok(value.as_deref().map(serde_json::from_str).transpose()?)
    }

    let ObservationKind::TraceSpan(span) = serde_json::from_str(&row.get::<_, String>(0)?)? else {
        return Err(StoreError::Invalid("unexpected retained trace span kind"));
    };
    let classification = match optional(row, 1)? {
        Some(ObservationKind::TraceSpanSummary(summary))
            if summary.trace_id == span.trace_id && summary.span_id == span.span_id =>
        {
            Some(Classification {
                agent_run: summary.agent_run,
                reported_error: summary.reported_error,
            })
        }
        _ => None,
    };
    let usage = match optional(row, 2)? {
        Some(ObservationKind::SpanTokenUsage(usage))
            if usage.span.trace_id == span.trace_id && usage.span.span_id == span.span_id =>
        {
            Some(usage)
        }
        _ => None,
    };
    let cost = match optional(row, 3)? {
        Some(ObservationKind::ReportedCost(cost))
            if cost
                .span
                .as_ref()
                .is_some_and(|id| id.trace_id == span.trace_id && id.span_id == span.span_id) =>
        {
            Some(cost)
        }
        _ => None,
    };
    Ok(TraceMember {
        span,
        classification,
        usage,
        cost,
    })
}
