//! Frozen exact-execution Review for an authenticated, already running 0.2.8 receiver.

pub(crate) mod cockpit;
pub(crate) mod continuity;
mod output;
mod projection;

use super::legacy::LegacyReceiver;
use crate::remote_review::LegacyReviewOptions;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) use output::output_extension;

const MAX_ROWS: usize = 1_000;
const PAGE_SIZE: usize = 200;
const MAX_BYTES: usize = 2 * 1024 * 1024;
const REVIEW_TIME: Duration = Duration::from_secs(15);
const SCOPE_ERROR: &str = "Python receiver did not prove the selected execution identity";
const POLICY_ERROR: &str = "Python receiver capture policy changed while reading Review evidence";

#[derive(Clone, Debug, Serialize)]
struct Scope {
    receiver: String,
    producer: String,
    agent_id: String,
    session_id: String,
    run_id: String,
    run_start_ns: Option<u64>,
    kind: &'static str,
    endpoint_fallback: bool,
    run_end_ns: Option<u64>,
    turn_id: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Owner {
    #[serde(default)]
    producer: String,
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    session_id: String,
}

impl Owner {
    fn matches(&self, scope: &Scope) -> bool {
        self.producer == scope.producer
            && self.agent_id == scope.agent_id
            && self.session_id == scope.session_id
    }

    fn admits(&self, scope: &Scope) -> bool {
        self.producer == scope.producer
            && (scope.agent_id.is_empty() || self.agent_id == scope.agent_id)
            && (scope.session_id.is_empty() || self.session_id == scope.session_id)
            && valid_identity(&self.agent_id)
            && valid_identity(&self.session_id)
            && !self.session_id.is_empty()
    }
}

impl Scope {
    fn initial(options: &LegacyReviewOptions, receiver: &str) -> Self {
        Self {
            receiver: receiver.into(),
            producer: options.service.clone(),
            agent_id: options.agent_id.clone(),
            session_id: options.session_id.clone(),
            run_id: options.trace_id.to_ascii_lowercase(),
            run_start_ns: None,
            kind: "run",
            endpoint_fallback: false,
            run_end_ns: None,
            turn_id: String::new(),
        }
    }

