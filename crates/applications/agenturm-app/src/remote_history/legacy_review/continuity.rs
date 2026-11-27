//! Rehydrate one frozen source selection without accepting text from a receipt.

use super::{
    CapturePolicy, Event, LegacyReceiver, ReviewAuthority, Scope, fetch_health, projection,
    read_evidence, resolve_run, run_witness, valid_identity, valid_trace,
};
use crate::RemoteHistoryOptions;
use agenturm_store::{
    LegacyActivityId, LegacyTaskOccurrence, LegacyTaskOccurrenceAuthority, LegacyTaskScope,
    LegacyTaskSourceProof,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

const RECEIPT_ERROR: &str = "Choose an exact Python Review with one recorded user request";
const POLICY_ERROR: &str = "Python Review capture policy changed or cannot be verified. Recollect the exact Review from its receiver.";
const EVENT_ERROR: &str =
    "The exact frozen Review user occurrence is unavailable within the retained evidence limit";

#[derive(Deserialize)]
struct Receipt {
    format: String,
    scope: ReceiptScope,
    request: ReceiptRequest,
    window: ReceiptWindow,
}

#[derive(Deserialize)]
struct ReceiptScope {
    receiver: String,
    producer: String,
    agent_id: String,
    session_id: String,
    run_id: String,
    turn_id: String,
    kind: String,
    #[serde(default)]
    endpoint_fallback: bool,
}

#[derive(Deserialize)]
struct ReceiptRequest {
    source: ReceiptReference,
}

#[derive(Deserialize)]
struct ReceiptReference {
    receiver_id: Option<String>,
    service: String,
    agent_id: Option<String>,
    session_id: String,
    id: Option<LegacyActivityId>,
    source_id: String,
    trace_id: Option<String>,
    span_id: Option<String>,
    turn_id: Option<String>,
    time_unix_nano: u64,
}

#[derive(Deserialize)]
struct ReceiptWindow {
    capture_revision: Value,
    #[serde(default)]
    reasons: Vec<String>,
}

pub(crate) struct Selection {
    scope: LegacyTaskScope,
    occurrence: LegacyTaskOccurrence,
    revision: Value,
}

impl Selection {
    pub(crate) fn from_receipt(input: &str, selected_id: Option<&str>) -> Result<Self, String> {
        let receipt: Receipt = serde_json::from_str(input).map_err(|_| RECEIPT_ERROR)?;
        let ReceiptScope {
            receiver,
            producer,
            agent_id,
            session_id,
            run_id,
            turn_id,
            kind,
            endpoint_fallback,
        } = receipt.scope;
        let reference = receipt.request.source;
        if receipt.format != "agenturm-review-v1"
            || endpoint_fallback
            || !matches!(kind.as_str(), "run" | "turn")
            || (kind == "turn") != !turn_id.is_empty()
            || reference
                .receiver_id
                .as_ref()
                .is_some_and(|identity| identity != &receiver)
            || reference.service != producer
            || reference.session_id != session_id
            || reference.agent_id.as_deref().unwrap_or_default() != agent_id
            || receipt
                .window
                .reasons
                .iter()
                .any(|reason| reason.eq_ignore_ascii_case("metadata-only capture"))
            || receipt.window.capture_revision == Value::Bool(false)
        {
            return Err(RECEIPT_ERROR.into());
        }
        if selected_id.is_some_and(|id| match &reference.id {
            Some(LegacyActivityId::Text(actual)) => actual != id,
            Some(LegacyActivityId::Number(actual)) => actual.to_string() != id,
            None => reference.source_id != id,
        }) {
            return Err("Selected event differs from the frozen Review's user occurrence".into());
        }
        let scope = LegacyTaskScope {
            receiver,
            producer,
            agent_id,
            session_id,
            run_id,
            turn_id,
        };
        let authority = if reference.trace_id.is_some() {
            LegacyTaskOccurrenceAuthority::StepMember
        } else if reference.receiver_id.as_deref() == Some(&scope.receiver) {
            LegacyTaskOccurrenceAuthority::TurnRequestPrelude
        } else {
            return Err(RECEIPT_ERROR.into());
        };
        let occurrence = LegacyTaskOccurrence {
            id: reference.id,
            source_id: reference.source_id,
            trace_id: reference.trace_id,
            span_id: reference.span_id,
            turn_id: reference.turn_id,
            time_unix_nano: reference.time_unix_nano,
            authority,
        };
        if !occurrence.binds_scope(&scope) {
            return Err(RECEIPT_ERROR.into());
        }
        Ok(Self {
            scope,
            occurrence,
            revision: receipt.window.capture_revision,
        })
    }
}

impl Scope {
    fn for_task(
        data: &LegacyTaskScope,
        event: &LegacyTaskOccurrence,
        receiver: &LegacyReceiver,
    ) -> Result<Self, String> {
        if data.receiver != receiver.native_identity()
            || data.producer.is_empty()
            || data.session_id.is_empty()
            || ![
                &data.producer,
                &data.agent_id,
                &data.session_id,
                &data.turn_id,
            ]
            .into_iter()
            .all(|part| valid_identity(part))
            || !valid_trace(&data.run_id)
            || data.run_id.bytes().any(|byte| byte.is_ascii_uppercase())
            || !event.binds_scope(data)
            || event.source_id.is_empty()
            || !valid_identity(&event.source_id)
            || matches!(&event.id, Some(LegacyActivityId::Text(id)) if id.is_empty() || !valid_identity(id))
            || event.span_id.as_deref().is_some_and(|id| {
                id.len() != 16
                    || !id
                        .bytes()
                        .all(|byte| matches!(byte,b'0'..=b'9'|b'a'..=b'f'))
            })
            || event
                .turn_id
                .as_deref()
                .is_some_and(|id| id.is_empty() || !valid_identity(id))
            || !(1..=i64::MAX as u64).contains(&event.time_unix_nano)
        {
            return Err(RECEIPT_ERROR.into());
        }
        Ok(Self {
            receiver: data.receiver.clone(),
            producer: data.producer.clone(),
            agent_id: data.agent_id.clone(),
            session_id: data.session_id.clone(),
            run_id: data.run_id.clone(),
            run_start_ns: None,
            run_end_ns: None,
            kind: if data.turn_id.is_empty() {
                "run"
            } else {
                "turn"
            },
            endpoint_fallback: false,
            turn_id: data.turn_id.clone(),
            turn_steps: std::collections::BTreeSet::new(),
        })
    }
}

fn matches_event(event: &Event, occurrence: &LegacyTaskOccurrence) -> bool {
    let id_matches = match (&occurrence.id, event.id.as_ref()) {
        (None, None) => true,
        (Some(LegacyActivityId::Text(id)), Some(Value::String(actual))) => id == actual,
        (Some(LegacyActivityId::Number(id)), Some(actual)) => actual.as_u64() == Some(*id),
        _ => false,
    };
    event.domain == "message"
        && event.action == "user"
        && id_matches
        && event.source_id.as_deref() == Some(occurrence.source_id.as_str())
        && event.trace_id == occurrence.trace_id
        && event.span_id == occurrence.span_id
        && event.turn_id == occurrence.turn_id
        && event.time_unix_nano == Some(occurrence.time_unix_nano)
}

fn read_current_prelude(
    receiver: &LegacyReceiver,
    scope: &Scope,
    occurrence: &LegacyTaskOccurrence,
    deadline: Instant,
) -> Result<Event, String> {
    read_evidence(receiver, scope, 1, true, deadline)?
        .request_prelude
        .filter(|event| matches_event(event, occurrence))
        .ok_or_else(|| EVENT_ERROR.into())
}

fn hash(value: &impl Serialize) -> Result<String, String> {
    let encoded =
        serde_json::to_vec(value).map_err(|_| "Cannot attest Python Review source evidence")?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn safety_binding(value: &str) -> Option<(u64, u64, &str)> {
    let mut fields = value.strip_prefix("feed-safety-v1:")?.split(':');
    let parse = |part: &str| {
        (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| part.parse::<u64>().ok())
            .flatten()
    };
    let global = parse(fields.next()?)?;
    let actor = parse(fields.next()?)?;
    let digest = fields.next()?;
    if fields.next().is_some()
        || digest.len() != 64
        || !digest.bytes().all(|b| matches!(b,b'0'..=b'9'|b'a'..=b'f'))
    {
        return None;
    }
    Some((global, actor, digest))
}

fn policy_epoch(policy: &CapturePolicy) -> (u64, u64) {
    if policy.schema == Some(1) {
        (
            policy.global_revision.unwrap_or_default(),
            policy
                .identities
                .iter()
                .filter_map(|row| row.capture_safety_revision)
                .max()
                .unwrap_or_default(),
        )
    } else {
        (0, 0)
    }
}

fn source_revision(policy: &CapturePolicy) -> Result<Option<&super::Revision>, String> {
    let mut markers = policy
        .identities
        .iter()
        .filter_map(|row| row.capture_revision.as_ref());
    let selected = markers.next();
    if markers.any(|revision| Some(revision) != selected) {
        return Err(POLICY_ERROR.into());
    }
    Ok(selected)
}

fn source_revision_digest(policy: &CapturePolicy) -> Result<String, String> {
    match source_revision(policy)? {
        Some(super::Revision::Label(label)) => match safety_binding(label) {
            Some((_, _, digest)) => Ok(digest.to_owned()),
            None if label.is_ascii() => hash(label),
            None => Err(POLICY_ERROR.into()),
        },
        Some(super::Revision::Flag(flag)) => hash(flag),
        Some(super::Revision::Number(number)) => hash(number),
        None => hash(&Value::Null),
    }
}

pub(super) fn frozen_capture_revision(policy: &CapturePolicy) -> Value {
    let (global, actor) = policy_epoch(policy);
    if policy.denied || (global, actor) == (0, 0) {
        return policy.revision();
    }
    source_revision_digest(policy)
        .map(|digest| Value::String(format!("feed-safety-v1:{global}:{actor}:{digest}")))
        .unwrap_or_default()
}

fn check_frozen_revision(selected: &Value, policy: &CapturePolicy) -> Result<(), String> {
    if policy.denied || selected == &Value::Bool(false) {
        return Err(POLICY_ERROR.into());
    }
    let (global, actor) = policy_epoch(policy);
    if let Some(value) = selected.as_str() {
        let Some((expected_global, expected_actor, digest)) = safety_binding(value) else {
            return Err(POLICY_ERROR.into());
        };
        if (global, actor) != (expected_global, expected_actor) || (global, actor) == (0, 0) {
            return Err(POLICY_ERROR.into());
        }
        if source_revision_digest(policy)? != digest {
            return Err(POLICY_ERROR.into());
        }
    } else if (global, actor) != (0, 0)
        || selected != &policy.revision()
        || matches!(source_revision(policy)?, Some(super::Revision::Label(_)))
    {
        return Err(POLICY_ERROR.into());
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Expected<'a> {
    Receipt(&'a Value),
    Proof(&'a LegacyTaskSourceProof),
}

pub(crate) fn with_selection<T, E: From<String>>(
    receiver: &RemoteHistoryOptions,
    selection: &Selection,
    consume: impl FnOnce(LegacyTaskSourceProof, &str) -> Result<T, E>,
) -> Result<T, E> {
    with_prompt(
        receiver,
        &selection.scope,
        &selection.occurrence,
        Expected::Receipt(&selection.revision),
        consume,
    )
}

pub(crate) fn with_proof<T, E: From<String>>(
    receiver: &RemoteHistoryOptions,
    source: &LegacyTaskSourceProof,
    consume: impl FnOnce(LegacyTaskSourceProof, &str) -> Result<T, E>,
) -> Result<T, E> {
    if receiver.endpoint() != source.endpoint {
        return Err(String::from(
            "Supply the exact Python receiver endpoint recorded in this task draft",
        )
        .into());
    }
    with_prompt(
        receiver,
        &source.scope,
        &source.occurrence,
        Expected::Proof(source),
        consume,
    )
}

fn with_prompt<T, E: From<String>>(
    options: &RemoteHistoryOptions,
    selected: &LegacyTaskScope,
    occurrence: &LegacyTaskOccurrence,
    expected: Expected<'_>,
    consume: impl FnOnce(LegacyTaskSourceProof, &str) -> Result<T, E>,
) -> Result<T, E> {
    if !options.is_python() {
        return Err(String::from("Select the explicit Python v0.2.8 receiver protocol").into());
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let receiver = LegacyReceiver::connect(options)?;
    let origin = Scope::for_task(selected, occurrence, &receiver)?;
    let mut scope = origin.clone();
    let health = fetch_health(&receiver, deadline)?;
    let policy = health.local_agents.for_scope(&scope)?;
    let capture_sha256 = hash(&policy)?;
    match expected {
        Expected::Receipt(revision) => check_frozen_revision(revision, &policy)?,
        Expected::Proof(source) if policy.denied || source.capture_sha256 != capture_sha256 => {
            return Err(POLICY_ERROR.to_owned().into());
        }
        Expected::Proof(_) => {}
    }
    let turn = !selected.turn_id.is_empty();
    let resolved = resolve_run(&receiver, &health, &mut scope, turn, deadline)?;
    if scope.session_id != selected.session_id
        || scope.agent_id != selected.agent_id
        || scope.turn_id != selected.turn_id
        || scope.run_id != selected.run_id
        || (turn
            && occurrence.trace_id.as_ref().is_some_and(|trace| {
                !resolved
                    .as_ref()
                    .is_some_and(|run| run.step_trace_ids.contains(trace))
            }))
    {
        return Err(RECEIPT_ERROR.to_owned().into());
    }
    let is_prelude = occurrence.authority == LegacyTaskOccurrenceAuthority::TurnRequestPrelude;
    let event = if is_prelude {
        read_current_prelude(&receiver, &scope, occurrence, deadline)?
    } else {
        let mut matches = read_evidence(&receiver, &scope, 5, false, deadline)?
            .members
            .into_iter()
            .filter(|event| matches_event(event, occurrence));
        let event = matches.next().ok_or_else(|| EVENT_ERROR.to_owned())?;
        if matches.next().is_some() {
            return Err(EVENT_ERROR.to_owned().into());
        }
        event
    };
    let event_sha256 = hash(&event)?;
    // A public subject alone can be a display label. Only the retained input
    // in the selected user record supplies the protected excerpt.
    let mut input = event.clone();
    input.subject.clear();
    let projected = projection::latest_message("user", &scope, &[input])
        .ok_or_else(|| EVENT_ERROR.to_owned())?;
    let excerpt = projected
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| EVENT_ERROR.to_owned())?;
    let excerpt = agenturm_store::bounded_text_preview(excerpt.to_owned(), 8192)?;
    let proof = LegacyTaskSourceProof {
        endpoint: options.endpoint(),
        scope: selected.clone(),
        occurrence: occurrence.clone(),
        capture_sha256,
        event_sha256,
        excerpt_sha256: format!("{:x}", Sha256::digest(excerpt.as_bytes())),
    };
    if matches!(expected, Expected::Proof(source) if &proof != source) {
        return Err(EVENT_ERROR.to_owned().into());
    }
    if is_prelude && read_current_prelude(&receiver, &scope, occurrence, deadline)? != event {
        return Err(EVENT_ERROR.to_owned().into());
    }
    ReviewAuthority {
        origin,
        scope,
        policy,
        capabilities: (health.review_turn_filter, health.review_run_summary),
        witness: run_witness(resolved.as_ref(), turn),
        whole_turn: turn,
    }
    .verify(&receiver, deadline)?;
    consume(proof, &excerpt)
}
