//! Metadata-only client of an already-running Agenturm Python 0.2.8 receiver.

mod projection;

use super::{RemoteHistoryOptions, http};
use crate::trace_receiver::ProducerToken;
use projection::{Activity, Connection, Pages, Report, Session, Trace};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;
use std::io::BufReader;
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

const MAX_REPLY: usize = 16 * 1024 * 1024;
const MAX_INVENTORY: usize = 1_000;
const MAX_READ_TIME: Duration = Duration::from_secs(5);
const SESSION_PAGE: usize = 200;

pub(crate) const PROTOCOL: &str = "python-v0.2.8";

#[derive(Clone, Copy)]
enum ReadError {
    Protocol(&'static str),
    Unavailable,
    ResponseBudget,
}

impl From<ReadError> for String {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::Protocol(message) => message,
            ReadError::Unavailable => "Python receiver metadata is temporarily unavailable",
            ReadError::ResponseBudget => "Python receiver response exceeds its bounded read budget",
        }
        .into()
    }
}

fn wire_error(error: std::io::Error) -> ReadError {
    match error.kind() {
        std::io::ErrorKind::InvalidData => {
            ReadError::Protocol("Python receiver sent an invalid HTTP response")
        }
        _ => ReadError::Unavailable,
    }
}

#[derive(Clone)]
pub(crate) struct LegacyReceiver {
    endpoint: SocketAddr,
    token: ProducerToken,
    identity: String,
}

impl LegacyReceiver {
    pub(crate) fn connect(options: &RemoteHistoryOptions) -> Result<Self, String> {
        let token = ProducerToken::from_python_receiver_file(options.token_file())?;
        let deadline = Instant::now() + MAX_READ_TIME;
        let (identity, health) = get::<Health>(
            options.endpoint(),
            &token,
            "/api/health",
            None,
            deadline,
            MAX_REPLY,
        )?;
        health.require_identity(&identity)?;
        Ok(Self {
            endpoint: options.endpoint(),
            token,
            identity,
        })
    }

    fn get<T: DeserializeOwned>(&self, path: &str, deadline: Instant) -> Result<T, ReadError> {
        let (_, value) = get(
            self.endpoint,
            &self.token,
            path,
            Some(&self.identity),
            deadline,
            MAX_REPLY,
        )?;
        Ok(value)
    }

    pub(crate) fn get_for_native<T: DeserializeOwned>(
        &self,
        path: &str,
        deadline: Instant,
    ) -> Result<T, String> {
        self.get(path, deadline).map_err(String::from)
    }

    pub(super) fn get_for_review<T: DeserializeOwned>(
        &self,
        path: &str,
        deadline: Instant,
    ) -> Result<T, String> {
        const REVIEW_REPLY_LIMIT: usize = 2 * 1024 * 1024;
        let (_, value) = get(
            self.endpoint,
            &self.token,
            path,
            Some(&self.identity),
            deadline,
            REVIEW_REPLY_LIMIT,
        )?;
        Ok(value)
    }

