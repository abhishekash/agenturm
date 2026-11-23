//! Exact, authenticated lifecycle delivery to the explicitly configured receiver.

use super::model::Binding;
use crate::remote_history::{
    legacy::{LegacyReceiver, PROTOCOL},
    parse_attach,
};
use agenturm_store::native_outbox::{EventKind, QueuedEvent};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    time::{Duration, Instant},
};

const PROOF_BUDGET: Duration = Duration::from_secs(5);

pub(super) fn connect(binding: &Binding, token_file: &Path) -> Result<LegacyReceiver, String> {
    binding.validate()?;
    let token_file = token_file
        .to_str()
        .ok_or("Invalid native token file path")?;
    let options = parse_attach(&[
        "--endpoint".into(),
        binding.endpoint.clone(),
        "--token-file".into(),
        token_file.into(),
        "--receiver-protocol".into(),
        PROTOCOL.into(),
    ])?;
    let receiver = LegacyReceiver::connect(&options)?;
    if receiver.native_identity() != binding.receiver_id {
        return Err("Native receiver identity changed".into());
    }
    Ok(receiver)
}

pub(super) fn verify(
    receiver: &LegacyReceiver,
    binding: &Binding,
    trace_id: &str,
    turn_id: &str,
) -> Result<(), String> {
    let deadline = Instant::now() + PROOF_BUDGET;
    let health: NativeHealth = receiver.get_for_native("/api/health", deadline)?;
    if health.receiver_id != binding.receiver_id
        || !health.indexing.ready
        || !health.native_lifecycle_v1
    {
        return Err("Native receiver identity or readiness could not be confirmed".into());
    }
    // The binding and event identity fields allow no URL query delimiters.
    let mut route = format!(
        "/api/activity?limit=1&subtree=0&service={}&agent_id={}&session_id={}&trace_id={trace_id}",
        binding.service, binding.agent_id, binding.session_id,
    );
    if !turn_id.is_empty() {
        route.push_str("&turn_id=");
        route.push_str(turn_id);
    }
    let activity: Activities = receiver.get_for_native(&route, deadline)?;
    if activity.receiver_id != binding.receiver_id
        || !activity.activity.iter().any(|row| {
            row.scope.matches(binding)
                && row.trace_id.as_deref() == Some(trace_id)
                && (turn_id.is_empty() || row.turn_id.as_deref() == Some(turn_id))
        })
    {
        return Err("Native actor, session, trace or turn is absent from recorded activity".into());
    }
    let hierarchy: Hierarchy =
        receiver.get_for_native("/api/hierarchy?limit=10000&include_reviews=1", deadline)?;
    let mut actor = hierarchy
        .nodes
        .iter()
        .filter(|node| node.agent_id == binding.agent_id);
    let node = actor.next();
    if actor.next().is_some() {
        return Err("Native actor identity is ambiguous in recorded hierarchy".into());
    }
    let native = health.local_agents.sessions.iter().any(|row| {
        row.scope.matches(binding) && row.source_id.as_deref() == Some(binding.source_id.as_str())
    });
    if (binding.follow_recorded_turns
        || node.and_then(|node| node.source_id.as_deref()) != Some(&binding.source_id))
        && !native
    {
        return Err("Native provider source is absent from the recorded actor binding".into());
    }
    if !binding.parent_agent_id.is_empty()
        && !node.is_some_and(|node| {
            node.parent_agent_id.as_deref() == Some(&binding.parent_agent_id)
                && node.relationship_state.as_deref() == Some("confirmed")
        })
    {
        return Err("Native parent relationship is unconfirmed".into());
    }
    receiver.native_health_check(deadline)
}

pub(super) fn send(
    receiver: &LegacyReceiver,
    binding: &Binding,
    queued: &QueuedEvent,
) -> Result<(), String> {
    let event = &queued.event;
    verify(receiver, binding, &event.trace_id, &event.turn_id)?;
    let body = log_body(binding, queued)?;
    receiver.post_native(&body, Instant::now() + PROOF_BUDGET)
}

