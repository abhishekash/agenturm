//! Structured request and internal-review metadata from Codex rollout records.

use super::{Context, fact, identifier, opaque_operation, stamp, turn_from};
use agenturm_jsonl_source::Result;
use agenturm_protocol::native_review::{
    InternalReview, InternalReviewState, NativeRequest, NativeRequestKind, NativeRequestState,
};
use serde::{Deserialize, de::IgnoredAny};
use serde_json::{Value, json, value::RawValue};

// Deserialize authority fields directly so a duplicate key cannot replace an
// earlier identity or hide a review marker through Value's last-wins behavior.
#[derive(Deserialize)]
struct Envelope<T> {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "timestamp")]
    _timestamp: Option<IgnoredAny>,
    payload: T,
}

#[derive(Deserialize)]
struct HeaderScope<'a> {
    #[serde(rename = "id", borrow)]
    _id: &'a RawValue,
    #[serde(rename = "session_id", borrow)]
    _session_id: Option<&'a RawValue>,
    #[serde(rename = "timestamp")]
    _timestamp: Option<IgnoredAny>,
    #[serde(borrow)]
    source: Option<&'a RawValue>,
    #[serde(borrow)]
    thread_source: Option<&'a RawValue>,
    #[serde(borrow)]
    parent_thread_id: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct SourceMarker<'a> {
    #[serde(borrow)]
    subagent: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct SubagentMarker<'a> {
    #[serde(borrow)]
    other: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct EventScope {
    #[serde(rename = "type")]
    kind: String,
    turn_id: Option<String>,
    request_id: Option<String>,
    #[serde(rename = "error")]
    _error: Option<IgnoredAny>,
}

fn optional_object<'a, T: Deserialize<'a>>(value: Option<&'a RawValue>) -> Result<Option<T>> {
    value
        .map(RawValue::get)
        .filter(|raw| raw.starts_with('{'))
        .map(serde_json::from_str)
        .transpose()
        .map_err(Into::into)
}

fn optional_string(value: Option<&RawValue>) -> Option<String> {
    serde_json::from_str(value?.get()).ok()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Guardian {
    parent_session_id: Option<String>,
}

impl Guardian {
    pub(super) fn from_header(raw: &[u8], session_id: &str) -> Result<Option<Self>> {
        #[derive(Deserialize)]
        struct Header<'a> {
            #[serde(rename = "type")]
            _kind: String,
            #[serde(rename = "timestamp")]
            _timestamp: Option<IgnoredAny>,
            #[serde(borrow)]
            payload: HeaderScope<'a>,
        }
        let Header { payload, .. } = serde_json::from_slice(raw)?;
        let source = optional_object::<SourceMarker<'_>>(payload.source)?;
        let subagent = optional_object::<SubagentMarker<'_>>(source.and_then(|row| row.subagent))?;
        let marked = optional_string(payload.thread_source).as_deref() == Some("guardian_review")
            || optional_string(subagent.and_then(|row| row.other)).as_deref() == Some("guardian");
        if !marked {
            return Ok(None);
        }
        let parent_session_id = optional_string(payload.parent_thread_id)
            .filter(|parent| identifier(parent) && parent != session_id);
        Ok(Some(Self { parent_session_id }))
    }

    pub(super) fn classification(&self, context: &Context, origin: &str, time: &str) -> Value {
        self.record(
            context,
            &format!("codex:{origin}:0"),
            time,
            InternalReviewState::Classified,
        )
    }

    pub(super) fn translate(
        &self,
        raw: &[u8],
        row: &Value,
        context: &Context,
        kind: &str,
        base: &str,
    ) -> Result<(Vec<Value>, Context)> {
        let payload = &row["payload"];
        let state = match (kind, payload["type"].as_str()) {
            ("event_msg", Some("task_started")) => InternalReviewState::Started,
            ("event_msg", Some("task_complete")) if payload["error"].is_null() => {
                InternalReviewState::Completed
            }
            ("event_msg", Some("task_aborted" | "turn_aborted")) => {
                InternalReviewState::Interrupted
            }
            _ => return Ok((Vec::new(), context.clone())),
        };
        let parsed: Envelope<EventScope> = serde_json::from_slice(raw)?;
        // Exact native turn identity is required for every lifecycle fact.
        // In particular a later unscoped row cannot complete the active turn.
        if parsed.payload.turn_id.is_none() {
            return Ok((Vec::new(), context.clone()));
        }
        let mut occurrence = context.clone();
        turn_from(payload, &mut occurrence, true)?;
        let record = self.record(&occurrence, base, &stamp(row)?, state);
        let next = if state == InternalReviewState::Started {
            occurrence
        } else {
            context.clone()
        };
        Ok((vec![record], next))
    }

    fn record(
        &self,
        context: &Context,
        base: &str,
        time: &str,
        state: InternalReviewState,
    ) -> Value {
        let payload = InternalReview {
            session_id: context.session_id.clone(),
            parent_session_id: self.parent_session_id.clone(),
            turn_id: (!context.turn_id.is_empty() && state != InternalReviewState::Classified)
                .then(|| context.turn_id.clone()),
            worker_id: (!context.worker_id.is_empty()).then(|| context.worker_id.clone()),
            state,
        };
        // Source scope keeps the internal review out of the regular roster.
        json!({"event_id":format!("{base}:internal-review"),"revision":0,
               "kind":"internal_review","time_ns":time,"payload":payload})
    }
}

pub(super) fn request(raw: &[u8], context: &Context, base: &str, time: &str) -> Option<Value> {
    let row: Envelope<EventScope> = serde_json::from_slice(raw).ok()?;
    if row.kind != "event_msg" {
        return None;
    }
    let request_kind = match row.payload.kind.as_str() {
        "approval_request" => NativeRequestKind::Approval,
        "request_user_input" => NativeRequestKind::UserInput,
        _ => return None,
    };
    // A call ID, clock or ordinal cannot stand in for a native request ID.
    let request_id = opaque_operation(row.payload.request_id.as_deref()?).ok()?;
    if context.turn_id.is_empty()
        || row.payload.turn_id.as_deref() != Some(context.turn_id.as_str())
    {
        return None;
    }
    Some(fact(
        format!("{base}:native-request"),
        "native_request",
        time,
        context,
        json!(NativeRequest {
            request_id,
            request_kind,
            state: NativeRequestState::Requested
        }),
        None,
    ))
}
