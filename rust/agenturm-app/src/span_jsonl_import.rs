//! Offline import of structural spans from the native JSONL export format.

use crate::trace_import::{
    self, Attribute, MAX_EXPORT_BYTES, MAX_EXPORT_SPANS, OtlpSpan, Status, Timestamp,
};
#[cfg(target_os = "linux")]
use agenturm_kernel::ObservationStore;
use agenturm_kernel::{ExtensionId, Observation, ObservationKind};
use serde::de::{Error, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read};
use std::path::PathBuf;

pub(super) const USAGE: &str = concat!(
    "agenturm import-jsonl --file <span-jsonl-path|-> --db <path> ",
    "--source-id otlp.<id> [--session-id <id>]"
);
const COMMIT_ERROR: &str = "Cannot commit span JSONL. Check the Rust history, source ownership, and retained span identities.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpanJsonlImportOptions {
    file: PathBuf,
    db: PathBuf,
    source_id: ExtensionId,
    session_id: Option<String>,
}

pub(super) fn parse(args: &[String]) -> Result<SpanJsonlImportOptions, String> {
    let (pairs, remainder) = args.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(USAGE.into());
    }
    let (mut file, mut db, mut source_id, mut session_id) = (None, None, None, None);
    for [flag, value] in pairs {
        if value.is_empty() || value.starts_with("--") {
            return Err(USAGE.into());
        }
        match flag.as_str() {
            "--file" if file.is_none() => file = Some(PathBuf::from(value)),
            "--db" if db.is_none() && value != "-" && value != ":memory:" => {
                db = Some(PathBuf::from(value));
            }
            "--source-id" if source_id.is_none() => {
                source_id = Some(trace_import::parse_source_id(value)?);
            }
            "--session-id" if session_id.is_none() && trace_import::valid_session_id(value) => {
                session_id = Some(value.clone());
            }
            _ => return Err(USAGE.into()),
        }
    }
    Ok(SpanJsonlImportOptions {
        file: file.ok_or(USAGE)?,
        db: db.ok_or(USAGE)?,
        source_id: source_id.ok_or(USAGE)?,
        session_id,
    })
}

pub fn execute_span_jsonl_import(options: &SpanJsonlImportOptions) -> Result<String, String> {
    let observations = prepare(options)?;
    let inserted = trace_import::commit(&options.db, &options.source_id, &observations)
        .map_err(|_| COMMIT_ERROR)?;
    Ok(report(inserted, &observations))
}

#[cfg(target_os = "linux")]
pub(super) fn execute_with_store(
    options: &SpanJsonlImportOptions,
    store: &mut impl ObservationStore,
) -> Result<String, String> {
    let observations = prepare(options)?;
    let inserted = trace_import::commit_with_store(store, &options.source_id, &observations)
        .map_err(|_| COMMIT_ERROR)?;
    Ok(report(inserted, &observations))
}

fn report(inserted: usize, observations: &[Observation]) -> String {
    if let Some(evidence) = trace_import::metadata_evidence_summary(observations) {
        format!("Imported {inserted} observations from {evidence}.\n")
    } else {
        let projected = observations.len();
        format!("Imported {inserted} spans. Projected {projected} structural spans.\n")
    }
}

fn prepare(options: &SpanJsonlImportOptions) -> Result<Vec<Observation>, String> {
    let source = &options.source_id;
    let session = options.session_id.as_deref();
    if options.file.as_os_str() == "-" {
        decode(io::stdin().lock(), source, session)
    } else {
        decode(
            trace_import::open_regular_export(&options.file, "Span JSONL input")?,
            source,
            session,
        )
    }
}

