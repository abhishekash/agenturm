//! Keyset reads of exact retained OTLP metric series, points, and exemplars.

use crate::{
    MAX_EVENT_ID_BYTES, ObservationBatch, ObservationKind, Result, Store, StoreError, StoredScope,
    bounded_text_preview, retention, validate_identifier,
};
use agenturm_kernel::metric::{
    MAX_METRIC_DESCRIPTION_BYTES, MAX_METRIC_EXEMPLARS, MetricDescriptor, MetricPoint,
    MetricSeries, is_metric_description, is_metric_series_id, is_metric_source,
};
use agenturm_kernel::{ExtensionId, ReadSnapshot, TraceSpanIdentity};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub use agenturm_kernel::retained_history::{
    MAX_METRIC_HISTORY_PAGE, MetricExemplarTrace, MetricHistoryPage, MetricHistoryQuery,
    MetricPointEntry, MetricPointPosition, MetricSeriesEntry, MetricSeriesKey,
    MetricSeriesPosition,
};

const INDEX_SQL: &str = "CREATE INDEX agenturm_metric_series_points ON agenturm_observations(
            source_id, json_extract(kind_json, '$.series.id'), observed_at_unix_ns DESC, event_id DESC)
         WHERE json_extract(kind_json, '$.kind') = 'metric_point';";

pub(super) fn create_index(connection: &Connection) -> Result<()> {
    connection.execute_batch(INDEX_SQL)?;
    Ok(())
}

pub(super) fn validate_index(connection: &Connection) -> Result<()> {
    let sql: String = connection.query_row(
        "SELECT sql FROM sqlite_schema WHERE type='index' AND name='agenturm_metric_series_points'",
        [],
        |row| row.get(0),
    )?;
    if sql != INDEX_SQL.trim_end_matches(';') {
        return Err(StoreError::UnrecognizedDatabase);
    }
    Ok(())
}

const DESCRIPTOR_INDEX_SQL: &str =
    "CREATE INDEX agenturm_metric_descriptions_by_series ON agenturm_observations(
            source_id, json_extract(kind_json, '$.series_id'), request_id)
         WHERE json_extract(kind_json, '$.kind') = 'metric_descriptor';";

pub(super) fn create_descriptor_index(connection: &Connection) -> Result<()> {
    connection.execute_batch(DESCRIPTOR_INDEX_SQL)?;
    Ok(())
}

pub(super) fn validate_descriptor_index(connection: &Connection) -> Result<()> {
    let sql: String = connection.query_row(
        "SELECT sql FROM sqlite_schema WHERE type='index' AND name='agenturm_metric_descriptions_by_series'",
        [],
        |row| row.get(0),
    )?;
    if sql != DESCRIPTOR_INDEX_SQL.trim_end_matches(';') {
        return Err(StoreError::UnrecognizedDatabase);
    }
    Ok(())
}

pub fn metric_descriptor_event_id(descriptor: &MetricDescriptor) -> String {
    let mut digest = Sha256::new();
    digest.update(b"otlp.metric.descriptor/v1\0");
    digest.update(descriptor.point_event_id.as_bytes());
    digest.update([0]);
    digest.update(descriptor.description.as_bytes());
    format!("otlp-metric-description:{:x}", digest.finalize())
}

pub fn valid_metric_description(description: &str) -> bool {
    is_metric_description(description)
        && bounded_text_preview(description.into(), MAX_METRIC_DESCRIPTION_BYTES)
            .is_ok_and(|safe| safe == description)
}

