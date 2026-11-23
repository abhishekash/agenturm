//! Attach the existing cockpit and attention workflow to bounded metadata reads.

use super::{Reply, Request, VERSION, http, loopback_address, parse_address_and_token};
use crate::trace_receiver::ProducerToken;
use crate::{AgenturmApp, HistoryReader, cockpit_startup, cockpit_workflows, workspace_response};
use agenturm_kernel::retained_history::{
    MetricHistoryPage, MetricHistoryQuery, UsageHistoryPage, UsageHistoryQuery,
};
use agenturm_kernel::{
    ExtensionId, Observation, ObservationPage, ObservationQuery, ObservationStore, StoreCommit,
};
use agenturm_storage_service::native_read::ReadFailure;
use agenturm_storage_service::{
    NativeSelection, ReaderHistoryReply, ReaderHistoryRequest, logical_read_identity,
};
use agenturm_store::{
    McpTraceQuery, ResolvedTraceReview, SessionPage, SessionQuery, StandaloneTraceQuery,
    TraceReviewQuery, WorkspaceAnchor, WorkspaceHistory, WorkspaceLedger,
};
use agenturm_ui::{WorkCommand, WorkspaceRequest};
use sha2::{Digest, Sha256};
use std::io::BufReader;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Instant;

const ATTACH_USAGE: &str = "agenturm cockpit --endpoint http://<loopback-IP:port> --token-file <path> [--route-file <path>] [--receiver-protocol python-v0.2.8]";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteHistoryOptions {
    endpoint: SocketAddr,
    token_file: PathBuf,
    route_file: Option<PathBuf>,
    python_receiver: bool,
}

impl RemoteHistoryOptions {
    pub(crate) fn is_python(&self) -> bool {
        self.python_receiver
    }
    pub(crate) fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }
    pub(super) fn token_file(&self) -> &Path {
        &self.token_file
    }
    pub(super) fn route_file(&self) -> Option<&Path> {
        self.route_file.as_deref()
    }
}

pub(crate) fn parse(args: &[String]) -> Result<RemoteHistoryOptions, String> {
    let mut python_receiver = false;
    let mut route_file = None;
    let mut native_args = Vec::with_capacity(args.len());
    let (pairs, extra) = args.as_chunks::<2>();
    for pair in pairs {
        if pair[0] == "--receiver-protocol" {
            if python_receiver || pair[1] != super::legacy::PROTOCOL {
                return Err(ATTACH_USAGE.into());
            }
            python_receiver = true;
        } else if pair[0] == "--route-file" {
            if route_file.is_some() {
                return Err(ATTACH_USAGE.into());
            }
            route_file = Some(PathBuf::from(&pair[1]));
        } else {
            native_args.extend_from_slice(pair);
        }
    }
    if !extra.is_empty() {
        return Err(ATTACH_USAGE.into());
    }
    if python_receiver && route_file.is_some() {
        return Err("Host routes require the native receiver protocol".into());
    }
    let (address, token_file) = parse_address_and_token(&native_args, "--endpoint", ATTACH_USAGE)?;
    let address = address
        .strip_prefix("http://")
        .ok_or("History attach expects a loopback HTTP endpoint")?;
    let endpoint = loopback_address(address.strip_suffix('/').unwrap_or(address), false)?;
    Ok(RemoteHistoryOptions {
        endpoint,
        token_file,
        route_file,
        python_receiver,
    })
}