fn decode(
    reader: impl Read,
    source: &ExtensionId,
    session: Option<&str>,
) -> Result<Vec<Observation>, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_EXPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read span JSONL input")?;
    if bytes.len() as u64 > MAX_EXPORT_BYTES {
        return Err("Span JSONL input exceeds 4 MiB".into());
    }
    let mut spans = Vec::new();
    for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if spans.len() == MAX_EXPORT_SPANS {
            return Err("Span JSONL input exceeds 1,024 records".into());
        }
        let span: JsonlSpan = serde_json::from_slice(line)
            .map_err(|_| format!("Invalid span JSONL record at line {}", index + 1))?;
        spans.push(span.into_otlp(session));
    }
    let (observations, unscoped) = trace_import::project(spans, source, None)?;
    if unscoped != 0 {
        return Err("Every span needs an explicit session identity or --session-id".into());
    }
    validate_ancestry(&observations)?;
    Ok(observations)
}

/// Missing parents are valid in a partial export. Cycles among rows present
/// in the same file cannot describe a trace tree.
fn validate_ancestry(observations: &[Observation]) -> Result<(), String> {
    let parents = observations
        .iter()
        .filter_map(|row| {
            let ObservationKind::TraceSpan(span) = &row.kind else {
                return None;
            };
            Some((
                (span.trace_id.as_str(), span.span_id.as_str()),
                span.parent_span_id.as_deref()?,
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let mut checked = BTreeSet::new();
    for origin in parents.keys().copied() {
        let mut pending = BTreeSet::new();
        let mut node = origin;
        while let Some(parent) = parents.get(&node) {
            if checked.contains(&node) {
                break;
            }
            if !pending.insert(node) {
                return Err("Span JSONL contains a trace parent cycle".into());
            }
            node = (node.0, parent);
        }
        checked.extend(pending);
    }
    Ok(())
}

#[derive(Deserialize)]
struct JsonlSpan {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    start_unix_nano: Timestamp,
    end_unix_nano: Timestamp,
    #[serde(default)]
    status: JsonlStatus,
    attributes: Option<JsonlAttributes>,
}

impl JsonlSpan {
    fn into_otlp(self, session: Option<&str>) -> OtlpSpan {
        let mut attributes = self.attributes.unwrap_or_default().0;
        if let Some(session) = session {
            attributes.push(Attribute::text("session.id", session.to_owned()));
        }
        OtlpSpan {
            trace_id: self.trace_id,
            span_id: self.span_id,
            parent_span_id: self.parent_span_id,
            start_time_unix_nano: self.start_unix_nano,
            end_time_unix_nano: self.end_unix_nano,
            status: Some(Status {
                code: Some(self.status as u8),
            }),
            attributes,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
enum JsonlStatus {
    #[default]
    Unset = 0,
    Ok = 1,
    Error = 2,
}

#[derive(Default)]
struct JsonlAttributes(Vec<Attribute>);

impl<'de> Deserialize<'de> for JsonlAttributes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KnownAttributes;

        impl<'de> Visitor<'de> for KnownAttributes {
            type Value = JsonlAttributes;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a span attribute object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut attributes = Vec::new();
                let mut known = BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    let scope_key = jsonl_identity_key(&key);
                    let metadata = trace_import::cost::is_cost_key(&key)
                        || trace_import::token_usage::is_token_key(&key);
                    if scope_key.is_none() && !metadata {
                        map.next_value::<IgnoredAny>()?;
                        continue;
                    }
                    let attribute = if let Some(target) = scope_key {
                        Attribute::text(target, map.next_value::<String>()?)
                    } else {
                        Attribute::raw_literal(&key, map.next_value::<Box<RawValue>>()?)
                    };
                    if !known.insert(key) {
                        return Err(A::Error::custom("duplicate recognized span attribute"));
                    }
                    attributes.push(attribute);
                }
                Ok(JsonlAttributes(attributes))
            }
        }

        deserializer.deserialize_map(KnownAttributes)
    }
}

fn jsonl_identity_key(key: &str) -> Option<&str> {
    match key {
        "session.id"
        | "gen_ai.conversation.id"
        | "agent.session_id"
        | "session_id"
        | "conversation_id"
        | "thread_id"
        | "langgraph.thread_id"
        | "langchain.session_id"
        | "cursor.conversation.id" => Some("session.id"),
        "agenturm.source.id" | "agenturm.turn.id" | "agenturm.worker.id" => Some(key),
        _ => None,
    }
}
