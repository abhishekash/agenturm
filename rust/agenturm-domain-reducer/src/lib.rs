//! Pure, bounded domain reduction for admitted plugin snapshots.
//!
//! This crate reads a versioned test/interchange fixture. It has no Store,
//! filesystem, process, or policy authority and is not on the receiver path.

#![forbid(unsafe_code)]

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Number, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::error::Error;
use std::fmt;

pub const MAX_FIXTURE_BYTES: usize = 16 * 1024 * 1024;
const MAX_EVIDENCE: usize = 50_000;
const MAX_SESSIONS: usize = 2_000;
const MAX_NESTING: usize = 20;
const JSON_SAFE_INTEGER: u128 = (1_u128 << 53) - 1;

#[derive(Debug)]
pub struct ReduceError(&'static str);

impl fmt::Display for ReduceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for ReduceError {}

fn invalid(message: &'static str) -> ReduceError {
    ReduceError(message)
}

struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StrictVisitor;

        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = StrictValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("JSON without duplicate object keys")
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Bool(value)))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Number(Number::from(value))))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Number(Number::from(value))))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                Number::from_f64(value)
                    .map(|number| StrictValue(Value::Number(number)))
                    .ok_or_else(|| E::custom("non-finite JSON number"))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::String(value.to_owned())))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::String(value)))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<StrictValue>()? {
                    values.push(value.0);
                }
                Ok(StrictValue(Value::Array(values)))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some((key, value)) = map.next_entry::<String, StrictValue>()? {
                    if values.insert(key, value.0).is_some() {
                        return Err(<A::Error as de::Error>::custom("duplicate JSON key"));
                    }
                }
                Ok(StrictValue(Value::Object(values)))
            }
        }

        deserializer.deserialize_any(StrictVisitor)
    }
}

fn bounded_nesting(value: &Value, parent_depth: usize) -> bool {
    match value {
        Value::Array(items) => {
            parent_depth < MAX_NESTING
                && items
                    .iter()
                    .all(|item| bounded_nesting(item, parent_depth + 1))
        }
        Value::Object(items) => {
            parent_depth < MAX_NESTING
                && items
                    .values()
                    .all(|item| bounded_nesting(item, parent_depth + 1))
        }
        _ => true,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    schema_version: u32,
    receiver_id: String,
    source_id: String,
    policy_epoch: u64,
    dirty_seq: u64,
    #[serde(default)]
    plugin_id: Option<String>,
    rows: Vec<Row>,
    #[serde(default)]
    invalidated: Vec<Invalidation>,
    #[serde(default)]
    source_gaps: Option<Vec<Invalidation>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    event_id_hash: String,
    revision: u32,
    kind: Kind,
    time_ns: String,
    admitted_ns: String,
    native_session_id: Option<String>,
    #[serde(default)]
    native_worker_id: Option<String>,
    #[serde(default)]
    native_turn_id: Option<String>,
    #[serde(default)]
    native_operation_id: Option<String>,
    payload: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Lifecycle,
    Usage,
    Relationship,
    Message,
    Tool,
    Check,
    Diagnostic,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Invalidation {
    native_session_id: String,
    last_record_ns: String,
}

enum Fact {
    Lifecycle {
        state: String,
        reason: String,
    },
    Usage(BTreeMap<String, u64>),
    Relationship {
        parent: String,
        kind: String,
    },
    Activity {
        kind: &'static str,
        action: String,
        result: String,
    },
    Diagnostic,
}

struct Admitted {
    event_id_hash: String,
    revision: u32,
    time_ns: u64,
    admitted_ns: u64,
    native_session_id: Option<String>,
    native_worker_id: Option<String>,
    native_turn_id: Option<String>,
    native_operation_id: Option<String>,
    fact: Fact,
}

struct ToolCall {
    turn_id: Option<String>,
    category: String,
    time_ns: u64,
    admitted_ns: u64,
}

#[derive(Default)]
struct SessionFacts {
    last_record_ns: u64,
    lifecycle: Vec<(u64, String, String)>,
    usage: BTreeMap<String, u128>,
    relationships: Vec<(u64, String, String)>,
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub schema_version: u32,
    pub sessions: Vec<Session>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activities: Option<Vec<Value>>,
}

#[derive(Debug, Serialize)]
pub struct Session {
    pub native_session_id: String,
    pub session_key: String,
    pub reported_state: String,
    pub run_state: String,
    pub reason_code: String,
    pub state_time_ns: u64,
    pub last_record_ns: u64,
    pub metadata_usage: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_native_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relationship_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relationship_state: Option<String>,
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= 256
        && !value
            .chars()
            .any(|character| character < ' ' || character == '\u{7f}')
}

fn source_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn plugin_identifier(value: &str) -> bool {
    if value.is_empty() || value.len() > 256 || !value.as_bytes()[0].is_ascii_lowercase() {
        return false;
    }
    let mut separator = false;
    for byte in value.bytes() {
        if matches!(byte, b'.' | b'-') {
            if separator {
                return false;
            }
            separator = true;
        } else if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            separator = false;
        } else {
            return false;
        }
    }
    !separator
}

fn decimal(raw: &str) -> Result<u64, ReduceError> {
    if raw.is_empty()
        || raw.len() > 19
        || (raw.starts_with('0') && raw != "0")
        || !raw.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid("invalid nanosecond timestamp"));
    }
    let value = raw
        .parse::<u64>()
        .map_err(|_| invalid("invalid nanosecond timestamp"))?;
    if value > i64::MAX as u64 {
        return Err(invalid("nanosecond timestamp out of range"));
    }
    Ok(value)
}

