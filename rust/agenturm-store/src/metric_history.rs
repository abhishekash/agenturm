//! Keyset reads of exact retained OTLP metric series, points, and exemplars.

use crate::{
    MAX_EVENT_ID_BYTES, ObservationKind, Result, Store, StoreError, retention, validate_identifier,
};
use agenturm_kernel::metric::{
    MAX_METRIC_EXEMPLARS, MetricPoint, MetricSeries, is_metric_series_id, is_metric_source,
};
use agenturm_kernel::{ExtensionId, ReadSnapshot, TraceSpanIdentity};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeSet;

pub use agenturm_kernel::retained_history::{
    MAX_METRIC_HISTORY_PAGE, MetricExemplarTrace, MetricHistoryPage, MetricHistoryQuery,
    MetricPointEntry, MetricPointPosition, MetricSeriesEntry, MetricSeriesKey,
    MetricSeriesPosition,
};

pub(super) fn create_index(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE INDEX agenturm_metric_series_points ON agenturm_observations(
            source_id, json_extract(kind_json, '$.series.id'), observed_at_unix_ns DESC, event_id DESC)
         WHERE json_extract(kind_json, '$.kind') = 'metric_point';",
    )?;
    Ok(())
}

impl Store {
    pub fn query_metric_history(
        &self,
        query: &MetricHistoryQuery,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
    ) -> Result<MetricHistoryPage> {
        if let Some(source) = query.source_id() {
            validate_source(source, allowed_sources)?;
        }
        let transaction = self.connection.unchecked_transaction()?;
        match query {
            MetricHistoryQuery::Series {
                source_id,
                snapshot_id,
                after,
                limit,
            } => {
                validate_limit(*limit)?;
                if let Some(cursor) = after {
                    validate_series(&cursor.series, allowed_sources)?;
                    if *snapshot_id != Some(cursor.snapshot_id)
                        || source_id
                            .as_ref()
                            .is_some_and(|source| source != &cursor.series.source_id)
                    {
                        return Err(StoreError::Invalid(
                            "metric catalog cursor does not match its scope",
                        ));
                    }
                }
                let (snapshot, watermark) =
                    retention::resolve_read_snapshot(&transaction, *snapshot_id)?;
                catalog(
                    &transaction,
                    source_id.as_ref(),
                    snapshot,
                    watermark,
                    after.as_ref(),
                    *limit,
                    allowed_sources,
                )
            }
            MetricHistoryQuery::Points {
                series,
                snapshot_id,
                before,
                limit,
            } => {
                validate_limit(*limit)?;
                validate_series(series, allowed_sources)?;
                if let Some(cursor) = before {
                    validate_position(cursor, allowed_sources)?;
                    if cursor.snapshot_id != *snapshot_id || cursor.series != *series {
                        return Err(StoreError::Invalid(
                            "metric point cursor does not match its scope",
                        ));
                    }
                }
                let (snapshot, watermark) =
                    retention::resolve_read_snapshot(&transaction, Some(*snapshot_id))?;
                points(
                    &transaction,
                    series,
                    snapshot,
                    watermark,
                    before.as_ref(),
                    *limit,
                )
            }
            MetricHistoryQuery::Exemplar {
                point,
                exemplar_index,
            } => {
                validate_position(point, allowed_sources)?;
                if *exemplar_index >= MAX_METRIC_EXEMPLARS {
                    return Err(StoreError::Invalid("metric exemplar index is invalid"));
                }
                let (_, watermark) =
                    retention::resolve_read_snapshot(&transaction, Some(point.snapshot_id))?;
                Ok(MetricHistoryPage::Exemplar {
                    trace: exemplar(&transaction, point, *exemplar_index, watermark)?.map(Box::new),
                })
            }
        }
    }
}

fn validate_source(
    source: &ExtensionId,
    allowed_sources: Option<&BTreeSet<ExtensionId>>,
) -> Result<()> {
    if !is_metric_source(source.as_str())
        || allowed_sources.is_some_and(|allowed| !allowed.contains(source))
    {
        return Err(StoreError::Invalid("metric source is unavailable"));
    }
    Ok(())
}

