//! Persisted age policy. The process holding the writer lease runs the policy.

use crate::{Result, RetentionOutcome, Store, StoreError, retention};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_RETENTION_AGE_NS: u64 = 14 * 86_400 * 1_000_000_000;
const POLICY_KEY: &str = "automatic_retention";

/// The process owning the writer lease checks age policy at most once a minute.
pub struct AutomaticRetentionSchedule {
    next_run: Instant,
}

impl Default for AutomaticRetentionSchedule {
    fn default() -> Self {
        Self {
            next_run: Instant::now(),
        }
    }
}

impl AutomaticRetentionSchedule {
    pub fn tick(&mut self, store: &mut Store) -> Result<Option<RetentionOutcome>> {
        let now = Instant::now();
        if now < self.next_run {
            return Ok(None);
        }
        self.next_run = now + Duration::from_secs(60);
        store.apply_current_automatic_retention()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AutomaticRetentionPolicy {
    /// `None` explicitly disables age retention; absence of a policy is distinct.
    pub max_age_ns: Option<u64>,
}

impl AutomaticRetentionPolicy {
    pub fn validate(self) -> Result<Self> {
        if self
            .max_age_ns
            .is_some_and(|age| age == 0 || age > i64::MAX as u64)
        {
            return Err(StoreError::Invalid(
                "retention age must be positive and fit SQLite time",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AutomaticRetentionStatus {
    pub policy: Option<AutomaticRetentionPolicy>,
    pub cutoff_unix_ns: u64,
}

impl Store {
    pub fn automatic_retention_status(&self) -> Result<AutomaticRetentionStatus> {
        let transaction = self.connection.unchecked_transaction()?;
        let policy = read_policy(&transaction)?;
        let cutoff_unix_ns = retention::read_state(&transaction)?.cutoff_unix_ns as u64;
        Ok(AutomaticRetentionStatus {
            policy,
            cutoff_unix_ns,
        })
    }

    pub fn set_automatic_retention(&mut self, policy: AutomaticRetentionPolicy) -> Result<()> {
        let policy = policy.validate()?;
        #[cfg(target_os = "linux")]
        if self.writer_lease.is_none() {
            return Err(StoreError::Invalid(
                "retention policy requires a persistent writer lease",
            ));
        }
        self.verify_write_admission()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO agenturm_store_meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [POLICY_KEY, &serde_json::to_string(&policy)?],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Only a file created by this open can receive the managed default.
    /// Existing histories, empty existing files, and restored histories keep
    /// their current policy, including an explicit disabled policy.
    pub fn initialize_managed_retention(&mut self) -> Result<()> {
        if self.created_new && self.automatic_retention_status()?.policy.is_none() {
            self.set_automatic_retention(AutomaticRetentionPolicy {
                max_age_ns: Some(DEFAULT_RETENTION_AGE_NS),
            })?;
        }
        Ok(())
    }

    /// Clock rollback and a longer policy never lower an established cutoff.
    /// The caller schedules this on its single writer, outside content fences.
    pub fn apply_automatic_retention(
        &mut self,
        now_unix_ns: u64,
    ) -> Result<Option<RetentionOutcome>> {
        if now_unix_ns > i64::MAX as u64 {
            return Err(StoreError::Invalid("retention clock exceeds SQLite time"));
        }
        self.verify_write_admission()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(age) = read_policy(&transaction)?.and_then(|policy| policy.max_age_ns) else {
            return Ok(None);
        };
        let cutoff = now_unix_ns.saturating_sub(age);
        if cutoff <= retention::read_state(&transaction)?.cutoff_unix_ns as u64 {
            return Ok(None);
        }
        #[cfg(target_os = "linux")]
        if self.writer_lease.is_none() {
            return Err(StoreError::Invalid(
                "automatic retention requires a persistent writer lease",
            ));
        }
        let outcome = retention::prune_in(&transaction, cutoff)?;
        transaction.commit()?;
        Ok(Some(outcome))
    }

    pub fn apply_current_automatic_retention(&mut self) -> Result<Option<RetentionOutcome>> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| StoreError::Invalid("retention clock precedes the Unix epoch"))?;
        let nanos = u64::try_from(now.as_nanos())
            .map_err(|_| StoreError::Invalid("retention clock is outside the supported range"))?;
        self.apply_automatic_retention(nanos)
    }
}

fn read_policy(connection: &Connection) -> Result<Option<AutomaticRetentionPolicy>> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT value FROM agenturm_store_meta WHERE key = ?1",
            [POLICY_KEY],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| serde_json::from_str::<AutomaticRetentionPolicy>(&raw))
        .transpose()?
        .map(AutomaticRetentionPolicy::validate)
        .transpose()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use agenturm_kernel::{
        ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationQuery,
        ObservationStore,
    };
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static UNIQUE: AtomicU64 = AtomicU64::new(0);
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "agenturm-age-policy-{}-{}",
                std::process::id(),
                UNIQUE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn managed_default_only_enrolls_a_file_created_by_this_open() {
        let root = Scratch::new();
        let path = root.0.join("new.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.initialize_managed_retention().unwrap();
        assert_eq!(
            store
                .automatic_retention_status()
                .unwrap()
                .policy
                .unwrap()
                .max_age_ns,
            Some(DEFAULT_RETENTION_AGE_NS)
        );
        store
            .set_automatic_retention(AutomaticRetentionPolicy { max_age_ns: None })
            .unwrap();
        drop(store);
        let mut reopened = Store::open(&path).unwrap();
        reopened.initialize_managed_retention().unwrap();
        assert_eq!(
            reopened
                .automatic_retention_status()
                .unwrap()
                .policy
                .unwrap()
                .max_age_ns,
            None
        );
        drop(reopened);
        let preexisting = root.0.join("preexisting.sqlite");
        std::fs::File::create(&preexisting).unwrap();
        let mut store = Store::open(&preexisting).unwrap();
        store.initialize_managed_retention().unwrap();
        assert!(store.automatic_retention_status().unwrap().policy.is_none());
        drop(store);
        let readonly = Store::open_read_only(&preexisting).unwrap();
        assert!(
            readonly
                .automatic_retention_status()
                .unwrap()
                .policy
                .is_none()
        );
    }

    #[test]
    fn age_policy_retains_checkpoint_replay_and_never_rewinds_the_cutoff() {
        let root = Scratch::new();
        let path = root.0.join("history.sqlite");
        let mut store = Store::open(&path).unwrap();
        let row = |event: &str, timestamp| Observation {
            harness_id: ExtensionId::parse("harness.test").unwrap(),
            event_id: event.into(),
            revision: 0,
            identity: ObservationIdentity::session("one"),
            observed_at_unix_ns: timestamp,
            kind: ObservationKind::SessionStarted,
        };
        let rows = [row("old", 10), row("recent", 30)];
        store
            .commit("harness.test", None, "cursor-one", &rows)
            .unwrap();
        let before = store
            .query_observations(&ObservationQuery::latest(10))
            .unwrap()
            .snapshot_id;
        store
            .set_automatic_retention(AutomaticRetentionPolicy {
                max_age_ns: Some(10),
            })
            .unwrap();
        let outcome = store.apply_automatic_retention(30).unwrap().unwrap();
        assert_eq!(outcome.removed_observations, 1);
        assert_eq!(outcome.cutoff_unix_ns, 20);
        assert_eq!(
            store
                .query_observations(&ObservationQuery::latest(10))
                .unwrap()
                .observations[0]
                .event_id,
            "recent"
        );
        assert_eq!(
            store.source_cursor("harness.test").unwrap().as_deref(),
            Some("cursor-one")
        );
        assert!(
            store
                .commit("harness.test", None, "cursor-one", &rows)
                .is_ok()
        );
        let mut expired = ObservationQuery::latest(10);
        expired.snapshot_id = Some(before);
        assert!(store.query_observations(&expired).is_err());
        assert!(store.apply_automatic_retention(29).unwrap().is_none());
        store
            .set_automatic_retention(AutomaticRetentionPolicy {
                max_age_ns: Some(100),
            })
            .unwrap();
        assert!(store.apply_automatic_retention(31).unwrap().is_none());
        store
            .set_automatic_retention(AutomaticRetentionPolicy { max_age_ns: None })
            .unwrap();
        assert!(store.apply_automatic_retention(1_000).unwrap().is_none());
        assert_eq!(
            store.automatic_retention_status().unwrap().cutoff_unix_ns,
            20
        );
        let backup = root.0.join("backup.sqlite");
        store.backup_to(&backup).unwrap();
        let saved = Store::open_read_only(&backup).unwrap();
        assert_eq!(
            saved.automatic_retention_status().unwrap(),
            store.automatic_retention_status().unwrap()
        );
    }

    #[test]
    fn invalid_policy_cannot_replace_current_policy_or_delete_history() {
        let root = Scratch::new();
        let mut store = Store::open(root.0.join("history.sqlite")).unwrap();
        store
            .set_automatic_retention(AutomaticRetentionPolicy {
                max_age_ns: Some(10),
            })
            .unwrap();
        let original = store.automatic_retention_status().unwrap();
        for max_age_ns in [Some(0), Some(u64::MAX)] {
            assert!(
                store
                    .set_automatic_retention(AutomaticRetentionPolicy { max_age_ns })
                    .is_err()
            );
            assert_eq!(store.automatic_retention_status().unwrap(), original);
        }
        assert!(store.apply_automatic_retention(u64::MAX).is_err());
        assert_eq!(store.automatic_retention_status().unwrap(), original);
    }
}