    pub(super) fn get_for_investigation<T: DeserializeOwned>(
        &self,
        path: &str,
        deadline: Instant,
        remaining_bytes: usize,
    ) -> Result<Option<(T, usize)>, String> {
        match get_with_size(
            self.endpoint,
            &self.token,
            path,
            Some(&self.identity),
            deadline,
            remaining_bytes,
        ) {
            Ok((_, value, size)) => Ok(Some((value, size))),
            Err(ReadError::ResponseBudget) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn native_identity(&self) -> &str {
        &self.identity
    }

    pub(crate) fn post_native(&self, body: &[u8], deadline: Instant) -> Result<(), String> {
        const UNCERTAIN: &str =
            "Native lifecycle delivery was not confirmed; pending event retained";
        let budget = deadline
            .checked_duration_since(Instant::now())
            .filter(|budget| !budget.is_zero())
            .ok_or(UNCERTAIN)?;
        let endpoint = self.endpoint;
        let stream = TcpStream::connect_timeout(&endpoint, budget).map_err(|_| UNCERTAIN)?;
        let head = format!(
            "POST /v1/logs HTTP/1.1\r\nHost: {endpoint}\r\nAuthorization: {}\r\nX-Agenturm-Expected-Receiver-ID: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.token.bearer_header(),
            self.identity,
            body.len()
        );
        http::write(&stream, deadline, &head, body).map_err(|_| UNCERTAIN)?;
        let mut reader = BufReader::new(&stream);
        let head = http::read_python_head(&mut reader, deadline, 8192).map_err(|_| UNCERTAIN)?;
        let mut status = head.start.split_whitespace();
        if !matches!(status.next(), Some("HTTP/1.0" | "HTTP/1.1"))
            || status.next() != Some("200")
            || head
                .receiver_id
                .is_some_and(|receiver| receiver != self.identity)
        {
            return Err(UNCERTAIN.into());
        }
        let body = http::read_body(&mut reader, deadline, head.length).map_err(|_| UNCERTAIN)?;
        let receipt: NativeWriteReceipt = serde_json::from_slice(&body).map_err(|_| UNCERTAIN)?;
        if receipt
            .receiver_id
            .is_some_and(|receiver| receiver != self.identity)
            || receipt.accepted != 1
            || receipt.partial_success.rejected_log_records != 0
        {
            return Err(UNCERTAIN.into());
        }
        self.native_health_check(deadline)?;
        Ok(())
    }

    pub(crate) fn native_health_check(&self, deadline: Instant) -> Result<(), String> {
        let health = self.health(deadline)?;
        if !health.ready() || !health.native_lifecycle_v1 {
            return Err("Python receiver native lifecycle is unavailable or indexing".into());
        }
        Ok(())
    }

    fn health(&self, deadline: Instant) -> Result<Health, ReadError> {
        let health: Health = self.get("/api/health", deadline)?;
        health.require_identity(&self.identity)?;
        Ok(health)
    }

    fn report(&self, limit: usize, attention: &mut AttentionState) -> Result<Report, String> {
        let deadline = Instant::now() + MAX_READ_TIME;
        let initial = self.health(deadline)?;
        let mut report = Report::new(self.identity.clone());
        if !initial.ready() {
            return Ok(report);
        }
        let sessions: Sessions = self.get(&format!("/api/sessions?limit={limit}"), deadline)?;
        let connections: Connections = self.get("/api/connections", deadline)?;
        let activity: Activities = self.get("/api/activity?limit=200", deadline)?;
        let traces: Traces = self.get("/api/traces?limit=100", deadline)?;
        let inventory = self.inventory(&initial, deadline)?;
        let final_health = self.health(deadline)?;
        if !final_health.ready()
            || initial.local_agents.capture_content != final_health.local_agents.capture_content
        {
            return Ok(report);
        }
        let requests = attention.observe(&self.identity, &initial, &final_health, inventory);
        report.prepare(
            limit,
            Pages {
                local_sessions: initial.local_agents.sessions,
                sessions,
                connections,
                activity,
                traces,
            },
            requests,
        )?;
        Ok(report)
    }

    fn inventory(
        &self,
        health: &Health,
        deadline: Instant,
    ) -> Result<Option<Inventory>, ReadError> {
        if !health.ready() || !health.native_lifecycle_v1 {
            return Ok(None);
        }
        match self.get("/api/native-lifecycle", deadline) {
            Ok(inventory) => Ok(Some(inventory)),
            Err(ReadError::Unavailable) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn pending(&self, attention: &mut AttentionState) -> Result<AttentionReport, ReadError> {
        self.pending_with_clock(attention).map(|(report, _)| report)
    }

    fn pending_with_clock(
        &self,
        attention: &mut AttentionState,
    ) -> Result<(AttentionReport, Option<u64>), ReadError> {
        let deadline = Instant::now() + MAX_READ_TIME;
        let initial = self.health(deadline)?;
        let inventory = self.inventory(&initial, deadline)?;
        let final_health = self.health(deadline)?;
        let report = attention.observe(&self.identity, &initial, &final_health, inventory);
        let clock = initial
            .observed_time_unix_nano
            .zip(final_health.observed_time_unix_nano)
            .filter(|(initial, final_clock)| report.ready && initial <= final_clock)
            .map(|(_, final_clock)| final_clock);
        Ok((report, clock))
    }
}

pub(super) fn cockpit(options: &RemoteHistoryOptions) -> Result<(), String> {
    let options = options.clone();
    let mut receiver = None;
    let mut attention = AttentionState::default();
    agenturm_ui::run_live_receiver(move || {
        let receiver = match &receiver {
            Some(receiver) => receiver,
            None => receiver.insert(LegacyReceiver::connect(&options)?),
        };
        receiver
            .report(SESSION_PAGE, &mut attention)
            .map(Report::into_live_page)
    })
    .map_err(|_| "Cannot run the Python receiver cockpit".into())
}

pub(crate) fn summary(
    options: &RemoteHistoryOptions,
    limit: usize,
    json: bool,
) -> Result<String, String> {
    let receiver = LegacyReceiver::connect(options)?;
    let mut report = receiver.report(limit, &mut AttentionState::default())?;
    report.hide_internal_reviews();
    if json {
        serde_json::to_string_pretty(&report)
            .map(|json| json + "\n")
            .map_err(|_| "Cannot encode Python receiver live metadata".into())
    } else {
        Ok(report.text())
    }
}

pub(crate) fn attention_reader(
    options: &RemoteHistoryOptions,
) -> Result<impl FnMut() -> Result<AttentionReport, String>, String> {
    let receiver = LegacyReceiver::connect(options)?;
    let mut attention = AttentionState::default();
    Ok(move || match receiver.pending(&mut attention) {
        Ok(report) => Ok(report),
        Err(ReadError::Unavailable) => Ok(attention.report(&receiver.identity, false)),
        Err(error) => Err(error.into()),
    })
}

pub(crate) struct DesktopAttentionReader {
    receiver: LegacyReceiver,
    attention: AttentionState,
}

pub(crate) struct DesktopAttentionSample {
    pub report: AttentionReport,
    pub clock: Option<u64>,
}

impl DesktopAttentionReader {
    pub(crate) fn connect(options: &RemoteHistoryOptions) -> Result<Self, String> {
        Ok(Self {
            receiver: LegacyReceiver::connect(options)?,
            attention: AttentionState::default(),
        })
    }

    pub(crate) fn identity(&self) -> &str {
        &self.receiver.identity
    }

    pub(crate) fn read(&mut self) -> Result<DesktopAttentionSample, String> {
        match self.receiver.pending_with_clock(&mut self.attention) {
            Ok((report, clock)) => Ok(DesktopAttentionSample { report, clock }),
            Err(ReadError::Unavailable) => Ok(DesktopAttentionSample {
                report: self.attention.report(&self.receiver.identity, false),
                clock: None,
            }),
            Err(error) => Err(error.into()),
        }
    }
}

fn get<T: DeserializeOwned>(
    endpoint: SocketAddr,
    token: &ProducerToken,
    path: &str,
    expected: Option<&str>,
    deadline: Instant,
    max_reply: usize,
) -> Result<(String, T), ReadError> {
    get_with_size(endpoint, token, path, expected, deadline, max_reply)
        .map(|(identity, value, _)| (identity, value))
}

fn get_with_size<T: DeserializeOwned>(
    endpoint: SocketAddr,
    token: &ProducerToken,
    path: &str,
    expected: Option<&str>,
    deadline: Instant,
    max_reply: usize,
) -> Result<(String, T, usize), ReadError> {
    let budget = deadline
        .checked_duration_since(Instant::now())
        .filter(|budget| !budget.is_zero())
        .ok_or(ReadError::Unavailable)?;
    let stream =
        TcpStream::connect_timeout(&endpoint, budget).map_err(|_| ReadError::Unavailable)?;
    let head = format!(
        "GET {path} HTTP/1.1\r\nHost: {endpoint}\r\nAuthorization: {}\r\nConnection: close\r\n\r\n",
        token.bearer_header(),
    );
    http::write(&stream, deadline, &head, &[]).map_err(wire_error)?;
    let mut reader = BufReader::new(&stream);
    let head = http::read_python_head(&mut reader, deadline, MAX_REPLY).map_err(wire_error)?;
    let status = head.start.split_whitespace().collect::<Vec<_>>();
    match status.as_slice() {
        ["HTTP/1.0" | "HTTP/1.1", "200", ..] => {}
        ["HTTP/1.0" | "HTTP/1.1", "401", ..] => {
            return Err(ReadError::Protocol(
                "Python receiver denied the bearer token",
            ));
        }
        ["HTTP/1.0" | "HTTP/1.1", "409", ..] => {
            return Err(ReadError::Protocol(
                "Python receiver identity changed during the read",
            ));
        }
        [
            "HTTP/1.0" | "HTTP/1.1",
            "429" | "500" | "502" | "503" | "504",
            ..,
        ] => return Err(ReadError::Unavailable),
        ["HTTP/1.0" | "HTTP/1.1", "404", ..] if path == "/api/native-lifecycle" => {
            return Err(ReadError::Unavailable);
        }
        _ => {
            return Err(ReadError::Protocol(
                "Python receiver refused the metadata read",
            ));
        }
    }
    let identity = head
        .receiver_id
        .filter(|id| hex_id(id, 32))
        .ok_or(ReadError::Protocol(
            "Python receiver identity header is missing or invalid",
        ))?;
    if expected.is_some_and(|expected| expected != identity) {
        return Err(ReadError::Protocol(
            "Python receiver identity changed. Reopen the cockpit or watcher.",
        ));
    }
    if head.length > max_reply {
        return Err(ReadError::ResponseBudget);
    }
    let body = http::read_body(&mut reader, deadline, head.length).map_err(wire_error)?;
    let declared: BodyIdentity = serde_json::from_slice(&body)
        .map_err(|_| ReadError::Protocol("Python receiver metadata is invalid"))?;
    if declared
        .receiver_id
        .is_some_and(|claimed| claimed != identity)
    {
        return Err(ReadError::Protocol(
            "Python receiver body and header identities differ",
        ));
    }
    let value = serde_json::from_slice(&body)
        .map_err(|_| ReadError::Protocol("Python receiver metadata is invalid"))?;
    Ok((identity, value, body.len()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeWriteReceipt {
    accepted: u64,
    #[serde(rename = "receiver_id")]
    receiver_id: Option<String>,
    partial_success: NativePartialSuccess,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativePartialSuccess {
    rejected_log_records: u64,
}

#[derive(Deserialize)]
struct BodyIdentity {
    #[serde(default, deserialize_with = "declared_identity")]
    receiver_id: Option<String>,
}

fn declared_identity<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
struct Health {
    ok: bool,
    receiver_id: String,
    indexing: Indexing,
    security: Security,
    observed_time_unix_nano: Option<u64>,
    #[serde(default)]
    native_lifecycle_v1: bool,
    #[serde(default)]
    local_agents: LocalAgents,
}

impl Health {
    fn require_identity(&self, identity: &str) -> Result<(), ReadError> {
        if !self.ok || self.receiver_id != identity {
            return Err(ReadError::Protocol(
                "Endpoint did not return matching Python receiver health",
            ));
        }
        if !self.security.authentication_required || !self.security.host_validation {
            return Err(ReadError::Protocol(
                "Python receiver must require bearer authentication and validate its host",
            ));
        }
        Ok(())
    }
    fn ready(&self) -> bool {
        self.indexing.ready
    }
    fn attention_ready(&self) -> bool {
        let local = &self.local_agents;
        self.ready()
            && self.native_lifecycle_v1
            && self.observed_time_unix_nano.is_some_and(time_ns)
            && (!local.enabled
                || (local.initial_poll_complete
                    && local.backlog_bytes == 0
                    && local.omitted_sources == 0
                    && local
                        .sessions
                        .iter()
                        .all(|session| session.diagnostics.ready())))
    }
}

#[derive(Deserialize)]
struct Security {
    authentication_required: bool,
    host_validation: bool,
}

#[derive(Deserialize)]
struct Indexing {
    ready: bool,
}

#[derive(Default, Deserialize)]
struct LocalAgents {
    #[serde(default)]
    enabled: bool,
    capture_content: Option<bool>,
    #[serde(default)]
    initial_poll_complete: bool,
    #[serde(default)]
    backlog_bytes: u64,
    #[serde(default)]
    omitted_sources: u64,
    #[serde(default, deserialize_with = "capped")]
    sessions: Vec<Session>,
}

#[derive(Deserialize)]
struct Sessions {
    #[serde(deserialize_with = "capped")]
    sessions: Vec<Session>,
}
#[derive(Deserialize)]
struct Connections {
    #[serde(deserialize_with = "capped")]
    connections: Vec<Connection>,
}
#[derive(Deserialize)]
struct Activities {
    #[serde(deserialize_with = "capped")]
    activity: Vec<Activity>,
    next_cursor: Option<String>,
}
#[derive(Deserialize)]
struct Traces {
    #[serde(deserialize_with = "capped")]
    traces: Vec<Trace>,
    next_cursor: Option<String>,
    has_more: Option<bool>,
}

#[derive(Deserialize)]
struct Inventory {
    receiver_id: String,
    complete: bool,
    #[serde(deserialize_with = "capped")]
    observations: Vec<RequestState>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct RequestState {
    schema: u32,
    receiver_id: String,
    sender_id: String,
    event_id: String,
    source_id: String,
    service: String,
    agent_id: String,
    session_id: String,
    trace_id: String,
    turn_id: String,
    request_id: String,
    sequence: u64,
    kind: RequestKind,
    request_kind: RequestKind,
    source_clock: SourceClock,
    source_order: String,
    time_unix_nano: u64,
    resolved: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RequestKind {
    ApprovalRequested,
    ReplyRequested,
    RequestResolved,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SourceClock {
    Receiver,
    Unknown,
}

impl RequestState {
    fn key(&self) -> (&str, &str) {
        (&self.sender_id, &self.request_id)
    }
    fn scope(&self) -> (&str, &str, &str, &str, &str, &str) {
        (
            &self.source_id,
            &self.service,
            &self.agent_id,
            &self.session_id,
            &self.trace_id,
            &self.turn_id,
        )
    }

    fn valid(&self, receiver: &str) -> bool {
        self.schema == 1
            && self.receiver_id == receiver
            && self.valid_identities()
            && self.sequence > 0
            && self.sequence <= i64::MAX as u64
            && self.time_unix_nano <= i64::MAX as u64
            && self.source_order == "sender-sequence"
            && self.resolved == (self.kind == RequestKind::RequestResolved)
            && self.request_kind != RequestKind::RequestResolved
            && (self.resolved || self.kind == self.request_kind)
    }

    fn valid_identities(&self) -> bool {
        identifier(&self.sender_id)
            && identifier(&self.event_id)
            && identifier(&self.source_id)
            && label(&self.service, 128)
            && agent_id(&self.agent_id)
            && label(&self.session_id, 512)
            && hex_id(&self.trace_id, 32)
            && (self.turn_id.is_empty() || hex_id(&self.turn_id, 32))
            && identifier(&self.request_id)
    }
}

#[derive(Default)]
struct AttentionState {
    states: BTreeMap<String, BTreeMap<String, RequestState>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AttentionReport {
    format: &'static str,
    receiver_id: String,
    pub(crate) ready: bool,
    partial: bool,
    pending: Vec<PendingRequest>,
}

impl AttentionReport {
    /// Only a complete report confirms that all unresolved rows were present
    /// in the current authenticated inventory.
    pub(crate) fn desktop_requests(&self) -> Option<&[PendingRequest]> {
        self.ready.then_some(&self.pending)
    }
}

/// Only derived public identity and explicitly reported request state enter the output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PendingRequest {
    source_id: String,
    service: String,
    agent_id: String,
    session_id: String,
    trace_id: String,
    turn_id: Option<String>,
    sender_id: String,
    request_id: String,
    event_id: String,
    sequence: u64,
    reason_code: RequestKind,
    observed_at_unix_ns: String,
    source_clock: SourceClock,
}

impl PendingRequest {
    pub(crate) fn receiver_clock(&self) -> Option<u64> {
        let time = self.observed_at_unix_ns.parse().ok()?;
        (self.source_clock == SourceClock::Receiver && time_ns(time)).then_some(time)
    }

    pub(crate) fn desktop_key(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};

        let mut key = Sha256::new();
        for field in [
            self.source_id.as_str(),
            self.service.as_str(),
            self.agent_id.as_str(),
            self.session_id.as_str(),
            self.trace_id.as_str(),
            self.turn_id.as_deref().unwrap_or_default(),
            self.sender_id.as_str(),
            self.request_id.as_str(),
            self.event_id.as_str(),
        ] {
            key.update((field.len() as u64).to_be_bytes());
            key.update(field.as_bytes());
        }
        key.update(self.sequence.to_be_bytes());
        key.finalize().into()
    }
}

impl From<&RequestState> for PendingRequest {
    fn from(row: &RequestState) -> Self {
        Self {
            source_id: row.source_id.clone(),
            service: row.service.clone(),
            agent_id: row.agent_id.clone(),
            session_id: row.session_id.clone(),
            trace_id: row.trace_id.clone(),
            turn_id: (!row.turn_id.is_empty()).then(|| row.turn_id.clone()),
            sender_id: row.sender_id.clone(),
            request_id: row.request_id.clone(),
            event_id: row.event_id.clone(),
            sequence: row.sequence,
            reason_code: row.request_kind,
            observed_at_unix_ns: row.time_unix_nano.to_string(),
            source_clock: row.source_clock,
        }
    }
}

impl AttentionState {
    fn observe(
        &mut self,
        receiver: &str,
        initial: &Health,
        final_health: &Health,
        inventory: Option<Inventory>,
    ) -> AttentionReport {
        let ready = initial.attention_ready()
            && final_health.attention_ready()
            && initial.local_agents.capture_content == final_health.local_agents.capture_content
            && inventory.is_some_and(|inventory| self.merge(receiver, inventory));
        self.report(receiver, ready)
    }

    fn report(&self, receiver: &str, ready: bool) -> AttentionReport {
        let mut pending: Vec<_> = self
            .states
            .values()
            .flat_map(BTreeMap::values)
            .filter(|row| !row.resolved)
            .map(PendingRequest::from)
            .collect();
        pending.sort_by(|a, b| {
            (&a.service, &a.agent_id, &a.sender_id, &a.request_id).cmp(&(
                &b.service,
                &b.agent_id,
                &b.sender_id,
                &b.request_id,
            ))
        });
        AttentionReport {
            format: "agenturm-python-receiver-attention/1",
            receiver_id: receiver.into(),
            ready,
            partial: !ready,
            pending,
        }
    }

    fn merge(&mut self, receiver: &str, inventory: Inventory) -> bool {
        if !inventory.complete || inventory.receiver_id != receiver {
            return false;
        }
        let mut by_sender = BTreeMap::new();
        let mut sequences = BTreeMap::new();
        let mut current = BTreeMap::new();
        let previous_rows = || self.states.values().flat_map(BTreeMap::values);
        // A receiver can prune an occurrence while this reader still retains
        // evidence of the sequence already used by that sender.
        for row in previous_rows().chain(&inventory.observations) {
            let sender = row.sender_id.as_str();
            if !row.valid(receiver)
                || by_sender
                    .insert(sender, row.scope())
                    .is_some_and(|scope| scope != row.scope())
                || sequences
                    .insert((sender, row.sequence), row)
                    .is_some_and(|previous| previous != row)
            {
                return false;
            }
        }
        for row in &inventory.observations {
            if current.insert(row.key(), row).is_some() {
                return false;
            }
        }
        let new_count = current
            .keys()
            .filter(|(sender, request)| {
                !self
                    .states
                    .get(*sender)
                    .is_some_and(|requests| requests.contains_key(*request))
            })
            .count();
        if previous_rows().count() + new_count > MAX_INVENTORY {
            return false;
        }
        let mut all_pending_confirmed = true;
        for previous in previous_rows() {
            if let Some(row) = current.get(&previous.key()) {
                if row.request_kind != previous.request_kind
                    || row.sequence < previous.sequence
                    || (row.sequence == previous.sequence && *row != previous)
                    || (previous.resolved && !row.resolved)
                {
                    return false;
                }
            } else if !previous.resolved {
                // A missing row can be a retention or partial inventory effect.
                all_pending_confirmed = false;
            }
        }
        for row in inventory.observations {
            self.states
                .entry(row.sender_id.clone())
                .or_default()
                .insert(row.request_id.clone(), row);
        }
        all_pending_confirmed
    }
}

fn capped<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Vec<T>, D::Error> {
    struct Capped<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Capped<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "an array with at most {MAX_INVENTORY} receiver rows")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut rows =
                Vec::with_capacity(seq.size_hint().unwrap_or_default().min(MAX_INVENTORY));
            while rows.len() < MAX_INVENTORY {
                let Some(row) = seq.next_element()? else {
                    return Ok(rows);
                };
                rows.push(row);
            }
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::invalid_length(MAX_INVENTORY + 1, &self));
            }
            Ok(rows)
        }
    }
    deserializer.deserialize_seq(Capped(std::marker::PhantomData))
}

fn time_ns(value: u64) -> bool {
    value > 0 && value <= i64::MAX as u64
}

fn hex_id(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && value.bytes().any(|b| b != b'0')
}

fn agent_id(value: &str) -> bool {
    value.strip_prefix("ag1_").is_some_and(|id| hex_id(id, 32))
}

fn label(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-' | b'/' | b'@' | b'+')
        })
        && agenturm_store::bounded_text_preview(value.to_owned(), limit)
            .is_ok_and(|safe| safe == value)
}

fn identifier(value: &str) -> bool {
    label(value, 512)
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECEIVER: &str = "11111111111111111111111111111111";

    fn fixture(sequence: u64, resolved: bool) -> RequestState {
        RequestState {
            schema: 1,
            receiver_id: RECEIVER.into(),
            sender_id: "sender".into(),
            event_id: format!("event-{sequence}"),
            source_id: "source".into(),
            service: "codex".into(),
            agent_id: format!("ag1_{}", "2".repeat(32)),
            session_id: "session".into(),
            trace_id: "3".repeat(32),
            turn_id: "4".repeat(32),
            request_id: "request".into(),
            sequence,
            kind: if resolved {
                RequestKind::RequestResolved
            } else {
                RequestKind::ApprovalRequested
            },
            request_kind: RequestKind::ApprovalRequested,
            source_clock: SourceClock::Receiver,
            source_order: "sender-sequence".into(),
            time_unix_nano: 9,
            resolved,
        }
    }

    fn inventory(complete: bool, observations: Vec<RequestState>) -> Inventory {
        Inventory {
            receiver_id: RECEIVER.into(),
            complete,
            observations,
        }
    }

    #[test]
    fn only_an_ordered_exact_resolution_advances_lifecycle_state() {
        let mut state = AttentionState::default();
        assert!(state.merge(RECEIVER, inventory(true, vec![fixture(1, false)])));
        assert!(!state.merge(RECEIVER, inventory(true, vec![])));
        assert!(!state.merge(RECEIVER, inventory(false, vec![fixture(2, true)])));
        let mut foreign = fixture(2, true);
        foreign.trace_id = "5".repeat(32);
        assert!(!state.merge(RECEIVER, inventory(true, vec![foreign])));
        assert!(!state.merge(RECEIVER, inventory(true, vec![fixture(1, true)])));
        assert!(!state.states["sender"]["request"].resolved);
        assert!(state.merge(RECEIVER, inventory(true, vec![fixture(2, true)])));
        assert!(!state.merge(RECEIVER, inventory(true, vec![fixture(3, false)])));
        assert!(state.states["sender"]["request"].resolved);
    }

    #[test]
    fn rejects_resolution_reusing_a_retired_request_sequence() {
        let mut state = AttentionState::default();
        let pending = fixture(1, false);
        let mut other = fixture(2, true);
        other.request_id = "other-request".into();
        let original = vec![pending, other];
        assert!(state.merge(RECEIVER, inventory(true, original.clone())));
        assert!(state.merge(RECEIVER, inventory(true, original)));
        let before = state.report(RECEIVER, true);
        assert_eq!(before.pending.len(), 1);

        // The receiver stopped retaining the other request, but this reader
        // still knows its sender had already used sequence 2.
        let mut conflict = fixture(2, true);
        conflict.event_id = "new-event-with-reused-sequence".into();
        let mut unrelated = fixture(3, false);
        unrelated.request_id = "unrelated".into();
        assert!(!state.merge(RECEIVER, inventory(true, vec![unrelated, conflict])));
        assert_eq!(state.report(RECEIVER, true), before);

        assert!(state.merge(RECEIVER, inventory(true, vec![fixture(3, true)])));
        assert!(state.report(RECEIVER, true).pending.is_empty());
    }

    #[test]
    fn an_explicit_protocol_choice_is_required_and_never_accepts_a_direct_remote_url() {
        let parse = |args: &[&str]| {
            super::super::parse_attach(
                &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
            )
        };
        let mut flags = vec![
            "--endpoint",
            "http://127.0.0.1:14318",
            "--token-file",
            "token",
        ];
        assert!(!parse(&flags).unwrap().is_python());
        flags.extend(["--receiver-protocol", PROTOCOL]);
        assert!(parse(&flags).unwrap().is_python());
        flags[1] = "https://example.invalid:14318";
        assert!(parse(&flags).is_err());
        flags[1] = "http://127.0.0.1:14318";
        flags.extend(["--receiver-protocol", PROTOCOL]);
        assert!(parse(&flags).is_err());
    }
}
