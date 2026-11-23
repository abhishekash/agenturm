//! Whitelist projection of legacy response shapes. Object bodies are ignored by serde.

use super::{
    Activities, AttentionReport, Connections, Sessions, Traces, agent_id, capped, hex_id, label,
    time_ns,
};
use agenturm_ui::{LiveOwner, LivePage, LiveRow};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Deserialize)]
pub(super) struct Session {
    service: Option<String>,
    #[serde(default, deserialize_with = "capped")]
    services: Vec<String>,
    agent_id: Option<String>,
    session_id: Option<String>,
    parent_agent_id: Option<String>,
    run_state: Option<String>,
    latest_status: Option<String>,
    last_unix_nano: Option<u64>,
    last_record_ns: Option<u64>,
    #[serde(default)]
    pub(super) diagnostics: Diagnostics,
}

#[derive(Default, Deserialize)]
pub(super) struct Diagnostics {
    read_error: Option<String>,
    #[serde(default)]
    pending_storage_records: u64,
}

impl Diagnostics {
    pub(super) fn ready(&self) -> bool {
        self.read_error.as_deref().is_none_or(str::is_empty) && self.pending_storage_records == 0
    }
}

#[derive(Deserialize)]
pub(super) struct Connection {
    source_id: String,
    service: String,
    state: String,
    last_seen_ns: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct Activity {
    receiver_id: Option<String>,
    service: String,
    agent_id: Option<String>,
    session_id: Option<String>,
    trace_id: Option<String>,
    turn_id: Option<String>,
    time_unix_nano: u64,
    domain: String,
    action: String,
    result: String,
}

#[derive(Deserialize)]
pub(super) struct Trace {
    trace_id: String,
    service: Option<String>,
    #[serde(default, deserialize_with = "capped")]
    services: Vec<String>,
    agent_id: Option<String>,
    session_id: Option<String>,
    turn_id: Option<String>,
    status: String,
    span_count: Option<u64>,
    last_unix_nano: Option<u64>,
    end_unix_nano: Option<u64>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct Owner {
    service: String,
    session_id: String,
    agent_id: Option<String>,
}

impl Owner {
    fn new(service: &str, session: &str, actor: Option<String>) -> Option<Self> {
        if !label(service, 128)
            || !label(session, 512)
            || actor.as_deref().is_some_and(|actor| !agent_id(actor))
        {
            return None;
        }
        Some(Self {
            service: service.into(),
            session_id: session.into(),
            agent_id: actor,
        })
    }

    fn live(&self) -> LiveOwner {
        LiveOwner {
            service: self.service.clone(),
            session_id: self.session_id.clone(),
            agent_id: self.agent_id.clone(),
        }
    }

    fn label(&self) -> String {
        format!("{} / {}", self.service, self.session_id)
    }
}

#[derive(Serialize)]
struct Agent {
    #[serde(flatten)]
    owner: Owner,
    parent_agent_id: Option<String>,
    state: &'static str,
    observed_at_unix_ns: Option<String>,
}

#[derive(Serialize)]
struct ConnectionMetadata {
    source_id: String,
    service: String,
    state: &'static str,
    last_seen_unix_ns: Option<String>,
}

#[derive(Serialize)]
struct ActivityMetadata {
    service: String,
    session_id: Option<String>,
    agent_id: Option<String>,
    trace_id: Option<String>,
    turn_id: Option<String>,
    observed_at_unix_ns: String,
    domain: &'static str,
    action: &'static str,
    result: &'static str,
}

#[derive(Serialize)]
struct TraceMetadata {
    service: Option<String>,
    agent_id: Option<String>,
    session_id: Option<String>,
    trace_id: String,
    turn_id: Option<String>,
    status: &'static str,
    observed_at_unix_ns: Option<String>,
    span_count: Option<u64>,
}

#[derive(Serialize)]
pub(super) struct Report {
    format: &'static str,
    coverage: &'static str,
    receiver_id: String,
    ready: bool,
    partial: bool,
    sessions: Vec<Agent>,
    connections: Vec<ConnectionMetadata>,
    activity: Vec<ActivityMetadata>,
    traces: Vec<TraceMetadata>,
    attention: Option<AttentionReport>,
}

pub(super) struct Pages {
    pub local_sessions: Vec<Session>,
    pub sessions: Sessions,
    pub connections: Connections,
    pub activity: Activities,
    pub traces: Traces,
}

impl Report {
    pub(super) fn new(receiver_id: String) -> Self {
        Self {
            format: "agenturm-python-receiver-live/1",
            coverage: "bounded live reads",
            receiver_id,
            ready: false,
            partial: true,
            sessions: Vec::new(),
            connections: Vec::new(),
            activity: Vec::new(),
            traces: Vec::new(),
            attention: None,
        }
    }

