//! Connection-scoped Rust client and typed dispatch for native history.

use crate::native_admission::{self, AdmissionFailure, AdmissionReply, AdmissionRequest};
use crate::native_read::{self, HistorySelection, ReadFailure};
use agenturm_kernel::{
    ExtensionId, Observation, ObservationPage, ObservationQuery, ObservationStore, StoreCommit,
};
use agenturm_storage_service_contract::{ContractError, ServerPolicy, native};
use agenturm_store::{ObservationBatch, SessionPage, SessionQuery, Store};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;
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
    },
    QuerySessions {
        query: SessionQuery,
    },
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
}

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum NativeReply {
    Admission(Result<AdmissionReply, AdmissionFailure>),
    History(Result<ObservationPage, ReadFailure>),
    Sessions(Result<SessionPage, ReadFailure>),
}

pub(crate) fn dispatch(
    store: &mut Store,
    allowed: &BTreeSet<ExtensionId>,
    operation: NativeOperation,
) -> NativeReply {
    let admission = match operation {
        NativeOperation::SourceCursor { source_id } => AdmissionRequest::SourceCursor { source_id },
        NativeOperation::Admit { batch } => AdmissionRequest::Admit { batch },
        NativeOperation::QueryHistory { query, selection } => {
            let selection = match &selection {
                NativeSelection::Observations => HistorySelection::Observations,
                NativeSelection::AllWorkers { turn_id } => HistorySelection::AllWorkers { turn_id },
                NativeSelection::Worker { turn_id, worker_id } => HistorySelection::Turn {
                    turn_id,
                    worker_id: worker_id.as_deref(),
                },
                NativeSelection::LinkedTraces => HistorySelection::LinkedTraces,
            };
            return NativeReply::History(native_read::query_history(
                store, allowed, &query, selection,
            ));
        }
        NativeOperation::QuerySessions { query } => {
            return NativeReply::Sessions(native_read::query_sessions(store, allowed, &query));
        }
    };
    NativeReply::Admission(native_admission::dispatch(store, allowed, admission))
}

#[derive(Debug)]
pub enum NativeClientError {
    Transport(ContractError),
    Admission(AdmissionFailure),
    Read(ReadFailure),
    InvalidReply,
    InvalidPath,
    InvalidBatch(String),
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
            Self::Read(ReadFailure::ExpiredSnapshot {
                minimum_snapshot_id,
            }) => {
                write!(
                    formatter,
                    "history snapshot expired (minimum {minimum_snapshot_id})"
                )
            }
            Self::Admission(AdmissionFailure::Unavailable)
            | Self::Read(ReadFailure::Unavailable) => {
                formatter.write_str("broker history is unavailable")
            }
            Self::InvalidReply => formatter.write_str("unexpected broker reply"),
            Self::InvalidPath => formatter.write_str("broker socket path must be absolute"),
            Self::InvalidBatch(error) => write!(formatter, "{error}"),
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
pub struct NativeStore {
    socket_path: PathBuf,
    receiver_id: String,
    policy: ServerPolicy,
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
        })
    }

    fn exchange<R: DeserializeOwned>(
        &self,
        operation: NativeOperation,
    ) -> Result<R, NativeClientError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let request = native::Request {
            version: native::VERSION,
            request_id: format!(
                "{:016x}{:016x}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
            receiver_id: self.receiver_id.clone(),
            operation,
        };
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
        })?
        .map_err(NativeClientError::Read)
    }

    pub fn query_sessions(&self, query: &SessionQuery) -> Result<SessionPage, NativeClientError> {
        self.exchange::<Result<SessionPage, ReadFailure>>(NativeOperation::QuerySessions {
            query: query.clone(),
        })?
        .map_err(NativeClientError::Read)
    }

    pub fn query_linked_traces(
        &self,
        query: &ObservationQuery,
    ) -> Result<ObservationPage, NativeClientError> {
        self.query(query, NativeSelection::LinkedTraces)
    }

    fn admission(&self, operation: NativeOperation) -> Result<AdmissionReply, NativeClientError> {
        self.exchange::<Result<AdmissionReply, AdmissionFailure>>(operation)?
            .map_err(NativeClientError::Admission)
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
