//! Exact legacy Findings metadata and guarded, selected source display.

#[cfg(test)]
mod tests;

use super::legacy::LegacyReceiver;
use super::legacy_review::{LocalPolicy, encode_query, valid_identity};
use agenturm_ui::{
    LiveFindingKind, LiveFindingMember, LiveFindingMemberKind, LiveFindingPage, LiveFindingRow,
    LiveFindingSeverity, LiveFindingTarget, PromptDelivery,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::Write;
use std::time::{Duration, Instant};

const PAGE_ROWS: usize = 20;
const SOURCE_MEMBERS: usize = 8;
const PAGE_BYTES: usize = 2 * 1024 * 1024;
const SOURCE_BYTES: usize = 64 * 1024;
const DISPLAY_BYTES: usize = 16_384;
const READ_TIME: Duration = Duration::from_secs(5);
const SOURCE_TIME: Duration = Duration::from_secs(25);
const INVALID_PAGE: &str = "Python receiver Findings page is invalid or changed";
const INVALID_SOURCE: &str = "Finding owner or exact source membership changed";
const POLICY: &str = "Capture policy denies private finding source";
const POLICY_CHANGED: &str = "Finding capture policy changed";

#[derive(Deserialize)]
struct Health {
    ok: bool,
    receiver_id: String,
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

fn health(receiver: &LegacyReceiver, deadline: Instant) -> Result<LocalPolicy, String> {
    let health: Health = receiver.get_for_review("/api/health", deadline)?;
    if !health.ok
        || !health.indexing.ready
        || health.receiver_id != receiver.native_identity()
        || !health.security.authentication_required
        || !health.security.host_validation
    {
        return Err(
            "Python receiver Findings identity, authorization, or index is unavailable".into(),
        );
    }
    Ok(health.local_agents)
}

#[derive(Deserialize)]
struct PageWire {
    receiver_id: String,
    ownership_ready: bool,
    findings: Vec<RowWire>,
    next_cursor: Option<String>,
    has_more: bool,
}

#[derive(Deserialize)]
struct RowWire {
    id: String,
    receiver_id: String,
    kind: String,
    severity: String,
    producer: Option<String>,
    #[serde(flatten)]
    owner: Owner,
    time_unix_nano: u64,
    occurrence_count: u64,
    trace_count: u64,
    session_count: u64,
    evidence: Vec<MemberWire>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct Owner {
    service: Option<String>,
    agent_id: Option<String>,
    session_id: Option<String>,
    trace_id: Option<String>,
    turn_id: Option<String>,
}

impl Owner {
    fn of(target: &LiveFindingTarget) -> Self {
        Self {
            service: target.producer.clone(),
            agent_id: target.actor.clone(),
            session_id: target.session_id.clone(),
            trace_id: target.trace_id.clone(),
            turn_id: target.turn_id.clone(),
        }
    }

    fn normalize(mut self) -> Self {
        for part in [
            &mut self.service,
            &mut self.agent_id,
            &mut self.session_id,
            &mut self.trace_id,
            &mut self.turn_id,
        ] {
            if part.as_deref() == Some("") {
                *part = None;
            }
        }
        self
    }

    fn is_valid(&self) -> bool {
        [
            self.service.as_deref(),
            self.agent_id.as_deref(),
            self.session_id.as_deref(),
            self.turn_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        .all(safe_owner_label)
            && self.trace_id.as_deref().is_none_or(|id| hex(id, 32))
    }

    fn policy(&self, local: &LocalPolicy) -> Result<Option<String>, String> {
        let Some(service) = &self.service else {
            return Ok(None);
        };
        let Some(policy) = local.for_finding(
            service,
            self.agent_id.as_deref(),
            self.session_id.as_deref(),
        )?
        else {
            return Ok(None);
        };
        let bytes = serde_json::to_vec(&policy).map_err(|_| POLICY)?;
        Ok(Some(format!("{:x}", Sha256::digest(bytes))))
    }
}

fn safe_owner_label(value: &str) -> bool {
    !value.trim().is_empty()
        && valid_identity(value)
        && agenturm_store::bounded_text_preview(value.into(), 512).is_ok_and(|safe| safe == value)
}

#[derive(Deserialize)]
struct MemberWire {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    span_id: Option<String>,
}

fn member(kind: &str, id: &str, span_id: Option<&str>) -> Option<LiveFindingMember> {
    let (kind, span) = match kind {
        "span" => {
            let base = if let Some((base, index)) = id.split_once(":event:") {
                if index.is_empty() || index.len() > 5 || !index.bytes().all(|b| b.is_ascii_digit())
                {
                    return None;
                }
                base
            } else {
                id
            };
            if !hex(base, 16) || span_id.is_some_and(|span| span != base) {
                return None;
            }
            (LiveFindingMemberKind::Span, Some(base))
        }
        "log" if hex(id, 64) && span_id.is_none_or(|span| hex(span, 16)) => {
            (LiveFindingMemberKind::Log, span_id)
        }
        "outcome" if hex(id, 32) && span_id.is_none() => (LiveFindingMemberKind::Outcome, None),
        _ => return None,
    };
    Some(LiveFindingMember {
        kind,
        id: id.into(),
        span_id: span.map(str::to_owned),
    })
}

fn hex(value: &str, width: usize) -> bool {
    value.len() == width
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn cursor_key(value: &str) -> Option<(u64, String)> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    if value.is_empty() || value.len() > 64 || value.len() % 4 == 1 {
        return None;
    }
    let mut unpacked = Vec::with_capacity(value.len() * 3 / 4);
    let (mut pending, mut bits) = (0u16, 0u8);
    for byte in value.bytes() {
        let sextet = ALPHABET.iter().position(|&entry| entry == byte)?;
        pending = (pending << 6) | sextet as u16;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            unpacked.push((pending >> bits) as u8);
            pending &= (1 << bits) - 1;
        }
    }
    if pending != 0 {
        return None;
    }
    let (stamp, id) = std::str::from_utf8(&unpacked).ok()?.split_once(',')?;
    if stamp.is_empty()
        || !stamp.bytes().all(|byte| byte.is_ascii_digit())
        || (stamp.len() > 1 && stamp.starts_with('0'))
        || !hex(id, 16)
    {
        return None;
    }
    Some((stamp.parse().ok()?, id.into()))
}

fn finding_kind(kind: &str) -> LiveFindingKind {
    match kind {
        "span_error" => LiveFindingKind::SpanError,
        "error_log" => LiveFindingKind::ErrorLog,
        "blocked_activity" => LiveFindingKind::BlockedActivity,
        "mcp_tool_error" => LiveFindingKind::McpToolError,
        "repeated_tool_error" => LiveFindingKind::RepeatedToolError,
        "incomplete_run" => LiveFindingKind::IncompleteRun,
        "failed_outcome" => LiveFindingKind::FailedOutcome,
        "credential_exposure" => LiveFindingKind::CredentialExposure,
        "repeated_compaction" => LiveFindingKind::RepeatedCompaction,
        "repeated_equivalent_call" => LiveFindingKind::RepeatedEquivalentCall,
        "completion_unresolved_error" => LiveFindingKind::CompletionUnresolvedError,
        _ => LiveFindingKind::Other,
    }
}

fn severity(value: &str) -> LiveFindingSeverity {
    match value {
        "info" => LiveFindingSeverity::Info,
        "medium" => LiveFindingSeverity::Medium,
        "high" => LiveFindingSeverity::High,
        _ => LiveFindingSeverity::Other,
    }
}

pub(crate) fn page(
    receiver: &LegacyReceiver,
    before: Option<&str>,
) -> Result<LiveFindingPage, String> {
    if before.is_some_and(|value| cursor_key(value).is_none()) {
        return Err(INVALID_PAGE.into());
    }
    let deadline = Instant::now() + READ_TIME;
    let before_policy = health(receiver, deadline)?;
    let path = format!(
        "/api/findings?limit={PAGE_ROWS}&cursor={}",
        encode_query(before.unwrap_or_default())
    );
    let Some((wire, _)) =
        receiver.get_for_investigation::<PageWire>(&path, deadline, PAGE_BYTES)?
    else {
        return Err("Findings page exceeds its bounded response limit".into());
    };
    let after_policy = health(receiver, deadline)?;
    project_page(
        wire,
        receiver.native_identity(),
        before,
        &before_policy,
        &after_policy,
    )
}

fn project_page(
    wire: PageWire,
    receiver: &str,
    before: Option<&str>,
    before_policy: &LocalPolicy,
    after_policy: &LocalPolicy,
) -> Result<LiveFindingPage, String> {
    let order = |row: &RowWire| (row.time_unix_nano, row.id.clone());
    let first_is_older = before
        .and_then(cursor_key)
        .is_none_or(|key| wire.findings.first().is_none_or(|row| order(row) < key));
    let continuation_matches = wire.next_cursor.as_deref().is_none_or(|next| {
        wire.findings.len() == PAGE_ROWS && cursor_key(next) == wire.findings.last().map(order)
    });
    if wire.receiver_id != receiver
        || !wire.ownership_ready
        || wire.findings.len() > PAGE_ROWS
        || wire.has_more != wire.next_cursor.is_some()
        || !continuation_matches
        || !first_is_older
        || wire.findings.windows(2).any(|pair| {
            (pair[0].time_unix_nano, &pair[0].id) <= (pair[1].time_unix_nano, &pair[1].id)
        })
    {
        return Err(INVALID_PAGE.into());
    }
    let mut ids = BTreeSet::new();
    let mut rows = Vec::with_capacity(wire.findings.len());
    for raw in wire.findings {
        let owner = raw.owner.normalize();
        if raw.receiver_id != receiver
            || !hex(&raw.id, 16)
            || !ids.insert(raw.id.clone())
            || !owner.is_valid()
            || raw.producer.filter(|value| !value.is_empty()) != owner.service
            || raw.occurrence_count == 0
            || raw.trace_count > raw.occurrence_count
            || raw.session_count > raw.occurrence_count
        {
            return Err(INVALID_PAGE.into());
        }

        let refs =
            raw.evidence.iter().take(SOURCE_MEMBERS).map(|reference| {
                member(&reference.kind, &reference.id, reference.span_id.as_deref())
            });
        let members = refs.collect::<Option<Vec<_>>>().unwrap_or_default();
        let witness = owner.policy(before_policy)?;
        let witness = witness.filter(|policy| {
            owner
                .policy(after_policy)
                .is_ok_and(|current| current.as_ref() == Some(policy))
        });
        let target = LiveFindingTarget {
            receiver_id: receiver.into(),
            finding_id: raw.id,
            producer: owner.service,
            actor: owner.agent_id,
            session_id: owner.session_id,
            trace_id: owner.trace_id,
            turn_id: owner.turn_id,
            members,
            references_total: raw.evidence.len(),
            policy_witness: witness,
        };
        let source_available = source_shape(&target);
        rows.push(LiveFindingRow {
            target,
            kind: finding_kind(&raw.kind),
            severity: severity(&raw.severity),
            time_unix_ns: raw.time_unix_nano,
            occurrence_count: raw.occurrence_count,
            trace_count: raw.trace_count,
            session_count: raw.session_count,
            source_available,
        });
    }
    Ok(LiveFindingPage {
        rows,
        next_cursor: wire.next_cursor,
    })
}

fn source_shape(target: &LiveFindingTarget) -> bool {
    hex(&target.finding_id, 16)
        && target
            .policy_witness
            .as_deref()
            .is_some_and(|witness| hex(witness, 64))
        && target.producer.is_some()
        && Owner::of(target).is_valid()
        && !target.members.is_empty()
        && target.members.len() == target.references_total.min(SOURCE_MEMBERS)
        && target.members.iter().enumerate().all(|(index, candidate)| {
            let kind = match candidate.kind {
                LiveFindingMemberKind::Span => "span",
                LiveFindingMemberKind::Log => "log",
                LiveFindingMemberKind::Outcome => "outcome",
            };
            member(kind, &candidate.id, candidate.span_id.as_deref()).as_ref() == Some(candidate)
                && !target.members[..index]
                    .iter()
                    .any(|prior| prior.kind == candidate.kind && prior.id == candidate.id)
                && match candidate.kind {
                    LiveFindingMemberKind::Span => target.trace_id.is_some(),
                    LiveFindingMemberKind::Log => true,
                    LiveFindingMemberKind::Outcome => {
                        target.trace_id.as_deref() == Some(candidate.id.as_str())
                    }
                }
        })
}

#[derive(Deserialize)]
struct SourceWire {
    receiver_id: String,
    finding_id: String,
    finding_scope: Option<ExactOwner>,
    #[serde(default)]
    unavailable: bool,
    sources: Vec<SourceMember>,
    references_total: Option<usize>,
    references_clipped: Option<bool>,
}

#[derive(Deserialize)]
struct SourceMember {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    record: Option<Value>,
    complete: bool,
    scope: SourceScope,
}

#[derive(Deserialize)]
struct ExactOwner {
    #[serde(deserialize_with = "optional_scope_field")]
    service: Option<String>,
    #[serde(deserialize_with = "optional_scope_field")]
    agent_id: Option<String>,
    #[serde(deserialize_with = "optional_scope_field")]
    session_id: Option<String>,
    #[serde(deserialize_with = "optional_scope_field")]
    trace_id: Option<String>,
    #[serde(deserialize_with = "optional_scope_field")]
    turn_id: Option<String>,
}

impl ExactOwner {
    fn owner(self) -> Owner {
        Owner {
            service: self.service,
            agent_id: self.agent_id,
            session_id: self.session_id,
            trace_id: self.trace_id,
            turn_id: self.turn_id,
        }
        .normalize()
    }
}

#[derive(Deserialize)]
struct SourceScope {
    #[serde(flatten)]
    exact_owner: ExactOwner,
    #[serde(deserialize_with = "optional_scope_field")]
    span_id: Option<String>,
}

fn optional_scope_field<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::deserialize(deserializer)
}

#[derive(PartialEq)]
struct SelectedSources {
    members: Vec<SelectedMember>,
    references_clipped: bool,
}

#[derive(PartialEq)]
struct SelectedMember {
    // A log may have a span reference even when its finding only records a
    // digest. Recheck that exact source metadata before redisplay.
    member: LiveFindingMember,
    record: Option<Vec<PrivateField>>,
    complete: bool,
}

#[derive(PartialEq)]
struct PrivateField {
    label: &'static str,
    text: String,
}

fn guarded_read(
    receiver: &LegacyReceiver,
    target: &LiveFindingTarget,
    owner: &Owner,
    deadline: Instant,
) -> Result<SelectedSources, String> {
    let local = health(receiver, deadline)?;
    match owner.policy(&local)? {
        None => return Err(POLICY.into()),
        Some(witness) if Some(&witness) != target.policy_witness.as_ref() => {
            return Err(POLICY_CHANGED.into());
        }
        Some(_) => {}
    }
    let path = format!("/api/findings/{}/source", target.finding_id);
    let Some((wire, _)) =
        receiver.get_for_investigation::<SourceWire>(&path, deadline, SOURCE_BYTES)?
    else {
        return Err("Exact finding source exceeds its 64 KiB limit".into());
    };
    let evidence = verify_source(wire, target, owner)?;
    let local = health(receiver, deadline)?;
    if owner.policy(&local)? != target.policy_witness {
        return Err(POLICY_CHANGED.into());
    }
    Ok(evidence)
}

fn verify_source(
    wire: SourceWire,
    target: &LiveFindingTarget,
    owner: &Owner,
) -> Result<SelectedSources, String> {
    if wire.receiver_id != target.receiver_id || wire.finding_id != target.finding_id {
        return Err(INVALID_SOURCE.into());
    }
    if wire.unavailable {
        return Err("Finding is no longer retained".into());
    }
    let clipped = target.references_total > SOURCE_MEMBERS;
    if wire.finding_scope.map(ExactOwner::owner).as_ref() != Some(owner)
        || wire.references_total != Some(target.references_total)
        || wire.references_clipped != Some(clipped)
        || wire.sources.len() != target.members.len()
    {
        return Err(INVALID_SOURCE.into());
    }
    let mut members = Vec::with_capacity(wire.sources.len());
    for (wire, expected) in wire.sources.into_iter().zip(&target.members) {
        let span_id = wire
            .scope
            .span_id
            .as_deref()
            .filter(|value| !value.is_empty());
        let actual = member(&wire.kind, &wire.id, span_id).ok_or(INVALID_SOURCE)?;
        if wire.scope.exact_owner.owner() != *owner
            || actual.kind != expected.kind
            || actual.id != expected.id
            || expected
                .span_id
                .as_deref()
                .is_some_and(|span| Some(span) != span_id)
            || (actual.kind == LiveFindingMemberKind::Outcome && span_id.is_some())
        {
            return Err(INVALID_SOURCE.into());
        }
        let record = wire
            .record
            .as_ref()
            .map(|record| project_record(&actual, record))
            .transpose()?;
        members.push(SelectedMember {
            member: actual,
            record,
            complete: wire.complete,
        });
    }
    Ok(SelectedSources {
        members,
        references_clipped: clipped,
    })
}

const INVALID_RECORD: &str = "Finding source text contains unsupported or sensitive fields";

fn project_record(member: &LiveFindingMember, record: &Value) -> Result<Vec<PrivateField>, String> {
    let fields = record.as_object().ok_or(INVALID_RECORD)?;
    if fields.len() == 2
        && fields.get("format").and_then(Value::as_str) == Some("Truncated JSON excerpt")
        && fields.get("excerpt").is_some_and(Value::is_string)
    {
        return Ok(vec![PrivateField {
            label: "Record",
            text: "Receiver source excerpt omitted".into(),
        }]);
    }
    let string = |key: &str| fields.get(key).is_some_and(Value::is_string);
    let object = |key: &str| fields.get(key).is_some_and(Value::is_object);
    let attrs = object("attributes") && object("resource_attributes");
    let mut output = Vec::new();
    match member.kind {
        LiveFindingMemberKind::Span => {
            let exact = if member.id.contains(":event:") {
                fields
                    .keys()
                    .all(|key| matches!(key.as_str(), "name" | "time_unix_nano" | "attributes"))
                    && string("name")
                    && object("attributes")
                    && fields
                        .get("time_unix_nano")
                        .is_none_or(|time| time.as_u64().is_some())
            } else {
                fields.len() == 4 && string("name") && string("status") && attrs
            };
            if !exact {
                return Err(INVALID_RECORD.into());
            }
            let attributes = fields["attributes"].as_object().ok_or(INVALID_RECORD)?;
            for (key, label) in [
                ("exception.message", "Exception"),
                ("exception.type", "Exception type"),
                ("error.message", "Error"),
                ("error.type", "Error type"),
                ("stderr", "Standard error"),
                ("process.stderr", "Process standard error"),
            ] {
                append_text(&mut output, attributes, key, label)?;
            }
        }
        LiveFindingMemberKind::Log => {
            if !(fields.len() == 5
                && string("event_name")
                && string("severity")
                && fields.contains_key("body")
                && attrs)
            {
                return Err(INVALID_RECORD.into());
            }
            if let Some(text) = fields["body"].as_str() {
                push_text(&mut output, "Body", text)?;
            } else if !fields["body"].is_null() {
                output.push(PrivateField {
                    label: "Body",
                    text: "Structured body omitted".into(),
                });
            }
        }
        LiveFindingMemberKind::Outcome => project_outcome(&mut output, fields)?,
    }
    Ok(output)
}

fn project_outcome(
    output: &mut Vec<PrivateField>,
    fields: &Map<String, Value>,
) -> Result<(), String> {
    if fields.len() != 3 || !fields.get("error").is_some_and(Value::is_string) {
        return Err(INVALID_RECORD.into());
    }
    let passed = fields
        .get("passed")
        .and_then(Value::as_bool)
        .ok_or(INVALID_RECORD)?;
    let scores = fields
        .get("scores")
        .and_then(Value::as_array)
        .ok_or(INVALID_RECORD)?;
    push_text(output, "Outcome", if passed { "Passed" } else { "Failed" })?;
    append_text(output, fields, "error", "Evaluation error")?;
    for score in scores.iter().take(20) {
        let score = score.as_object().ok_or(INVALID_RECORD)?;
        append_text(output, score, "scorer", "Scorer")?;
        if let Some(passed) = score.get("passed") {
            let passed = passed.as_bool().ok_or(INVALID_RECORD)?;
            push_text(output, "Check", if passed { "Passed" } else { "Failed" })?;
        }
        if let Some(value) = score.get("score") {
            let value = value.as_number().ok_or(INVALID_RECORD)?;
            push_text(output, "Score", &value.to_string())?;
        }
        append_text(output, score, "detail", "Detail")?;
    }
    if scores.len() > 20 {
        push_text(output, "Scores", "Some scores omitted")?;
    }
    Ok(())
}

fn append_text(
    output: &mut Vec<PrivateField>,
    fields: &Map<String, Value>,
    key: &str,
    label: &'static str,
) -> Result<(), String> {
    if let Some(text) = fields.get(key) {
        push_text(output, label, text.as_str().ok_or(INVALID_RECORD)?)?;
    }
    Ok(())
}

fn push_text(
    output: &mut Vec<PrivateField>,
    label: &'static str,
    text: &str,
) -> Result<(), String> {
    if !text.is_empty() {
        let safe =
            agenturm_store::bounded_text_preview(text.into(), 8_000).map_err(|_| INVALID_RECORD)?;
        if safe != text {
            return Err(INVALID_RECORD.into());
        }
        output.push(PrivateField { label, text: safe });
    }
    Ok(())
}

fn private_body(evidence: &SelectedSources) -> Result<Vec<u8>, String> {
    use std::fmt::Write as _;
    let mut text = String::from("Selected exact source references\n");
    let mut clipped = evidence.references_clipped;
    for entry in &evidence.members {
        let kind = match entry.member.kind {
            LiveFindingMemberKind::Span => "Span",
            LiveFindingMemberKind::Log => "Log",
            LiveFindingMemberKind::Outcome => "Outcome",
        };
        let mut section = format!("\n{kind} · {}\n", entry.member.id);
        if !entry.complete {
            section.push_str("Bounded or redacted record\n");
        }
        match entry.record.as_deref() {
            Some([]) => section.push_str("No supported source text was reported\n"),
            Some(fields) => {
                for field in fields {
                    let _ = writeln!(section, "{} · {}", field.label, field.text);
                }
            }
            None => section.push_str("Recorded source unavailable\n"),
        }
        if text.len() + section.len() > DISPLAY_BYTES {
            clipped = true;
            break;
        }
        text.push_str(&section);
    }
    let policy = "\nCapture policy is checked by polling.\n";
    let notice = "\nSome references or record text were omitted from this bounded display.\n";
    if !agenturm_store::bounded_text_preview(text.clone(), DISPLAY_BYTES)
        .is_ok_and(|safe| safe == text)
    {
        return Err(INVALID_RECORD.into());
    }
    let mut end = text.len();
    loop {
        let candidate = format!(
            "{}{}{policy}",
            &text[..end],
            if clipped { notice } else { "" }
        );
        let encoded = serde_json::to_vec(&candidate)
            .map_err(|_| "Cannot format selected finding evidence")?;
        if encoded.len() <= DISPLAY_BYTES {
            return Ok(encoded);
        }
        clipped = true;
        end = end.saturating_sub(256);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
    }
}

pub(crate) fn source(
    receiver: &LegacyReceiver,
    target: &LiveFindingTarget,
    delivery: &mut PromptDelivery,
) -> Result<(), String> {
    if target.receiver_id != receiver.native_identity() || !source_shape(target) {
        return Err(INVALID_SOURCE.into());
    }
    let owner = Owner::of(target);
    let deadline = Instant::now() + SOURCE_TIME;
    // Read exact current evidence again immediately before publication and
    // throughout the guarded display. Tagged /source responses are never
    // response-cached. A changed owner, membership or record clears the frame.
    let evidence = guarded_read(receiver, target, &owner, deadline)?;
    let body = private_body(&evidence)?;
    delivery
        .write_all(&body)
        .map_err(|_| "Private finding display closed")?;
    let mut published = false;
    delivery.flush_guarded(|verification_deadline| {
        let deadline = if published {
            verification_deadline
        } else {
            deadline.min(verification_deadline)
        };
        let current = guarded_read(receiver, target, &owner, deadline)?;
        if current != evidence {
            return Err("Selected finding source changed".into());
        }
        published = true;
        Ok(())
    })
}
