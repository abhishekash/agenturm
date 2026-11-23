//! Attach the existing cockpit and attention workflow to bounded metadata reads.

use super::{Reply, Request, VERSION, http, loopback_address, parse_address_and_token};
use crate::trace_receiver::ProducerToken;
use crate::{AgenturmApp, HistoryReader, cockpit_startup, cockpit_workflows, workspace_response};
use agenturm_kernel::{
    ExtensionId, Observation, ObservationPage, ObservationQuery, ObservationStore, StoreCommit,
};
use agenturm_storage_service::native_read::ReadFailure;
use agenturm_storage_service::{
    NativeSelection, ReaderHistoryReply, ReaderHistoryRequest, logical_read_identity,
};
use agenturm_store::{
    ResolvedTraceReview, SessionPage, SessionQuery, TraceReviewQuery, WorkspaceAnchor,
    WorkspaceHistory, WorkspaceLedger,
};
use agenturm_ui::{WorkCommand, WorkspaceRequest};
use sha2::{Digest, Sha256};
use std::io::BufReader;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Instant;

const ATTACH_USAGE: &str =
    "agenturm cockpit --endpoint http://<loopback-IP:port> --token-file <path>";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteHistoryOptions {
    endpoint: SocketAddr,
    token_file: PathBuf,
}

pub(crate) fn parse(args: &[String]) -> Result<RemoteHistoryOptions, String> {
    let (address, token_file) = parse_address_and_token(args, "--endpoint", ATTACH_USAGE)?;
    let address = address
        .strip_prefix("http://")
        .ok_or("History attach expects a loopback HTTP endpoint")?;
    let endpoint = loopback_address(address.strip_suffix('/').unwrap_or(address), false)?;
    Ok(RemoteHistoryOptions {
        endpoint,
        token_file,
    })
}

pub fn execute_remote_cockpit(options: &RemoteHistoryOptions) -> Result<(), String> {
    let (store, marker) = open_workspace(options)?;
    let mut app = AgenturmApp::default_composition();
    agenturm_ui::run_workspace(move |request| {
        if let WorkspaceRequest::Work(command) = &request {
            if cockpit_workflows::is_prompt_command(command) {
                return Err("This receiver supplies metadata only. Use a local or broker content reader for protected evidence.".into());
            }
            if matches!(command, WorkCommand::Locate(_)) {
                return Err("The native terminal belongs to the receiver host.".into());
            }
        }
        workspace_response(&mut app, &store, &marker, request, || None)
    })
    .map_err(|error| error.to_string())
}