    pub(super) fn prepare(
        &mut self,
        limit: usize,
        pages: Pages,
        requests: AttentionReport,
    ) -> Result<(), String> {
        self.ready = true;
        self.partial = pages.sessions.sessions.len() >= limit
            || pages.activity.next_cursor.is_some()
            || pages.traces.next_cursor.is_some()
            || pages.traces.has_more == Some(true)
            || !requests.ready;
        if pages.sessions.sessions.len() > limit
            || pages.connections.connections.len() > 50
            || pages.activity.activity.len() > 200
            || pages.traces.traces.len() > 100
        {
            return Err("Python receiver exceeded an agreed metadata page limit".into());
        }
        let mut sessions = BTreeMap::new();
        // Native discovery is the stronger current observation for the same exact key.
        for wire in pages
            .local_sessions
            .into_iter()
            .chain(pages.sessions.sessions)
        {
            if let Some(agent) = wire.project() {
                sessions.entry(agent.owner.clone()).or_insert(agent);
            } else {
                self.partial = true;
            }
        }
        self.sessions = sessions.into_values().collect();
        self.sessions.sort_by(|a, b| {
            b.observed_at_unix_ns
                .as_deref()
                .and_then(|ns| ns.parse::<u64>().ok())
                .cmp(
                    &a.observed_at_unix_ns
                        .as_deref()
                        .and_then(|ns| ns.parse::<u64>().ok()),
                )
                .then_with(|| a.owner.cmp(&b.owner))
        });
        self.partial |= self.sessions.len() > limit;
        self.sessions.truncate(limit);
        self.connections = project_rows(
            pages.connections.connections,
            Connection::project,
            &mut self.partial,
        );
        self.activity = project_rows(
            pages.activity.activity,
            |row| row.project(&self.receiver_id),
            &mut self.partial,
        );
        self.traces = project_rows(pages.traces.traces, Trace::project, &mut self.partial);
        self.attention = Some(requests);
        Ok(())
    }

    pub(super) fn text(&self) -> String {
        let mut output = format!(
            "Agenturm · Python receiver · bounded live metadata\nReceiver {}\n",
            self.receiver_id
        );
        if !self.ready {
            output.push_str("Receiver indexing or capture state is not ready.\n");
            return output;
        }
        if self.partial {
            output.push_str("Coverage is partial.\n");
        }
        for agent in &self.sessions {
            writeln!(output, "{} · {}", agent.owner.label(), agent.state)
                .expect("write into string");
        }
        writeln!(
            output,
            "{} sessions · {} activity events · {} traces",
            self.sessions.len(),
            self.activity.len(),
            self.traces.len()
        )
        .expect("write into string");
        output
    }