fn reason_code(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 64
        && raw.as_bytes()[0].is_ascii_lowercase()
        && raw.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'.' | b'/' | b'-')
        })
}

fn native_identity(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 256
        && raw.as_bytes()[0].is_ascii_alphanumeric()
        && raw
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn one_of<'a>(
    fields: &'a serde_json::Map<String, Value>,
    key: &str,
    choices: &[&str],
) -> Result<&'a str, ReduceError> {
    let value = fields
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("missing typed metadata"))?;
    if !choices.contains(&value) {
        return Err(invalid("invalid typed metadata"));
    }
    Ok(value)
}

fn parse_fact(kind: Kind, payload: Value) -> Result<Fact, ReduceError> {
    let fields = payload
        .as_object()
        .ok_or_else(|| invalid("payload must be an object"))?;
    match kind {
        Kind::Lifecycle => {
            if fields.is_empty()
                || fields.len() > 2
                || fields
                    .keys()
                    .any(|key| key != "state" && key != "reason_code")
            {
                return Err(invalid("invalid lifecycle payload fields"));
            }
            let state = fields
                .get("state")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("missing lifecycle state"))?;
            if !matches!(
                state,
                "started"
                    | "running"
                    | "paused"
                    | "completed"
                    | "failed"
                    | "interrupted"
                    | "cancelled"
            ) {
                return Err(invalid("invalid lifecycle state"));
            }
            let reason = match fields.get("reason_code") {
                None => "",
                Some(value) => value
                    .as_str()
                    .filter(|value| reason_code(value))
                    .ok_or_else(|| invalid("invalid reason code"))?,
            };
            Ok(Fact::Lifecycle {
                state: state.to_owned(),
                reason: reason.to_owned(),
            })
        }
        Kind::Usage => {
            if fields.is_empty() || fields.len() > 5 {
                return Err(invalid("invalid usage payload fields"));
            }
            let mut counters = BTreeMap::new();
            for (key, value) in fields {
                if !matches!(
                    key.as_str(),
                    "input_tokens"
                        | "output_tokens"
                        | "cache_read_tokens"
                        | "cache_write_tokens"
                        | "total_tokens"
                ) {
                    return Err(invalid("invalid usage counter"));
                }
                let count = value
                    .as_u64()
                    .filter(|count| *count <= i64::MAX as u64)
                    .ok_or_else(|| invalid("invalid usage value"))?;
                counters.insert(key.clone(), count);
            }
            Ok(Fact::Usage(counters))
        }
        Kind::Relationship => {
            if fields.len() != 2
                || !fields.contains_key("parent_session_id")
                || !fields.contains_key("kind")
            {
                return Err(invalid("invalid relationship payload fields"));
            }
            let parent = fields
                .get("parent_session_id")
                .and_then(Value::as_str)
                .filter(|value| native_identity(value))
                .ok_or_else(|| invalid("invalid parent identity"))?;
            let relation = one_of(fields, "kind", &["delegated", "review"])?;
            Ok(Fact::Relationship {
                parent: parent.to_owned(),
                kind: relation.to_owned(),
            })
        }
        Kind::Message | Kind::Tool | Kind::Check => {
            let (name, first_key, first, second_key, second) = match kind {
                Kind::Message => ("message", "role", &["user", "assistant"][..], "", &[][..]),
                Kind::Tool => (
                    "tool",
                    "category",
                    &["shell", "file", "mcp", "other"][..],
                    "result",
                    &["started", "succeeded", "failed", "denied"][..],
                ),
                Kind::Check => (
                    "check",
                    "category",
                    &["test", "lint", "build", "review", "other"][..],
                    "result",
                    &["passed", "failed", "skipped", "unknown"][..],
                ),
                _ => unreachable!(),
            };
            if fields.len() != if second_key.is_empty() { 1 } else { 2 }
                || !fields.contains_key(first_key)
                || (!second_key.is_empty() && !fields.contains_key(second_key))
            {
                return Err(invalid("invalid activity payload fields"));
            }
            let action = one_of(fields, first_key, first)?.to_owned();
            let result = if second_key.is_empty() {
                "observed".to_owned()
            } else {
                one_of(fields, second_key, second)?.to_owned()
            };
            Ok(Fact::Activity {
                kind: name,
                action,
                result,
            })
        }
        Kind::Diagnostic => {
            if fields.len() != 2 || !fields.contains_key("code") || !fields.contains_key("severity")
            {
                return Err(invalid("invalid diagnostic payload fields"));
            }
            fields
                .get("code")
                .and_then(Value::as_str)
                .filter(|value| reason_code(value))
                .ok_or_else(|| invalid("invalid diagnostic code"))?;
            one_of(fields, "severity", &["info", "warning", "error"])?;
            Ok(Fact::Diagnostic)
        }
    }
}