pub(crate) fn open_workspace(
    options: &RemoteHistoryOptions,
) -> Result<(RemoteStore, PathBuf), String> {
    let store = RemoteStore::open(options)?;
    let state = cockpit_startup::state_directory(
        std::env::var_os("XDG_STATE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )?;
    let encoded = serde_json::to_vec(&("agenturm-http-history/1", store.endpoint, &store.identity))
        .map_err(|_| "Cannot scope the receiver workspace")?;
    let namespace = format!("{:x}", Sha256::digest(encoded));
    let marker = crate::broker_cli::private_workspace_marker(
        &state.join("receivers").join(&namespace),
        &format!("agenturm-http-history/1\n{namespace}\n"),
    )?;
    Ok((store, marker))
}

#[derive(Clone)]
pub(crate) struct RemoteStore {
    endpoint: SocketAddr,
    token: ProducerToken,
    identity: String,
    read_binding: String,
}

impl RemoteStore {
    pub(crate) fn open(options: &RemoteHistoryOptions) -> Result<Self, String> {
        let token = ProducerToken::from_file(&options.token_file)?;
        let reply = exchange(
            options.endpoint,
            &token,
            None,
            ReaderHistoryRequest::Identity,
        )?;
        let ReaderHistoryReply::Identity(identity) = reply.result.map_err(read_error)? else {
            return Err("History receiver sent an unexpected identity response".into());
        };
        if logical_read_identity(&reply.history_identity) != Some(identity.as_str()) {
            return Err("History receiver identity changed".into());
        }
        Ok(Self {
            endpoint: options.endpoint,
            token,
            identity,
            read_binding: reply.history_identity,
        })
    }

    fn request(&self, request: ReaderHistoryRequest) -> Result<ReaderHistoryReply, String> {
        let reply = exchange(
            self.endpoint,
            &self.token,
            Some(&self.read_binding),
            request,
        )?;
        reply.result.map_err(read_error)
    }

    fn history(
        &self,
        query: &ObservationQuery,
        selection: NativeSelection,
    ) -> Result<ObservationPage, String> {
        match self.request(ReaderHistoryRequest::History {
            query: query.clone(),
            selection,
        })? {
            ReaderHistoryReply::History(page) => Ok(page),
            _ => Err("History receiver sent an unexpected observation page".into()),
        }
    }

    fn contains(&self, request: ReaderHistoryRequest) -> Result<bool, String> {
        match self.request(request)? {
            ReaderHistoryReply::Contains(value) => Ok(value),
            _ => Err("History receiver sent an unexpected workspace check".into()),
        }
    }
}

fn exchange(
    endpoint: SocketAddr,
    token: &ProducerToken,
    history_identity: Option<&str>,
    request: ReaderHistoryRequest,
) -> Result<Reply, String> {
    let body = serde_json::to_vec(&Request {
        version: VERSION,
        history_identity: history_identity.map(str::to_owned),
        request,
    })
    .map_err(|_| "Cannot encode a history request")?;
    if body.len() > http::MAX_REQUEST {
        return Err("History request is too large".into());
    }
    let deadline = Instant::now() + http::IO_BUDGET;
    let stream = TcpStream::connect_timeout(&endpoint, http::IO_BUDGET)
        .map_err(|_| "Cannot connect to the loopback history receiver")?;
    let head = format!(
        "POST {} HTTP/1.1\r\nHost: {endpoint}\r\nAuthorization: {}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        http::PATH,
        token.bearer_header(),
        body.len(),
    );
    http::write(&stream, deadline, &head, &body).map_err(|_| "Cannot send the history request")?;
    let mut reader = BufReader::new(&stream);
    let head = http::read_head(&mut reader, deadline, http::MAX_REPLY)
        .map_err(|_| "History receiver sent an invalid or slow response")?;
    match head.start.as_str() {
        "HTTP/1.1 200 OK" => {}
        "HTTP/1.1 401 Unauthorized" => {
            return Err("History receiver denied the bearer token".into());
        }
        _ => return Err("History receiver rejected the read".into()),
    }
    let bytes = http::read_body(&mut reader, deadline, head.length)
        .map_err(|_| "History receiver response was incomplete")?;
    let reply: Reply =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid history receiver response")?;
    if reply.version != VERSION || logical_read_identity(&reply.history_identity).is_none() {
        return Err("Incompatible history receiver identity or protocol".into());
    }
    if let Some(binding) = history_identity
        && binding != reply.history_identity
    {
        return Err(read_error(
            if logical_read_identity(binding) == logical_read_identity(&reply.history_identity) {
                ReadFailure::SnapshotReplaced
            } else {
                ReadFailure::HistoryReplaced
            },
        ));
    }
    Ok(reply)
}

fn read_error(error: ReadFailure) -> String {
    match error {
        ReadFailure::Unauthorized => "History scope is outside the receiver grant".into(),
        ReadFailure::InvalidRequest => "History receiver rejected the request parameters".into(),
        ReadFailure::HistoryReplaced => "History receiver identity changed".into(),
        ReadFailure::SnapshotReplaced => {
            "History was restored or replaced. Open a new read.".into()
        }
        ReadFailure::ExpiredSnapshot {
            minimum_snapshot_id,
        } => {
            format!("History snapshot expired. Refresh at snapshot {minimum_snapshot_id} or later.")
        }
        ReadFailure::TraceScopeTooLarge { max_rows } => {
            format!("Trace exceeds the {max_rows}-row turn-resolution limit")
        }
        ReadFailure::Unavailable => "History receiver is unavailable".into(),
        ReadFailure::EvalTraceNotRetained => "Evaluation trace is no longer retained".into(),
        ReadFailure::HostHistoryDisabled => {
            "Recorded Host history is disabled. The receiver can enable it with --include-host."
                .into()
        }
    }
}

impl ObservationStore for RemoteStore {
    fn source_cursor(&self, _: &str) -> Result<Option<String>, String> {
        Err("History receiver attachment is read-only".into())
    }

    fn load_observations(&self, limit: usize) -> Result<ObservationPage, String> {
        self.query_observations(&ObservationQuery::latest(limit))
    }

    fn query_observations(&self, query: &ObservationQuery) -> Result<ObservationPage, String> {
        self.history(query, NativeSelection::Observations)
    }

    fn query_turn_observations(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
    ) -> Result<ObservationPage, String> {
        self.history(
            query,
            NativeSelection::AllWorkers {
                turn_id: turn_id.into(),
            },
        )
    }

    fn query_worker_turn_observations(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
        worker_id: Option<&str>,
    ) -> Result<ObservationPage, String> {
        self.history(
            query,
            NativeSelection::Worker {
                turn_id: turn_id.into(),
                worker_id: worker_id.map(str::to_owned),
            },
        )
    }

    fn commit(
        &mut self,
        _: &str,
        _: Option<&str>,
        _: &str,
        _: &[Observation],
    ) -> Result<StoreCommit, String> {
        Err("History receiver attachment is read-only".into())
    }
}

impl HistoryReader for RemoteStore {
    fn query_selected_evals(
        &self,
        source: &ExtensionId,
        query: &agenturm_store::SelectedEvalQuery,
    ) -> Result<Option<agenturm_store::SelectedEvalPage>, String> {
        match self.request(ReaderHistoryRequest::SelectedEvals {
            source_id: source.clone(),
            query: query.clone(),
        })? {
            ReaderHistoryReply::SelectedEvals(page) => Ok(page),
            _ => Err("History receiver sent an unexpected evaluation page".into()),
        }
    }
    fn allows_private_telemetry(&self) -> bool {
        false
    }

    fn local_host_data(&self) -> bool {
        false
    }

    fn query_host_history(&self, query: &ObservationQuery) -> Result<ObservationPage, String> {
        match self.request(ReaderHistoryRequest::HostHistory {
            query: query.clone(),
        })? {
            ReaderHistoryReply::HostHistory(page) => Ok(page),
            _ => Err("History receiver sent an unexpected Host page".into()),
        }
    }

    fn workspace_ledger(&self, marker: &Path) -> Result<WorkspaceLedger, String> {
        WorkspaceLedger::open_with_history(marker, self.clone()).map_err(|error| error.to_string())
    }

    fn query_sessions(&self, query: &SessionQuery) -> Result<SessionPage, String> {
        match self.request(ReaderHistoryRequest::Sessions {
            query: query.clone(),
        })? {
            ReaderHistoryReply::Sessions(page) => Ok(page),
            _ => Err("History receiver sent an unexpected session page".into()),
        }
    }

    fn query_linked_traces(&self, query: &ObservationQuery) -> Result<ObservationPage, String> {
        self.history(query, NativeSelection::LinkedTraces)
    }

    fn query_linked_turn_traces(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
        worker_id: Option<&str>,
    ) -> Result<ObservationPage, String> {
        self.history(
            query,
            NativeSelection::LinkedTurnTraces {
                turn_id: turn_id.into(),
                worker_id: worker_id.map(str::to_owned),
            },
        )
    }

    fn review_trace_turn(
        &self,
        query: &TraceReviewQuery,
    ) -> Result<Option<ResolvedTraceReview>, String> {
        match self.request(ReaderHistoryRequest::TraceReview {
            query: query.clone(),
        })? {
            ReaderHistoryReply::TraceReview(page) => Ok(page),
            _ => Err("History receiver sent an unexpected trace Review page".into()),
        }
    }
}

impl WorkspaceHistory for RemoteStore {
    fn contains_anchor(&self, anchor: &WorkspaceAnchor) -> Result<bool, String> {
        self.contains(ReaderHistoryRequest::CheckAnchor {
            anchor: anchor.clone(),
        })
    }

    fn contains_event(
        &self,
        anchor: &WorkspaceAnchor,
        event_id: &str,
        revision: u32,
    ) -> Result<bool, String> {
        self.contains(ReaderHistoryRequest::CheckEvent {
            anchor: anchor.clone(),
            event_id: event_id.into(),
            revision,
        })
    }
}