    pub(super) fn into_live_page(self) -> LivePage {
        // The Python sessions API can derive its free-form task fields from
        // captured prompts. Use only authenticated structural evidence here.
        let mut roster_evidence = BTreeMap::new();
        for row in &self.activity {
            let label = match row.domain {
                "file" => "Recorded file activity",
                "shell" => "Recorded shell activity",
                "mcp" => "Recorded MCP operation",
                "tool" => "Recorded tool activity",
                _ => continue,
            };
            if let Some(session) = row.session_id.as_deref()
                && let Some(owner) = Owner::new(&row.service, session, row.agent_id.clone())
            {
                roster_evidence.entry(owner).or_insert(label);
            }
        }
        for row in &self.traces {
            if let Some((service, session)) = row.service.as_deref().zip(row.session_id.as_deref())
                && let Some(owner) = Owner::new(service, session, row.agent_id.clone())
            {
                roster_evidence.entry(owner).or_insert("Recorded trace");
            }
        }
        let sessions = self
            .sessions
            .iter()
            .map(|agent| {
                let mut title = format!("{} · {}", agent.owner.label(), agent.state);
                if let Some(label) = roster_evidence.get(&agent.owner) {
                    let _ = write!(title, " · {label}");
                }
                let detail = agent
                    .owner
                    .agent_id
                    .as_ref()
                    .map(|actor| format!("Actor {actor}"));
                LiveRow {
                    owner: Some(agent.owner.live()),
                    title,
                    detail,
                }
            })
            .collect();
        let activity = self
            .activity
            .into_iter()
            .map(|row| {
                let owner = row
                    .session_id
                    .as_deref()
                    .and_then(|session| Owner::new(&row.service, session, row.agent_id))
                    .map(|owner| owner.live());
                let detail = row.trace_id.map(|trace| format!("Trace {trace}"));
                LiveRow {
                    owner,
                    title: format!(
                        "{} · {} / {} · {}",
                        row.service, row.domain, row.action, row.result
                    ),
                    detail,
                }
            })
            .collect();
        let traces = self
            .traces
            .into_iter()
            .map(|row| {
                let owner = row
                    .service
                    .as_deref()
                    .zip(row.session_id.as_deref())
                    .and_then(|(service, session)| Owner::new(service, session, row.agent_id))
                    .map(|owner| owner.live());
                LiveRow {
                    owner,
                    title: format!("{} · {}", row.trace_id, row.status),
                    detail: row
                        .span_count
                        .map(|spans| format!("{spans} reported spans")),
                }
            })
            .collect();
        let attention_ready = self.attention.as_ref().is_some_and(|report| report.ready);
        let pending = self
            .attention
            .into_iter()
            .flat_map(|report| report.pending)
            .map(|row| {
                let owner = Owner::new(&row.service, &row.session_id, Some(row.agent_id))
                    .map(|owner| owner.live());
                LiveRow {
                    owner,
                    title: format!(
                        "{} / {} · {}",
                        row.service,
                        row.session_id,
                        match row.reason_code {
                            super::RequestKind::ApprovalRequested => "Awaiting approval",
                            super::RequestKind::ReplyRequested => "Awaiting reply",
                            super::RequestKind::RequestResolved => "Resolved",
                        }
                    ),
                    detail: Some(format!("Request {}", row.request_id)),
                }
            })
            .collect();
        LivePage {
            sessions,
            activity,
            traces,
            pending,
            ready: self.ready,
            partial: self.partial,
            attention_ready,
        }
    }
}

impl Session {
    fn project(self) -> Option<Agent> {
        let producer = one_service(self.service.as_deref(), &self.services)?;
        let owner = Owner::new(
            producer,
            self.session_id.as_deref()?,
            normalize(self.agent_id),
        )?;
        Some(Agent {
            owner,
            parent_agent_id: self.parent_agent_id.filter(|actor| agent_id(actor)),
            state: state(self.run_state.as_deref().or(self.latest_status.as_deref())),
            observed_at_unix_ns: timestamp(self.last_record_ns.or(self.last_unix_nano)),
        })
    }
}

impl Connection {
    fn project(self) -> Option<ConnectionMetadata> {
        if !label(&self.service, 128) || !label(&self.source_id, 512) {
            return None;
        }
        Some(ConnectionMetadata {
            source_id: self.source_id,
            service: self.service,
            state: state(Some(&self.state)),
            last_seen_unix_ns: timestamp(self.last_seen_ns),
        })
    }
}

impl Activity {
    fn project(self, receiver: &str) -> Option<ActivityMetadata> {
        if self.receiver_id.as_deref() != Some(receiver)
            || !label(&self.service, 128)
            || !time_ns(self.time_unix_nano)
            || self
                .agent_id
                .as_deref()
                .is_some_and(|id| !id.is_empty() && !agent_id(id))
            || self
                .session_id
                .as_deref()
                .is_some_and(|id| !id.is_empty() && !label(id, 512))
        {
            return None;
        }
        Some(ActivityMetadata {
            service: self.service,
            session_id: normalize(self.session_id),
            agent_id: normalize(self.agent_id),
            trace_id: normalized_hex(self.trace_id),
            turn_id: normalized_hex(self.turn_id),
            observed_at_unix_ns: self.time_unix_nano.to_string(),
            domain: domain(&self.domain),
            action: action(&self.action),
            result: state(Some(&self.result)),
        })
    }
}

impl Trace {
    fn project(self) -> Option<TraceMetadata> {
        if !hex_id(&self.trace_id, 32)
            || self
                .agent_id
                .as_deref()
                .is_some_and(|id| !id.is_empty() && !agent_id(id))
            || self
                .session_id
                .as_deref()
                .is_some_and(|id| !id.is_empty() && !label(id, 512))
        {
            return None;
        }
        let service = one_service(self.service.as_deref(), &self.services).map(str::to_owned);
        Some(TraceMetadata {
            service,
            agent_id: normalize(self.agent_id),
            session_id: normalize(self.session_id),
            trace_id: self.trace_id,
            turn_id: normalized_hex(self.turn_id),
            status: trace_status(&self.status),
            span_count: self.span_count,
            observed_at_unix_ns: timestamp(self.last_unix_nano.or(self.end_unix_nano)),
        })
    }
}

fn project_rows<T, R>(
    rows: Vec<T>,
    mut project: impl FnMut(T) -> Option<R>,
    partial: &mut bool,
) -> Vec<R> {
    rows.into_iter()
        .filter_map(|row| {
            let projected = project(row);
            *partial |= projected.is_none();
            projected
        })
        .collect()
}

fn one_service<'a>(service: Option<&'a str>, services: &'a [String]) -> Option<&'a str> {
    let selected = service.or_else(|| services.first().map(String::as_str))?;
    (label(selected, 128) && services.iter().all(|service| service == selected)).then_some(selected)
}

fn timestamp(time: Option<u64>) -> Option<String> {
    time.filter(|value| time_ns(*value))
        .map(|value| value.to_string())
}

fn normalized_hex(value: Option<String>) -> Option<String> {
    value.filter(|value| hex_id(value, 32))
}

fn normalize(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

fn state(value: Option<&str>) -> &'static str {
    match value.unwrap_or_default().to_ascii_lowercase().as_str() {
        "running" | "active" => "running",
        "waiting" | "paused" => "waiting",
        "completed" | "success" | "ok" => "completed",
        "failed" | "error" => "failed",
        "denied" | "blocked" | "policy_denied" => "denied",
        "incomplete" | "interrupted" | "cancelled" | "canceled" => "interrupted",
        "stopped" | "offline" => "offline",
        "announced" => "announced",
        _ => "unknown",
    }
}

fn trace_status(value: &str) -> &'static str {
    match value {
        "OK" => "OK",
        "ERROR" => "ERROR",
        "FAILED" => "FAILED",
        "INCOMPLETE" => "INCOMPLETE",
        "UNSET" => "UNSET",
        _ => "unknown",
    }
}

fn domain(value: &str) -> &'static str {
    match value {
        "message" => "message",
        "file" => "file",
        "shell" => "shell",
        "tool" => "tool",
        "mcp" => "mcp",
        "context" => "context",
        "agent" => "agent",
        "lifecycle" => "lifecycle",
        "usage" => "usage",
        "progress" => "progress",
        _ => "activity",
    }
}