pub fn execute_remote_cockpit(options: &RemoteHistoryOptions) -> Result<(), String> {
    if options.is_python() {
        if options.route_file.is_some() {
            return Err("Host routes require the native receiver protocol".into());
        }
        return super::legacy::cockpit(options);
    }
    let route = options
        .route_file()
        .map(crate::workflow_route::load_route_file)
        .transpose()?;
    let (store, marker) = open_workspace(options)?;
    let mut app = AgenturmApp::default_composition();
    let ui_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let loader_stop = std::sync::Arc::clone(&ui_stop);
    agenturm_ui::run_workspace_with_stop(move |request| {
        if let WorkspaceRequest::Work(command) = &request {
            if cockpit_workflows::is_prompt_command(command) {
                return Err("This receiver supplies metadata only. Use a local or broker content reader for protected evidence.".into());
            }
            if let WorkCommand::Locate(item) = command {
                if item.owner.worker.is_some() {
                    return Err(
                        "This request belongs to a worker. Its native terminal could not be verified."
                            .into(),
                    );
                }
                let Some(route) = route.as_ref() else {
                    return Err(
                        "The native terminal belongs to the receiver host. Configure an authenticated host route with --route-file to route there."
                            .into(),
                    );
                };
                cockpit_workflows::verify_current_request(&store, item)?;
                let message = crate::workflow_route::remote_locate(
                    route,
                    &item.owner.source,
                    &item.owner.session,
                )?;
                return Ok(agenturm_ui::WorkspaceResponse::Work(
                    agenturm_ui::WorkResult::Status(message),
                ));
            }
        }
        workspace_response(&mut app, &store, &marker, request, || None, &loader_stop)
    }, ui_stop)
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
        if options.is_python() {
            return Err(
                "Python receivers provide live metadata through their explicit protocol".into(),
            );
        }
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

    pub(crate) fn read_binding(&self) -> &str {
        &self.read_binding
    }

    pub(crate) fn query_execution_clocks(
        &self,
        query: &agenturm_store::native_attention::ExecutionClockQuery,
    ) -> Result<agenturm_store::native_attention::ExecutionClockPage, String> {
        match self.request(ReaderHistoryRequest::ExecutionClocks {
            query: query.clone(),
        })? {
            ReaderHistoryReply::ExecutionClocks(page) => Ok(page),
            _ => Err("History gateway returned the wrong execution clock reply".into()),
        }
    }

    pub(crate) fn query_native_attention(
        &self,
        query: &agenturm_store::native_attention::NativeAttentionQuery,
    ) -> Result<agenturm_store::native_attention::NativeAttentionPage, String> {
        match self.request(ReaderHistoryRequest::NativeAttention {
            query: query.clone(),
        })? {
            ReaderHistoryReply::NativeAttention(page) => Ok(page),
            _ => Err("History receiver sent an unexpected native attention page".into()),
        }
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
            format!("Trace exceeds the {max_rows}-row retained scope limit")
        }
        ReadFailure::Unavailable => "History receiver is unavailable".into(),
        ReadFailure::EvalTraceNotRetained => "Evaluation trace is no longer retained".into(),
        ReadFailure::EvalInventoryTooLarge {
            max_trace_keys,
            max_row_bytes,
        } => format!(
            "Evaluation inventory exceeds the {max_trace_keys}-key or {max_row_bytes}-byte limit"
        ),
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
    fn workspace_history_identity(&self) -> Option<String> {
        Some(self.identity.clone())
    }

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
    fn allows_private_capture(&self) -> bool {
        false
    }

    fn local_host_data(&self) -> bool {
        false
    }

    fn query_host_inventory(&self) -> Result<ObservationPage, String> {
        match self.request(ReaderHistoryRequest::HostInventory { snapshot_id: None })? {
            ReaderHistoryReply::HostInventory(page) => Ok(page),
            _ => Err("History receiver sent an unexpected Host inventory".into()),
        }
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

    fn query_execution_clocks(
        &self,
        query: &agenturm_store::native_attention::ExecutionClockQuery,
    ) -> Result<agenturm_store::native_attention::ExecutionClockPage, String> {
        RemoteStore::query_execution_clocks(self, query)
    }
    fn query_native_attention(
        &self,
        query: &agenturm_store::native_attention::NativeAttentionQuery,
    ) -> Result<agenturm_store::native_attention::NativeAttentionPage, String> {
        RemoteStore::query_native_attention(self, query)
    }
    fn query_standalone_trace(
        &self,
        query: &StandaloneTraceQuery,
    ) -> Result<ObservationPage, String> {
        match self.request(ReaderHistoryRequest::StandaloneTrace {
            query: query.clone(),
        })? {
            ReaderHistoryReply::StandaloneTrace(page) => Ok(page),
            _ => Err("History receiver sent an unexpected standalone trace page".into()),
        }
    }

    fn query_projected_history(
        &self,
        query: &ObservationQuery,
        projection: &agenturm_kernel::retained_history::HistoryProjection,
        turn: Option<&str>,
        worker: Option<Option<&str>>,
    ) -> Result<agenturm_kernel::ObservationPage, String> {
        let scope = agenturm_storage_service::ProjectedHistoryScope::from_filters(turn, worker)
            .ok_or("A worker projection requires a turn")?;
        self.history(
            query,
            NativeSelection::Projected {
                projection: projection.clone(),
                scope,
            },
        )
    }

    fn query_mcp_traces(&self, query: &McpTraceQuery) -> Result<Option<ObservationPage>, String> {
        match self.request(ReaderHistoryRequest::McpTraces {
            query: query.clone(),
        })? {
            ReaderHistoryReply::McpTraces(page) => Ok(page),
            _ => Err("History receiver sent an unexpected MCP trace page".into()),
        }
    }

    fn query_metric_history(
        &self,
        query: &MetricHistoryQuery,
    ) -> Result<MetricHistoryPage, String> {
        match self.request(ReaderHistoryRequest::MetricHistory {
            query: query.clone(),
        })? {
            ReaderHistoryReply::MetricHistory(page) => Ok(page),
            _ => Err("History receiver sent an unexpected metric page".into()),
        }
    }

    fn query_usage_history(&self, query: &UsageHistoryQuery) -> Result<UsageHistoryPage, String> {
        match self.request(ReaderHistoryRequest::UsageHistory {
            query: query.clone(),
        })? {
            ReaderHistoryReply::UsageHistory(page) => Ok(*page),
            _ => Err("History receiver sent an unexpected usage history page".into()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_options_parse_the_explicit_route_file() {
        let options = parse(&[
            "--endpoint".into(),
            "http://127.0.0.1:4318".into(),
            "--token-file".into(),
            "receiver.token".into(),
            "--route-file".into(),
            "route.txt".into(),
        ])
        .unwrap();
        assert_eq!(
            options.route_file().map(Path::new),
            Some(Path::new("route.txt"))
        );
        assert!(!options.is_python());
        let plain = parse(&[
            "--endpoint".into(),
            "http://127.0.0.1:4318".into(),
            "--token-file".into(),
            "receiver.token".into(),
        ])
        .unwrap();
        assert!(plain.route_file().is_none());
        // A repeated or dangling route flag is a usage error, never a guess.
        assert!(
            parse(&[
                "--endpoint".into(),
                "http://127.0.0.1:4318".into(),
                "--token-file".into(),
                "receiver.token".into(),
                "--route-file".into(),
                "route.txt".into(),
                "--route-file".into(),
                "route.txt".into(),
            ])
            .is_err()
        );
        assert!(
            parse(&[
                "--endpoint".into(),
                "http://127.0.0.1:4318".into(),
                "--token-file".into(),
                "receiver.token".into(),
                "--route-file".into(),
            ])
            .is_err()
        );
        // The route file is never accepted for the legacy Python protocol.
        assert!(
            parse(&[
                "--endpoint".into(),
                "http://127.0.0.1:4318".into(),
                "--token-file".into(),
                "receiver.token".into(),
                "--route-file".into(),
                "route.txt".into(),
                "--receiver-protocol".into(),
                super::super::legacy::PROTOCOL.into(),
            ])
            .is_err()
        );
    }
}