impl Row {
    fn admit(self, schema_version: u32) -> Result<Admitted, ReduceError> {
        if self.event_id_hash.len() != 64
            || !self
                .event_id_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || self.revision > i32::MAX as u32
            || self.native_session_id.as_deref().is_some_and(|native| {
                !native.is_empty()
                    && if schema_version >= 2 {
                        !native_identity(native)
                    } else {
                        !identifier(native)
                    }
            })
            || self
                .native_worker_id
                .as_deref()
                .is_some_and(|value| !value.is_empty() && !native_identity(value))
            || self
                .native_turn_id
                .as_deref()
                .is_some_and(|value| !value.is_empty() && !native_identity(value))
            || self
                .native_operation_id
                .as_deref()
                .is_some_and(|value| !value.is_empty() && !native_identity(value))
        {
            return Err(invalid("invalid admitted row identity or revision"));
        }
        let fact = parse_fact(self.kind, self.payload)?;
        if (matches!(fact, Fact::Relationship { .. } | Fact::Activity { .. })
            && self.native_session_id.as_deref().is_none_or(str::is_empty))
            || (self
                .native_operation_id
                .as_deref()
                .is_some_and(|value| !value.is_empty())
                && !matches!(fact, Fact::Activity { kind: "tool", .. }))
        {
            return Err(invalid("invalid fact identity"));
        }
        Ok(Admitted {
            event_id_hash: self.event_id_hash,
            revision: self.revision,
            time_ns: decimal(&self.time_ns)?,
            admitted_ns: decimal(&self.admitted_ns)?,
            // Admission stores both absent and empty identity as the same
            // empty native_session_id. Corrections must compare that form.
            native_session_id: self.native_session_id.filter(|value| !value.is_empty()),
            native_worker_id: self.native_worker_id.filter(|value| !value.is_empty()),
            native_turn_id: self.native_turn_id.filter(|value| !value.is_empty()),
            native_operation_id: self.native_operation_id.filter(|value| !value.is_empty()),
            fact,
        })
    }
}

fn session_key(receiver_id: &str, source_id: &str, native_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(receiver_id);
    hasher.update([0]);
    hasher.update(source_id);
    hasher.update([0]);
    hasher.update(native_id);
    format!("plugin:{:x}", hasher.finalize())
}

fn reported_run_state(state: &str) -> &str {
    match state {
        "started" => "running",
        "paused" => "waiting",
        "cancelled" => "interrupted",
        other => other,
    }
}

