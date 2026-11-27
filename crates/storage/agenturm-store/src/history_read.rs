//! Ordered history pages and sparse roster facts must not sort the whole store.
use crate::{Result, StoreError};
use rusqlite::Connection;

pub(super) const INDEX_NAMES: &[&str] = &[
    "agenturm_observations_by_time",
    "agenturm_observations_by_source_time",
    "agenturm_session_facts_by_kind",
];

const INDEXES: [&str; 3] = [
    "CREATE INDEX agenturm_observations_by_time ON agenturm_observations(
        observed_at_unix_ns DESC, source_id DESC, event_id DESC, revision DESC)",
    "CREATE INDEX agenturm_observations_by_source_time ON agenturm_observations(
        source_id, observed_at_unix_ns DESC, event_id DESC, revision DESC)",
    "CREATE INDEX agenturm_session_facts_by_kind ON agenturm_observations(
        source_id, session_id, json_extract(kind_json, '$.kind'),
        observed_at_unix_ns DESC, event_id DESC, revision DESC)
     WHERE json_extract(kind_json, '$.kind') IN ('lifecycle', 'relationship')",
];

pub(super) fn create_indexes(connection: &Connection) -> Result<()> {
    for sql in INDEXES {
        connection.execute_batch(sql)?;
    }
    Ok(())
}