fn validate_series(
    series: &MetricSeriesKey,
    allowed_sources: Option<&BTreeSet<ExtensionId>>,
) -> Result<()> {
    validate_source(&series.source_id, allowed_sources)?;
    if !is_metric_series_id(&series.series_id) {
        return Err(StoreError::Invalid("metric series identifier is invalid"));
    }
    Ok(())
}

fn validate_position(
    point: &MetricPointPosition,
    allowed_sources: Option<&BTreeSet<ExtensionId>>,
) -> Result<()> {
    validate_series(&point.series, allowed_sources)?;
    validate_identifier(&point.event_id, MAX_EVENT_ID_BYTES, "metric event ID")?;
    if point.observed_at_unix_ns == 0 || point.observed_at_unix_ns > i64::MAX as u64 {
        return Err(StoreError::Invalid(
            "metric observation timestamp is invalid",
        ));
    }
    Ok(())
}

fn validate_limit(limit: usize) -> Result<()> {
    if !(1..=MAX_METRIC_HISTORY_PAGE).contains(&limit) {
        return Err(StoreError::Invalid("metric page limit is invalid"));
    }
    Ok(())
}

fn catalog(
    connection: &Connection,
    source: Option<&ExtensionId>,
    snapshot: ReadSnapshot,
    watermark: i64,
    after: Option<&MetricSeriesPosition>,
    limit: usize,
    allowed_sources: Option<&BTreeSet<ExtensionId>>,
) -> Result<MetricHistoryPage> {
    let mut statement = connection.prepare(
        "SELECT o.source_id, json_extract(o.kind_json, '$.series.id') AS series_id,
                json_extract(o.kind_json, '$.series'), max(o.observed_at_unix_ns)
         FROM agenturm_observations o JOIN agenturm_history_batches b USING (request_id)
         WHERE json_extract(o.kind_json, '$.kind')='metric_point' AND b.sequence<=?1
           AND (?2 IS NULL OR o.source_id=?2)
           AND (?3 IS NULL OR (o.source_id, json_extract(o.kind_json, '$.series.id')) > (?3, ?4))
           AND (?5 IS NULL OR o.source_id IN (SELECT value FROM json_each(?5)))
         GROUP BY o.source_id, series_id ORDER BY o.source_id, series_id LIMIT ?6",
    )?;
    let parameters = params![
        watermark,
        source.map(ExtensionId::as_str),
        after.map(|item| item.series.source_id.as_str()),
        after.map(|item| item.series.series_id.as_str()),
        allowed_sources.map(serde_json::to_string).transpose()?,
        (limit + 1) as i64,
    ];
    let mut cursor = statement.query(parameters)?;
    let mut entries = Vec::with_capacity(limit);
    let mut has_more = false;
    while let Some(row) = cursor.next()? {
        if entries.len() == limit {
            has_more = true;
            break;
        }
        let source_id = ExtensionId::parse(row.get::<_, String>(0)?)
            .map_err(|_| StoreError::Invalid("stored metric source is invalid"))?;
        let key = MetricSeriesKey {
            source_id,
            series_id: row.get(1)?,
        };
        let series: MetricSeries = serde_json::from_str(&row.get::<_, String>(2)?)?;
        if series.id != key.series_id || !series.is_valid() {
            return Err(StoreError::Invalid("stored metric series is invalid"));
        }
        entries.push(MetricSeriesEntry {
            key,
            series,
            latest_unix_ns: row.get(3)?,
        });
    }
    let next = entries
        .last()
        .filter(|_| has_more)
        .map(|entry| MetricSeriesPosition {
            series: entry.key.clone(),
            snapshot_id: snapshot,
        });
    Ok(MetricHistoryPage::Series {
        snapshot_id: snapshot,
        rows: entries,
        next,
    })
}