fn output_session(
    receiver_id: &str,
    source_id: &str,
    native_id: String,
    facts: SessionFacts,
    relationship: Option<(String, String, String)>,
) -> Session {
    let state_time_ns = facts.lifecycle.iter().map(|item| item.0).max().unwrap_or(0);
    let latest: BTreeSet<_> = facts
        .lifecycle
        .iter()
        .filter(|item| item.0 == state_time_ns)
        .map(|item| (&item.1, &item.2))
        .collect();
    let (state, reason) = if latest.is_empty() {
        ("unknown".to_owned(), String::new())
    } else if latest.len() == 1 {
        let (state, reason) = latest.into_iter().next().expect("one lifecycle");
        (state.to_owned(), reason.to_owned())
    } else {
        ("unknown".to_owned(), "ambiguous_lifecycle".to_owned())
    };
    let metadata_usage = facts
        .usage
        .into_iter()
        .map(|(key, value)| {
            let value = if value <= JSON_SAFE_INTEGER {
                Value::Number(Number::from(value as u64))
            } else {
                Value::String(value.to_string())
            };
            (key, value)
        })
        .collect();
    Session {
        session_key: session_key(receiver_id, source_id, &native_id),
        native_session_id: native_id,
        run_state: reported_run_state(&state).to_owned(),
        reported_state: state,
        reason_code: reason,
        state_time_ns,
        last_record_ns: facts.last_record_ns,
        metadata_usage,
        parent_native_session_id: relationship.as_ref().map(|item| item.0.clone()),
        relationship_kind: relationship.as_ref().map(|item| item.1.clone()),
        relationship_state: relationship.as_ref().map(|item| item.2.clone()),
    }
}

fn relationships(
    sessions: &BTreeMap<String, SessionFacts>,
) -> BTreeMap<String, (String, String, String)> {
    let mut chosen = BTreeMap::new();
    for (native_id, facts) in sessions {
        let Some(newest) = facts.relationships.iter().map(|row| row.0).max() else {
            chosen.insert(
                native_id.clone(),
                (String::new(), "unknown".to_owned(), "unresolved".to_owned()),
            );
            continue;
        };
        let latest: BTreeSet<_> = facts
            .relationships
            .iter()
            .filter(|row| row.0 == newest)
            .map(|row| (&row.1, &row.2))
            .collect();
        if latest.len() != 1 {
            chosen.insert(
                native_id.clone(),
                (String::new(), "unknown".to_owned(), "conflicted".to_owned()),
            );
        } else {
            let (parent, kind) = latest.into_iter().next().expect("one relation");
            chosen.insert(
                native_id.clone(),
                (parent.clone(), kind.clone(), "unresolved".to_owned()),
            );
        }
    }
    for native_id in sessions.keys() {
        let (parent, _, state) = chosen.get(native_id).expect("chosen relation");
        if parent.is_empty() || state == "conflicted" {
            continue;
        }
        let parent = parent.clone();
        let mut seen = HashSet::from([native_id.as_str()]);
        let mut cursor = parent.as_str();
        while sessions.contains_key(cursor) && !seen.contains(cursor) {
            seen.insert(cursor);
            cursor = chosen.get(cursor).expect("chosen parent").0.as_str();
        }
        let state = if seen.contains(cursor) {
            "invalid"
        } else if sessions.contains_key(&parent) {
            "confirmed"
        } else {
            "unresolved"
        };
        chosen.get_mut(native_id).expect("chosen relation").2 = state.to_owned();
    }
    chosen
}