pub(super) fn validate_indexes(connection: &Connection) -> Result<()> {
    for (name, expected) in INDEX_NAMES.iter().zip(INDEXES) {
        let sql: String = connection.query_row(
            "SELECT sql FROM sqlite_schema WHERE type='index' AND name=?1",
            [name],
            |row| row.get(0),
        )?;
        if sql != expected {
            return Err(StoreError::UnrecognizedDatabase);
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn drop_indexes(connection: &Connection) {
    for name in INDEX_NAMES {
        connection
            .execute_batch(&format!("DROP INDEX {name}"))
            .unwrap();
    }
}

#[cfg(test)]
thread_local! {
    static LAST_WORK: std::cell::Cell<(i32, i32, i32)> = const { std::cell::Cell::new((0, 0, 0)) };
}

#[cfg(test)]
pub(super) fn record_work(statement: &rusqlite::Statement<'_>) {
    use rusqlite::StatementStatus::{FullscanStep, Sort, VmStep};
    LAST_WORK.set((
        statement.get_status(FullscanStep),
        statement.get_status(Sort),
        statement.get_status(VmStep),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ObservationQuery, Store, validate_marked_shape};
    use agenturm_kernel::ObservationPosition;
    use std::path::PathBuf;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "agenturm-history-read-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn db(&self) -> PathBuf {
            self.0.join("history.sqlite")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    // Bulk seed synthetic metadata to exercise the real read path. This does
    // not benchmark batch admission or use any native session records.
    fn seed(store: &mut Store, count: usize) {
        let transaction = store.connection.transaction().unwrap();
        transaction.execute(
            "INSERT INTO agenturm_ingest_receipts VALUES ('fixture', 'harness.fixture', 'synthetic', ?1)",
            [count],
        ).unwrap();
        transaction.execute(
            "INSERT INTO agenturm_history_batches(request_id, admitted_at_unix_ns) VALUES ('fixture', 1)",
            [],
        ).unwrap();
        transaction.execute(
            r#"WITH RECURSIVE numbers(n) AS (
                SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n < ?1
             )
             INSERT INTO agenturm_observations
                (source_id, event_id, revision, request_id, event_digest, scope,
                 session_id, observed_at_unix_ns, kind_json)
             SELECT 'harness.fixture', printf('event-%09d', n), 0, 'fixture', 'synthetic',
                    'session', printf('session-%03d', (n-1) / 1000), n,
                    CASE WHEN n % 1000 = 1 THEN '{"kind":"lifecycle","state":"working","reason_code":null}'
                         ELSE '{"kind":"message","role":"assistant"}' END
             FROM numbers"#,
            [count],
        ).unwrap();
        transaction.commit().unwrap();
    }

    fn bounded_page(store: &Store, query: &ObservationQuery) -> crate::ObservationPage {
        let page = store.query_observations(query).unwrap();
        let (scans, sorts, steps) = LAST_WORK.get();
        assert_eq!(sorts, 0, "history page must not build a temporary sort");
        assert!(scans <= query.limit as i32 + 1, "full scan steps: {scans}");
        assert!(steps < 50_000, "page VM steps: {steps}");
        page
    }

    #[test]
    fn recent_scoped_and_deep_keyset_pages_have_bounded_sql_work() {
        let fixture = Fixture::new();
        let mut store = Store::open(fixture.db()).unwrap();
        seed(&mut store, 50_000);
        let query = ObservationQuery::latest(64);
        let recent = bounded_page(&store, &query);
        assert_eq!(
            recent.observations.last().unwrap().observed_at_unix_ns,
            50_000
        );
        assert!(recent.has_more);
        let source_only = ObservationQuery {
            harness_id: Some("harness.fixture".into()),
            ..query.clone()
        };
        assert_eq!(bounded_page(&store, &source_only), recent);
        // Jump deep into history; do not visit every newer row before the cursor.
        let older = ObservationQuery {
            snapshot_id: Some(recent.snapshot_id),
            before: Some(ObservationPosition {
                observed_at_unix_ns: 1000,
                harness_id: "harness.fixture".into(),
                event_id: "event-000001000".into(),
                revision: 0,
                snapshot_id: recent.snapshot_id,
            }),
            ..query.clone()
        };
        let page = bounded_page(&store, &older);
        assert_eq!(page.observations.last().unwrap().observed_at_unix_ns, 999);
        let scoped = ObservationQuery {
            harness_id: Some("harness.fixture".into()),
            session_id: Some("session-000".into()),
            ..query
        };
        let page = bounded_page(&store, &scoped);
        assert_eq!(page.observations.last().unwrap().observed_at_unix_ns, 1000);
        let missing = ObservationQuery {
            session_id: Some("absent".into()),
            ..scoped
        };
        assert!(bounded_page(&store, &missing).observations.is_empty());
    }

    #[test]
    fn v27_readers_do_not_migrate_and_writer_upgrade_preserves_history_and_grants() {
        let fixture = Fixture::new();
        let mut store = Store::open(fixture.db()).unwrap();
        seed(&mut store, 2000);
        let scope = crate::ContentGrantScope::new(
            agenturm_kernel::ExtensionId::parse("harness.fixture").unwrap(),
            crate::ContentKind::PiUserText,
            [1; 32],
        );
        let grant = store.grant_content(&scope).unwrap();
        let identity = store.history_identity().unwrap();
        let expected = store.observation_page(64).unwrap();
        drop_indexes(&store.connection);
        store
            .connection
            .execute(
                "UPDATE agenturm_store_meta SET value=?1 WHERE key='format'",
                [crate::STORE_MARKER_V27],
            )
            .unwrap();
        store
            .connection
            .pragma_update(None, "user_version", 27)
            .unwrap();
        drop(store);
        let reader = Store::open_read_only(fixture.db()).unwrap();
        assert_eq!(reader.observation_page(64).unwrap(), expected);
        validate_marked_shape(&reader.connection, 27).unwrap();
        reader.verify_content_grant(&scope, &grant).unwrap();
        drop(reader);
        let upgraded = Store::open(fixture.db()).unwrap();
        validate_marked_shape(&upgraded.connection, crate::SCHEMA_VERSION).unwrap();
        assert_eq!(upgraded.observation_page(64).unwrap(), expected);
        assert_eq!(upgraded.history_identity().unwrap(), identity);
        upgraded.verify_content_grant(&scope, &grant).unwrap();
        drop(upgraded);
        Store::open_read_only(fixture.db()).unwrap();
    }

    #[test]
    fn wrong_history_index_definition_is_rejected() {
        let fixture = Fixture::new();
        let store = Store::open(fixture.db()).unwrap();
        store
            .connection
            .execute_batch(
                "DROP INDEX agenturm_observations_by_time;
             CREATE INDEX agenturm_observations_by_time ON agenturm_observations(event_id)",
            )
            .unwrap();
        drop(store);
        assert!(matches!(
            Store::open_read_only(fixture.db()),
            Err(StoreError::UnrecognizedDatabase)
        ));
    }

    #[test]
    fn roster_facts_use_sparse_index_even_when_relationship_is_absent() {
        use rusqlite::{OptionalExtension, StatementStatus};
        let fixture = Fixture::new();
        let mut store = Store::open(fixture.db()).unwrap();
        seed(&mut store, 20_000);
        let sql = crate::session_catalog::LATEST_FACT_SQL;
        let mut explain = store
            .connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let params = rusqlite::params!["session-000", "harness.fixture", i64::MAX, "relationship"];
        let plan = explain
            .query_map(params, |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(plan.contains("agenturm_session_facts_by_kind"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
        let mut statement = store.connection.prepare(sql).unwrap();
        let absent = statement
            .query_row(params, |row| row.get::<_, String>(0))
            .optional()
            .unwrap();
        assert!(absent.is_none());
        assert_eq!(statement.get_status(StatementStatus::FullscanStep), 0);
        assert!(statement.get_status(StatementStatus::VmStep) < 100);
    }

    #[test]
    #[ignore = "synthetic query benchmark; run explicitly with --ignored --nocapture"]
    fn benchmark_history_pages() {
        let fixture = Fixture::new();
        let mut store = Store::open(fixture.db()).unwrap();
        let count = std::env::var("AGENTURM_BENCH_ROWS")
            .ok()
            .map(|value| value.parse::<usize>().unwrap())
            .unwrap_or(1_000_000);
        drop_indexes(&store.connection);
        seed(&mut store, count);
        store
            .connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        let before_bytes = std::fs::metadata(fixture.db()).unwrap().len();
        let queries = [
            ObservationQuery::latest(256),
            ObservationQuery::latest(16_384),
        ];
        let expected: Vec<_> = queries
            .iter()
            .map(|query| store.query_observations(query).unwrap())
            .collect();
        for indexed in [false, true] {
            if indexed {
                let start = Instant::now();
                create_indexes(&store.connection).unwrap();
                store
                    .connection
                    .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                    .unwrap();
                eprintln!(
                    "index_build_ms={:.1} db_before_mib={:.2} db_after_mib={:.2}",
                    start.elapsed().as_secs_f64() * 1000.0,
                    before_bytes as f64 / 1048576.0,
                    std::fs::metadata(fixture.db()).unwrap().len() as f64 / 1048576.0
                );
            }
            for (query, expected) in queries.iter().zip(&expected) {
                let mut samples = Vec::new();
                for _ in 0..12 {
                    let start = Instant::now();
                    let actual = store.query_observations(query).unwrap();
                    samples.push(start.elapsed().as_secs_f64() * 1000.0);
                    assert_eq!(&actual, expected);
                }
                samples.sort_by(f64::total_cmp);
                eprintln!(
                    "rows={count} indexed={indexed} page={} n=12 median_ms={:.3} min_ms={:.3} max_ms={:.3} sql_work={:?}",
                    query.limit,
                    (samples[5] + samples[6]) / 2.0,
                    samples[0],
                    samples[11],
                    LAST_WORK.get()
                );
            }
        }
    }
}
