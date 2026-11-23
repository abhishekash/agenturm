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
use crate::reader_history::{self, ReaderHistoryReply, ReaderHistoryRequest};
use agenturm_kernel::{
    ExtensionId, Observation, ObservationPage, ObservationQuery, ObservationStore, StoreCommit,
};
use agenturm_kernel::{ReadIncarnation, ReadSnapshot};
use agenturm_storage_service_contract::{ContractError, ServerPolicy, native};
use agenturm_store::{
    AutomaticRetentionPolicy, AutomaticRetentionStatus, EvalImportReport, EvalOutcome,
    ObservationBatch, ResolvedTraceReview, RetainedEvalOutcome, RetentionOutcome, SessionPage,
    SessionQuery, Store, TelemetryBinding, TelemetryPayload, TelemetryPayloadRecord,
    TelemetryPayloadSelection, TraceReviewQuery, WorkspaceAnchor, WorkspaceHistory,
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
    CheckTelemetryAdmission {
        binding: TelemetryBinding,
    },
    AdmitTelemetry {
        binding: TelemetryBinding,
        batch: ObservationBatch,
        records: Vec<TelemetryPayloadRecord>,
    },
    RevealTelemetry {
        history_identity: String,
        selection: TelemetryPayloadSelection,
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
    ImportEvalOutcomes {
        source_id: ExtensionId,
        outcomes: Vec<EvalOutcome>,
    },
    EvalOutcome {
        source_id: ExtensionId,
        trace_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_history_identity: Option<String>,
    },
    HistoryIdentity {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_history_identity: Option<String>,
    },
    ReadBinding {
        history_identity: String,
    },
    ReaderHistory {
        expected_history_identity: Option<String>,
        request: ReaderHistoryRequest,
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
        snapshot_id: ReadSnapshot,
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
pub enum NativeSelection {
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

impl NativeSelection {
    pub(crate) fn as_history(&self) -> HistorySelection<'_> {
        match self {
            Self::Observations => HistorySelection::Observations,
            Self::AllWorkers { turn_id } => HistorySelection::AllWorkers { turn_id },
            Self::Worker { turn_id, worker_id } => HistorySelection::Turn {
                turn_id,
                worker_id: worker_id.as_deref(),
            },
            Self::LinkedTraces => HistorySelection::LinkedTraces,
            Self::LinkedTurnTraces { turn_id, worker_id } => HistorySelection::LinkedTurnTraces {
                turn_id,
                worker_id: worker_id.as_deref(),
            },
        }
    }
}

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum NativeReply {
    Admission(Result<AdmissionReply, AdmissionFailure>),
    History(Result<ObservationPage, ReadFailure>),
    Sessions(Result<SessionPage, ReadFailure>),
    TraceReview(Result<Option<ResolvedTraceReview>, ReadFailure>),
    EvalImported(Result<EvalImportReport, ReadFailure>),
    EvalOutcome(Result<Option<RetainedEvalOutcome>, ReadFailure>),
    Identity(Result<String, ReadFailure>),
    ReadBinding(Result<String, ReadFailure>),
    ReaderHistory(Result<ReaderHistoryReply, ReadFailure>),
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
                | Self::RevealTelemetry { .. }
        )
    }

    pub(crate) fn deliver_private(
        self,
        store: &Store,
        allowed: &BTreeSet<ExtensionId>,
        role: Role,
        private: Option<&PrivateConfig>,
        telemetry: &[TelemetryBinding],
        output: &mut Disclosure<'_>,
    ) -> Result<(), PrivateEvidenceFailure> {
        match self {
            Self::RevealTelemetry {
                history_identity,
                selection,
            } => {
                if !telemetry.contains(&selection.binding) {
                    return Err(PrivateEvidenceFailure::Unauthorized);
                }
                authorize_private(
                    store,
                    allowed,
                    role,
                    &history_identity,
                    &selection.binding.source_id,
                )?;
                native_content::disclose_telemetry(store, &selection, output)
            }
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

pub(crate) fn dispatch_with_telemetry(
    store: &mut Store,
    allowed: &BTreeSet<ExtensionId>,
    role: Role,
    operation: NativeOperation,
    backup_directory: Option<&Path>,
    private: Option<&PrivateConfig>,
    telemetry: &[TelemetryBinding],
) -> NativeReply {
    match operation {
        NativeOperation::CheckTelemetryAdmission { binding } => {
            NativeReply::Admission(if role == Role::Collector && telemetry.contains(&binding) {
                Ok(AdmissionReply::PayloadPolicyEnabled)
            } else {
                Err(AdmissionFailure::Unauthorized)
            })
        }
        NativeOperation::AdmitTelemetry {
            binding,
            batch,
            records,
        } => NativeReply::Admission(if role == Role::Collector && telemetry.contains(&binding) {
            native_admission::admit_telemetry(store, &binding, batch, &records)
        } else {
            Err(AdmissionFailure::Unauthorized)
        }),
        NativeOperation::Admin { request }
            if matches!(&request, AdminRequest::ApproveGrant { binding }
            if binding.kind == agenturm_store::ContentKind::TelemetrySpanPayload
                && !telemetry.iter().any(|configured| configured.grant_scope().is_ok_and(
                    |scope| GrantBinding::from(&scope) == *binding
                ))) =>
        {
            NativeReply::Admin(Err(AdminFailure::Unauthorized))
        }
        operation => dispatch(store, allowed, role, operation, backup_directory, private),
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
        NativeOperation::CheckTelemetryAdmission { .. }
        | NativeOperation::AdmitTelemetry { .. } => {
            return NativeReply::Admission(Err(AdmissionFailure::Unauthorized));
        }
        NativeOperation::Admit { batch } => AdmissionRequest::Admit { batch },
        NativeOperation::QueryHistory {
            query,
            selection,
            expected_history_identity,
        } => {
            if let Err(error) = authorize_read(store, role, expected_history_identity.as_deref()) {
                return NativeReply::History(Err(error));
            }
            return NativeReply::History(native_read::query_history(
                store,
                allowed,
                &query,
                selection.as_history(),
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
        NativeOperation::ImportEvalOutcomes {
            source_id,
            outcomes,
        } => {
            return NativeReply::EvalImported(
                if role == Role::Collector && allowed.contains(&source_id) {
                    store
                        .import_eval_outcomes(&source_id, &outcomes)
                        .map_err(native_read::failure)
                } else {
                    Err(ReadFailure::Unauthorized)
                },
            );
        }
        NativeOperation::EvalOutcome {
            source_id,
            trace_id,
            expected_history_identity,
        } => {
            return NativeReply::EvalOutcome(if allowed.contains(&source_id) {
                authorize_read(store, role, expected_history_identity.as_deref()).and_then(|()| {
                    store
                        .eval_outcome(&source_id, &trace_id)
                        .map_err(native_read::failure)
                })
            } else {
                Err(ReadFailure::Unauthorized)
            });
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
        NativeOperation::ReadBinding { history_identity } => {
            return NativeReply::ReadBinding(authorize_read(store, role, None).and_then(|()| {
                if !valid_history_identity(&history_identity) {
                    return Err(ReadFailure::InvalidRequest);
                }
                let identity = store
                    .history_identity()
                    .map_err(|_| ReadFailure::Unavailable)?;
                if identity != history_identity {
                    return Err(ReadFailure::HistoryReplaced);
                }
                let incarnation = store
                    .read_incarnation()
                    .map_err(|_| ReadFailure::Unavailable)?;
                Ok(match incarnation {
                    Some(incarnation) => format!("{identity}:{incarnation}"),
                    None => identity,
                })
            }));
        }
        NativeOperation::ReaderHistory {
            expected_history_identity,
            request,
        } => {
            return NativeReply::ReaderHistory(reader_history::dispatch(
                store,
                allowed,
                role,
                expected_history_identity.as_deref(),
                request,
            ));
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
        | NativeOperation::VerifyPrivateOperation { .. }
        | NativeOperation::RevealTelemetry { .. } => {
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
        ReadFailure::HistoryReplaced | ReadFailure::SnapshotReplaced => {
            PrivateEvidenceFailure::HistoryReplaced
        }
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
    if let Some(binding) = expected {
        verify_read_binding(store, binding)?;
    }
    Ok(())
}

fn parse_read_binding(binding: &str) -> Option<(&str, Option<ReadIncarnation>)> {
    let (identity, incarnation) = match binding.split_once(':') {
        Some((identity, marker)) => (identity, Some(marker.parse().ok()?)),
        None => (binding, None),
    };
    valid_history_identity(identity).then_some((identity, incarnation))
}

/// Extract the persistent workspace identity from a validated read binding.
pub fn logical_read_identity(binding: &str) -> Option<&str> {
    parse_read_binding(binding).map(|(identity, _)| identity)
}

pub(crate) fn verify_read_binding(store: &Store, expected: &str) -> Result<(), ReadFailure> {
    let (identity, incarnation) =
        parse_read_binding(expected).ok_or(ReadFailure::InvalidRequest)?;
    if store
        .history_identity()
        .map_err(|_| ReadFailure::Unavailable)?
        != identity
    {
        return Err(ReadFailure::HistoryReplaced);
    }
    if store
        .read_incarnation()
        .map_err(|_| ReadFailure::Unavailable)?
        != incarnation
    {
        return Err(ReadFailure::SnapshotReplaced);
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
            Self::Read(ReadFailure::SnapshotReplaced) => {
                formatter.write_str("broker history was restored or replaced; start a new query")
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
            Self::Read(ReadFailure::EvalTraceNotRetained) => formatter
                .write_str("an evaluation trace is absent from the selected retained source"),
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
    read_binding: Option<String>,
}

impl NativeStore {
    pub fn import_eval_outcomes(
        &self,
        source_id: &ExtensionId,
        outcomes: &[EvalOutcome],
    ) -> Result<EvalImportReport, NativeClientError> {
        self.exchange::<Result<EvalImportReport, ReadFailure>>(
            NativeOperation::ImportEvalOutcomes {
                source_id: source_id.clone(),
                outcomes: outcomes.to_vec(),
            },
        )?
        .map_err(NativeClientError::Read)
    }

    pub fn eval_outcome(
        &self,
        source_id: &ExtensionId,
        trace_id: &str,
    ) -> Result<Option<RetainedEvalOutcome>, NativeClientError> {
        self.exchange::<Result<Option<RetainedEvalOutcome>, ReadFailure>>(
            NativeOperation::EvalOutcome {
                source_id: source_id.clone(),
                trace_id: trace_id.into(),
                expected_history_identity: self.read_binding.clone(),
            },
        )?
        .map_err(NativeClientError::Read)
    }

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
            read_binding: None,
        })
    }

    /// Binds subsequent history reads and workspace-reference checks to the exact history
    /// opened by the caller, even if the broker socket later outlives it.
    pub fn with_workspace_identity(mut self, identity: &str) -> Result<Self, NativeClientError> {
        if !valid_history_identity(identity) {
            return Err(NativeClientError::InvalidHistoryIdentity);
        }
        if let Some(bound) = self.read_binding.as_deref() {
            return if logical_read_identity(bound) == Some(identity) {
                Ok(self)
            } else {
                Err(NativeClientError::Read(ReadFailure::HistoryReplaced))
            };
        }
        let binding = self
            .exchange::<Result<String, ReadFailure>>(NativeOperation::ReadBinding {
                history_identity: identity.to_owned(),
            })?
            .map_err(NativeClientError::Read)?;
        if logical_read_identity(&binding) != Some(identity) {
            return Err(NativeClientError::InvalidReply);
        }
        self.read_binding = Some(binding);
        Ok(self)
    }

    /// Read binding for a transport that forwards these pinned reads.
    pub fn read_binding(&self) -> Option<&str> {
        self.read_binding.as_deref()
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
            expected_history_identity: self.read_binding.clone(),
        })?
        .map_err(NativeClientError::Read)
    }

    pub fn query_sessions(&self, query: &SessionQuery) -> Result<SessionPage, NativeClientError> {
        self.exchange::<Result<SessionPage, ReadFailure>>(NativeOperation::QuerySessions {
            query: query.clone(),
            expected_history_identity: self.read_binding.clone(),
        })?
        .map_err(NativeClientError::Read)
    }

    pub fn history_identity(&self) -> Result<String, NativeClientError> {
        let identity = self
            .exchange::<Result<String, ReadFailure>>(NativeOperation::HistoryIdentity {
                expected_history_identity: self.read_binding.clone(),
            })?
            .map_err(NativeClientError::Read)?;
        if !valid_history_identity(&identity) {
            return Err(NativeClientError::InvalidReply);
        }
        Ok(identity)
    }

    /// Reads metadata only, and only when the service authenticated this
    /// process as the dedicated reader principal.
    pub fn reader_history(
        &self,
        request: ReaderHistoryRequest,
    ) -> Result<ReaderHistoryReply, NativeClientError> {
        self.exchange::<Result<ReaderHistoryReply, ReadFailure>>(NativeOperation::ReaderHistory {
            expected_history_identity: self.read_binding.clone(),
            request,
        })?
        .map_err(NativeClientError::Read)
    }

    pub fn list_private_prompts(
        &self,
        source: &ExtensionId,
        session_id: &str,
    ) -> Result<Vec<agenturm_content::PromptReference>, NativeClientError> {
        let history_identity = self
            .read_binding
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
        snapshot_id: Option<ReadSnapshot>,
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
        snapshot_id: ReadSnapshot,
        consume: impl FnOnce(&agenturm_content::VerifiedEdit) -> Result<T, String>,
    ) -> Result<T, NativeClientError> {
        self.consume_private_operation(selection, snapshot_id, consume)
    }

    /// Consume a recorded Codex invocation under its independent grant.
    pub fn with_verified_command<T>(
        &self,
        selection: &PrivateEvidenceSelection,
        snapshot_id: ReadSnapshot,
        consume: impl FnOnce(&agenturm_content::VerifiedCommand) -> Result<T, String>,
    ) -> Result<T, NativeClientError> {
        self.consume_private_operation(selection, snapshot_id, consume)
    }

    pub fn with_telemetry_payload<T>(
        &self,
        selection: &TelemetryPayloadSelection,
        consume: impl FnOnce(&TelemetryPayload) -> Result<T, String>,
    ) -> Result<T, NativeClientError> {
        self.consume_private(
            NativeOperation::RevealTelemetry {
                history_identity: self.private_history_identity()?,
                selection: selection.clone(),
            },
            |payload: &TelemetryPayload| {
                payload
                    .validate()
                    .map_err(|_| NativeClientError::InvalidReply)?;
                consume(payload).map_err(|_| NativeClientError::PrivateConsumer)
            },
        )
    }

    pub fn check_telemetry_admission(
        &self,
        binding: &TelemetryBinding,
    ) -> Result<(), NativeClientError> {
        match self.admission(NativeOperation::CheckTelemetryAdmission {
            binding: binding.clone(),
        })? {
            AdmissionReply::PayloadPolicyEnabled => Ok(()),
            _ => Err(NativeClientError::InvalidReply),
        }
    }

    pub fn commit_telemetry_batch(
        &self,
        binding: &TelemetryBinding,
        batch: ObservationBatch,
        records: Vec<TelemetryPayloadRecord>,
    ) -> Result<usize, NativeClientError> {
        match self.admission(NativeOperation::AdmitTelemetry {
            binding: binding.clone(),
            batch,
            records,
        })? {
            AdmissionReply::Committed { inserted } => Ok(inserted),
            AdmissionReply::Replay => Ok(0),
            _ => Err(NativeClientError::InvalidReply),
        }
    }

    fn consume_private_operation<P: DeserializeOwned, T>(
        &self,
        selection: &PrivateEvidenceSelection,
        snapshot_id: ReadSnapshot,
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
        self.read_binding
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
            .read_binding
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

    pub fn automatic_retention_status(
        &self,
    ) -> Result<AutomaticRetentionStatus, NativeClientError> {
        match self.admin(AdminRequest::GetRetentionPolicy {})? {
            AdminReply::RetentionPolicy { status } => Ok(status),
            _ => Err(NativeClientError::InvalidReply),
        }
    }

    pub fn set_automatic_retention(
        &self,
        policy: AutomaticRetentionPolicy,
    ) -> Result<AutomaticRetentionStatus, NativeClientError> {
        match self.admin(AdminRequest::SetRetentionPolicy { policy })? {
            AdminReply::RetentionPolicy { status } => Ok(status),
            _ => Err(NativeClientError::InvalidReply),
        }
    }

    pub fn apply_current_automatic_retention(
        &self,
    ) -> Result<Option<RetentionOutcome>, NativeClientError> {
        match self.admin(AdminRequest::ApplyRetentionPolicy {})? {
            AdminReply::RetentionApplied { outcome } => Ok(outcome),
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
            .read_binding
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
            AdmissionReply::Cursor { .. } | AdmissionReply::PayloadPolicyEnabled => {
                Err(NativeClientError::InvalidReply.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{ObservationIdentity, ObservationKind, TraceSpan, TraceStatus};
    use agenturm_store::TraceTurnQuery;

    #[test]
    fn evaluation_transport_checks_collector_role_and_exact_source_allowlist() {
        let directory =
            std::env::temp_dir().join(format!("agenturm-broker-evaluation-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let mut store = Store::open_collector(directory.join("history.sqlite")).unwrap();
        let allowed_source = ExtensionId::parse("otlp.allowed").unwrap();
        let sibling_source = ExtensionId::parse("otlp.sibling").unwrap();
        let allowed = BTreeSet::from([allowed_source.clone()]);
        let trace_id = "11111111111111111111111111111111";
        let import = |source_id: &ExtensionId| NativeOperation::ImportEvalOutcomes {
            source_id: source_id.clone(),
            outcomes: vec![EvalOutcome {
                trace_id: trace_id.into(),
                passed: false,
                task_id: None,
                provider_model: None,
                scores: Vec::new(),
            }],
        };
        let read = |source_id: &ExtensionId| NativeOperation::EvalOutcome {
            source_id: source_id.clone(),
            trace_id: trace_id.into(),
            expected_history_identity: None,
        };
        for (role, source) in [
            (Role::Reader, &allowed_source),
            (Role::Collector, &sibling_source),
            (Role::Maintenance, &allowed_source),
        ] {
            assert!(matches!(
                dispatch(&mut store, &allowed, role, import(source), None, None),
                NativeReply::EvalImported(Err(ReadFailure::Unauthorized))
            ));
        }
        assert!(matches!(
            dispatch(
                &mut store,
                &allowed,
                Role::Collector,
                import(&allowed_source),
                None,
                None
            ),
            NativeReply::EvalImported(Err(ReadFailure::EvalTraceNotRetained))
        ));
        for (role, source) in [
            (Role::Reader, &sibling_source),
            (Role::GrantAdministrator, &allowed_source),
        ] {
            assert!(matches!(
                dispatch(&mut store, &allowed, role, read(source), None, None),
                NativeReply::EvalOutcome(Err(ReadFailure::Unauthorized))
            ));
        }
        assert!(matches!(
            dispatch(
                &mut store,
                &allowed,
                Role::Reader,
                read(&allowed_source),
                None,
                None
            ),
            NativeReply::EvalOutcome(Ok(None))
        ));
        let span_id = "2222222222222222";
        store
            .commit(
                allowed_source.as_str(),
                None,
                "trace",
                &[Observation {
                    harness_id: allowed_source.clone(),
                    event_id: format!("otlp:{trace_id}:{span_id}"),
                    revision: 0,
                    identity: ObservationIdentity::session("session-1"),
                    observed_at_unix_ns: 2,
                    kind: ObservationKind::TraceSpan(TraceSpan {
                        trace_id: trace_id.into(),
                        span_id: span_id.into(),
                        parent_span_id: None,
                        target_source_id: None,
                        started_at_unix_ns: 1,
                        status: TraceStatus::Ok,
                    }),
                }],
            )
            .unwrap();
        let request = serde_json::to_vec(&import(&allowed_source)).unwrap();
        assert!(matches!(
            dispatch(
                &mut store,
                &allowed,
                Role::Collector,
                serde_json::from_slice(&request).unwrap(),
                None,
                None
            ),
            NativeReply::EvalImported(Ok(EvalImportReport {
                inserted: 1,
                corrected: 0,
                replayed: 0
            }))
        ));
        let reply = dispatch(
            &mut store,
            &allowed,
            Role::Reader,
            read(&allowed_source),
            None,
            None,
        );
        let NativeReply::EvalOutcome(Ok(Some(evaluation))) = reply else {
            panic!("reader should receive retained metadata");
        };
        assert_eq!(evaluation.trace_source_id, allowed_source);
        assert_eq!(evaluation.outcome.trace_id, trace_id);
        assert!(!evaluation.outcome.passed);
        drop(store);
        std::fs::remove_dir_all(&directory).unwrap();
    }

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
        let split_read = |query, binding| NativeOperation::QueryHistory {
            query,
            selection: NativeSelection::Worker {
                turn_id: resolved.turn_id.clone(),
                worker_id: resolved.worker_id.clone(),
            },
            expected_history_identity: binding,
        };
        let current = restored.observation_page(10).unwrap().snapshot_id;
        assert_eq!(current.watermark(), resolved.snapshot_id.watermark());
        assert_ne!(current, resolved.snapshot_id);
        assert!(current.incarnation().is_some());
        for binding in [None, Some(history_identity.clone())] {
            assert!(matches!(
                dispatch(
                    &mut restored,
                    &allowed,
                    Role::Reader,
                    split_read(query.clone(), binding),
                    None,
                    None
                ),
                NativeReply::History(Err(ReadFailure::SnapshotReplaced))
            ));
        }
        let NativeReply::ReadBinding(Ok(current_binding)) = dispatch(
            &mut restored,
            &allowed,
            Role::Reader,
            NativeOperation::ReadBinding {
                history_identity: history_identity.clone(),
            },
            None,
            None,
        ) else {
            panic!("restored branch must allow a fresh read pin");
        };
        assert_eq!(
            logical_read_identity(&current_binding),
            Some(history_identity.as_str())
        );
        assert_ne!(current_binding, history_identity);
        let mut fresh = query.clone();
        fresh.snapshot_id = None;
        assert!(matches!(
            dispatch(
                &mut restored,
                &allowed,
                Role::Reader,
                split_read(fresh.clone(), Some(history_identity.clone())),
                None,
                None
            ),
            NativeReply::History(Err(ReadFailure::SnapshotReplaced))
        ));
        assert!(matches!(
            dispatch(&mut restored, &allowed, Role::Reader,
                split_read(fresh.clone(), Some(current_binding.clone())), None, None),
            NativeReply::History(Ok(page))
                if page.snapshot_id == current
                    && page.observations.iter().any(|row| row.event_id == "rollback-branch")
        ));
        for binding in [history_identity.clone(), current_binding.clone()] {
            let answer = dispatch(
                &mut restored,
                &allowed,
                Role::Reader,
                NativeOperation::ReaderHistory {
                    expected_history_identity: Some(binding.clone()),
                    request: ReaderHistoryRequest::History {
                        query: fresh.clone(),
                        selection: NativeSelection::Observations,
                    },
                },
                None,
                None,
            );
            if binding == current_binding {
                assert!(
                    matches!(answer, NativeReply::ReaderHistory(Ok(ReaderHistoryReply::History(page)))
                    if page.snapshot_id == current)
                );
            } else {
                assert!(matches!(
                    answer,
                    NativeReply::ReaderHistory(Err(ReadFailure::SnapshotReplaced))
                ));
            }
        }
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
                NativeReply::TraceReview(Err(ReadFailure::SnapshotReplaced))
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
        query.snapshot_id = Some(0.into());
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
            NativeReply::History(Ok(page)) if page.snapshot_id == 0.into()
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