    fn activity_path(&self, cursor: Option<&str>) -> String {
        let trace = if self.turn_id.is_empty() {
            &self.run_id
        } else {
            ""
        };
        format!(
            "/api/activity?limit={PAGE_SIZE}&service={}&agent_id={}&session_id={}&trace_id={}&turn_id={}&subtree=0&review=1&cursor={}",
            encode_query(&self.producer),
            encode_query(&self.agent_id),
            encode_query(&self.session_id),
            encode_query(trace),
            encode_query(&self.turn_id),
            encode_query(cursor.unwrap_or_default()),
        )
    }
}

#[derive(Deserialize)]
struct Health {
    ok: bool,
    receiver_id: String,
    #[serde(default)]
    review_trace_filter: bool,
    #[serde(default)]
    review_turn_filter: bool,
    #[serde(default)]
    review_run_summary: bool,
    indexing: Indexing,
    security: Security,
    #[serde(default)]
    local_agents: LocalPolicy,
}

#[derive(Deserialize)]
struct Indexing {
    ready: bool,
}

#[derive(Deserialize)]
struct Security {
    authentication_required: bool,
    host_validation: bool,
}

#[derive(Default, Deserialize)]
pub(super) struct LocalPolicy {
    capture_content: Option<bool>,
    capture_policy_schema: Option<u64>,
    capture_global_revision: Option<u64>,
    #[serde(default)]
    capture_policy_revisions: Vec<PolicyRow>,
    #[serde(default)]
    sessions: Vec<PolicyRow>,
}

#[derive(Default, Deserialize)]
struct PolicyRow {
    service: Option<String>,
    agent_id: Option<String>,
    session_id: Option<String>,
    runtime: Option<String>,
    capture_content: Option<bool>,
    capture_revision: Option<Revision>,
    capture_safety_revision: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
enum Revision {
    Flag(bool),
    Number(u64),
    Label(String),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct CapturePolicy {
    denied: bool,
    global: Option<bool>,
    schema: Option<u64>,
    global_revision: Option<u64>,
    identities: Vec<PolicyWitness>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct PolicyWitness {
    service: String,
    agent_id: String,
    session_id: String,
    capture_content: Option<bool>,
    capture_revision: Option<Revision>,
    capture_safety_revision: Option<u64>,
}

impl LocalPolicy {
    pub(super) fn for_investigation(
        &self,
        service: &str,
        agent: Option<&str>,
        session: Option<&str>,
    ) -> Result<CapturePolicy, String> {
        let scope = Scope {
            receiver: String::new(),
            producer: service.into(),
            agent_id: agent.unwrap_or_default().into(),
            session_id: session.unwrap_or_default().into(),
            run_id: String::new(),
            run_start_ns: None,
            kind: "investigation",
            endpoint_fallback: false,
            run_end_ns: None,
            turn_id: String::new(),
        };
        let policy = self.for_scope(&scope)?;
        // Tagged investigation references have no capture-safety generation.
        // They cannot authorize a source after its safety generation changes.
        let revised = policy.schema == Some(1)
            && (policy.global_revision.is_some_and(|revision| revision > 0)
                || policy.identities.iter().any(|witness| {
                    witness
                        .capture_safety_revision
                        .is_some_and(|revision| revision > 0)
                }));
        if policy.denied || revised {
            return Err("Python receiver capture policy forbids investigation evidence".into());
        }
        Ok(policy)
    }

    pub(super) fn investigation_ready(&self) -> bool {
        self.capture_content != Some(false)
            && self
                .capture_global_revision
                .is_none_or(|revision| revision == 0)
    }

    fn for_scope(&self, scope: &Scope) -> Result<CapturePolicy, String> {
        if self.sessions.len() > MAX_ROWS || self.capture_policy_revisions.len() > MAX_ROWS {
            return Err("Python receiver capture policy exceeds its bounded inventory".into());
        }
        let mut witnesses = self
            .sessions
            .iter()
            .chain(&self.capture_policy_revisions)
            .filter(|row| row.matches(scope))
            .map(PolicyRow::witness)
            .collect::<Result<Vec<_>, _>>()?;
        // Canonical ordering makes polling/reordering harmless even when more
        // than one policy row can conservatively deny the selected identity.
        witnesses.sort_by_cached_key(|witness| serde_json::to_string(witness).unwrap_or_default());
        witnesses.dedup();
        let denied = scope.producer == "codex-review"
            || self.capture_content == Some(false)
            || witnesses
                .iter()
                .any(|row| row.capture_content == Some(false));
        Ok(CapturePolicy {
            denied,
            global: self.capture_content,
            schema: self.capture_policy_schema,
            global_revision: self.capture_global_revision,
            identities: witnesses,
        })
    }

    fn unique_session(&self, scope: &Scope) -> Option<String> {
        let sessions: BTreeSet<_> = self
            .sessions
            .iter()
            .filter(|row| {
                row.service.as_deref() == Some(&scope.producer)
                    && row.agent_id.as_deref() == Some(&scope.agent_id)
            })
            .filter_map(|row| row.session_id.as_ref())
            .filter(|id| !id.is_empty() && valid_identity(id))
            .collect();
        if sessions.len() == 1 {
            sessions.into_iter().next().cloned()
        } else {
            None
        }
    }
}

impl PolicyRow {
    fn matches(&self, scope: &Scope) -> bool {
        let service = self.service.as_deref().unwrap_or_default();
        let session = self.session_id.as_deref().unwrap_or_default();
        let agent = self.agent_id.as_deref().unwrap_or_default();
        let service_matches = service == scope.producer
            || (scope.producer == "codex"
                && service == "codex-review"
                && self.runtime.as_deref() == Some("Codex")
                && (self.capture_content == Some(false) || self.capture_safety_revision.is_some()));
        service_matches
            && agent == scope.agent_id
            && (scope.session_id.is_empty()
                || session == scope.session_id
                || (!agent.is_empty()
                    && session.is_empty()
                    && (self.capture_content == Some(false)
                        || self.capture_safety_revision.is_some())))
    }

    fn witness(&self) -> Result<PolicyWitness, String> {
        let service = self.service.clone().unwrap_or_default();
        let agent_id = self.agent_id.clone().unwrap_or_default();
        let session_id = self.session_id.clone().unwrap_or_default();
        if [&service, &agent_id, &session_id]
            .into_iter()
            .any(|id| !valid_identity(id))
            || matches!(&self.capture_revision, Some(Revision::Label(label)) if !valid_identity(label))
        {
            return Err("Python receiver capture policy is invalid".into());
        }
        Ok(PolicyWitness {
            service,
            agent_id,
            session_id,
            capture_content: self.capture_content,
            capture_revision: self.capture_revision.clone(),
            capture_safety_revision: self.capture_safety_revision,
        })
    }
}

impl CapturePolicy {
    fn revision(&self) -> Value {
        if self.denied {
            return Value::Bool(false);
        }
        let revision = self
            .identities
            .iter()
            .filter_map(|witness| witness.capture_revision.as_ref())
            .next();
        match revision {
            Some(Revision::Flag(value)) => Value::Bool(*value),
            Some(Revision::Number(value)) => (*value).into(),
            // Producer revision strings are comparison inputs. Keep arbitrary
            // receiver text out of otherwise metadata-only receipts.
            Some(Revision::Label(_)) => Value::Null,
            None => self.global.map(Value::Bool).unwrap_or_default(),
        }
    }
}

#[derive(Default, Deserialize)]
struct RunResponse {
    run: Option<Run>,
}

#[derive(Default, Deserialize)]
struct TurnResponse {
    turn: Option<Run>,
}

#[derive(Default, Deserialize)]
struct Run {
    trace_id: String,
    #[serde(default)]
    turn_id: String,
    review_owner: Option<Owner>,
    scope_owner: Option<Owner>,
    #[serde(default)]
    step_trace_ids: Vec<String>,
    #[serde(default)]
    turn_coverage_reasons: Vec<String>,
    turn_closed: Option<bool>,
    turn_membership: Option<String>,
    turn_usage_complete: Option<bool>,
    agent_count: Option<u64>,
    agent_count_complete: Option<bool>,
    task_outcome: Option<String>,
    #[serde(flatten)]
    usage: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
struct ActivityPage {
    receiver_id: String,
    activity: Vec<Event>,
    #[serde(default)]
    review_context: Vec<Event>,
    next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Event {
    id: Option<Value>,
    source_id: Option<String>,
    trace_id: Option<String>,
    span_id: Option<String>,
    service: Option<String>,
    agent_id: Option<String>,
    session_id: Option<String>,
    receiver_id: Option<String>,
    turn_id: Option<String>,
    time_unix_nano: Option<u64>,
    end_unix_nano: Option<u64>,
    #[serde(default)]
    domain: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    result: String,
    reported_result: Option<String>,
    detail: Option<Value>,
    detail_role: Option<String>,
    detail_clipped: Option<bool>,
    operation_evidence: Option<Value>,
    from_path: Option<String>,
    to_path: Option<String>,
    old_path: Option<String>,
    new_path: Option<String>,
}

type Evidence = (Vec<Event>, Option<String>, Vec<&'static str>);

fn fetch_health(receiver: &LegacyReceiver, deadline: Instant) -> Result<Health, String> {
    let health: Health = receiver.get_for_native("/api/health", deadline)?;
    if !health.ok
        || !health.security.authentication_required
        || !health.security.host_validation
        || health.receiver_id != receiver.native_identity()
    {
        return Err("Python receiver Review identity or authentication changed".into());
    }
    if !health.indexing.ready || !health.review_trace_filter {
        return Err("Python receiver exact Review is unavailable or indexing".into());
    }
    Ok(health)
}

pub(crate) fn execute(options: &LegacyReviewOptions, stdout: &mut dyn Write) -> Result<(), String> {
    let deadline = Instant::now() + REVIEW_TIME;
    let receiver = LegacyReceiver::connect(&options.receiver)?;
    let mut scope = Scope::initial(options, receiver.native_identity());
    let health = fetch_health(&receiver, deadline)?;
    let run = if health.local_agents.for_scope(&scope)?.denied {
        // The tagged receiver can put historical task text in run summaries.
        // An already denied scope needs only its projected metadata receipt.
        None
    } else {
        resolve_run(&receiver, &health, &mut scope, options.whole_turn, deadline)?
    };
    let policy = health.local_agents.for_scope(&scope)?;
    let (events, cursor, reasons) = if policy.denied {
        (Vec::new(), None, vec!["Metadata-only capture"])
    } else {
        read_evidence(&receiver, &scope, options.pages, deadline)?
    };
    let captured_at = if policy.denied {
        0.0
    } else {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "Review clock is unavailable")?
            .as_secs_f64()
    };
    let projection = projection::build(
        &scope,
        run.as_ref(),
        &events,
        cursor,
        reasons,
        &policy,
        captured_at,
    );
    let verify = || {
        let current = fetch_health(&receiver, deadline)?;
        if health.review_turn_filter != current.review_turn_filter
            || health.review_run_summary != current.review_run_summary
            || policy != current.local_agents.for_scope(&scope)?
        {
            return Err(POLICY_ERROR.into());
        }
        Ok(())
    };
    output::publish(
        &projection,
        options.format,
        options.output.as_deref(),
        verify,
        stdout,
    )
}

fn resolve_run(
    receiver: &LegacyReceiver,
    health: &Health,
    scope: &mut Scope,
    whole_turn: bool,
    deadline: Instant,
) -> Result<Option<Run>, String> {
    if whole_turn {
        if !health.review_turn_filter {
            return Err("Python receiver cannot resolve recorded OpenCode turns".into());
        }
        let response: TurnResponse = receiver.get_for_review(
            &format!("/api/review-turn?trace_id={}", scope.run_id),
            deadline,
        )?;
        let turn = response
            .turn
            .ok_or("Python receiver has no authoritative OpenCode turn for the selected step")?;
        let owner = turn
            .scope_owner
            .as_ref()
            .filter(|owner| owner.admits(scope))
            .ok_or(SCOPE_ERROR)?;
        if owner.agent_id.is_empty()
            || turn.trace_id != scope.run_id
            || !valid_trace(&turn.turn_id)
            || turn.step_trace_ids.len() > MAX_ROWS
            || !turn.step_trace_ids.iter().any(|id| id == &scope.run_id)
            || turn.step_trace_ids.iter().any(|id| !valid_trace(id))
        {
            return Err(SCOPE_ERROR.into());
        }
        scope.kind = "turn";
        scope.agent_id.clone_from(&owner.agent_id);
        scope.session_id.clone_from(&owner.session_id);
        scope.turn_id.clone_from(&turn.turn_id);
        return Ok(Some(turn));
    }
    let run = if health.review_run_summary {
        let response: RunResponse = receiver.get_for_review(
            &format!("/api/review-run?trace_id={}", scope.run_id),
            deadline,
        )?;
        response.run
    } else {
        None
    };
    if run.as_ref().is_some_and(|run| run.trace_id != scope.run_id) {
        return Err(SCOPE_ERROR.into());
    }
    if scope.session_id.is_empty() {
        let owner_session = run
            .as_ref()
            .and_then(|run| run.review_owner.as_ref())
            .filter(|owner| owner.admits(scope) && owner.agent_id == scope.agent_id)
            .map(|owner| owner.session_id.clone());
        scope.session_id = owner_session
            .or_else(|| health.local_agents.unique_session(scope))
            .ok_or("Supply --session-id to disambiguate the selected Python execution")?;
    }
    // An independently reported owner is sufficient to disprove a requested
    // actor. An unallocated/mixed run can still supply individually scoped
    // activity rows, but cannot supply per-agent usage or an outcome.
    if run
        .as_ref()
        .and_then(|run| run.review_owner.as_ref())
        .is_some_and(|owner| !owner.producer.is_empty() && !owner.matches(scope))
    {
        return Err(SCOPE_ERROR.into());
    }
    Ok(run)
}

fn read_evidence(
    receiver: &LegacyReceiver,
    scope: &Scope,
    pages: usize,
    deadline: Instant,
) -> Result<Evidence, String> {
    let mut events = BTreeMap::new();
    let mut previous_cursors = BTreeSet::new();
    let mut cursor = None;
    let mut bytes = 0;
    let mut reasons = Vec::new();
    for _ in 0..pages {
        let page: ActivityPage =
            receiver.get_for_review(&scope.activity_path(cursor.as_deref()), deadline)?;
        if page.receiver_id != scope.receiver
            || page.activity.len() > PAGE_SIZE
            || page.review_context.len() > 2
            || (cursor.is_some() && !page.review_context.is_empty())
            || page
                .next_cursor
                .as_ref()
                .is_some_and(|value| !valid_cursor(value))
        {
            return Err("Python receiver sent invalid Review activity or cursor".into());
        }
        cursor = page.next_cursor;
        for event in page.review_context.into_iter().chain(page.activity) {
            if event
                .receiver_id
                .as_ref()
                .is_some_and(|id| id != &scope.receiver)
            {
                return Err("Python receiver activity changed Review identity".into());
            }
            if !event.matches(scope) {
                // The Python server's session-only filter can still return
                // named siblings. Never admit them into this receipt.
                continue;
            }
            let key = event
                .key()
                .ok_or("Python receiver Review event has no bounded source identity")?;
            if let Some(prior) = events.get(&key) {
                if prior != &event {
                    return Err("Python receiver repeated a conflicting Review event".into());
                }
                continue;
            }
            let size = serde_json::to_vec(&event)
                .map_err(|_| "Invalid Review activity")?
                .len();
            if events.len() == MAX_ROWS || bytes + size > MAX_BYTES {
                reasons.push("Review event or payload limit reached");
                break;
            }
            bytes += size;
            events.insert(key, event);
        }
        if !reasons.is_empty() || cursor.is_none() {
            break;
        }
        if !previous_cursors.insert(cursor.clone().unwrap_or_default()) {
            return Err("Python receiver repeated a Review activity cursor".into());
        }
    }
    let mut events: Vec<_> = events.into_values().collect();
    events.sort_by_cached_key(|row| (row.time_unix_nano.unwrap_or_default(), row.key()));
    if reasons.is_empty() && cursor.is_some() {
        reasons.push("Earlier evidence available; load explicitly");
    }
    Ok((events, cursor, reasons))
}

impl Event {
    fn matches(&self, scope: &Scope) -> bool {
        self.service.as_deref() == Some(&scope.producer)
            && self.session_id.as_deref() == Some(&scope.session_id)
            && self.agent_id.as_deref().unwrap_or_default() == scope.agent_id
            && if scope.turn_id.is_empty() {
                self.trace_id.as_deref() == Some(&scope.run_id)
            } else {
                self.turn_id.as_deref() == Some(&scope.turn_id)
            }
    }

    fn key(&self) -> Option<(String, String)> {
        let source = self
            .source_id
            .as_deref()
            .filter(|id| !id.is_empty() && valid_identity(id));
        let id = source
            .map(str::to_owned)
            .or_else(|| match self.id.as_ref()? {
                Value::String(id) if !id.is_empty() && valid_identity(id) => Some(id.clone()),
                Value::Number(id) if id.is_i64() || id.is_u64() => Some(id.to_string()),
                _ => None,
            })?;
        let trace = self.trace_id.as_deref().unwrap_or_default();
        if !trace.is_empty() && !valid_trace(trace) {
            return None;
        }
        Some((trace.into(), id))
    }
}

pub(crate) fn valid_identity(value: &str) -> bool {
    value.len() <= 512 && value.chars().all(|ch| !unsafe_display(ch))
}

pub(crate) fn valid_trace(value: &str) -> bool {
    value.len() == 32
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        && value.bytes().any(|byte| byte != b'0')
}

fn valid_cursor(value: &str) -> bool {
    !value.is_empty() && value.len() <= 8_192 && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn unsafe_display(ch: char) -> bool {
    ch.is_control()
        || matches!(ch, '\u{00ad}' | '\u{061c}' | '\u{180e}' | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}' | '\u{e0001}'..='\u{e007f}')
}

pub(super) fn encode_query(value: &str) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scope() -> Scope {
        Scope {
            receiver: "a".repeat(32),
            producer: "codex".into(),
            agent_id: "ag1_fixture".into(),
            session_id: "test/session".into(),
            run_id: "b".repeat(32),
            run_start_ns: None,
            run_end_ns: None,
            kind: "run",
            endpoint_fallback: false,
            turn_id: String::new(),
        }
    }

    #[test]
    fn policy_changes_follow_the_exact_actor_across_runs() {
        let scope = scope();
        let policy = |row| {
            serde_json::from_value::<LocalPolicy>(json!({
                "capture_content": true,
                "capture_policy_schema": 1,
                "capture_global_revision": 7,
                "sessions": [row],
            }))
            .unwrap()
            .for_scope(&scope)
            .unwrap()
        };
        let unrelated = policy(json!({
            "service": "codex", "agent_id": "ag1_sibling", "session_id": scope.session_id,
            "capture_content": false, "capture_revision": false,
        }));
        assert!(!unrelated.denied);
        let review = policy(json!({
            "service": "codex-review", "runtime": "Codex", "agent_id": scope.agent_id,
            "capture_content": false, "capture_revision": 3, "trace_id": "newer-run",
        }));
        assert!(review.denied);
        assert_ne!(review, unrelated);
        assert_eq!(review.revision(), json!(false));
        let exact = |revision| {
            policy(json!({
                "service": "codex", "agent_id": scope.agent_id, "session_id": scope.session_id,
                "capture_content": true, "capture_revision": revision,
            }))
        };
        assert_ne!(exact(3), exact(4));
        assert!(!exact(4).denied);
    }

    #[test]
    fn projection_rejects_label_only_checks_and_never_moves_success_past_an_edit() {
        let scope = scope();
        let event = |id, time, domain, action, subject, result, detail: Value| {
            serde_json::from_value::<Event>(json!({
                "source_id": id, "trace_id": scope.run_id, "service": scope.producer,
                "agent_id": scope.agent_id, "session_id": scope.session_id,
                "time_unix_nano": time, "domain": domain, "action": action, "subject": subject,
                "result": result, "detail": detail,
                "detail_role": if domain == "shell" { Some("operation_v1") } else { None },
            }))
            .unwrap()
        };
        let events = [
            event(
                "one",
                10,
                "message",
                "user",
                "User message",
                "unknown",
                json!("<user_instructions>private wrapper</user_instructions>Change the fixture"),
            ),
            event(
                "two",
                20,
                "shell",
                "exec",
                "pytest label",
                "success",
                Value::Null,
            ),
            event(
                "three",
                30,
                "shell",
                "exec",
                "Bash",
                "success",
                json!({
                    "version": 1, "commands": [["native.command", "bash -lc 'cd /fixture && cargo test'"]],
                    "exits": [["native.exit_code", 0]], "cwd": "/fixture", "tool": "Bash",
                    "outputs": [["native.output", "stdout", "captured"]],
                }),
            ),
            event(
                "four",
                40,
                "file",
                "edit",
                "src/lib.rs",
                "success",
                Value::Null,
            ),
        ];
        assert!(events.iter().all(|event| event.matches(&scope)));
        let mut sibling = scope.clone();
        sibling.agent_id = "ag1_sibling".into();
        assert!(events.iter().all(|event| !event.matches(&sibling)));
        assert!(
            scope
                .activity_path(Some("cursor&agent_id=sibling"))
                .contains("session_id=test%2Fsession")
        );
        assert!(
            scope
                .activity_path(Some("cursor&agent_id=sibling"))
                .ends_with("cursor=cursor%26agent_id%3Dsibling")
        );
        let policy = LocalPolicy::default().for_scope(&scope).unwrap();
        let receipt = projection::build(&scope, None, &events, None, Vec::new(), &policy, 1.0);
        assert_eq!(receipt["request"]["text"], "Change the fixture");
        assert_eq!(receipt["checks"][0]["result"], "unknown");
        assert_eq!(receipt["checks"][1]["result"], "passed");
        assert_eq!(receipt["checks"][1]["later_edit_count"], 1);
        assert_eq!(receipt["checks"][1]["later_edits"][0]["source_id"], "four");
        assert_eq!(receipt["reported_outcome"], "Unknown");
        assert_eq!(receipt["usage"]["scope"], "unavailable");
    }
}