fn log_body(binding: &Binding, queued: &QueuedEvent) -> Result<Vec<u8>, String> {
    let event = &queued.event;
    let action = match event.kind {
        EventKind::ApprovalRequested => "approval_request",
        EventKind::ReplyRequested => "input_request",
        EventKind::RequestResolved => "request_resolved",
        EventKind::TurnStarted => "started",
        EventKind::TurnCompleted => "completed",
        EventKind::TurnInterrupted => "interrupted",
    };
    let stamp = match event.source_time_unix_ns {
        0 => queued.received_at_unix_ns,
        source => source,
    };
    // v1 fingerprints ASCII identifiers using the exact JSON tuple bytes.
    let pair = serde_json::to_vec(&(&binding.sender_id, &event.event_id))
        .map_err(|_| "Cannot encode native event identity")?;
    let record_id = format!("{:x}", Sha256::digest(pair));
    let strings = [
        ("agent.id", binding.agent_id.as_str()),
        ("session.id", binding.session_id.as_str()),
        ("agent.turn.id", event.turn_id.as_str()),
        ("agent.activity.domain", "agent"),
        ("agent.activity.action", action),
        ("agent.activity.subject", "Native lifecycle"),
        ("agent.activity.result", "observed"),
        ("observatory.record_id", record_id.as_str()),
        (
            "observatory.native.receiver_id",
            binding.receiver_id.as_str(),
        ),
        ("observatory.native.sender_id", binding.sender_id.as_str()),
        ("observatory.native.event_id", event.event_id.as_str()),
        ("observatory.native.kind", event.kind.as_str()),
        (
            "observatory.native.request_id",
            event.request_id.as_deref().unwrap_or(""),
        ),
        (
            "observatory.native.request_kind",
            queued.request_kind.map_or("", EventKind::as_str),
        ),
        (
            "observatory.native.source_clock",
            binding.source_clock.as_str(),
        ),
        (
            "observatory.native.source_order",
            if event.sequence.is_some() {
                "sender-sequence"
            } else {
                "transport-only"
            },
        ),
        (
            "observatory.native.time_basis",
            if event.source_time_unix_ns == 0 {
                "bridge-receipt"
            } else {
                "source"
            },
        ),
    ];
    let mut attributes: Vec<_> = strings
        .into_iter()
        .map(|(key, value)| text(key, value))
        .collect();
    for (key, value) in [
        ("observatory.native.schema", 1),
        ("observatory.native.sequence", queued.sequence),
        (
            "observatory.native.source_time_ns",
            event.source_time_unix_ns,
        ),
    ] {
        attributes.push(json!({"key":key,"value":{"intValue":value.to_string()}}));
    }
    let payload = json!({"resourceLogs":[{
        "resource":{"attributes":[text("service.name",&binding.service),text("observatory.source_id",&binding.source_id)]},
        "scopeLogs":[{"scope":{"name":"observatory.native-lifecycle.v1"},"logRecords":[{
            "eventName":"agent.activity","severityNumber":9,"severityText":"INFO",
            "traceId":event.trace_id,"timeUnixNano":stamp.to_string(),
            "attributes":attributes,"body":{"stringValue":""}
        }]}]
    }]});
    serde_json::to_vec(&payload).map_err(|_| "Cannot encode native lifecycle metadata".into())
}

fn text(key: &str, value: &str) -> Value {
    json!({"key":key,"value":{"stringValue":value}})
}

#[derive(Deserialize)]
struct NativeHealth {
    receiver_id: String,
    indexing: Indexing,
    native_lifecycle_v1: bool,
    #[serde(default)]
    local_agents: LocalAgents,
}

#[derive(Deserialize)]
struct Indexing {
    ready: bool,
}

#[derive(Default, Deserialize)]
struct LocalAgents {
    #[serde(default)]
    sessions: Vec<LocalSession>,
}

#[derive(Deserialize)]
struct LocalSession {
    #[serde(flatten)]
    scope: ActorScope,
    source_id: Option<String>,
}

#[derive(Deserialize)]
struct Activities {
    receiver_id: String,
    activity: Vec<Activity>,
}

#[derive(Deserialize)]
struct Activity {
    #[serde(flatten)]
    scope: ActorScope,
    trace_id: Option<String>,
    turn_id: Option<String>,
}

#[derive(Deserialize)]
struct ActorScope {
    service: Option<String>,
    agent_id: Option<String>,
    session_id: Option<String>,
}

impl ActorScope {
    fn matches(&self, binding: &Binding) -> bool {
        self.service.as_deref() == Some(&binding.service)
            && self.agent_id.as_deref() == Some(&binding.agent_id)
            && self.session_id.as_deref() == Some(&binding.session_id)
    }
}

#[derive(Deserialize)]
struct Hierarchy {
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct Node {
    agent_id: String,
    source_id: Option<String>,
    parent_agent_id: Option<String>,
    relationship_state: Option<String>,
}
