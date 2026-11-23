//! Private Workflow policy and durable notifier attempt transitions.

use super::desktop_attention::{
    ClaimRow, REQUEST_FRESHNESS_NS, claim_new, positive_time, request_key,
};
use super::*;
use agenturm_kernel::workflow_notifications::{
    WorkflowDelivery, WorkflowDeliveryState, WorkflowNotificationAuthority,
    WorkflowNotificationPolicy, WorkflowPolicyChange, WorkflowThresholds,
};

const MAX_ALERT_SAMPLES: usize = 3 * MAX_WORKSPACE_PAGE_SIZE;
const MAX_MEASURED_COHORTS: i64 = 4_096;
const TEN_MINUTES_NS: u64 = 600_000_000_000;
const POLICY_CONFLICT: &str = "Workflow settings changed. Reload settings and try again.";
const RETRY_DENIED: &str = "Retry needs a fresh, exact failed or unavailable delivery.";

#[derive(Clone, Copy)]
pub struct WorkflowNotificationRead<'a> {
    pub authority: &'a WorkflowNotificationAuthority,
    pub clock_unix_ns: u64,
    pub policy_revision: u64,
    pub desktop_opt_in: bool,
}

/// An occurrence whose timing or measured threshold was positively verified.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkflowAlertCandidate {
    Elapsed {
        key: [u8; 32],
        crossed_at_unix_ns: u64,
    },
    Measured {
        key: [u8; 32],
        exceeded: Option<bool>,
    },
}

impl WorkflowAlertCandidate {
    pub fn key(self) -> [u8; 32] {
        match self {
            Self::Elapsed { key, .. } | Self::Measured { key, .. } => key,
        }
    }
}

pub struct WorkflowDeliveryAttempt {
    namespace: [u8; 32],
    key: [u8; 32],
    number: i64,
    observed_at: i64,
}

impl WorkflowDeliveryAttempt {
    pub fn key(&self) -> [u8; 32] {
        self.key
    }

    pub fn is_fresh_at(&self, clock_unix_ns: u64) -> bool {
        positive_time(clock_unix_ns).is_ok_and(|now| fresh(self.observed_at, now))
    }
}

pub(super) fn migrate(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE workflow_policy (
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            revision INTEGER NOT NULL CHECK(revision>0),
            desktop_enabled INTEGER CHECK(desktop_enabled IN (0,1)),
            tokens INTEGER NOT NULL CHECK(tokens>=0),
            usd_nanos INTEGER NOT NULL CHECK(usd_nanos>=0),
            duration_ns INTEGER NOT NULL CHECK(duration_ns>=0),
            snoozed_until INTEGER CHECK(snoozed_until>0),
            changed_at INTEGER NOT NULL CHECK(changed_at>0)
         );
         CREATE TABLE workflow_measured_cohorts (
             namespace BLOB NOT NULL, digest BLOB NOT NULL,
             observed_at INTEGER NOT NULL, watermark INTEGER NOT NULL,
             policy_revision INTEGER NOT NULL, exceeded INTEGER NOT NULL CHECK(exceeded IN (0,1)),
             crossed_at INTEGER,
             PRIMARY KEY (namespace,digest)
         ) WITHOUT ROWID;
         ALTER TABLE desktop_claims ADD COLUMN state INTEGER NOT NULL DEFAULT 0
             CHECK(state BETWEEN 0 AND 4);
         ALTER TABLE desktop_claims ADD COLUMN attempt INTEGER NOT NULL DEFAULT 1
             CHECK(attempt>0);
         ALTER TABLE desktop_claims ADD COLUMN policy_revision INTEGER NOT NULL DEFAULT 0
             CHECK(policy_revision>=0);",
    )?;
    Ok(())
}

pub(super) fn read_policy(db: &Connection) -> Result<WorkflowNotificationPolicy> {
    Ok(db
        .query_row(
            "SELECT revision,desktop_enabled,tokens,usd_nanos,duration_ns,snoozed_until
         FROM workflow_policy WHERE singleton=1",
            [],
            |row| {
                Ok(WorkflowNotificationPolicy {
                    revision: row.get(0)?,
                    desktop_enabled: row.get(1)?,
                    thresholds: WorkflowThresholds {
                        tokens: row.get(2)?,
                        usd_nanos: row.get(3)?,
                        duration_ns: row.get(4)?,
                    },
                    snoozed_until_unix_ns: row.get(5)?,
                })
            },
        )
        .optional()?
        .unwrap_or_default())
}

fn encode_state(state: WorkflowDeliveryState) -> i64 {
    match state {
        WorkflowDeliveryState::Claimed => 0,
        WorkflowDeliveryState::Delivered => 1,
        WorkflowDeliveryState::Failed => 2,
        WorkflowDeliveryState::Unavailable => 3,
        WorkflowDeliveryState::Uncertain => 4,
    }
}

