//! Pure, bounded lifecycle and usage reduction for admitted plugin snapshots.
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
    rows: Vec<Row>,
    #[serde(default)]
    invalidated: Vec<Invalidation>,
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
    payload: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Lifecycle,
    Usage,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Invalidation {
    native_session_id: String,
    last_record_ns: String,
}

enum Fact {
    Lifecycle { state: String, reason: String },
    Usage(BTreeMap<String, u64>),
}

struct Admitted {
    event_id_hash: String,
    revision: u32,
    time_ns: u64,
    admitted_ns: u64,
    native_session_id: Option<String>,
    fact: Fact,
}

#[derive(Default)]
struct SessionFacts {
    last_record_ns: u64,
    lifecycle: Vec<(u64, String, String)>,
    usage: BTreeMap<String, u128>,
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub schema_version: u32,
    pub sessions: Vec<Session>,
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
    }
}

impl Row {
    fn admit(self) -> Result<Admitted, ReduceError> {
        if self.event_id_hash.len() != 64
            || !self
                .event_id_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || self.revision > i32::MAX as u32
            || self
                .native_session_id
                .as_deref()
                .is_some_and(|native| !native.is_empty() && !identifier(native))
        {
            return Err(invalid("invalid admitted row identity or revision"));
        }
        Ok(Admitted {
            event_id_hash: self.event_id_hash,
            revision: self.revision,
            time_ns: decimal(&self.time_ns)?,
            admitted_ns: decimal(&self.admitted_ns)?,
            // Admission stores both absent and empty identity as the same
            // empty native_session_id. Corrections must compare that form.
            native_session_id: self.native_session_id.filter(|value| !value.is_empty()),
            fact: parse_fact(self.kind, self.payload)?,
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
    }
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
    let fixture: Fixture =
        serde_json::from_value(raw.0).map_err(|_| invalid("invalid fixture JSON"))?;
    if fixture.schema_version != 1
        || !identifier(&fixture.receiver_id)
        || !source_identifier(&fixture.source_id)
        || fixture.policy_epoch > i64::MAX as u64
        || fixture.dirty_seq > i64::MAX as u64
        || (fixture.dirty_seq == 0 && (!fixture.rows.is_empty() || !fixture.invalidated.is_empty()))
        || fixture.rows.len() > MAX_EVIDENCE
        || fixture.invalidated.len() > MAX_SESSIONS
    {
        return Err(invalid("invalid fixture header or capacity"));
    }
    let mut seen_revisions = HashSet::new();
    let mut latest = BTreeMap::<String, Admitted>::new();
    for row in fixture.rows {
        let row = row.admit()?;
        if !seen_revisions.insert((row.event_id_hash.clone(), row.revision)) {
            return Err(invalid("duplicate admitted revision"));
        }
        let old = latest.get(&row.event_id_hash);
        if old.is_some_and(|old| {
            old.native_session_id != row.native_session_id
                || std::mem::discriminant(&old.fact) != std::mem::discriminant(&row.fact)
        }) {
            return Err(invalid("event kind or identity changed across revisions"));
        }
        if old.is_none_or(|old| row.revision > old.revision) {
            latest.insert(row.event_id_hash.clone(), row);
        }
    }
    let mut sessions = BTreeMap::<String, SessionFacts>::new();
    for row in latest.into_values() {
        let Some(native_id) = row.native_session_id.filter(|value| !value.is_empty()) else {
            continue;
        };
        let entry = sessions.entry(native_id).or_default();
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
        }
    }
    if sessions.len() > MAX_SESSIONS {
        return Err(invalid("session capacity reached"));
    }
    let mut projected: BTreeMap<_, _> = sessions
        .into_iter()
        .map(|(native_id, facts)| {
            let row = output_session(
                &fixture.receiver_id,
                &fixture.source_id,
                native_id.clone(),
                facts,
            );
            (native_id, row)
        })
        .collect();
    let mut invalidated = HashSet::new();
    for item in fixture.invalidated {
        if !identifier(&item.native_session_id)
            || !invalidated.insert(item.native_session_id.clone())
        {
            return Err(invalid("invalid retention invalidation"));
        }
        let last_record_ns = decimal(&item.last_record_ns)?;
        let row = projected
            .entry(item.native_session_id.clone())
            .or_insert_with(|| Session {
                session_key: session_key(
                    &fixture.receiver_id,
                    &fixture.source_id,
                    &item.native_session_id,
                ),
                native_session_id: item.native_session_id,
                reported_state: String::new(),
                run_state: String::new(),
                reason_code: String::new(),
                state_time_ns: 0,
                last_record_ns,
                metadata_usage: BTreeMap::new(),
            });
        row.reported_state = "unknown".to_owned();
        row.run_state = "unknown".to_owned();
        row.reason_code = "retention_gap".to_owned();
        row.metadata_usage.clear();
    }
    if projected.len() > MAX_SESSIONS {
        return Err(invalid("session capacity reached"));
    }
    Ok(Output {
        schema_version: 1,
        sessions: projected.into_values().collect(),
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
