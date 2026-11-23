//! Latest retained, authorized Host sample per source at one read watermark.

use crate::{MAX_HOST_HISTORY_PAGE, Result, Store, StoreError, retention};
use agenturm_kernel::{
    ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationPage, ReadSnapshot,
};
use rusqlite::params;
use std::collections::BTreeSet;

impl Store {
    pub fn query_host_inventory(
        &self,
        snapshot_id: Option<ReadSnapshot>,
        allowed_sources: &BTreeSet<ExtensionId>,
    ) -> Result<ObservationPage> {
        let transaction = self.connection.unchecked_transaction()?;
        let (snapshot_id, watermark) = retention::resolve_read_snapshot(&transaction, snapshot_id)?;
        // Rank only the authorized keys. Load the sample JSON for the
        // bounded winners, rather than storing bodies in the window result.
        let mut statement = transaction.prepare_cached(
            "WITH ranked AS (
                 SELECT observation.source_id, observation.event_id, observation.revision,
                        observation.observed_at_unix_ns,
                        ROW_NUMBER() OVER (
                            PARTITION BY observation.source_id
                            ORDER BY observation.observed_at_unix_ns DESC,
                                     observation.event_id DESC, observation.revision DESC
                        ) AS position
                   FROM agenturm_observations AS observation
                   JOIN agenturm_history_batches AS batch USING (request_id)
                  WHERE batch.sequence <= ?1
                    AND observation.source_id IN (SELECT value FROM json_each(?2))
                    AND substr(observation.source_id, 1, 5) = 'host.'
                    AND observation.scope = 'source'
                    AND json_extract(observation.kind_json, '$.kind') = 'host_sample'
             ), selected AS (
                 SELECT source_id, event_id, revision, observed_at_unix_ns
                   FROM ranked WHERE position = 1
                  ORDER BY observed_at_unix_ns DESC, source_id DESC, event_id DESC, revision DESC
                  LIMIT ?3
             )
             SELECT source_id, event_id, revision, selected.observed_at_unix_ns, observation.kind_json
               FROM selected
               JOIN agenturm_observations AS observation USING (source_id, event_id, revision)
              ORDER BY selected.observed_at_unix_ns DESC, source_id DESC, event_id DESC, revision DESC",
        )?;
        let rows = statement.query_map(
            params![
                watermark,
                serde_json::to_string(allowed_sources)?,
                MAX_HOST_HISTORY_PAGE + 1
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )?;
        let mut observations = Vec::new();
        for row in rows {
            let (source, event_id, revision, observed_at_unix_ns, json) = row?;
            let harness_id = ExtensionId::parse(source)
                .map_err(|_| StoreError::Invalid("invalid retained Host source"))?;
            let kind: ObservationKind = serde_json::from_str(&json)?;
            if !matches!(&kind, ObservationKind::HostSample(sample) if sample.is_valid()) {
                return Err(StoreError::Invalid("invalid retained Host sample"));
            }
            observations.push(Observation {
                harness_id,
                event_id,
                revision,
                identity: ObservationIdentity::source(),
                observed_at_unix_ns,
                kind,
            });
        }
        let has_more = observations.len() > MAX_HOST_HISTORY_PAGE;
        observations.truncate(MAX_HOST_HISTORY_PAGE);
        observations.reverse();
        Ok(ObservationPage {
            observations,
            has_more,
            snapshot_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ObservationBatch;
    use agenturm_kernel::{HostPlatform, HostSample};

    fn id(source: &str) -> ExtensionId {
        ExtensionId::parse(source).unwrap()
    }

    fn sample(source: &ExtensionId, number: u64) -> Observation {
        Observation {
            harness_id: source.clone(),
            event_id: format!("sample-{number}"),
            revision: 0,
            observed_at_unix_ns: number,
            identity: ObservationIdentity::source(),
            kind: ObservationKind::HostSample(HostSample {
                boot_id: "13572468-1234-1234-1234-123456789abc".into(),
                hostname: Some("collector-1".into()),
                platform: Some(HostPlatform::Linux),
                clock_ticks_per_second: None,
                cpu: None,
                load_1m_milli: None,
                load_5m_milli: None,
                load_15m_milli: None,
                memory_total_bytes: None,
                memory_available_bytes: None,
                processes: Vec::new(),
                processes_partial: false,
            }),
        }
    }

    fn commit(
        store: &mut Store,
        source: &ExtensionId,
        expected: Option<&str>,
        next: &str,
        rows: &[Observation],
    ) {
        let batch =
            ObservationBatch::from_observations(source.as_str(), expected, next, rows).unwrap();
        store.commit_batch(&batch).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ranks_all_retained_rows_before_bounding_authorized_sources_and_pins_snapshot() {
        let directory = std::env::temp_dir().join(format!(
            "agenturm-host-inventory-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir(&directory).unwrap();
        let mut store = Store::open_collector(directory.join("history.sqlite")).unwrap();
        let source = id("host.authorized");
        let sibling = id("host.sibling");
        commit(
            &mut store,
            &source,
            None,
            "one",
            &(1..=70)
                .map(|time| sample(&source, time))
                .collect::<Vec<_>>(),
        );
        commit(&mut store, &sibling, None, "one", &[sample(&sibling, 80)]);
        let allowed = BTreeSet::from([source.clone()]);
        let page = store.query_host_inventory(None, &allowed).unwrap();
        assert_eq!(page.observations, [sample(&source, 70)]);
        assert!(!page.has_more);
        commit(
            &mut store,
            &source,
            Some("one"),
            "two",
            &[sample(&source, 90)],
        );
        assert_eq!(
            store
                .query_host_inventory(Some(page.snapshot_id), &allowed)
                .unwrap(),
            page
        );
        let all = BTreeSet::from([source.clone(), sibling.clone()]);
        assert_eq!(
            store.query_host_inventory(None, &all).unwrap().observations,
            [sample(&sibling, 80), sample(&source, 90)]
        );
        assert!(
            store
                .query_host_inventory(None, &BTreeSet::new())
                .unwrap()
                .observations
                .is_empty()
        );
        store.prune_observations_before(85).unwrap();
        assert!(matches!(
            store.query_host_inventory(Some(page.snapshot_id), &all),
            Err(StoreError::ExpiredHistorySnapshot { .. })
        ));
        assert_eq!(
            store.query_host_inventory(None, &all).unwrap().observations,
            [sample(&source, 90)]
        );
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn excess_distinct_sources_return_an_explicit_partial_inventory() {
        let mut store = Store::open(":memory:").unwrap();
        let allowed = (0..=MAX_HOST_HISTORY_PAGE)
            .map(|number| {
                let source = id(&format!("host.fixture-{number}"));
                commit(
                    &mut store,
                    &source,
                    None,
                    "one",
                    &[sample(&source, number as u64 + 1)],
                );
                source
            })
            .collect();
        let page = store.query_host_inventory(None, &allowed).unwrap();
        assert!(page.has_more);
        assert_eq!(page.observations.len(), MAX_HOST_HISTORY_PAGE);
        assert_eq!(page.observations[0].observed_at_unix_ns, 2);
    }
}