fn points(
    connection: &Connection,
    series: &MetricSeriesKey,
    snapshot: ReadSnapshot,
    watermark: i64,
    before: Option<&MetricPointPosition>,
    limit: usize,
) -> Result<MetricHistoryPage> {
    let mut statement = connection.prepare(
        "SELECT o.event_id, o.observed_at_unix_ns, o.kind_json
         FROM agenturm_observations o JOIN agenturm_history_batches b USING (request_id)
         WHERE o.source_id=?1 AND json_extract(o.kind_json, '$.kind')='metric_point'
           AND json_extract(o.kind_json, '$.series.id')=?2 AND b.sequence<=?3
           AND (?4 IS NULL OR (o.observed_at_unix_ns, o.event_id) < (?4, ?5))
         ORDER BY o.observed_at_unix_ns DESC, o.event_id DESC LIMIT ?6",
    )?;
    let mut rows = statement.query(params![
        series.source_id.as_str(),
        series.series_id,
        watermark,
        before.map(|point| point.observed_at_unix_ns as i64),
        before.map(|point| point.event_id.as_str()),
        (limit + 1) as i64,
    ])?;
    let mut entries = Vec::with_capacity(limit);
    let mut has_more = false;
    while let Some(row) = rows.next()? {
        if entries.len() == limit {
            has_more = true;
            break;
        }
        let position = MetricPointPosition {
            series: series.clone(),
            snapshot_id: snapshot,
            observed_at_unix_ns: row.get(1)?,
            event_id: row.get(0)?,
        };
        entries.push(MetricPointEntry {
            point: read_point(&row.get::<_, String>(2)?, &position)?,
            position,
        });
    }
    let next = entries
        .last()
        .filter(|_| has_more)
        .map(|entry| entry.position.clone());
    Ok(MetricHistoryPage::Points {
        snapshot_id: snapshot,
        rows: entries,
        next,
    })
}

fn read_point(json: &str, position: &MetricPointPosition) -> Result<MetricPoint> {
    let ObservationKind::MetricPoint(point) = serde_json::from_str(json)? else {
        return Err(StoreError::Invalid("stored metric point is invalid"));
    };
    if point.series.id != position.series.series_id
        || !point.is_valid_at(position.observed_at_unix_ns)
        || !point.matches_event(&position.event_id, position.observed_at_unix_ns)
    {
        return Err(StoreError::Invalid(
            "stored metric point does not match its position",
        ));
    }
    Ok(point)
}