fn action(value: &str) -> &'static str {
    match value {
        "user" => "user",
        "assistant" => "assistant",
        "read" => "read",
        "write" => "write",
        "edit" => "edit",
        "execute" | "command" => "execute",
        "call" => "call",
        "result" => "result",
        "start" => "start",
        "end" | "finish" => "finish",
        "approval_request" => "approval requested",
        "input_request" => "reply requested",
        "request_resolved" => "request resolved",
        "progress" => "progress",
        _ => "reported",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_roster_uses_only_exact_structural_evidence_for_its_labels() {
        let make_session = |service: &str, actor: &str| {
            serde_json::from_value::<Session>(serde_json::json!({
                "service": service, "session_id": "shared", "agent_id": actor,
                "initial_task": "Captured native prompt text",
                "task": "Captured provider message body",
            }))
            .unwrap()
            .project()
            .unwrap()
        };
        let actor = format!("ag1_{}", "1".repeat(32));
        let sibling = format!("ag1_{}", "2".repeat(32));
        let mut report = Report::new("receiver".into());
        report.sessions = vec![
            make_session("codex", &actor),
            make_session("codex", &sibling),
            make_session("pi", &actor),
        ];
        report.activity.push(ActivityMetadata {
            service: "codex".into(),
            session_id: Some("shared".into()),
            agent_id: Some(actor),
            trace_id: None,
            turn_id: None,
            observed_at_unix_ns: "1".into(),
            domain: "file",
            action: "edit",
            result: "ok",
        });
        let live = report.into_live_page();
        assert!(live.sessions[0].title.contains("Recorded file activity"));
        assert!(!live.sessions[1].title.contains("Recorded file activity"));
        assert!(!live.sessions[2].title.contains("Recorded file activity"));
        assert!(
            live.sessions
                .iter()
                .all(|row| !row.title.contains("Captured"))
        );
    }
}