fn decode_state(state: i64) -> Result<WorkflowDeliveryState> {
    match state {
        0 => Ok(WorkflowDeliveryState::Claimed),
        1 => Ok(WorkflowDeliveryState::Delivered),
        2 => Ok(WorkflowDeliveryState::Failed),
        3 => Ok(WorkflowDeliveryState::Unavailable),
        4 => Ok(WorkflowDeliveryState::Uncertain),
        _ => Err(WorkspaceError::Invalid("Invalid Workflow delivery state")),
    }
}

fn workflow_rows(requests: &[DesktopAttentionRequest]) -> Result<Vec<ClaimRow>> {
    if requests.len() > MAX_WORKSPACE_PAGE_SIZE {
        return Err(WorkspaceError::Invalid(
            "Workflow delivery inventory capacity reached",
        ));
    }
    requests
        .iter()
        .map(request_key)
        .map(|result| result.map(ClaimRow::request))
        .collect()
}

fn alert_rows(
    db: &Connection,
    namespace: &[u8; 32],
    watermark: i64,
    now: i64,
    policy: WorkflowNotificationPolicy,
    alerts: &[WorkflowAlertCandidate],
    retry: bool,
) -> Result<Vec<ClaimRow>> {
    let policy_changed: i64 = db
        .query_row(
            "SELECT changed_at FROM workflow_policy WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_default();
    let mut result = Vec::new();
    let mut present = BTreeSet::new();
    let mut select = db.prepare(
        "SELECT observed_at,watermark,policy_revision,exceeded,crossed_at
        FROM workflow_measured_cohorts WHERE namespace=?1 AND digest=?2",
    )?;
    let mut insert = db.prepare("INSERT INTO workflow_measured_cohorts VALUES (?1,?2,?3,?4,?5,?6,?7)
        ON CONFLICT(namespace,digest) DO UPDATE SET observed_at=excluded.observed_at,
        watermark=excluded.watermark,policy_revision=excluded.policy_revision,exceeded=excluded.exceeded,
        crossed_at=excluded.crossed_at")?;
    for &alert in alerts {
        let key = alert.key();
        if !present.insert(key) {
            return Err(WorkspaceError::Invalid("Duplicate Workflow alert proof"));
        }
        let (exceeded, due_at) = match alert {
            WorkflowAlertCandidate::Elapsed {
                crossed_at_unix_ns, ..
            } => {
                let due = positive_time(crossed_at_unix_ns)?;
                (Some(due <= now), Some(due))
            }
            WorkflowAlertCandidate::Measured { exceeded, .. } => (exceeded, None),
        };
        let Some(exceeded) = exceeded else {
            if !retry {
                db.execute(
                    "DELETE FROM workflow_measured_cohorts WHERE namespace=?1 AND digest=?2 AND crossed_at IS NULL",
                    params![namespace, key],
                )?;
            }
            continue;
        };
        let previous: Option<(i64, i64, u64, bool, Option<i64>)> = select
            .query_row(params![namespace, key], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .optional()?;
        let crossed_at =
            if let Some((last, old_watermark, revision, was_above, crossing)) = previous {
                if old_watermark > watermark || last > now {
                    return Err(WorkspaceError::Invalid(
                        "Workflow measurement predates its last complete read",
                    ));
                }
                crossing.or_else(|| {
                    let advanced = due_at.map_or(old_watermark < watermark, |due| last < due);
                    (!retry
                        && exceeded
                        && !was_above
                        && revision == policy.revision
                        && advanced
                        && last < now
                        && fresh(last, now))
                    .then_some(due_at.unwrap_or(now))
                })
            } else {
                None
            };
        if !retry {
            insert.execute(params![
                namespace,
                key,
                now,
                watermark,
                policy.revision,
                exceeded,
                crossed_at
            ])?;
        }
        let crossed_at = crossed_at.filter(|_| exceeded);
        if let Some(observed_at) =
            crossed_at.filter(|time| *time > policy_changed && fresh(*time, now))
        {
            result.push(ClaimRow {
                key,
                observed_at,
                clock_transition: true,
            });
        }
    }
    if !retry {
        // Omitted scopes carry no new evidence. Their unspent baselines expire with freshness.
        db.execute("DELETE FROM workflow_measured_cohorts WHERE namespace=?1 AND observed_at<?2 AND crossed_at IS NULL",
            params![namespace, now.saturating_sub(REQUEST_FRESHNESS_NS)])?;
        let count: i64 = db.query_row(
            "SELECT COUNT(*) FROM workflow_measured_cohorts",
            [],
            |row| row.get(0),
        )?;
        if count > MAX_MEASURED_COHORTS {
            return Err(WorkspaceError::Invalid(
                "Workflow measured cohort capacity reached",
            ));
        }
    }
    Ok(result)
}

fn current_policy(db: &Connection, expected: u64) -> Result<WorkflowNotificationPolicy> {
    let policy = read_policy(db)?;
    if policy.revision != expected {
        return Err(WorkspaceError::Invalid(POLICY_CONFLICT));
    }
    Ok(policy)
}

fn fresh(observed_at: i64, now: i64) -> bool {
    (now.saturating_sub(REQUEST_FRESHNESS_NS)..=now).contains(&observed_at)
}

impl WorkspaceLedger {
    pub fn workflow_notification_policy(&self) -> Result<WorkflowNotificationPolicy> {
        Ok(self
            .read_desktop(|db, version| {
                if version >= 2 {
                    read_policy(db)
                } else {
                    Ok(WorkflowNotificationPolicy::default())
                }
            })?
            .unwrap_or_default())
    }

    pub fn change_workflow_notification_policy(
        &self,
        authority: &WorkflowNotificationAuthority,
        expected_revision: u64,
        clock_unix_ns: u64,
        change: WorkflowPolicyChange,
        verify_current: impl FnOnce() -> std::result::Result<(), String>,
    ) -> Result<WorkflowNotificationPolicy> {
        let (namespace, _) =
            self.desktop_namespace(&authority.history_identity, authority.snapshot_id)?;
        let now = positive_time(clock_unix_ns)?;
        self.write_desktop(|db| {
            let mut policy = current_policy(db, expected_revision)?;
            let previous = policy;
            match change {
                WorkflowPolicyChange::SetDesktop(enabled) => policy.desktop_enabled = Some(enabled),
                WorkflowPolicyChange::SetThresholds(thresholds) if thresholds.is_valid() => {
                    policy.thresholds = thresholds
                }
                WorkflowPolicyChange::SetThresholds(_) => {
                    return Err(WorkspaceError::Invalid(
                        "Workflow thresholds exceed their supported range",
                    ));
                }
                WorkflowPolicyChange::ToggleSnooze => {
                    policy.snoozed_until_unix_ns = if policy
                        .snoozed_until_unix_ns
                        .is_some_and(|until| until > clock_unix_ns)
                    {
                        None
                    } else {
                        Some(
                            clock_unix_ns
                                .checked_add(TEN_MINUTES_NS)
                                .filter(|until| *until <= i64::MAX as u64)
                                .ok_or(WorkspaceError::Invalid(
                                    "Workflow snooze exceeds its supported range",
                                ))?,
                        )
                    };
                }
            }
            verify_current().map_err(WorkspaceError::HistoryUnavailable)?;
            if policy == previous {
                return Ok(policy);
            }
            policy.revision = policy
                .revision
                .checked_add(1)
                .filter(|revision| *revision <= i64::MAX as u64)
                .ok_or(WorkspaceError::Invalid(
                    "Workflow settings revision limit reached",
                ))?;
            db.execute(
                "INSERT INTO workflow_policy VALUES (1,?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(singleton) DO UPDATE SET revision=excluded.revision,
                 desktop_enabled=excluded.desktop_enabled, tokens=excluded.tokens,
                 usd_nanos=excluded.usd_nanos, duration_ns=excluded.duration_ns,
                 snoozed_until=excluded.snoozed_until,changed_at=excluded.changed_at",
                params![
                    policy.revision,
                    policy.desktop_enabled,
                    policy.thresholds.tokens,
                    policy.thresholds.usd_nanos,
                    policy.thresholds.duration_ns,
                    policy.snoozed_until_unix_ns,
                    now
                ],
            )?;
            if !matches!(change, WorkflowPolicyChange::SetThresholds(_)) {
                // The next complete observation, rather than the settings read, is silent.
                db.execute("DELETE FROM desktop_histories WHERE namespace=?1", [namespace])?;
                db.execute("DELETE FROM workflow_measured_cohorts WHERE namespace=?1 AND crossed_at IS NULL", [namespace])?;
            }
            Ok(policy)
        })
    }

    pub fn workflow_delivery_states(
        &self,
        authority: &WorkflowNotificationAuthority,
        requests: &[DesktopAttentionRequest],
    ) -> Result<Vec<WorkflowDelivery>> {
        let keys = workflow_rows(requests)?
            .into_iter()
            .map(|row| (row.key, Some(row.observed_at)))
            .collect::<Vec<_>>();
        self.read_workflow_deliveries(authority, &keys)
    }

    pub fn workflow_alert_delivery_states(
        &self,
        authority: &WorkflowNotificationAuthority,
        keys: &[[u8; 32]],
    ) -> Result<Vec<WorkflowDelivery>> {
        if keys.len() > MAX_ALERT_SAMPLES {
            return Err(WorkspaceError::Invalid(
                "Workflow alert inventory capacity reached",
            ));
        }
        let keys = keys.iter().map(|key| (*key, None)).collect::<Vec<_>>();
        self.read_workflow_deliveries(authority, &keys)
    }

    fn read_workflow_deliveries(
        &self,
        authority: &WorkflowNotificationAuthority,
        keys: &[([u8; 32], Option<i64>)],
    ) -> Result<Vec<WorkflowDelivery>> {
        let (namespace, _) =
            self.desktop_namespace(&authority.history_identity, authority.snapshot_id)?;
        Ok(self
            .read_desktop(|db, version| {
                if version < 2 {
                    return Ok(Vec::new());
                }
                let policy = read_policy(db)?;
                let mut select = db.prepare(
                    "SELECT state,attempt FROM desktop_claims WHERE namespace=?1 AND digest=?2
                AND (?3 IS NULL OR observed_at=?3)",
                )?;
                let mut states = Vec::new();
                for &(key, observed_at) in keys {
                    if let Some((state, attempt)) = select
                        .query_row(params![namespace, key, observed_at], |row| {
                            Ok((row.get(0)?, row.get(1)?))
                        })
                        .optional()?
                    {
                        states.push(WorkflowDelivery {
                            authority: authority.clone(),
                            key,
                            policy_revision: policy.revision,
                            attempt,
                            state: decode_state(state)?,
                        });
                    }
                }
                Ok(states)
            })?
            .unwrap_or_default())
    }

    pub fn workflow_delivery_key(request: &DesktopAttentionRequest) -> Result<[u8; 32]> {
        Ok(request_key(request)?.0)
    }

    pub fn claim_workflow_delivery(
        &self,
        read: WorkflowNotificationRead<'_>,
        requests: &[DesktopAttentionRequest],
        alerts: &[WorkflowAlertCandidate],
        retry: Option<([u8; 32], u64)>,
    ) -> Result<Vec<WorkflowDeliveryAttempt>> {
        let (namespace, watermark) =
            self.desktop_namespace(&read.authority.history_identity, read.authority.snapshot_id)?;
        let now = positive_time(read.clock_unix_ns)?;
        if alerts.len() > MAX_ALERT_SAMPLES {
            return Err(WorkspaceError::Invalid(
                "Workflow alert inventory capacity reached",
            ));
        }
        let mut keys = workflow_rows(requests)?;
        self.write_desktop(|db| {
            let policy = read_policy(db)?;
            if policy.revision != read.policy_revision {
                if retry.is_some() { return Err(WorkspaceError::Invalid(POLICY_CONFLICT)); }
                return Ok(Vec::new());
            }
            let enabled = policy.can_deliver(read.desktop_opt_in, read.clock_unix_ns);
            if enabled {
                keys.extend(alert_rows(db,&namespace,watermark,now,policy,alerts,retry.is_some())?);
            } else {
                db.execute("DELETE FROM workflow_measured_cohorts WHERE namespace=?1 AND crossed_at IS NULL", [namespace])?;
            }
            let mut unique = BTreeSet::new();
            if !keys.iter().all(|row| unique.insert(row.key)) {
                return Err(WorkspaceError::Invalid("Duplicate Workflow delivery proof"));
            }
            if let Some((key, expected_attempt)) = retry {
                let expected_attempt = i64::try_from(expected_attempt).ok()
                    .filter(|attempt| (1..i64::MAX).contains(attempt))
                    .ok_or(WorkspaceError::Invalid(RETRY_DENIED))?;
                if !enabled {
                    return Err(WorkspaceError::Invalid("Enable desktop alerts and end snooze before retrying"));
                }
                let observed_at = keys.iter().find(|row| row.key == key && fresh(row.observed_at, now))
                    .map(|row| row.observed_at)
                    .ok_or(WorkspaceError::Invalid(RETRY_DENIED))?;
                let number = db.query_row(
                    "UPDATE desktop_claims SET state=0,attempt=attempt+1,policy_revision=?4
                     WHERE namespace=?1 AND digest=?2 AND observed_at=?3 AND state IN (2,3) AND attempt=?5
                     RETURNING attempt",
                    params![namespace,key,observed_at,policy.revision,expected_attempt],
                    |row| row.get(0),
                ).optional()?.ok_or(WorkspaceError::Invalid(RETRY_DENIED))?;
                return Ok(vec![WorkflowDeliveryAttempt { namespace, key, number, observed_at }]);
            }
            if let Some(until) = policy.snoozed_until_unix_ns {
                keys.retain(|row| (row.observed_at as u64) > until);
            }
            let claimed = claim_new(db, namespace, Some(watermark), now, &keys, enabled, policy.revision)?;
            Ok(keys.into_iter().filter(|row| claimed.contains(&row.key))
                .map(|row| WorkflowDeliveryAttempt { namespace, key: row.key, number: 1, observed_at: row.observed_at })
                .collect())
        })
    }

    /// Checks the current settings and claim under the same write lock used for policy edits.
    /// The callback revalidates reader authority and starts the notifier before releasing it.
    pub fn start_workflow_delivery<T>(
        &self,
        read: WorkflowNotificationRead<'_>,
        attempts: &[WorkflowDeliveryAttempt],
        start: impl FnOnce() -> std::result::Result<T, String>,
    ) -> Result<Option<T>> {
        let (namespace, _) =
            self.desktop_namespace(&read.authority.history_identity, read.authority.snapshot_id)?;
        let now = positive_time(read.clock_unix_ns)?;
        if attempts.is_empty()
            || attempts
                .iter()
                .any(|attempt| attempt.namespace != namespace || !fresh(attempt.observed_at, now))
        {
            return Ok(None);
        }
        self.write_desktop(|db| {
            let policy = read_policy(db)?;
            if policy.revision != read.policy_revision
                || !policy.can_deliver(read.desktop_opt_in, read.clock_unix_ns)
            {
                return Ok(None);
            }
            let mut select = db.prepare(
                "SELECT EXISTS (SELECT 1 FROM desktop_claims WHERE namespace=?1 AND digest=?2
                 AND attempt=?3 AND policy_revision=?4 AND state=0)",
            )?;
            for attempt in attempts {
                if !select.query_row(
                    params![
                        attempt.namespace,
                        attempt.key,
                        attempt.number,
                        policy.revision
                    ],
                    |row| row.get::<_, bool>(0),
                )? {
                    return Ok(None);
                }
            }
            self.verify_history()?;
            start()
                .map(Some)
                .map_err(WorkspaceError::HistoryUnavailable)
        })
    }

    pub fn finish_workflow_delivery(
        &self,
        attempts: Vec<WorkflowDeliveryAttempt>,
        state: WorkflowDeliveryState,
    ) -> Result<()> {
        if attempts.is_empty() || state == WorkflowDeliveryState::Claimed {
            return Err(WorkspaceError::Invalid(
                "A completed notifier attempt requires a terminal state",
            ));
        }
        self.write_desktop(|db| {
            let mut finish = db.prepare("UPDATE desktop_claims SET state=?4 WHERE namespace=?1 AND digest=?2 AND attempt=?3 AND state=0")?;
            for attempt in attempts {
                finish.execute(params![attempt.namespace,attempt.key,attempt.number,encode_state(state)])?;
            }
            Ok(())
        })
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        marker: PathBuf,
        ledger: WorkspaceLedger,
    }

    impl Fixture {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "agenturm-workflow-policy-{}-{stamp}",
                std::process::id()
            ));
            fs::create_dir(&root).unwrap();
            let marker = root.join("history.marker");
            fs::write(&marker, b"source history authority").unwrap();
            let ledger = WorkspaceLedger::open(&marker).unwrap();
            Self {
                root,
                marker,
                ledger,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn authority(watermark: u64) -> WorkflowNotificationAuthority {
        WorkflowNotificationAuthority {
            history_identity: "a".repeat(32),
            snapshot_id: watermark.into(),
        }
    }

    fn read(
        authority: &WorkflowNotificationAuthority,
        clock: u64,
        revision: u64,
    ) -> WorkflowNotificationRead<'_> {
        WorkflowNotificationRead {
            authority,
            clock_unix_ns: clock,
            policy_revision: revision,
            desktop_opt_in: false,
        }
    }

    fn request(timestamp: u64) -> DesktopAttentionRequest {
        DesktopAttentionRequest {
            anchor: WorkspaceAnchor {
                harness_id: ExtensionId::parse("harness.codex").unwrap(),
                session_id: "session".into(),
                turn_id: Some("turn".into()),
                worker_id: None,
            },
            event_id: "codex:fixture:1:lifecycle".into(),
            revision: 0,
            observed_at_unix_ns: timestamp,
        }
    }

    fn enabled(ledger: &WorkspaceLedger) -> WorkflowNotificationPolicy {
        let at = authority(1);
        let policy = ledger
            .change_workflow_notification_policy(
                &at,
                0,
                10,
                WorkflowPolicyChange::SetDesktop(true),
                || Ok(()),
            )
            .unwrap();
        assert!(
            ledger
                .claim_workflow_delivery(read(&at, 10, policy.revision), &[], &[], None)
                .unwrap()
                .is_empty()
        );
        policy
    }

    #[test]
    fn policy_defaults_threshold_validation_and_snooze_are_durable_and_atomic() {
        let fixture = Fixture::new();
        let ledger = &fixture.ledger;
        let before = ledger.workflow_notification_policy().unwrap();
        assert_eq!(before, WorkflowNotificationPolicy::default());
        assert!(!before.can_deliver(false, 10));
        assert!(before.can_deliver(true, 10));
        assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), 1);
        assert!(
            ledger
                .change_workflow_notification_policy(
                    &authority(1),
                    0,
                    10,
                    WorkflowPolicyChange::SetThresholds(WorkflowThresholds {
                        tokens: u64::MAX,
                        ..WorkflowThresholds::default()
                    }),
                    || Ok(())
                )
                .is_err()
        );
        assert_eq!(ledger.workflow_notification_policy().unwrap(), before);
        let thresholds = WorkflowThresholds {
            tokens: 23,
            usd_nanos: 12_500_000,
            duration_ns: 90_000_000_000,
        };
        let change = || {
            ledger.change_workflow_notification_policy(
                &authority(1),
                0,
                10,
                WorkflowPolicyChange::SetThresholds(thresholds),
                || Ok(()),
            )
        };
        let update_count = std::thread::scope(|scope| {
            let a = scope.spawn(change);
            let b = scope.spawn(change);
            u8::from(a.join().unwrap().is_ok()) + u8::from(b.join().unwrap().is_ok())
        });
        assert_eq!(update_count, 1);
        let disabled = ledger
            .change_workflow_notification_policy(
                &authority(2),
                1,
                20,
                WorkflowPolicyChange::SetDesktop(false),
                || Ok(()),
            )
            .unwrap();
        assert!(!disabled.can_deliver(true, 20));
        assert!(
            ledger
                .change_workflow_notification_policy(
                    &authority(2),
                    2,
                    30,
                    WorkflowPolicyChange::SetDesktop(true),
                    || Err("independent history".into())
                )
                .is_err()
        );
        let snoozed = ledger
            .change_workflow_notification_policy(
                &authority(2),
                2,
                30,
                WorkflowPolicyChange::ToggleSnooze,
                || Ok(()),
            )
            .unwrap();
        assert_eq!(snoozed.snoozed_until_unix_ns, Some(30 + TEN_MINUTES_NS));
        let reopened = WorkspaceLedger::open(&fixture.marker).unwrap();
        assert_eq!(reopened.workflow_notification_policy().unwrap(), snoozed);
        assert_eq!(snoozed.thresholds, thresholds);
        let ended = reopened
            .change_workflow_notification_policy(
                &authority(2),
                3,
                31,
                WorkflowPolicyChange::ToggleSnooze,
                || Ok(()),
            )
            .unwrap();
        assert_eq!(ended.snoozed_until_unix_ns, None);
        assert_eq!(ended.revision, 4);
    }

    #[test]
    fn only_explicit_fresh_failed_or_unavailable_attempts_retry() {
        let fixture = Fixture::new();
        let ledger = &fixture.ledger;
        enabled(ledger);
        let first = request(11);
        let items = std::slice::from_ref(&first);
        let at = authority(2);
        let current = read(&at, 12, 1);
        let claims = ledger
            .claim_workflow_delivery(current, items, &[], None)
            .unwrap();
        assert_eq!(claims.len(), 1);
        let key = claims[0].key();
        assert!(
            ledger
                .claim_workflow_delivery(current, items, &[], Some((key, 1)))
                .is_err()
        );
        assert_eq!(
            ledger
                .start_workflow_delivery(current, &claims, || Ok(5))
                .unwrap(),
            Some(5)
        );
        ledger
            .finish_workflow_delivery(claims, WorkflowDeliveryState::Unavailable)
            .unwrap();
        let reopened = WorkspaceLedger::open(&fixture.marker).unwrap();
        assert_eq!(
            reopened.workflow_delivery_states(&at, items).unwrap()[0].state,
            WorkflowDeliveryState::Unavailable
        );
        assert!(
            reopened
                .claim_workflow_delivery(current, items, &[], None)
                .unwrap()
                .is_empty()
        );
        let wrong_worker = DesktopAttentionRequest {
            anchor: WorkspaceAnchor {
                worker_id: Some("other".into()),
                ..first.anchor.clone()
            },
            ..first.clone()
        };
        assert!(
            reopened
                .claim_workflow_delivery(current, &[wrong_worker], &[], Some((key, 1)))
                .is_err()
        );
        assert!(
            reopened
                .claim_workflow_delivery(
                    read(&at, 13 + REQUEST_FRESHNESS_NS as u64, 1),
                    items,
                    &[],
                    Some((key, 1))
                )
                .is_err()
        );
        let mut restored = authority(2);
        restored.snapshot_id = ReadSnapshot::new(2, Some("1".repeat(32).parse().unwrap()));
        assert!(
            reopened
                .workflow_delivery_states(&restored, items)
                .unwrap()
                .is_empty()
        );
        assert!(
            reopened
                .claim_workflow_delivery(read(&restored, 12, 1), items, &[], Some((key, 1)))
                .is_err()
        );
        let retry = reopened
            .claim_workflow_delivery(current, items, &[], Some((key, 1)))
            .unwrap();
        reopened
            .finish_workflow_delivery(retry, WorkflowDeliveryState::Failed)
            .unwrap();
        let newer = reopened.workflow_delivery_states(&at, items).unwrap();
        assert_eq!(newer[0].attempt, 2);
        assert_eq!(newer[0].state, WorkflowDeliveryState::Failed);
        assert!(
            reopened
                .claim_workflow_delivery(current, items, &[], Some((key, 1)))
                .is_err()
        );
        let retry = reopened
            .claim_workflow_delivery(current, items, &[], Some((key, 2)))
            .unwrap();
        reopened
            .finish_workflow_delivery(retry, WorkflowDeliveryState::Uncertain)
            .unwrap();
        assert!(
            reopened
                .claim_workflow_delivery(current, items, &[], Some((key, 3)))
                .is_err()
        );
    }

    #[test]
    fn policy_change_after_claim_fences_launch_and_snooze_has_no_catchup() {
        let fixture = Fixture::new();
        let ledger = &fixture.ledger;
        enabled(ledger);
        let at = authority(2);
        let first = request(11);
        let items = std::slice::from_ref(&first);
        let current = read(&at, 12, 1);
        let claims = ledger
            .claim_workflow_delivery(current, items, &[], None)
            .unwrap();
        let snoozed = ledger
            .change_workflow_notification_policy(
                &at,
                1,
                13,
                WorkflowPolicyChange::ToggleSnooze,
                || Ok(()),
            )
            .unwrap();
        assert_eq!(snoozed.snoozed_until_unix_ns, Some(13 + TEN_MINUTES_NS));
        assert!(
            ledger
                .start_workflow_delivery::<()>(current, &claims, || panic!(
                    "stale policy launched a notifier"
                ))
                .unwrap()
                .is_none()
        );
        let later = authority(4);
        let until = 13 + TEN_MINUTES_NS;
        let during_snooze = request(until - 1);
        assert!(
            ledger
                .claim_workflow_delivery(read(&later, until + 1, 2), &[during_snooze], &[], None)
                .unwrap()
                .is_empty()
        );
        let after_snooze = DesktopAttentionRequest {
            revision: 1,
            ..request(until + 2)
        };
        assert_eq!(
            ledger
                .claim_workflow_delivery(
                    read(&authority(5), until + 3, 2),
                    &[after_snooze],
                    &[],
                    None
                )
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn elapsed_can_cross_without_new_history_and_measured_needs_a_fresh_positive_baseline() {
        let fixture = Fixture::new();
        let ledger = &fixture.ledger;
        enabled(ledger);
        let elapsed = WorkflowAlertCandidate::Elapsed {
            key: [1; 32],
            crossed_at_unix_ns: 12,
        };
        let below = WorkflowAlertCandidate::Measured {
            key: [2; 32],
            exceeded: Some(false),
        };
        let above = WorkflowAlertCandidate::Measured {
            key: [2; 32],
            exceeded: Some(true),
        };
        let initially_above = WorkflowAlertCandidate::Measured {
            key: [3; 32],
            exceeded: Some(true),
        };
        let claim_at = |watermark, time, alerts: &[WorkflowAlertCandidate]| {
            ledger
                .claim_workflow_delivery(read(&authority(watermark), time, 1), &[], alerts, None)
                .unwrap()
        };
        let already_crossed = WorkflowAlertCandidate::Elapsed {
            key: [4; 32],
            crossed_at_unix_ns: 11,
        };
        assert!(claim_at(1, 11, &[elapsed, below, initially_above, already_crossed]).is_empty());
        let elapsed_attempt = claim_at(1, 12, &[elapsed, below, initially_above]);
        assert_eq!(
            elapsed_attempt
                .iter()
                .map(WorkflowDeliveryAttempt::key)
                .collect::<Vec<_>>(),
            [[1; 32]]
        );
        ledger
            .finish_workflow_delivery(elapsed_attempt, WorkflowDeliveryState::Delivered)
            .unwrap();
        let measured = claim_at(2, 13, &[elapsed, above, initially_above]);
        assert_eq!(
            measured
                .iter()
                .map(WorkflowDeliveryAttempt::key)
                .collect::<Vec<_>>(),
            [[2; 32]]
        );
        ledger
            .finish_workflow_delivery(measured, WorkflowDeliveryState::Failed)
            .unwrap();
        assert!(claim_at(3, 14, &[above, initially_above]).is_empty());
        let retry = ledger
            .claim_workflow_delivery(
                read(&authority(3), 15, 1),
                &[],
                &[above],
                Some(([2; 32], 1)),
            )
            .unwrap();
        ledger
            .finish_workflow_delivery(retry, WorkflowDeliveryState::Delivered)
            .unwrap();
        assert!(claim_at(4, 16, &[below]).is_empty());
        assert!(claim_at(5, 17, &[above]).is_empty());
    }

    #[test]
    fn missing_or_stale_measured_baseline_cannot_alert() {
        let fixture = Fixture::new();
        let ledger = &fixture.ledger;
        enabled(ledger);
        let observation = |key, exceeded| WorkflowAlertCandidate::Measured {
            key: [key; 32],
            exceeded: Some(exceeded),
        };
        let claim_at = |watermark, time, alerts: &[WorkflowAlertCandidate]| {
            ledger
                .claim_workflow_delivery(read(&authority(watermark), time, 1), &[], alerts, None)
                .unwrap()
        };
        assert!(claim_at(2, 11, &[observation(2, false), observation(3, false)]).is_empty());
        assert!(claim_at(3, 12, &[observation(3, false)]).is_empty());
        let stale = 12 + REQUEST_FRESHNESS_NS as u64 + 1;
        assert!(claim_at(4, stale, &[observation(2, true), observation(3, true)]).is_empty());
        assert!(claim_at(5, stale + 1, &[observation(2, true), observation(3, true)]).is_empty());
    }

    #[test]
    fn omitted_scopes_keep_fresh_baselines_but_explicit_unknown_clears_one_key() {
        let fixture = Fixture::new();
        let ledger = &fixture.ledger;
        enabled(ledger);
        let measurement = |key, exceeded| WorkflowAlertCandidate::Measured {
            key: [key; 32],
            exceeded,
        };
        let claim_at = |watermark, time, alerts: &[WorkflowAlertCandidate]| {
            ledger
                .claim_workflow_delivery(read(&authority(watermark), time, 1), &[], alerts, None)
                .unwrap()
        };
        assert!(
            claim_at(
                2,
                11,
                &[measurement(2, Some(false)), measurement(3, Some(false))]
            )
            .is_empty()
        );
        assert!(claim_at(3, 12, &[measurement(3, None)]).is_empty());
        let attempts = claim_at(
            4,
            13,
            &[measurement(2, Some(true)), measurement(3, Some(true))],
        );
        assert_eq!(
            attempts
                .iter()
                .map(WorkflowDeliveryAttempt::key)
                .collect::<Vec<_>>(),
            [[2; 32]]
        );
    }

    #[test]
    fn version_one_claims_survive_upgrade_without_becoming_retryable() {
        let fixture = Fixture::new();
        let ledger = &fixture.ledger;
        let path = fixture.root.join("history.marker.desktop-attention.sqlite");
        create_private_file(&path).unwrap();
        let legacy = Connection::open(&path).unwrap();
        legacy.execute_batch("CREATE TABLE desktop_histories (namespace BLOB PRIMARY KEY NOT NULL,
            baseline_time INTEGER NOT NULL,baseline_watermark INTEGER NOT NULL,last_time INTEGER NOT NULL,last_watermark INTEGER NOT NULL);
            CREATE TABLE desktop_claims (namespace BLOB NOT NULL,digest BLOB NOT NULL,observed_at INTEGER NOT NULL,
            PRIMARY KEY(namespace,digest)) WITHOUT ROWID; PRAGMA application_id=1096238145; PRAGMA user_version=1;").unwrap();
        let at = authority(2);
        let (namespace, _) = ledger
            .desktop_namespace(&at.history_identity, at.snapshot_id)
            .unwrap();
        let item = request(11);
        let digest = WorkspaceLedger::workflow_delivery_key(&item).unwrap();
        legacy
            .execute(
                "INSERT INTO desktop_histories VALUES (?1,10,1,12,2)",
                [namespace],
            )
            .unwrap();
        legacy
            .execute(
                "INSERT INTO desktop_claims VALUES (?1,?2,11)",
                params![namespace, digest],
            )
            .unwrap();
        assert_eq!(
            ledger.workflow_notification_policy().unwrap(),
            WorkflowNotificationPolicy::default()
        );
        assert_eq!(
            legacy
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        let mut observed = read(&at, 12, 0);
        observed.desktop_opt_in = true;
        let inventory = std::slice::from_ref(&item);
        assert!(
            ledger
                .claim_workflow_delivery(observed, inventory, &[], None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            legacy
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            ledger.workflow_delivery_states(&at, inventory).unwrap()[0].state,
            WorkflowDeliveryState::Claimed
        );
        assert!(
            ledger
                .claim_workflow_delivery(observed, inventory, &[], Some((digest, 1)))
                .is_err()
        );
    }
}