fn exemplar(
    connection: &Connection,
    position: &MetricPointPosition,
    index: usize,
    watermark: i64,
) -> Result<Option<MetricExemplarTrace>> {
    let source = &position.series.source_id;
    let json: Option<String> = connection.query_row(
        "SELECT o.kind_json FROM agenturm_observations o JOIN agenturm_history_batches b USING (request_id)
         WHERE o.source_id=?1 AND o.event_id=?2 AND o.revision=0
            AND o.observed_at_unix_ns=?3 AND b.sequence<=?4",
        params![source.as_str(), position.event_id, position.observed_at_unix_ns as i64, watermark],
        |row| row.get(0),
    ).optional()?;
    let Some(json) = json else { return Ok(None) };
    let point = read_point(&json, position)?;
    let Some(exemplar) = point.exemplars.get(index) else {
        return Ok(None);
    };
    let (Some(trace), Some(span)) = (&exemplar.trace_id, &exemplar.span_id) else {
        return Ok(None);
    };
    let event_id = format!("otlp:{trace}:{span}");
    let retained = connection.query_row(
        "SELECT o.session_id, o.turn_id, o.worker_id, o.kind_json
         FROM agenturm_observations o JOIN agenturm_history_batches b USING (request_id)
         WHERE o.source_id=?1 AND o.event_id=?2 AND o.revision=0 AND o.scope='session' AND b.sequence<=?3",
        params![source.as_str(), event_id, watermark],
        |row| Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?, row.get::<_, String>(3)?)),
    ).optional()?;
    let Some((session_id, turn_id, worker_id, json)) = retained else {
        return Ok(None);
    };
    let ObservationKind::TraceSpan(body) = serde_json::from_str(&json)? else {
        return Ok(None);
    };
    let identity = TraceSpanIdentity {
        trace_id: body.trace_id.clone(),
        span_id: body.span_id.clone(),
    };
    if !identity.is_valid() || !identity.matches_event("otlp:", &event_id) {
        return Ok(None);
    }
    Ok(Some(MetricExemplarTrace {
        snapshot_id: position.snapshot_id,
        source_id: source.clone(),
        session_id,
        turn_id,
        worker_id,
        span: body,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CommitOutcome, ObservationBatch, ObservationIdentity, StoredObservation, StoredScope,
    };
    use agenturm_kernel::metric::{
        MetricData, MetricExemplar, MetricKind, MetricNumber, MetricTemporality,
    };
    use agenturm_kernel::{TraceSpan, TraceStatus};

    const TRACE_ID: &str = "1234567890abcdef1234567890abcdef";
    const SPAN_ID: &str = "1234567890abcdef";
    const OPEN: &str = "otlp.fixture";
    const CLOSED: &str = "otlp.other";

    fn point(at: u64, series: &str, value: i64) -> StoredObservation {
        let body = MetricPoint {
            series: MetricSeries {
                id: series.into(),
                name: "agent.requests".into(),
                unit: "{request}".into(),
                service: Some("agenturm-test".into()),
                scope: None,
                kind: MetricKind::Gauge,
                temporality: MetricTemporality::Unspecified,
                monotonic: None,
            },
            start_unix_ns: None,
            flags: 0,
            data: MetricData::Gauge {
                value: Some(MetricNumber::Int(value)),
            },
            exemplars: vec![MetricExemplar {
                time_unix_ns: at,
                value: Some(MetricNumber::Int(value)),
                trace_id: Some(TRACE_ID.into()),
                span_id: Some(SPAN_ID.into()),
            }],
        };
        StoredObservation {
            event_id: body.event_id(at),
            revision: 0,
            identity: ObservationIdentity {
                scope: StoredScope::Source,
                session_id: None,
                worker_id: None,
                turn_id: None,
                operation_id: None,
            },
            observed_at_unix_ns: at,
            kind: ObservationKind::MetricPoint(body),
        }
    }

    fn span() -> StoredObservation {
        StoredObservation {
            event_id: format!("otlp:{TRACE_ID}:{SPAN_ID}"),
            revision: 0,
            identity: ObservationIdentity {
                scope: StoredScope::Session,
                session_id: Some("fixture-session".into()),
                worker_id: Some("fixture-worker".into()),
                turn_id: Some("fixture-turn".into()),
                operation_id: None,
            },
            observed_at_unix_ns: 4,
            kind: ObservationKind::TraceSpan(TraceSpan {
                trace_id: TRACE_ID.into(),
                span_id: SPAN_ID.into(),
                parent_span_id: None,
                target_source_id: Some("harness.fixture".into()),
                target_operation_id: None,
                started_at_unix_ns: 1,
                status: TraceStatus::Ok,
            }),
        }
    }

    fn commit(
        store: &mut Store,
        source: &str,
        before: Option<&str>,
        next: &str,
        rows: Vec<StoredObservation>,
    ) -> ObservationBatch {
        let batch =
            ObservationBatch::new_deterministic(source, before.map(str::to_owned), next, rows)
                .unwrap();
        assert!(matches!(
            store.commit_batch(&batch).unwrap(),
            CommitOutcome::Committed { .. }
        ));
        batch
    }

    #[test]
    fn pinned_pages_and_exact_source_exemplar_never_resolve_a_sibling() {
        let mut store = Store::open(":memory:").unwrap();
        let source = ExtensionId::parse(OPEN).unwrap();
        let allowed = [source.clone()].into_iter().collect();
        let first = "0".repeat(64);
        let second = "1".repeat(64);
        let original = commit(
            &mut store,
            OPEN,
            None,
            "first",
            vec![
                point(2, &first, 0),
                point(3, &first, -2),
                point(2, &second, 9),
            ],
        );
        assert_eq!(
            store.commit_batch(&original).unwrap(),
            CommitOutcome::Replay
        );
        commit(
            &mut store,
            CLOSED,
            None,
            "closed",
            vec![span(), point(4, &first, 8)],
        );
        let series_page = |snapshot_id, after| MetricHistoryQuery::Series {
            source_id: None,
            snapshot_id,
            after,
            limit: 1,
        };
        let MetricHistoryPage::Series {
            snapshot_id,
            rows,
            next,
        } = store
            .query_metric_history(&series_page(None, None), Some(&allowed))
            .unwrap()
        else {
            panic!("metric catalog")
        };
        assert_eq!(rows[0].key.source_id, source);
        assert_eq!(rows[0].key.series_id, first);
        assert_eq!(rows[0].latest_unix_ns, 3);
        let series = rows[0].key.clone();
        let MetricHistoryPage::Series {
            rows, next: None, ..
        } = store
            .query_metric_history(&series_page(Some(snapshot_id), next), Some(&allowed))
            .unwrap()
        else {
            panic!("next metric catalog page")
        };
        assert_eq!(rows[0].key.series_id, second);
        let points_page = |snapshot_id, before| MetricHistoryQuery::Points {
            series: series.clone(),
            snapshot_id,
            before,
            limit: 1,
        };
        let MetricHistoryPage::Points { rows, next, .. } = store
            .query_metric_history(&points_page(snapshot_id, None), Some(&allowed))
            .unwrap()
        else {
            panic!("metric points")
        };
        let old_position = rows[0].position.clone();
        assert_eq!(old_position.observed_at_unix_ns, 3);
        let exemplar = |point| MetricHistoryQuery::Exemplar {
            point,
            exemplar_index: 0,
        };
        assert_eq!(
            store
                .query_metric_history(&exemplar(old_position.clone()), Some(&allowed))
                .unwrap(),
            MetricHistoryPage::Exemplar { trace: None },
        );
        commit(
            &mut store,
            OPEN,
            Some("first"),
            "second",
            vec![span(), point(5, &first, 7)],
        );
        assert_eq!(
            store
                .query_metric_history(&exemplar(old_position.clone()), Some(&allowed))
                .unwrap(),
            MetricHistoryPage::Exemplar { trace: None },
        );
        let MetricHistoryPage::Points {
            rows, next: None, ..
        } = store
            .query_metric_history(&points_page(snapshot_id, next), Some(&allowed))
            .unwrap()
        else {
            panic!("pinned earlier metric point")
        };
        assert_eq!(
            rows[0].point.data,
            MetricData::Gauge {
                value: Some(MetricNumber::Int(0))
            }
        );
        let MetricHistoryPage::Series {
            snapshot_id: fresh, ..
        } = store
            .query_metric_history(&series_page(None, None), Some(&allowed))
            .unwrap()
        else {
            panic!("current metric snapshot")
        };
        let updated = MetricPointPosition {
            snapshot_id: fresh,
            ..old_position
        };
        let MetricHistoryPage::Exemplar { trace: Some(trace) } = store
            .query_metric_history(&exemplar(updated.clone()), Some(&allowed))
            .unwrap()
        else {
            panic!("exact source trace span")
        };
        assert_eq!(trace.source_id, source);
        assert_eq!(trace.session_id, "fixture-session");
        assert_eq!(trace.span.span_id, SPAN_ID);
        let other = MetricPointPosition {
            series: MetricSeriesKey {
                source_id: ExtensionId::parse(CLOSED).unwrap(),
                ..updated.series
            },
            ..updated
        };
        assert!(
            store
                .query_metric_history(&exemplar(other), Some(&allowed))
                .is_err()
        );
    }
}