pub(super) fn validate_descriptor_batch(batch: &ObservationBatch) -> Result<()> {
    if !batch
        .observations
        .iter()
        .any(|row| matches!(row.kind, ObservationKind::MetricDescriptor(_)))
    {
        return Ok(());
    }
    if !is_metric_source(&batch.source_id) {
        return Err(StoreError::Invalid(
            "metric descriptor requires an OTLP source",
        ));
    }
    let points: BTreeMap<_, _> = batch
        .observations
        .iter()
        .filter_map(|row| {
            if let ObservationKind::MetricPoint(point) = &row.kind {
                Some((
                    row.event_id.as_str(),
                    (point.series.id.as_str(), row.observed_at_unix_ns),
                ))
            } else {
                None
            }
        })
        .collect();
    let mut described = BTreeSet::new();
    for row in &batch.observations {
        let ObservationKind::MetricDescriptor(descriptor) = &row.kind else {
            continue;
        };
        if row.identity.scope != StoredScope::Source
            || row.revision != 0
            || !descriptor.is_valid_at(row.observed_at_unix_ns)
            || !valid_metric_description(&descriptor.description)
            || row.event_id != metric_descriptor_event_id(descriptor)
            || points.get(descriptor.point_event_id.as_str()).copied()
                != Some((descriptor.series_id.as_str(), row.observed_at_unix_ns))
            || !described.insert(descriptor.series_id.as_str())
        {
            return Err(StoreError::Invalid(
                "metric descriptor needs one valid description and an exact metric point in the batch",
            ));
        }
    }
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
                search,
                limit,
            } => {
                validate_limit(*limit)?;
                if search
                    .as_deref()
                    .is_some_and(|value| !MetricHistoryQuery::valid_search(value))
                {
                    return Err(StoreError::Invalid("metric catalog search is invalid"));
                }
                if let Some(cursor) = after {
                    validate_series(&cursor.series, allowed_sources)?;
                    if *snapshot_id != Some(cursor.snapshot_id)
                        || &cursor.source_filter != source_id
                        || &cursor.search != search
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
                    CatalogSelection {
                        after: after.as_ref(),
                        search: search.as_deref(),
                        limit: *limit,
                        allowed_sources,
                    },
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

struct CatalogSelection<'a> {
    after: Option<&'a MetricSeriesPosition>,
    search: Option<&'a str>,
    limit: usize,
    allowed_sources: Option<&'a BTreeSet<ExtensionId>>,
}

fn catalog(
    connection: &Connection,
    source: Option<&ExtensionId>,
    snapshot: ReadSnapshot,
    watermark: i64,
    selection: CatalogSelection<'_>,
) -> Result<MetricHistoryPage> {
    let CatalogSelection {
        after,
        search,
        limit,
        allowed_sources,
    } = selection;
    let mut statement = connection.prepare(
        "WITH series AS (
            SELECT o.source_id, json_extract(o.kind_json, '$.series.id') AS series_id,
                   json_extract(o.kind_json, '$.series') AS metadata,
                   max(o.observed_at_unix_ns) AS latest_unix_ns
              FROM agenturm_observations o JOIN agenturm_history_batches b USING (request_id)
             WHERE json_extract(o.kind_json, '$.kind')='metric_point' AND b.sequence<=?1
               AND (?2 IS NULL OR o.source_id=?2)
               AND (?3 IS NULL OR (o.source_id, json_extract(o.kind_json, '$.series.id')) > (?3, ?4))
               AND (?5 IS NULL OR o.source_id IN (SELECT value FROM json_each(?5)))
             GROUP BY o.source_id, series_id
        ),
        catalog AS (
            SELECT series.*,
                   (SELECT json_extract(d.kind_json, '$.description')
                      FROM agenturm_observations d
                      JOIN agenturm_history_batches h USING (request_id)
                     WHERE json_extract(d.kind_json, '$.kind')='metric_descriptor'
                       AND d.source_id=series.source_id
                       AND json_extract(d.kind_json, '$.series_id')=series.series_id
                       AND h.sequence<=?1
                     ORDER BY h.sequence DESC LIMIT 1) AS description
              FROM series
        )
        SELECT source_id, series_id, metadata, latest_unix_ns, description
          FROM catalog
         WHERE ?6 IS NULL
            OR instr(lower(source_id), lower(?6))>0
            OR instr(lower(json_extract(metadata, '$.name')), lower(?6))>0
            OR instr(lower(json_extract(metadata, '$.service')), lower(?6))>0
            OR instr(lower(json_extract(metadata, '$.kind')), lower(?6))>0
            OR instr(lower(json_extract(metadata, '$.unit')), lower(?6))>0
            OR instr(lower(json_extract(metadata, '$.scope')), lower(?6))>0
            OR instr(lower(description), lower(?6))>0
         ORDER BY source_id, series_id LIMIT ?7",
    )?;
    let parameters = params![
        watermark,
        source.map(ExtensionId::as_str),
        after.map(|item| item.series.source_id.as_str()),
        after.map(|item| item.series.series_id.as_str()),
        allowed_sources.map(serde_json::to_string).transpose()?,
        search,
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
        let description: Option<String> = row.get(4)?;
        if description
            .as_deref()
            .is_some_and(|description| !valid_metric_description(description))
        {
            return Err(StoreError::Invalid("stored metric description is invalid"));
        }
        entries.push(MetricSeriesEntry {
            key,
            series,
            description,
            latest_unix_ns: row.get(3)?,
        });
    }
    let next = entries
        .last()
        .filter(|_| has_more)
        .map(|entry| MetricSeriesPosition {
            series: entry.key.clone(),
            snapshot_id: snapshot,
            source_filter: source.cloned(),
            search: search.map(str::to_owned),
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

    fn descriptor(anchor: &StoredObservation, description: &str) -> StoredObservation {
        let ObservationKind::MetricPoint(point) = &anchor.kind else {
            panic!("metric anchor");
        };
        let descriptor = MetricDescriptor {
            series_id: point.series.id.clone(),
            point_event_id: anchor.event_id.clone(),
            description: description.into(),
        };
        StoredObservation {
            event_id: metric_descriptor_event_id(&descriptor),
            revision: 0,
            identity: anchor.identity.clone(),
            observed_at_unix_ns: anchor.observed_at_unix_ns,
            kind: ObservationKind::MetricDescriptor(descriptor),
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
    fn v20_metric_point_preserves_event_and_series_identity_through_upgrade() {
        use std::fs;
        use std::path::PathBuf;
        use std::time::{SystemTime, UNIX_EPOCH};

        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let directory = std::env::temp_dir().join(format!(
            "agenturm-metric-upgrade-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let scratch = Scratch(directory);
        let path = scratch.0.join("history.sqlite");
        let mut store = Store::open(&path).unwrap();
        let original = point(10, &"a".repeat(64), 42);
        commit(&mut store, OPEN, None, "retained", vec![original.clone()]);
        let expected = [(OPEN.to_owned(), original)];
        assert_eq!(store.observations().unwrap(), expected);
        store.connection.execute_batch(
            "DROP INDEX agenturm_metric_descriptions_by_series;
             ALTER TABLE agenturm_history_batches DROP COLUMN admitted_at_unix_ns;
             UPDATE agenturm_store_meta SET value='agenturm.observation-store.v20' WHERE key='format';
             PRAGMA user_version=20;",
        ).unwrap();
        drop(store);

        let schema = |store: &Store| {
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap()
        };
        let reader = Store::open_read_only(&path).unwrap();
        assert_eq!(schema(&reader), 20);
        assert_eq!(reader.observations().unwrap(), expected);
        drop(reader);

        let upgraded = Store::open(&path).unwrap();
        assert_eq!(schema(&upgraded), crate::SCHEMA_VERSION);
        assert_eq!(upgraded.observations().unwrap(), expected);
    }

    #[test]
    fn literal_catalog_search_runs_before_paging_and_binds_cursor_and_acl() {
        let mut store = Store::open(":memory:").unwrap();
        let source = ExtensionId::parse(OPEN).unwrap();
        let allowed = [source.clone()].into_iter().collect();
        let first = "0".repeat(64);
        let second = "1".repeat(64);
        let third = "2".repeat(64);
        let second_point = point(2, &second, 0);
        let third_point = point(3, &third, 0);
        commit(
            &mut store,
            OPEN,
            None,
            "catalog",
            vec![
                point(1, &first, 0),
                descriptor(&second_point, "Queue latency 95%"),
                second_point,
                descriptor(&third_point, "Queue latency 50%"),
                third_point,
            ],
        );
        let outside = point(9, &first, 0);
        commit(
            &mut store,
            CLOSED,
            None,
            "outside",
            vec![descriptor(&outside, "Queue latency 75%"), outside],
        );

        let query =
            |source_id, snapshot_id, after, search: Option<&str>| MetricHistoryQuery::Series {
                source_id,
                snapshot_id,
                after,
                search: search.map(str::to_owned),
                limit: 1,
            };
        let catalog = |query| store.query_metric_history(&query, Some(&allowed)).unwrap();
        let MetricHistoryPage::Series { rows, .. } = catalog(query(None, None, None, None)) else {
            panic!("metric catalog");
        };
        assert_eq!(rows[0].key.series_id, first);
        assert_eq!(rows[0].description, None);

        let MetricHistoryPage::Series {
            snapshot_id,
            rows,
            next: Some(next),
        } = catalog(query(Some(source.clone()), None, None, Some("LaTeNcY")))
        else {
            panic!("filtered metric catalog continuation");
        };
        assert_eq!(rows[0].key.series_id, second);
        assert_eq!(rows[0].description.as_deref(), Some("Queue latency 95%"));

        let MetricHistoryPage::Series {
            rows, next: None, ..
        } = catalog(query(
            Some(source.clone()),
            Some(snapshot_id),
            Some(next.clone()),
            Some("LaTeNcY"),
        ))
        else {
            panic!("exact next match");
        };
        assert_eq!(rows[0].key.series_id, third);
        for wrong in [
            query(
                Some(source.clone()),
                Some(snapshot_id),
                Some(next.clone()),
                Some("%"),
            ),
            query(None, Some(snapshot_id), Some(next.clone()), Some("LaTeNcY")),
            query(Some(source), None, Some(next), Some("LaTeNcY")),
        ] {
            assert!(store.query_metric_history(&wrong, Some(&allowed)).is_err());
        }

        let MetricHistoryPage::Series { rows, .. } = catalog(query(None, None, None, Some("95%")))
        else {
            panic!("literal wildcard character");
        };
        assert_eq!(rows[0].key.series_id, second);
        let MetricHistoryPage::Series {
            rows, next: None, ..
        } = catalog(query(None, None, None, Some("75%")))
        else {
            panic!("denied description invisible");
        };
        assert!(rows.is_empty());
    }

    #[test]
    fn descriptor_requires_a_same_batch_point_and_rejects_ambiguous_updates() {
        let mut store = Store::open(":memory:").unwrap();
        let point = point(4, &"f".repeat(64), 5);
        let described = descriptor(&point, "A valid description");
        for invalid in [
            vec![described.clone()],
            vec![
                point.clone(),
                described.clone(),
                descriptor(&point, "Other description"),
            ],
            vec![point.clone(), descriptor(&point, "Password: unsafe")],
        ] {
            assert!(
                ObservationBatch::new_deterministic(OPEN, None, "invalid", invalid.clone())
                    .is_err()
            );
            let batch = ObservationBatch {
                source_id: OPEN.into(),
                request_id: "invalid-request".into(),
                expected_cursor: None,
                next_cursor: "invalid".into(),
                observations: invalid,
            };
            assert!(store.commit_batch(&batch).is_err());
            assert_eq!(store.source_cursor(OPEN).unwrap(), None);
        }
        commit(&mut store, OPEN, None, "valid", vec![point, described]);
    }

    #[test]
    fn a_descriptor_only_durable_page_does_not_require_its_point_in_that_page() {
        use agenturm_kernel::{Kernel, ObservationPosition, ObservationQuery};
        let mut store = Store::open(":memory:").unwrap();
        let anchor = point(5, &"f".repeat(64), 2);
        let described = descriptor(&anchor, "A retained public description");
        commit(
            &mut store,
            OPEN,
            None,
            "described",
            vec![anchor.clone(), described.clone()],
        );
        let page = store.observation_page(1).unwrap();
        assert_eq!(page.observations[0].event_id, anchor.event_id);
        assert!(page.has_more);
        let query = ObservationQuery {
            harness_id: Some(OPEN.into()),
            session_id: None,
            before: Some(ObservationPosition {
                observed_at_unix_ns: anchor.observed_at_unix_ns,
                harness_id: OPEN.into(),
                event_id: anchor.event_id,
                revision: anchor.revision,
                snapshot_id: page.snapshot_id,
            }),
            snapshot_id: Some(page.snapshot_id),
            limit: 1,
        };
        let snapshot = Kernel::new().refresh_query_with_store(&store, query);
        assert!(snapshot.failures.is_empty());
        assert_eq!(snapshot.observations.len(), 1);
        assert_eq!(snapshot.observations[0].event_id, described.event_id);
        assert!(matches!(
            snapshot.observations[0].kind,
            agenturm_kernel::ObservationKind::MetricDescriptor(_)
        ));
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
            search: None,
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
