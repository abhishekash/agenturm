//! Connection-scoped Rust client and typed dispatch for native history.

use crate::native_admin::{
    self, AdminFailure, AdminReply, AdminRequest, BackupReceipt, GrantBinding, GrantStatus, Role,
};
use crate::native_admission::{self, AdmissionFailure, AdmissionReply, AdmissionRequest};
use crate::native_content::{
    self, DeliveryCompletion, Disclosure, EvidenceFrame, PrivateConfig, PrivateEvidenceFailure,
    PrivateEvidenceSelection, PrivatePromptSelection,
};
use crate::native_read::{self, HistorySelection, ReadFailure};
use agenturm_kernel::{
    ExtensionId, Observation, ObservationPage, ObservationQuery, ObservationStore, StoreCommit,
};
use agenturm_storage_service_contract::{ContractError, ServerPolicy, native};
use agenturm_store::{
    ObservationBatch, ResolvedTraceReview, RetentionOutcome, SessionPage, SessionQuery, Store,
    TraceReviewQuery, WorkspaceAnchor, WorkspaceHistory,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum NativeOperation {
    SourceCursor {
        source_id: String,
    },
    Admit {
        batch: ObservationBatch,
    },
    QueryHistory {
        query: ObservationQuery,
        selection: NativeSelection,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_history_identity: Option<String>,
    },
    QuerySessions {
        query: SessionQuery,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_history_identity: Option<String>,
    },
    ReviewTraceTurn {
        query: TraceReviewQuery,
        expected_history_identity: String,
    },
    HistoryIdentity {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_history_identity: Option<String>,
    },
    CheckWorkspace {
        history_identity: String,
        anchor: WorkspaceAnchor,
        selection: WorkspaceSelection,
    },
    ListPrivatePrompts {
        history_identity: String,
        source_id: ExtensionId,
        session_id: String,
    },
    RevealPrivate {
        history_identity: String,
        selection: PrivateEvidenceSelection,
        terminal: bool,
    },
    VerifyPrivatePrompt {
        history_identity: String,
        selection: PrivatePromptSelection,
    },
    VerifyPrivateOperation {
        history_identity: String,
        selection: PrivateEvidenceSelection,
        snapshot_id: u64,
    },
    Admin {
        request: AdminRequest,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WorkspaceSelection {
    Anchor {},
    Event { event_id: String, revision: u32 },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum NativeSelection {
    Observations,
    AllWorkers {
        turn_id: String,
    },
    Worker {
        turn_id: String,
        worker_id: Option<String>,
    },
    LinkedTraces,
    LinkedTurnTraces {
        turn_id: String,
        worker_id: Option<String>,
    },
}

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum NativeReply {
    Admission(Result<AdmissionReply, AdmissionFailure>),
    History(Result<ObservationPage, ReadFailure>),
    Sessions(Result<SessionPage, ReadFailure>),
    TraceReview(Result<Option<ResolvedTraceReview>, ReadFailure>),
    Identity(Result<String, ReadFailure>),
    Workspace(Result<bool, ReadFailure>),
    PrivatePrompts(Result<Vec<agenturm_content::PromptReference>, PrivateEvidenceFailure>),
    Admin(Result<AdminReply, AdminFailure>),
}

impl NativeOperation {
    pub(crate) fn delivers_private(&self) -> bool {
        matches!(
            self,
            Self::RevealPrivate { .. }
                | Self::VerifyPrivatePrompt { .. }
                | Self::VerifyPrivateOperation { .. }
        )
    }

    pub(crate) fn deliver_private(
        self,
        store: &Store,
        allowed: &BTreeSet<ExtensionId>,
        role: Role,
        private: Option<&PrivateConfig>,
        output: &mut Disclosure<'_>,
    ) -> Result<(), PrivateEvidenceFailure> {
        match self {
            Self::RevealPrivate {
                history_identity,
                selection,
                terminal,
            } => {
                authorize_private(
                    store,
                    allowed,
                    role,
                    &history_identity,
                    &selection.source_id,
                )?;
                native_content::disclose(store, private, allowed, &selection, terminal, output)
            }
            Self::VerifyPrivatePrompt {
                history_identity,
                selection,
            } => {
                authorize_private(
                    store,
                    allowed,
                    role,
                    &history_identity,
                    &selection.source_id,
                )?;
                native_content::verify_prompt(store, private, allowed, &selection, output)
            }
            Self::VerifyPrivateOperation {
                history_identity,
                selection,
                snapshot_id,
            } => {
                authorize_private(
                    store,
                    allowed,
                    role,
                    &history_identity,
                    &selection.source_id,
                )?;
                native_content::verify_operation(
                    store,
                    private,
                    allowed,
                    &selection,
                    snapshot_id,
                    output,
                )
            }
            _ => Err(PrivateEvidenceFailure::Unauthorized),
        }
    }

    pub(crate) fn requests_backup(&self, role: Role) -> bool {
        role == Role::Maintenance
            && matches!(
                self,
                Self::Admin {
                    request: AdminRequest::Backup {}
                }
            )
    }
}

pub(crate) fn dispatch(
    store: &mut Store,
    allowed: &BTreeSet<ExtensionId>,
    role: Role,
    operation: NativeOperation,
    backup_directory: Option<&Path>,
    private: Option<&PrivateConfig>,
) -> NativeReply {
    let admission = match operation {
        NativeOperation::SourceCursor { source_id } => AdmissionRequest::SourceCursor { source_id },
        NativeOperation::Admit { batch } => AdmissionRequest::Admit { batch },
        NativeOperation::QueryHistory {
            query,
            selection,
            expected_history_identity,
        } => {
            if let Err(error) = authorize_read(store, role, expected_history_identity.as_deref()) {
                return NativeReply::History(Err(error));
            }
            let selection = match &selection {
                NativeSelection::Observations => HistorySelection::Observations,
                NativeSelection::AllWorkers { turn_id } => HistorySelection::AllWorkers { turn_id },
                NativeSelection::Worker { turn_id, worker_id } => HistorySelection::Turn {
                    turn_id,
                    worker_id: worker_id.as_deref(),
                },
                NativeSelection::LinkedTraces => HistorySelection::LinkedTraces,
                NativeSelection::LinkedTurnTraces { turn_id, worker_id } => {
                    HistorySelection::LinkedTurnTraces {
                        turn_id,
                        worker_id: worker_id.as_deref(),
                    }
                }
            };
            return NativeReply::History(native_read::query_history(
                store, allowed, &query, selection,
            ));
        }
        NativeOperation::QuerySessions {
            query,
            expected_history_identity,
        } => {
            return NativeReply::Sessions(
                authorize_read(store, role, expected_history_identity.as_deref())
                    .and_then(|()| native_read::query_sessions(store, allowed, &query)),
            );
        }
        NativeOperation::ReviewTraceTurn {
            query,
            expected_history_identity,
        } => {
            return NativeReply::TraceReview(
                authorize_read(store, role, Some(&expected_history_identity))
                    .and_then(|()| native_read::review_trace_turn(store, allowed, query)),
            );
        }
        NativeOperation::HistoryIdentity {
            expected_history_identity,
        } => {
            return NativeReply::Identity(
                authorize_read(store, role, expected_history_identity.as_deref()).and_then(|()| {
                    store
                        .history_identity()
                        .map_err(|_| ReadFailure::Unavailable)
                }),
            );
        }
        NativeOperation::CheckWorkspace {
            history_identity,
            anchor,
            selection,
        } => {
            return NativeReply::Workspace(check_workspace(
                store,
                allowed,
                role,
                &history_identity,
                &anchor,
                selection,
            ));
        }
        NativeOperation::ListPrivatePrompts {
            history_identity,
            source_id,
            session_id,
        } => {
            return NativeReply::PrivatePrompts(
                authorize_private(store, allowed, role, &history_identity, &source_id).and_then(
                    |()| native_content::list(store, private, allowed, &source_id, &session_id),
                ),
            );
        }
        NativeOperation::RevealPrivate { .. }
        | NativeOperation::VerifyPrivatePrompt { .. }
        | NativeOperation::VerifyPrivateOperation { .. } => {
            return NativeReply::PrivatePrompts(Err(PrivateEvidenceFailure::Unauthorized));
        }
        NativeOperation::Admin { request } => {
            return NativeReply::Admin(native_admin::dispatch(
                store,
                allowed,
                role,
                request,
                backup_directory,
            ));
        }
    };
    if role != Role::Collector {
        return NativeReply::Admission(Err(AdmissionFailure::Unauthorized));
    }
    NativeReply::Admission(native_admission::dispatch(store, allowed, admission))
}

fn authorize_private(
    store: &Store,
    allowed: &BTreeSet<ExtensionId>,
    role: Role,
    history_identity: &str,
    source: &ExtensionId,
) -> Result<(), PrivateEvidenceFailure> {
    if role != Role::Reader || !allowed.contains(source) {
        return Err(PrivateEvidenceFailure::Unauthorized);
    }
    authorize_read(store, role, Some(history_identity)).map_err(|error| match error {
        ReadFailure::HistoryReplaced => PrivateEvidenceFailure::HistoryReplaced,
        _ => PrivateEvidenceFailure::Unavailable,
    })
}

fn check_workspace(
    store: &Store,
    allowed: &BTreeSet<ExtensionId>,
    role: Role,
    identity: &str,
    anchor: &WorkspaceAnchor,
    selection: WorkspaceSelection,
) -> Result<bool, ReadFailure> {
    if !allowed.contains(&anchor.harness_id) {
        return Err(ReadFailure::Unauthorized);
    }
    authorize_read(store, role, Some(identity))?;
    match selection {
        WorkspaceSelection::Anchor {} => store.contains_anchor(anchor),
        WorkspaceSelection::Event { event_id, revision } => {
            store.contains_event(anchor, &event_id, revision)
        }
    }
    .map_err(|_| ReadFailure::Unavailable)
}

fn authorize_read(store: &Store, role: Role, expected: Option<&str>) -> Result<(), ReadFailure> {
    if !role.reads_history() {
        return Err(ReadFailure::Unauthorized);
    }
    if let Some(identity) = expected {
        if !valid_history_identity(identity) {
            return Err(ReadFailure::InvalidRequest);
        }
        if store
            .history_identity()
            .map_err(|_| ReadFailure::Unavailable)?
            != identity
        {
            return Err(ReadFailure::HistoryReplaced);
        }
    }
    Ok(())
}

fn valid_history_identity(identity: &str) -> bool {
    identity.len() == 32
        && identity
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[derive(Debug)]
pub enum NativeClientError {
    Transport(ContractError),
    Admission(AdmissionFailure),
    Read(ReadFailure),
    Admin(AdminFailure),
    InvalidReply,
    InvalidPath,
    InvalidBatch(String),
    InvalidHistoryIdentity,
    Private(PrivateEvidenceFailure),
    PrivateOutput,
    PrivateConsumer,
}

impl Display for NativeClientError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "{error}"),
            Self::Admission(AdmissionFailure::Unauthorized)
            | Self::Read(ReadFailure::Unauthorized) => {
                formatter.write_str("source is outside the broker grant")
            }
            Self::Admission(AdmissionFailure::InvalidRequest)
            | Self::Read(ReadFailure::InvalidRequest) => {
                formatter.write_str("invalid broker request")
            }
            Self::Admission(AdmissionFailure::Conflict) => {
                formatter.write_str("broker admission conflicts with existing evidence")
            }
            Self::Admission(AdmissionFailure::StaleCursor) => {
                formatter.write_str("broker admission cursor is stale")
            }
            Self::Read(ReadFailure::HistoryReplaced) => {
                formatter.write_str("broker history changed since this workspace opened")
            }
            Self::Read(ReadFailure::ExpiredSnapshot {
                minimum_snapshot_id,
            }) => {
                write!(
                    formatter,
                    "history snapshot expired (minimum {minimum_snapshot_id})"
                )
            }
            Self::Read(ReadFailure::TraceScopeTooLarge { max_rows }) => write!(
                formatter,
                "trace exceeds the {max_rows}-row turn-resolution limit; select an explicit turn"
            ),
            Self::Admission(AdmissionFailure::Unavailable)
            | Self::Read(ReadFailure::Unavailable)
            | Self::Admin(AdminFailure::Unavailable) => {
                formatter.write_str("broker history is unavailable")
            }
            Self::Admin(AdminFailure::Unauthorized) => {
                formatter.write_str("broker operation is outside the caller grant")
            }
            Self::Admin(AdminFailure::InvalidRequest) => {
                formatter.write_str("invalid broker maintenance request")
            }
            Self::Admin(AdminFailure::IntegrityFailed) => {
                formatter.write_str("broker history failed integrity verification")
            }
            Self::InvalidReply => formatter.write_str("unexpected broker reply"),
            Self::InvalidPath => formatter.write_str("broker socket path must be absolute"),
            Self::InvalidBatch(error) => write!(formatter, "{error}"),
            Self::InvalidHistoryIdentity => {
                formatter.write_str("history identity is unavailable or invalid")
            }
            Self::Private(PrivateEvidenceFailure::Unauthorized) => {
                formatter.write_str("private evidence is outside the broker reader grant")
            }
            Self::Private(PrivateEvidenceFailure::HistoryReplaced) => {
                formatter.write_str("broker history changed since this workspace opened")
            }
            Self::Private(PrivateEvidenceFailure::SnapshotExpired) => {
                formatter.write_str("broker history snapshot expired after retention")
            }
            Self::Private(PrivateEvidenceFailure::Unavailable) => formatter
                .write_str("broker could not verify private evidence for the selected occurrence"),
            Self::PrivateOutput => {
                formatter.write_str("could not deliver private evidence to its output")
            }
            Self::PrivateConsumer => {
                formatter.write_str("could not apply verified private evidence")
            }
        }
    }
}

impl std::error::Error for NativeClientError {}

impl From<ContractError> for NativeClientError {
    fn from(error: ContractError) -> Self {
        Self::Transport(error)
    }
}

/// Explicit endpoint, verified before request bytes are sent. An unavailable
/// broker yields an error with no local file fallback.
#[derive(Clone)]
pub struct NativeStore {
    socket_path: PathBuf,
    receiver_id: String,
    policy: ServerPolicy,
    workspace_identity: Option<String>,
}

impl NativeStore {
    pub fn new(
        socket_path: impl Into<PathBuf>,
        receiver_id: impl Into<String>,
        service_uid: u32,
        service_gid: u32,
    ) -> Result<Self, NativeClientError> {
        let socket_path = socket_path.into();
        if !socket_path.is_absolute() {
            return Err(NativeClientError::InvalidPath);
        }
        let receiver_id = receiver_id.into();
        native::Request {
            version: native::VERSION,
            request_id: "00000000000000000000000000000000".into(),
            receiver_id: receiver_id.clone(),
            operation: (),
        }
        .validate()?;
        Ok(Self {
            socket_path,
            receiver_id,
            policy: ServerPolicy::for_current_client(service_uid, service_gid)?,
            workspace_identity: None,
        })
    }

    /// Binds subsequent history reads and workspace-reference checks to the exact history
    /// opened by the caller, even if the broker socket later outlives it.
    pub fn with_workspace_identity(mut self, identity: &str) -> Result<Self, NativeClientError> {
        if !valid_history_identity(identity) {
            return Err(NativeClientError::InvalidHistoryIdentity);
        }
        self.workspace_identity = Some(identity.to_owned());
        Ok(self)
    }

    fn request(&self, operation: NativeOperation) -> native::Request<NativeOperation> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        native::Request {
            version: native::VERSION,
            request_id: format!(
                "{:016x}{:016x}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
            receiver_id: self.receiver_id.clone(),
            operation,
        }
    }

    fn exchange<R: DeserializeOwned>(
        &self,
        operation: NativeOperation,
    ) -> Result<R, NativeClientError> {
        let request = self.request(operation);
        let mut stream = native::connect(&self.socket_path).map_err(ContractError::from)?;
        let identity = native::send_request(&mut stream, &request, self.policy)?;
        Ok(native::read_reply(&mut stream, &identity, self.policy)?)
    }

    fn query(
        &self,
        query: &ObservationQuery,
        selection: NativeSelection,
    ) -> Result<ObservationPage, NativeClientError> {
        self.exchange::<Result<ObservationPage, ReadFailure>>(NativeOperation::QueryHistory {
            query: query.clone(),
            selection,
            expected_history_identity: self.workspace_identity.clone(),
        })?
        .map_err(NativeClientError::Read)
    }

    pub fn query_sessions(&self, query: &SessionQuery) -> Result<SessionPage, NativeClientError> {
        self.exchange::<Result<SessionPage, ReadFailure>>(NativeOperation::QuerySessions {
            query: query.clone(),
            expected_history_identity: self.workspace_identity.clone(),
        })?
        .map_err(NativeClientError::Read)
    }

    pub fn history_identity(&self) -> Result<String, NativeClientError> {
        let identity = self
            .exchange::<Result<String, ReadFailure>>(NativeOperation::HistoryIdentity {
                expected_history_identity: self.workspace_identity.clone(),
            })?
            .map_err(NativeClientError::Read)?;
        if !valid_history_identity(&identity) {
            return Err(NativeClientError::InvalidReply);
        }
        Ok(identity)
    }

    pub fn list_private_prompts(
        &self,
        source: &ExtensionId,
        session_id: &str,
    ) -> Result<Vec<agenturm_content::PromptReference>, NativeClientError> {
        let history_identity = self
            .workspace_identity
            .clone()
            .ok_or(NativeClientError::InvalidHistoryIdentity)?;
        self.exchange::<Result<Vec<agenturm_content::PromptReference>, PrivateEvidenceFailure>>(
            NativeOperation::ListPrivatePrompts {
                history_identity,
                source_id: source.clone(),
                session_id: session_id.into(),
            },
        )?
        .map_err(NativeClientError::Private)
    }

    /// Delivers one proven occurrence and ACKs only after the final sink
    /// flushes. For terminal callers, the sink flush waits for redaction.
    /// The sink must enforce its own publication deadline below the broker's
    /// 32-second ACK wait. Arbitrary blocking writes cannot be interrupted here.
    pub fn reveal_private(
        &self,
        selection: &PrivateEvidenceSelection,
        terminal: bool,
        output: &mut dyn Write,
    ) -> Result<(), NativeClientError> {
        self.consume_private(
            NativeOperation::RevealPrivate {
                history_identity: self.private_history_identity()?,
                selection: selection.clone(),
                terminal,
            },
            |payload: &String| {
                output
                    .write_all(payload.as_bytes())
                    .and_then(|()| output.flush())
                    .map_err(|_| NativeClientError::PrivateOutput)
            },
        )
    }

    /// Consume an exact prompt while the broker's revocation and retention
    /// fence is held. The callback must be bounded and use local resources.
    /// If it publishes private bytes, it must use a sink whose publication
    /// deadline expires before the broker's 32-second ACK wait.
    /// A failed callback closes the connection without acknowledging delivery.
    pub fn with_verified_prompt<T>(
        &self,
        source: &ExtensionId,
        session_id: &str,
        event_id: &str,
        revision: u32,
        consume: impl FnOnce(&agenturm_content::VerifiedPrompt) -> Result<T, String>,
    ) -> Result<T, NativeClientError> {
        self.with_verified_prompt_at_snapshot(source, session_id, event_id, revision, None, consume)
    }

    /// Recheck an optional history snapshot under the same fence as the proof.
    pub fn with_verified_prompt_at_snapshot<T>(
        &self,
        source: &ExtensionId,
        session_id: &str,
        event_id: &str,
        revision: u32,
        snapshot_id: Option<u64>,
        consume: impl FnOnce(&agenturm_content::VerifiedPrompt) -> Result<T, String>,
    ) -> Result<T, NativeClientError> {
        self.consume_private(
            NativeOperation::VerifyPrivatePrompt {
                history_identity: self.private_history_identity()?,
                selection: PrivatePromptSelection {
                    source_id: source.clone(),
                    session_id: session_id.into(),
                    event_id: event_id.into(),
                    revision,
                    snapshot_id,
                },
            },
            |prompt: &agenturm_content::VerifiedPrompt| {
                if prompt.session_id != session_id {
                    return Err(NativeClientError::InvalidReply);
                }
                consume(prompt).map_err(|_| NativeClientError::PrivateConsumer)
            },
        )
    }

    /// Consume one exact edit before acknowledging the broker's proof.
    pub fn with_verified_edit<T>(
        &self,
        selection: &PrivateEvidenceSelection,
        snapshot_id: u64,
        consume: impl FnOnce(&agenturm_content::VerifiedEdit) -> Result<T, String>,
    ) -> Result<T, NativeClientError> {
        self.consume_private_operation(selection, snapshot_id, consume)
    }

    /// Consume a recorded Codex invocation under its independent grant.
    pub fn with_verified_command<T>(
        &self,
        selection: &PrivateEvidenceSelection,
        snapshot_id: u64,
        consume: impl FnOnce(&agenturm_content::VerifiedCommand) -> Result<T, String>,
    ) -> Result<T, NativeClientError> {
        self.consume_private_operation(selection, snapshot_id, consume)
    }

    fn consume_private_operation<P: DeserializeOwned, T>(
        &self,
        selection: &PrivateEvidenceSelection,
        snapshot_id: u64,
        consume: impl FnOnce(&P) -> Result<T, String>,
    ) -> Result<T, NativeClientError> {
        self.consume_private(
            NativeOperation::VerifyPrivateOperation {
                history_identity: self.private_history_identity()?,
                selection: selection.clone(),
                snapshot_id,
            },
            |proof| consume(proof).map_err(|_| NativeClientError::PrivateConsumer),
        )
    }

    fn private_history_identity(&self) -> Result<String, NativeClientError> {
        self.workspace_identity
            .clone()
            .ok_or(NativeClientError::InvalidHistoryIdentity)
    }

    fn consume_private<P: DeserializeOwned, T>(
        &self,
        operation: NativeOperation,
        consume: impl FnOnce(&P) -> Result<T, NativeClientError>,
    ) -> Result<T, NativeClientError> {
        let request = self.request(operation);
        let mut stream = native::connect(&self.socket_path).map_err(ContractError::from)?;
        let identity = native::send_delivery_request(&mut stream, &request, self.policy)?;
        let evidence = native::read_delivery_reply::<
            Result<EvidenceFrame<P>, PrivateEvidenceFailure>,
        >(&mut stream, &identity, self.policy)?
        .map_err(NativeClientError::Private)?;
        let result = consume(&evidence.payload)?;
        native::send_delivery_ack(&mut stream, &identity, evidence.challenge, self.policy)?;
        let DeliveryCompletion::Acknowledged =
            native::read_reply(&mut stream, &identity, self.policy)?;
        Ok(result)
    }

    fn contains(
        &self,
        anchor: &WorkspaceAnchor,
        selection: WorkspaceSelection,
    ) -> Result<bool, NativeClientError> {
        let history_identity = self
            .workspace_identity
            .clone()
            .ok_or(NativeClientError::InvalidHistoryIdentity)?;
        self.exchange::<Result<bool, ReadFailure>>(NativeOperation::CheckWorkspace {
            history_identity,
            anchor: anchor.clone(),
            selection,
        })?
        .map_err(NativeClientError::Read)
    }

    fn admin(&self, request: AdminRequest) -> Result<AdminReply, NativeClientError> {
        self.exchange::<Result<AdminReply, AdminFailure>>(NativeOperation::Admin { request })?
            .map_err(NativeClientError::Admin)
    }

    pub fn check_grant(&self, binding: &GrantBinding) -> Result<GrantStatus, NativeClientError> {
        match self.admin(AdminRequest::CheckGrant {
            binding: binding.clone(),
        })? {
            AdminReply::Grant { status } => Ok(status),
            _ => Err(NativeClientError::InvalidReply),
        }
    }

    pub fn approve_grant(&self, binding: &GrantBinding) -> Result<GrantStatus, NativeClientError> {
        match self.admin(AdminRequest::ApproveGrant {
            binding: binding.clone(),
        })? {
            AdminReply::Grant { status } => Ok(status),
            _ => Err(NativeClientError::InvalidReply),
        }
    }

    pub fn revoke_grant(&self, binding: &GrantBinding) -> Result<bool, NativeClientError> {
        match self.admin(AdminRequest::RevokeGrant {
            binding: binding.clone(),
        })? {
            AdminReply::Revoked { changed } => Ok(changed),
            _ => Err(NativeClientError::InvalidReply),
        }
    }

    pub fn prune_before(&self, cutoff_unix_ns: u64) -> Result<RetentionOutcome, NativeClientError> {
        match self.admin(AdminRequest::PruneBefore { cutoff_unix_ns })? {
            AdminReply::Pruned { outcome } => Ok(outcome),
            _ => Err(NativeClientError::InvalidReply),
        }
    }

    pub fn backup(&self) -> Result<BackupReceipt, NativeClientError> {
        match self.admin(AdminRequest::Backup {})? {
            AdminReply::BackedUp { receipt } => Ok(receipt),
            _ => Err(NativeClientError::InvalidReply),
        }
    }

    pub fn check_integrity(&self) -> Result<(), NativeClientError> {
        match self.admin(AdminRequest::CheckIntegrity {})? {
            AdminReply::IntegrityPassed => Ok(()),
            _ => Err(NativeClientError::InvalidReply),
        }
    }

    pub fn query_linked_traces(
        &self,
        query: &ObservationQuery,
    ) -> Result<ObservationPage, NativeClientError> {
        self.query(query, NativeSelection::LinkedTraces)
    }

    pub fn query_linked_turn_traces(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
        worker_id: Option<&str>,
    ) -> Result<ObservationPage, NativeClientError> {
        self.query(
            query,
            NativeSelection::LinkedTurnTraces {
                turn_id: turn_id.into(),
                worker_id: worker_id.map(str::to_owned),
            },
        )
    }

    pub fn review_trace_turn(
        &self,
        query: &TraceReviewQuery,
    ) -> Result<Option<ResolvedTraceReview>, NativeClientError> {
        let identity = self
            .workspace_identity
            .as_ref()
            .ok_or(NativeClientError::Read(ReadFailure::InvalidRequest))?;
        self.exchange::<Result<Option<ResolvedTraceReview>, ReadFailure>>(
            NativeOperation::ReviewTraceTurn {
                query: query.clone(),
                expected_history_identity: identity.clone(),
            },
        )?
        .map_err(NativeClientError::Read)
    }

    fn admission(&self, operation: NativeOperation) -> Result<AdmissionReply, NativeClientError> {
        self.exchange::<Result<AdmissionReply, AdmissionFailure>>(operation)?
            .map_err(NativeClientError::Admission)
    }
}

impl WorkspaceHistory for NativeStore {
    fn contains_anchor(&self, anchor: &WorkspaceAnchor) -> Result<bool, String> {
        self.contains(anchor, WorkspaceSelection::Anchor {})
            .map_err(|error| error.to_string())
    }

    fn contains_event(
        &self,
        anchor: &WorkspaceAnchor,
        event_id: &str,
        revision: u32,
    ) -> Result<bool, String> {
        self.contains(
            anchor,
            WorkspaceSelection::Event {
                event_id: event_id.into(),
                revision,
            },
        )
        .map_err(|error| error.to_string())
    }
}

impl ObservationStore for NativeStore {
    fn source_cursor(&self, source_id: &str) -> Result<Option<String>, String> {
        let reply = self
            .admission(NativeOperation::SourceCursor {
                source_id: source_id.into(),
            })
            .map_err(|error| error.to_string())?;
        match reply {
            AdmissionReply::Cursor { cursor } => Ok(cursor),
            _ => Err(NativeClientError::InvalidReply.to_string()),
        }
    }

    fn load_observations(&self, limit: usize) -> Result<ObservationPage, String> {
        self.query_observations(&ObservationQuery::latest(limit))
    }

    fn query_observations(&self, query: &ObservationQuery) -> Result<ObservationPage, String> {
        self.query(query, NativeSelection::Observations)
            .map_err(|error| error.to_string())
    }

    fn query_turn_observations(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
    ) -> Result<ObservationPage, String> {
        self.query(
            query,
            NativeSelection::AllWorkers {
                turn_id: turn_id.into(),
            },
        )
        .map_err(|error| error.to_string())
    }

    fn query_worker_turn_observations(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
        worker_id: Option<&str>,
    ) -> Result<ObservationPage, String> {
        self.query(
            query,
            NativeSelection::Worker {
                turn_id: turn_id.into(),
                worker_id: worker_id.map(str::to_owned),
            },
        )
        .map_err(|error| error.to_string())
    }

    fn commit(
        &mut self,
        source_id: &str,
        expected_cursor: Option<&str>,
        next_cursor: &str,
        observations: &[Observation],
    ) -> Result<StoreCommit, String> {
        let batch = ObservationBatch::from_observations(
            source_id,
            expected_cursor,
            next_cursor,
            observations,
        )
        .map_err(|error| NativeClientError::InvalidBatch(error).to_string())?;
        let reply = self
            .admission(NativeOperation::Admit { batch })
            .map_err(|error| error.to_string())?;
        match reply {
            AdmissionReply::Committed { inserted } => Ok(StoreCommit {
                inserted,
                replay: false,
            }),
            AdmissionReply::Replay => Ok(StoreCommit {
                inserted: 0,
                replay: true,
            }),
            AdmissionReply::Cursor { .. } => Err(NativeClientError::InvalidReply.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{ObservationIdentity, ObservationKind, TraceSpan, TraceStatus};
    use agenturm_store::TraceTurnQuery;

    #[test]
    fn trace_review_resolves_and_reads_under_the_same_incarnation_after_rollback() {
        let directory = std::env::temp_dir().join(format!(
            "agenturm-broker-trace-rollback-{}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let source = "harness.codex";
        let trace_source = "otlp.test";
        let trace_id = "11111111111111111111111111111111";
        let span_id = "2222222222222222";
        let session = "session-1";
        let turn = "turn-1";
        let allowed = BTreeSet::from([
            ExtensionId::parse(source).unwrap(),
            ExtensionId::parse(trace_source).unwrap(),
        ]);
        let mut scope = ObservationIdentity::session(session);
        scope.turn_id = Some(turn.into());
        let native = |event_id: &str, timestamp| Observation {
            harness_id: ExtensionId::parse(source).unwrap(),
            event_id: event_id.into(),
            revision: 0,
            identity: scope.clone(),
            observed_at_unix_ns: timestamp,
            kind: ObservationKind::Message {
                role: "user".into(),
            },
        };

        let mut original = Store::open_collector(directory.join("original.sqlite")).unwrap();
        let history_identity = original.ensure_history_identity().unwrap();
        original
            .commit(source, None, "first", &[native("before-backup", 1)])
            .unwrap();
        let backup = directory.join("backup.sqlite");
        original.backup_to(&backup).unwrap();
        original
            .commit(
                trace_source,
                None,
                "trace",
                &[Observation {
                    harness_id: ExtensionId::parse(trace_source).unwrap(),
                    event_id: format!("otlp:{trace_id}:{span_id}"),
                    revision: 0,
                    identity: scope.clone(),
                    observed_at_unix_ns: 3,
                    kind: ObservationKind::TraceSpan(TraceSpan {
                        trace_id: trace_id.into(),
                        span_id: span_id.into(),
                        parent_span_id: None,
                        target_source_id: Some(source.into()),
                        started_at_unix_ns: 2,
                        status: TraceStatus::Ok,
                    }),
                }],
            )
            .unwrap();
        let resolved = original
            .resolve_trace_turn(&TraceTurnQuery {
                harness_id: source.into(),
                session_id: session.into(),
                trace_source_id: trace_source.into(),
                trace_id: trace_id.into(),
                snapshot_id: None,
            })
            .unwrap()
            .unwrap();
        let query = ObservationQuery {
            harness_id: Some(source.into()),
            session_id: Some(session.into()),
            snapshot_id: Some(resolved.snapshot_id),
            ..ObservationQuery::latest(10)
        };
        let request = |linked_traces| NativeOperation::ReviewTraceTurn {
            query: TraceReviewQuery {
                page: query.clone(),
                trace_source_id: trace_source.into(),
                trace_id: trace_id.into(),
                expected_worker_id: None,
                linked_traces,
            },
            expected_history_identity: history_identity.clone(),
        };
        for linked_traces in [false, true] {
            let native_request = serde_json::to_vec(&request(linked_traces)).unwrap();
            let decoded = serde_json::from_slice(&native_request).unwrap();
            let NativeReply::TraceReview(Ok(Some(review))) =
                dispatch(&mut original, &allowed, Role::Reader, decoded, None, None)
            else {
                panic!("original turn must resolve");
            };
            assert_eq!(review.turn_id, turn);
            assert_eq!(review.page.snapshot_id, resolved.snapshot_id);
            assert_eq!(review.page.observations.len(), 1);
            let returned_source = if linked_traces { trace_source } else { source };
            assert_eq!(
                review.page.observations[0].harness_id.as_str(),
                returned_source
            );
        }

        // Model an older backup taking over the same endpoint, then receiving
        // enough writes to recreate the original numeric snapshot ID.
        let restored_path = directory.join("restored.sqlite");
        Store::open_read_only(&backup)
            .unwrap()
            .backup_to(&restored_path)
            .unwrap();
        let mut restored = Store::open_collector(restored_path).unwrap();
        assert_eq!(
            restored.ensure_history_identity().unwrap(),
            history_identity
        );
        restored
            .commit(
                source,
                Some("first"),
                "regrown",
                &[native("rollback-branch", 4)],
            )
            .unwrap();
        let split_read = NativeOperation::QueryHistory {
            query: query.clone(),
            selection: NativeSelection::Worker {
                turn_id: resolved.turn_id,
                worker_id: resolved.worker_id,
            },
            expected_history_identity: Some(history_identity.clone()),
        };
        assert!(matches!(
            dispatch(&mut restored, &allowed, Role::Reader, split_read, None, None),
            NativeReply::History(Ok(page))
                if page.snapshot_id == resolved.snapshot_id
                    && page.observations.iter().any(|row| row.event_id == "rollback-branch")
        ));
        for linked_traces in [false, true] {
            assert!(matches!(
                dispatch(
                    &mut restored,
                    &allowed,
                    Role::Reader,
                    request(linked_traces),
                    None,
                    None
                ),
                NativeReply::TraceReview(Ok(None))
            ));
        }
        drop((original, restored));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn trace_review_requests_reject_a_replacement_history_at_the_same_snapshot() {
        let directory = std::env::temp_dir().join(format!(
            "agenturm-broker-trace-history-{}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let mut original = Store::open_collector(directory.join("original.sqlite")).unwrap();
        let identity = original.ensure_history_identity().unwrap();
        let mut replacement = Store::open_collector(directory.join("replacement.sqlite")).unwrap();
        assert_ne!(replacement.ensure_history_identity().unwrap(), identity);

        let mut query = ObservationQuery::latest(1);
        query.harness_id = Some("harness.codex".into());
        query.session_id = Some("session-1".into());
        query.snapshot_id = Some(0);
        let allowed = BTreeSet::from([
            ExtensionId::parse("harness.codex").unwrap(),
            ExtensionId::parse("otlp.test").unwrap(),
        ]);
        let resolve = || NativeOperation::ReviewTraceTurn {
            query: TraceReviewQuery {
                page: query.clone(),
                trace_source_id: "otlp.test".into(),
                trace_id: "11111111111111111111111111111111".into(),
                expected_worker_id: None,
                linked_traces: false,
            },
            expected_history_identity: identity.clone(),
        };
        let linked = || NativeOperation::QueryHistory {
            query: query.clone(),
            selection: NativeSelection::LinkedTurnTraces {
                turn_id: "turn-1".into(),
                worker_id: None,
            },
            expected_history_identity: Some(identity.clone()),
        };
        assert!(matches!(
            dispatch(&mut original, &allowed, Role::Reader, resolve(), None, None),
            NativeReply::TraceReview(Ok(None))
        ));
        assert!(matches!(
            dispatch(&mut original, &allowed, Role::Reader, linked(), None, None),
            NativeReply::History(Ok(page)) if page.snapshot_id == 0
        ));
        assert!(matches!(
            dispatch(
                &mut replacement,
                &allowed,
                Role::Reader,
                resolve(),
                None,
                None
            ),
            NativeReply::TraceReview(Err(ReadFailure::HistoryReplaced))
        ));
        assert!(matches!(
            dispatch(
                &mut replacement,
                &allowed,
                Role::Reader,
                linked(),
                None,
                None
            ),
            NativeReply::History(Err(ReadFailure::HistoryReplaced))
        ));
        drop((original, replacement));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