fn opaque(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for (index, part) in parts.iter().enumerate() {
        if index != 0 {
            hasher.update([0]);
        }
        hasher.update(part.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn activity_fact(
    row: &Admitted,
    receiver_id: &str,
    source_id: &str,
    plugin_id: &str,
    calls: &BTreeMap<(String, String), Vec<ToolCall>>,
    correlate_tool_results: bool,
) -> Option<Value> {
    let Fact::Activity {
        kind,
        action,
        result,
    } = &row.fact
    else {
        return None;
    };
    let native = row.native_session_id.as_deref()?;
    let mut action = action.clone();
    let mut native_turn = row.native_turn_id.clone().unwrap_or_default();
    let mut relation = None;
    let mut category_conflict = false;
    if correlate_tool_results
        && *kind == "tool"
        && result != "started"
        && let Some(operation) = row.native_operation_id.as_deref()
    {
        let matches = calls.get(&(native.to_owned(), operation.to_owned()));
        match matches.map_or(0, Vec::len) {
            1 => {
                let call = &matches.expect("one call")[0];
                let call_turn = call.turn_id.as_deref().unwrap_or("");
                if !call_turn.is_empty()
                    && call.time_ns < row.time_ns
                    && call.admitted_ns <= row.admitted_ns
                    && (native_turn.is_empty() || native_turn == call_turn)
                {
                    native_turn = call_turn.to_owned();
                    relation = Some("matched_call");
                    if action == "other" {
                        action.clone_from(&call.category);
                    } else if action != call.category && call.category != "other" {
                        action = "other".to_owned();
                        category_conflict = true;
                    }
                } else {
                    native_turn.clear();
                    relation = Some(
                        if call.time_ns > row.time_ns || call.admitted_ns > row.admitted_ns {
                            "future_call"
                        } else if call.time_ns == row.time_ns {
                            "unproven_order"
                        } else if call_turn.is_empty() {
                            "call_turn_unavailable"
                        } else {
                            "conflicting_turn"
                        },
                    );
                }
            }
            0 => {
                if !native_turn.is_empty() && plugin_id != "example.rust-pi-harness" {
                    relation = Some("unverified_native_turn");
                } else {
                    native_turn.clear();
                    relation = Some("missing_call");
                }
            }
            _ => {
                native_turn.clear();
                relation = Some("ambiguous_call");
            }
        }
    }
    let event_key = format!(
        "plugin-event:{}",
        opaque(&[receiver_id, source_id, &row.event_id_hash])
    );
    let session_key = session_key(receiver_id, source_id, native);
    let agent_material = opaque(&[plugin_id, &format!("plugin:{source_id}"), native, ""]);
    let agent_id = format!("ag1_{}", &agent_material[..32]);
    let turn_id = if native_turn.is_empty() {
        String::new()
    } else {
        opaque(&["plugin-turn", receiver_id, source_id, native, &native_turn])[..32].to_owned()
    };
    let operation_key = if *kind == "tool" {
        row.native_operation_id.as_deref().map(|operation| {
            format!(
                "plugin-operation:{}",
                opaque(&[receiver_id, source_id, native, operation])
            )
        })
    } else {
        None
    };
    let subject = if *kind == "message" {
        format!(
            "{} message recorded",
            if action == "user" {
                "User"
            } else {
                "Assistant"
            }
        )
    } else {
        let capital = format!("{}{}", action[..1].to_ascii_uppercase(), &action[1..]);
        format!(
            "{} {} recorded",
            capital,
            if *kind == "check" {
                "check"
            } else {
                "operation"
            }
        )
    };
    let mut fact = serde_json::json!({
        "activity_scope_version": 1, "id": event_key, "time_unix_nano": row.time_ns,
        "domain": kind, "action": action, "subject": subject, "result": result,
        "actor": plugin_id, "agent_id": agent_id, "session_id": session_key,
        "service": plugin_id, "source": "plugin", "source_id": event_key,
        "span_id": null, "trace_id": null, "turn_id": turn_id,
        "operation_id": operation_key, "detail": "", "mcp_server": "", "mcp_tool": "",
        "duration_ms": null,
        "metadata": {"plugin_source_id": source_id,
            "native_worker_observed": row.native_worker_id.is_some(),
            "native_turn_observed": row.native_turn_id.is_some(),
            "native_operation_observed": row.native_operation_id.is_some(),
            "capture": "content-free", "coverage": "partial"},
        "capture_content": false, "detail_present": false, "operation_present": false
    });
    if let Some(relation) = relation {
        fact["metadata"]["tool_turn_relation"] = Value::String(relation.to_owned());
    }
    if category_conflict {
        fact["metadata"]["tool_category_conflict"] = Value::Bool(true);
    }
    Some(fact)
}

/// Reduce a fixture without opening a database or writing any file.
pub fn reduce_json(input: &str) -> Result<Output, ReduceError> {
    if input.len() > MAX_FIXTURE_BYTES {
        return Err(invalid("fixture exceeds byte limit"));
    }
    let raw: StrictValue =
        serde_json::from_str(input).map_err(|_| invalid("invalid fixture JSON"))?;
    if !bounded_nesting(&raw.0, 0) {
        return Err(invalid("fixture nesting exceeds limit"));
    }
    let has_source_gaps = raw
        .0
        .as_object()
        .is_some_and(|fields| fields.contains_key("source_gaps"));
    let fixture: Fixture =
        serde_json::from_value(raw.0).map_err(|_| invalid("invalid fixture JSON"))?;
    if !matches!(fixture.schema_version, 1..=3)
        || !identifier(&fixture.receiver_id)
        || !source_identifier(&fixture.source_id)
        || (fixture.schema_version >= 2
            && fixture
                .plugin_id
                .as_deref()
                .is_none_or(|id| !plugin_identifier(id)))
        || (fixture.schema_version == 1 && fixture.plugin_id.is_some())
        || (fixture.schema_version == 3 && fixture.source_gaps.is_none())
        || (fixture.schema_version < 3 && has_source_gaps)
        || fixture.policy_epoch > i64::MAX as u64
        || fixture.dirty_seq > i64::MAX as u64
        || (fixture.dirty_seq == 0
            && (!fixture.rows.is_empty()
                || !fixture.invalidated.is_empty()
                || fixture
                    .source_gaps
                    .as_ref()
                    .is_some_and(|items| !items.is_empty())))
        || fixture.rows.len() > MAX_EVIDENCE
        || fixture.invalidated.len() > MAX_SESSIONS
        || fixture
            .source_gaps
            .as_ref()
            .is_some_and(|items| items.len() > MAX_SESSIONS)
    {
        return Err(invalid("invalid fixture header or capacity"));
    }
    let mut seen_revisions = HashSet::new();
    let mut latest = BTreeMap::<String, Admitted>::new();
    for row in fixture.rows {
        if fixture.schema_version == 1
            && (row.native_worker_id.is_some()
                || row.native_turn_id.is_some()
                || row.native_operation_id.is_some())
        {
            return Err(invalid("new identity requires fixture version 2"));
        }
        let row = row.admit(fixture.schema_version)?;
        if fixture.schema_version == 1
            && matches!(
                row.fact,
                Fact::Relationship { .. } | Fact::Activity { .. } | Fact::Diagnostic
            )
        {
            return Err(invalid("new fact requires fixture version 2"));
        }
        if !seen_revisions.insert((row.event_id_hash.clone(), row.revision)) {
            return Err(invalid("duplicate admitted revision"));
        }
        let old = latest.get(&row.event_id_hash);
        if old.is_some_and(|old| {
            old.native_session_id != row.native_session_id
                || old.native_worker_id != row.native_worker_id
                || old.native_turn_id != row.native_turn_id
                || old.native_operation_id != row.native_operation_id
                || std::mem::discriminant(&old.fact) != std::mem::discriminant(&row.fact)
                || matches!((&old.fact, &row.fact),
                    (Fact::Activity { kind: left, .. }, Fact::Activity { kind: right, .. })
                    if left != right)
        }) {
            return Err(invalid("event kind or identity changed across revisions"));
        }
        if old.is_none_or(|old| row.revision > old.revision) {
            latest.insert(row.event_id_hash.clone(), row);
        }
    }
    let mut calls = BTreeMap::<(String, String), Vec<ToolCall>>::new();
    if fixture.schema_version >= 3 {
        for row in latest.values() {
            if let (
                Some(native),
                Some(operation),
                Fact::Activity {
                    kind: "tool",
                    action: category,
                    result,
                },
            ) = (
                row.native_session_id.as_deref(),
                row.native_operation_id.as_deref(),
                &row.fact,
            ) && result == "started"
            {
                calls
                    .entry((native.to_owned(), operation.to_owned()))
                    .or_default()
                    .push(ToolCall {
                        turn_id: row.native_turn_id.clone(),
                        category: category.clone(),
                        time_ns: row.time_ns,
                        admitted_ns: row.admitted_ns,
                    });
            }
        }
    }
    let mut sessions = BTreeMap::<String, SessionFacts>::new();
    let mut activity_candidates = Vec::<(String, Value)>::new();
    for row in latest.into_values() {
        let Some(native_id) = row.native_session_id.as_deref() else {
            continue;
        };
        if fixture.schema_version >= 2
            && let Some(activity) = activity_fact(
                &row,
                &fixture.receiver_id,
                &fixture.source_id,
                fixture.plugin_id.as_deref().expect("v2+ plugin ID"),
                &calls,
                fixture.schema_version >= 3,
            )
        {
            activity_candidates.push((native_id.to_owned(), activity));
        }
        let entry = sessions.entry(native_id.to_owned()).or_default();
        entry.last_record_ns = entry.last_record_ns.max(row.admitted_ns);
        match row.fact {
            Fact::Lifecycle { state, reason } => {
                entry.lifecycle.push((row.time_ns, state, reason));
            }
            Fact::Usage(counters) => {
                for (key, count) in counters {
                    let sum = entry.usage.entry(key).or_default();
                    *sum = sum
                        .checked_add(u128::from(count))
                        .ok_or_else(|| invalid("usage aggregate overflow"))?;
                }
            }
            Fact::Relationship { parent, kind } => {
                entry.relationships.push((row.time_ns, parent, kind))
            }
            Fact::Activity { .. } => {}
            Fact::Diagnostic => {}
        }
    }
    if sessions.len() > MAX_SESSIONS {
        return Err(invalid("session capacity reached"));
    }
    let chosen = if fixture.schema_version >= 2 {
        relationships(&sessions)
    } else {
        BTreeMap::new()
    };
    let mut projected: BTreeMap<_, _> = sessions
        .into_iter()
        .map(|(native_id, facts)| {
            let row = output_session(
                &fixture.receiver_id,
                &fixture.source_id,
                native_id.clone(),
                facts,
                chosen.get(&native_id).cloned(),
            );
            (native_id, row)
        })
        .collect();
    let mut invalidated = BTreeMap::new();
    for item in fixture.invalidated {
        if !identifier(&item.native_session_id) {
            return Err(invalid("invalid retention invalidation"));
        }
        let last_record_ns = decimal(&item.last_record_ns)?;
        if invalidated
            .insert(item.native_session_id, last_record_ns)
            .is_some()
        {
            return Err(invalid("duplicate retention invalidation"));
        }
    }
    let mut source_gaps = BTreeMap::new();
    for item in fixture.source_gaps.unwrap_or_default() {
        let last_record_ns = decimal(&item.last_record_ns)?;
        if (item.native_session_id != "*" && !native_identity(&item.native_session_id))
            || (item.native_session_id == "*" && last_record_ns != 0)
            || source_gaps
                .insert(item.native_session_id, last_record_ns)
                .is_some()
        {
            return Err(invalid("invalid source-gap invalidation"));
        }
    }
    let source_wide_gap = source_gaps.remove("*").is_some();
    if source_wide_gap && !source_gaps.is_empty() {
        return Err(invalid("mixed source-wide and session source gaps"));
    }
    let missing: BTreeSet<_> = invalidated
        .keys()
        .chain(source_gaps.keys())
        .cloned()
        .collect();
    for native_id in missing {
        let last_record_ns = source_gaps
            .get(&native_id)
            .or_else(|| invalidated.get(&native_id))
            .copied()
            .expect("invalidation entry");
        projected
            .entry(native_id.clone())
            .or_insert_with(|| Session {
                session_key: session_key(&fixture.receiver_id, &fixture.source_id, &native_id),
                native_session_id: native_id,
                reported_state: String::new(),
                run_state: String::new(),
                reason_code: String::new(),
                state_time_ns: 0,
                last_record_ns,
                metadata_usage: BTreeMap::new(),
                parent_native_session_id: (fixture.schema_version >= 2).then(String::new),
                relationship_kind: (fixture.schema_version >= 2).then(|| "unknown".to_owned()),
                relationship_state: (fixture.schema_version >= 2).then(|| "unresolved".to_owned()),
            });
    }
    for (native_id, row) in &mut projected {
        let reason = if invalidated.contains_key(native_id) {
            "retention_gap"
        } else if source_wide_gap || source_gaps.contains_key(native_id) {
            "source_gap"
        } else {
            continue;
        };
        row.reported_state = "unknown".to_owned();
        row.run_state = "unknown".to_owned();
        row.reason_code = reason.to_owned();
        row.metadata_usage.clear();
        if fixture.schema_version >= 2 {
            row.parent_native_session_id = Some(String::new());
            row.relationship_kind = Some("unknown".to_owned());
            row.relationship_state = Some("unresolved".to_owned());
        }
    }
    if projected.len() > MAX_SESSIONS {
        return Err(invalid("session capacity reached"));
    }
    let activities = (fixture.schema_version >= 2).then(|| {
        let mut facts: Vec<_> = activity_candidates
            .into_iter()
            .filter(|(native, _)| {
                !source_wide_gap
                    && !invalidated.contains_key(native)
                    && !source_gaps.contains_key(native)
            })
            .map(|(_, fact)| fact)
            .collect();
        facts.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
        facts
    });
    Ok(Output {
        schema_version: fixture.schema_version,
        sessions: projected.into_values().collect(),
        activities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_golden_fixtures_match_and_invalid_inputs_fail() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/rust-domain-reducer-v1.json"
        ))
        .unwrap();
        assert_eq!(corpus["fixture_version"], 1);
        for case in corpus["cases"].as_array().unwrap() {
            let input = case["raw_input"]
                .as_str()
                .map_or_else(|| case["input"].to_string(), str::to_owned);
            let reduced = reduce_json(&input);
            if let Some(expected) = case.get("expected") {
                let actual = serde_json::to_value(reduced.unwrap()).unwrap();
                assert_eq!(actual, *expected, "case {}", case["name"]);
            } else {
                assert!(reduced.is_err(), "accepted case {}", case["name"]);
            }
        }
    }

    #[test]
    fn relationship_and_activity_goldens_match_and_invalid_inputs_fail() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/rust-domain-reducer-v2.json"
        ))
        .unwrap();
        assert_eq!(corpus["fixture_version"], 2);
        for case in corpus["cases"].as_array().unwrap() {
            let reduced = reduce_json(&case["input"].to_string());
            if let Some(expected) = case.get("expected") {
                let actual = serde_json::to_value(reduced.unwrap()).unwrap();
                assert_eq!(actual, *expected, "case {}", case["name"]);
            } else {
                assert!(reduced.is_err(), "accepted case {}", case["name"]);
            }
        }
    }

    #[test]
    fn source_gap_goldens_match_and_invalid_inputs_fail() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/rust-domain-reducer-v3.json"
        ))
        .unwrap();
        assert_eq!(corpus["fixture_version"], 3);
        for case in corpus["cases"].as_array().unwrap() {
            let reduced = reduce_json(&case["input"].to_string());
            if let Some(expected) = case.get("expected") {
                let actual = serde_json::to_value(reduced.unwrap()).unwrap();
                assert_eq!(actual, *expected, "case {}", case["name"]);
            } else {
                assert!(reduced.is_err(), "accepted case {}", case["name"]);
            }
        }
    }

    #[test]
    fn source_gap_capacity_is_bounded() {
        let gaps: Vec<_> = (0..=MAX_SESSIONS)
            .map(|index| {
                serde_json::json!({"native_session_id": format!("session-{index:04}"),
                    "last_record_ns": "1"})
            })
            .collect();
        let fixture = serde_json::json!({"schema_version":3,"receiver_id":"receiver-fixture",
            "source_id":"source-a","plugin_id":"fixture.plugin","policy_epoch":1,
            "dirty_seq":1,"rows":[],"invalidated":[],"source_gaps":gaps});
        assert!(reduce_json(&fixture.to_string()).is_err());
    }

    #[test]
    fn input_byte_limit_precedes_json_parse() {
        let oversized = " ".repeat(MAX_FIXTURE_BYTES + 1);
        assert_eq!(
            reduce_json(&oversized).unwrap_err().to_string(),
            "fixture exceeds byte limit"
        );
    }

    #[test]
    fn duplicate_keys_and_deep_nesting_fail_before_fixture_validation() {
        assert!(serde_json::from_str::<StrictValue>(r#"{"a":1,"a":2}"#).is_err());
        let nested = format!("{}0{}", "[".repeat(21), "]".repeat(21));
        let value: StrictValue = serde_json::from_str(&nested).unwrap();
        assert!(!bounded_nesting(&value.0, 0));
        assert!(bounded_nesting(
            &serde_json::json!({"rows":[{"payload":{}}]}),
            0
        ));
    }

    #[test]
    fn session_capacity_is_bounded_beyond_display_default() {
        let fixture = |count| {
            let rows: Vec<_> = (0..count)
                .map(|index| {
                    serde_json::json!({
                        "event_id_hash": format!("{:x}", Sha256::digest(format!("event-{index}").as_bytes())),
                        "revision": 0, "kind": "lifecycle", "time_ns": (index + 1).to_string(),
                        "admitted_ns": (index + 1).to_string(),
                        "native_session_id": format!("session-{index:04}"),
                        "payload": {"state": "completed"}
                    })
                })
                .collect();
            serde_json::json!({"schema_version":1,"receiver_id":"receiver-fixture",
                "source_id":"source-a","policy_epoch":1,"dirty_seq":1,
                "rows":rows,"invalidated":[]})
            .to_string()
        };
        assert_eq!(reduce_json(&fixture(201)).unwrap().sessions.len(), 201);
        assert_eq!(reduce_json(&fixture(2000)).unwrap().sessions.len(), 2000);
        assert_eq!(
            reduce_json(&fixture(2001)).unwrap_err().to_string(),
            "session capacity reached"
        );
    }
}
