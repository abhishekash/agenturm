//! Administrative broker operations. Request data never selects a path.

use agenturm_kernel::ExtensionId;
use agenturm_store::{ContentGrantScope, ContentKind, RetentionOutcome, Store, StoreError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Role {
    Collector,
    Reader,
    GrantAdministrator,
    Maintenance,
}

impl Role {
    pub(crate) fn reads_history(self) -> bool {
        matches!(self, Self::Collector | Self::Reader)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GrantBinding {
    pub source_id: ExtensionId,
    pub kind: ContentKind,
    pub source_fingerprint: [u8; 32],
}

impl From<&ContentGrantScope> for GrantBinding {
    fn from(scope: &ContentGrantScope) -> Self {
        Self {
            source_id: scope.source_id().clone(),
            kind: scope.kind(),
            source_fingerprint: scope.source_fingerprint(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GrantStatus {
    /// No epoch means the exact binding lacks current approval.
    pub epoch: Option<i64>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackupReceipt {
    /// Server-assigned filename inside the broker's private backup root.
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum AdminRequest {
    CheckGrant { binding: GrantBinding },
    ApproveGrant { binding: GrantBinding },
    RevokeGrant { binding: GrantBinding },
    PruneBefore { cutoff_unix_ns: u64 },
    Backup {},
    CheckIntegrity {},
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum AdminReply {
    Grant { status: GrantStatus },
    Revoked { changed: bool },
    Pruned { outcome: RetentionOutcome },
    BackedUp { receipt: BackupReceipt },
    IntegrityPassed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminFailure {
    Unauthorized,
    InvalidRequest,
    Unavailable,
    IntegrityFailed,
}

pub(crate) fn dispatch(
    store: &mut Store,
    allowed: &BTreeSet<ExtensionId>,
    role: Role,
    request: AdminRequest,
    backup_directory: Option<&Path>,
) -> Result<AdminReply, AdminFailure> {
    let permitted = match &request {
        AdminRequest::CheckGrant { binding } => {
            (role.reads_history() || role == Role::GrantAdministrator)
                && allowed.contains(&binding.source_id)
        }
        AdminRequest::ApproveGrant { binding } | AdminRequest::RevokeGrant { binding } => {
            role == Role::GrantAdministrator && allowed.contains(&binding.source_id)
        }
        AdminRequest::PruneBefore { .. }
        | AdminRequest::Backup {}
        | AdminRequest::CheckIntegrity {} => role == Role::Maintenance,
    };
    if !permitted {
        return Err(AdminFailure::Unauthorized);
    }

    match request {
        AdminRequest::CheckGrant { binding } => {
            let grant = store
                .current_content_grant(&binding.scope())
                .map_err(failure)?;
            Ok(AdminReply::Grant {
                status: GrantStatus {
                    epoch: grant.map(|grant| grant.epoch()),
                },
            })
        }
        AdminRequest::ApproveGrant { binding } => {
            let grant = store.grant_content(&binding.scope()).map_err(failure)?;
            Ok(AdminReply::Grant {
                status: GrantStatus {
                    epoch: Some(grant.epoch()),
                },
            })
        }
        AdminRequest::RevokeGrant { binding } => {
            let changed = store
                .revoke_bound_content(&binding.scope())
                .map_err(failure)?;
            Ok(AdminReply::Revoked { changed })
        }
        AdminRequest::PruneBefore { cutoff_unix_ns } => {
            let outcome = store
                .prune_observations_before(cutoff_unix_ns)
                .map_err(failure)?;
            Ok(AdminReply::Pruned { outcome })
        }
        AdminRequest::Backup {} => {
            let directory = backup_directory.ok_or(AdminFailure::Unavailable)?;
            let receipt = backup(store, directory)?;
            Ok(AdminReply::BackedUp { receipt })
        }
        AdminRequest::CheckIntegrity {} => match store.check_history_integrity() {
            Ok(()) => Ok(AdminReply::IntegrityPassed),
            Err(StoreError::Invalid(_)) => Err(AdminFailure::IntegrityFailed),
            Err(_) => Err(AdminFailure::Unavailable),
        },
    }
}

impl GrantBinding {
    fn scope(self) -> ContentGrantScope {
        ContentGrantScope::new(self.source_id, self.kind, self.source_fingerprint)
    }
}

fn failure(error: StoreError) -> AdminFailure {
    match error {
        StoreError::Invalid(_) => AdminFailure::InvalidRequest,
        _ => AdminFailure::Unavailable,
    }
}

fn backup(store: &Store, directory: &Path) -> Result<BackupReceipt, AdminFailure> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AdminFailure::Unavailable)?;
    let name = format!(
        "history-{:032x}-{:08x}-{:016x}.sqlite",
        now.as_nanos(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    store
        .backup_to(directory.join(&name))
        .map_err(|_| AdminFailure::Unavailable)?;
    Ok(BackupReceipt { name })
}

pub(crate) fn valid_backup_name(name: &str) -> bool {
    let Some(segments) = name
        .strip_prefix("history-")
        .and_then(|name| name.strip_suffix(".sqlite"))
    else {
        return false;
    };
    let mut segments = segments.split('-');
    [32, 8, 16].into_iter().all(|len| {
        segments.next().is_some_and(|part| {
            part.len() == len
                && part
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
    }) && segments.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::ObservationQuery;
    use agenturm_store::{
        CommitOutcome, ObservationBatch, ObservationIdentity, ObservationKind, StoredObservation,
        StoredScope,
    };
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::path::PathBuf;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "agenturm-broker-admin-{}-{nonce}",
                std::process::id()
            ));
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
            Self(directory)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn binding(source: &str, fingerprint: u8) -> GrantBinding {
        GrantBinding {
            source_id: ExtensionId::parse(source).unwrap(),
            kind: ContentKind::PiUserText,
            source_fingerprint: [fingerprint; 32],
        }
    }

    #[test]
    fn grant_checks_are_source_bound_and_mutations_require_grant_admin() {
        let fixture = Fixture::new();
        let mut store = Store::open_collector(fixture.0.join("history.sqlite")).unwrap();
        let approved = binding("harness.pi", 1);
        let replacement = binding("harness.pi", 2);
        let allowed = BTreeSet::from([approved.source_id.clone()]);
        let mut apply = |role, request| dispatch(&mut store, &allowed, role, request, None);
        assert!(matches!(
            apply(
                Role::Collector,
                AdminRequest::ApproveGrant {
                    binding: approved.clone()
                }
            ),
            Err(AdminFailure::Unauthorized)
        ));
        assert!(matches!(
            apply(
                Role::GrantAdministrator,
                AdminRequest::ApproveGrant {
                    binding: binding("harness.other", 1)
                }
            ),
            Err(AdminFailure::Unauthorized)
        ));
        let approve = |binding: &GrantBinding| AdminRequest::ApproveGrant {
            binding: binding.clone(),
        };
        let check = |binding: &GrantBinding| AdminRequest::CheckGrant {
            binding: binding.clone(),
        };
        let revoke = |binding: &GrantBinding| AdminRequest::RevokeGrant {
            binding: binding.clone(),
        };
        assert!(matches!(
            apply(Role::GrantAdministrator, approve(&approved)),
            Ok(AdminReply::Grant {
                status: GrantStatus { epoch: Some(1) }
            })
        ));
        assert!(matches!(
            apply(Role::Reader, check(&approved)),
            Ok(AdminReply::Grant {
                status: GrantStatus { epoch: Some(1) }
            })
        ));
        assert!(matches!(
            apply(Role::Reader, check(&replacement)),
            Ok(AdminReply::Grant {
                status: GrantStatus { epoch: None }
            })
        ));
        assert!(matches!(
            apply(Role::Reader, revoke(&approved)),
            Err(AdminFailure::Unauthorized)
        ));
        assert!(matches!(
            apply(Role::GrantAdministrator, approve(&replacement)),
            Ok(AdminReply::Grant {
                status: GrantStatus { epoch: Some(2) }
            })
        ));
        assert!(matches!(
            apply(Role::GrantAdministrator, revoke(&approved)),
            Ok(AdminReply::Revoked { changed: false })
        ));
        assert!(matches!(
            apply(Role::Reader, check(&replacement)),
            Ok(AdminReply::Grant {
                status: GrantStatus { epoch: Some(2) }
            })
        ));
        assert!(matches!(
            apply(Role::GrantAdministrator, revoke(&replacement)),
            Ok(AdminReply::Revoked { changed: true })
        ));
        assert!(matches!(
            apply(Role::Reader, check(&replacement)),
            Ok(AdminReply::Grant {
                status: GrantStatus { epoch: None }
            })
        ));
    }

    #[test]
    fn maintenance_keeps_replay_and_backup_restores_with_fresh_approvals() {
        use crate::native_read::ReadFailure;
        use crate::native_store::{
            NativeOperation, NativeReply, NativeSelection, WorkspaceSelection,
        };
        use agenturm_store::{SessionQuery, WorkspaceAnchor};

        let fixture = Fixture::new();
        let history = fixture.0.join("history.sqlite");
        let directory = fixture.0.join("backups");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let approved = binding("harness.pi", 1);
        let allowed = BTreeSet::from([approved.source_id.clone()]);
        let mut store = Store::open_collector(&history).unwrap();
        let identity = store.ensure_history_identity().unwrap();
        assert_eq!(store.ensure_history_identity().unwrap(), identity);
        store.grant_content(&approved.clone().scope()).unwrap();
        let batch = ObservationBatch::new_deterministic(
            approved.source_id.as_str(),
            None,
            "cursor-1",
            vec![StoredObservation {
                event_id: "event-1".into(),
                revision: 0,
                identity: ObservationIdentity {
                    scope: StoredScope::Session,
                    session_id: Some("session-1".into()),
                    worker_id: None,
                    turn_id: None,
                    operation_id: None,
                },
                observed_at_unix_ns: 1,
                kind: ObservationKind::SessionStarted,
            }],
        )
        .unwrap();
        store.commit_batch(&batch).unwrap();
        let snapshot = store.observation_page(10).unwrap().snapshot_id;
        let stale = "0".repeat(32);
        let anchor = WorkspaceAnchor {
            harness_id: approved.source_id.clone(),
            session_id: "session-1".into(),
            worker_id: None,
            turn_id: None,
        };
        let mut read = |role, operation| {
            crate::native_store::dispatch(&mut store, &allowed, role, operation, None, None)
        };
        assert!(matches!(
            read(
                Role::Reader,
                NativeOperation::CheckWorkspace {
                    history_identity: identity.clone(),
                    anchor: anchor.clone(),
                    selection: WorkspaceSelection::Event {
                        event_id: "event-1".into(),
                        revision: 0
                    },
                }
            ),
            NativeReply::Workspace(Ok(true))
        ));
        assert!(matches!(
            read(
                Role::Reader,
                NativeOperation::CheckWorkspace {
                    history_identity: stale.clone(),
                    anchor: anchor.clone(),
                    selection: WorkspaceSelection::Anchor {},
                }
            ),
            NativeReply::Workspace(Err(ReadFailure::HistoryReplaced))
        ));
        assert!(matches!(
            read(
                Role::Reader,
                NativeOperation::QueryHistory {
                    query: ObservationQuery::latest(10),
                    selection: NativeSelection::Observations,
                    expected_history_identity: Some(stale.clone()),
                }
            ),
            NativeReply::History(Err(ReadFailure::HistoryReplaced))
        ));
        assert!(matches!(
            read(
                Role::Reader,
                NativeOperation::QuerySessions {
                    query: SessionQuery::latest(10),
                    expected_history_identity: Some(stale),
                }
            ),
            NativeReply::Sessions(Err(ReadFailure::HistoryReplaced))
        ));
        assert!(matches!(
            read(
                Role::Maintenance,
                NativeOperation::HistoryIdentity {
                    expected_history_identity: None
                }
            ),
            NativeReply::Identity(Err(ReadFailure::Unauthorized))
        ));
        assert!(matches!(
            read(
                Role::Reader,
                NativeOperation::Admit {
                    batch: batch.clone()
                }
            ),
            NativeReply::Admission(Err(crate::native_admission::AdmissionFailure::Unauthorized))
        ));
        let prune = || AdminRequest::PruneBefore { cutoff_unix_ns: 2 };
        let mut apply =
            |role, request| dispatch(&mut store, &allowed, role, request, Some(&directory));
        assert!(matches!(
            apply(Role::GrantAdministrator, prune()),
            Err(AdminFailure::Unauthorized)
        ));
        assert!(matches!(
            apply(Role::Reader, AdminRequest::Backup {}),
            Err(AdminFailure::Unauthorized)
        ));
        assert!(matches!(
            apply(Role::Maintenance, AdminRequest::CheckIntegrity {}),
            Ok(AdminReply::IntegrityPassed)
        ));
        let Ok(AdminReply::BackedUp { receipt }) =
            apply(Role::Maintenance, AdminRequest::Backup {})
        else {
            panic!("expected verified backup receipt");
        };
        let path = directory.join(&receipt.name);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o600
        );
        let detached = Store::open_read_only(path).unwrap();
        assert_eq!(detached.history_identity().unwrap(), identity);
        assert_eq!(detached.observation_page(10).unwrap().snapshot_id, snapshot);
        assert!(
            detached
                .current_content_grant(&approved.scope())
                .unwrap()
                .is_none()
        );
        assert!(
            matches!(apply(Role::Maintenance, prune()), Ok(AdminReply::Pruned { outcome }) if outcome.removed_observations == 1)
        );
        assert_eq!(
            store.source_cursor("harness.pi").unwrap().as_deref(),
            Some("cursor-1")
        );
        assert_eq!(store.commit_batch(&batch).unwrap(), CommitOutcome::Replay);
        assert!(matches!(
            store.query_observations(&ObservationQuery {
                snapshot_id: Some(snapshot),
                ..ObservationQuery::latest(10)
            }),
            Err(StoreError::ExpiredHistorySnapshot { .. })
        ));
        drop(store);
        let mut reopened = Store::open_collector(history).unwrap();
        assert_eq!(reopened.ensure_history_identity().unwrap(), identity);
        let mut fresh = Store::open_collector(fixture.0.join("another.sqlite")).unwrap();
        assert_ne!(fresh.ensure_history_identity().unwrap(), identity);
    }
}
