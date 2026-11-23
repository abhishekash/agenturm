//! Offline import of structural spans and opted-in text from native JSONL.

use crate::trace_import::{
    self, Attribute, MAX_EXPORT_BYTES, MAX_EXPORT_SPANS, OtlpSpan, PAYLOAD_INPUT, PAYLOAD_OUTPUT,
    Status, Timestamp,
};
use crate::trace_receiver::parse_payload_scope;
use agenturm_kernel::{ExtensionId, Observation, ObservationKind};
#[cfg(target_os = "linux")]
use agenturm_storage_service::NativeStore;
use agenturm_store::{
    MAX_TELEMETRY_PAYLOAD_BATCH_BYTES, TelemetryBinding, TelemetryPayload, TelemetryPayloadRecord,
};
use serde::de::{Error, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read};
use std::path::PathBuf;

pub(super) const USAGE: &str = concat!(
    "agenturm import-jsonl --file <span-jsonl-path|-> --db <path> ",
    "--source-id otlp.<id> [--session-id <id>] ",
    "[--payload-scope <standalone|harness-id> [--payload-format agent-harness]]"
);
const COMMIT_ERROR: &str = "Cannot commit span JSONL. Check the Rust history, source ownership, and retained span identities.";
pub(super) const PAYLOAD_BUDGET_ERROR: &str = "Span JSONL payload exceeds 64 KiB";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpanJsonlImportOptions {
    pub(super) file: PathBuf,
    pub(super) db: PathBuf,
    pub(super) source_id: ExtensionId,
    pub(super) session_id: Option<String>,
    pub(super) payload_binding: Option<TelemetryBinding>,
    pub(super) payload_format: PayloadFormat,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum PayloadFormat {
    #[default]
    Telemetry,
    AgentHarness,
}

pub(super) struct PreparedImport {
    pub(super) observations: Vec<Observation>,
    pub(super) payloads: Vec<TelemetryPayloadRecord>,
}

pub(super) fn parse(args: &[String]) -> Result<SpanJsonlImportOptions, String> {
    let (pairs, remainder) = args.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(USAGE.into());
    }
    let (mut file, mut db, mut source_id, mut session_id) = (None, None, None, None);
    let mut payload_scope = None;
    let mut payload_format = None;
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
            "--payload-scope" if payload_scope.is_none() => payload_scope = Some(value.as_str()),
            "--payload-format" if payload_format.is_none() && value == "agent-harness" => {
                payload_format = Some(PayloadFormat::AgentHarness);
            }
            _ => return Err(USAGE.into()),
        }
    }
    let source_id = source_id.ok_or(USAGE)?;
    let payload_binding = payload_scope
        .map(|scope| parse_payload_scope(&source_id, scope))
        .transpose()?;
    if payload_format.is_some() && payload_binding.is_none() {
        return Err(USAGE.into());
    }
    Ok(SpanJsonlImportOptions {
        file: file.ok_or(USAGE)?,
        db: db.ok_or(USAGE)?,
        source_id,
        session_id,
        payload_binding,
        payload_format: payload_format.unwrap_or_default(),
    })
}

pub fn execute_span_jsonl_import(options: &SpanJsonlImportOptions) -> Result<String, String> {
    let PreparedImport {
        observations,
        payloads,
    } = prepare(options)?;
    let inserted = match options.payload_binding.as_ref() {
        Some(binding) => {
            trace_import::commit_payloads(&options.db, binding, &observations, &payloads)
        }
        None => trace_import::commit(&options.db, &options.source_id, &observations),
    }
    .map_err(|_| COMMIT_ERROR)?;
    Ok(report(inserted, &observations))
}

#[cfg(target_os = "linux")]
pub(super) fn execute_with_store(
    options: &SpanJsonlImportOptions,
    store: &mut NativeStore,
) -> Result<String, String> {
    let PreparedImport {
        observations,
        payloads,
    } = prepare(options)?;
    if let Some(binding) = &options.payload_binding {
        store
            .check_telemetry_admission(binding)
            .map_err(|error| error.to_string())?;
    }
    let binding = options
        .payload_binding
        .as_ref()
        .filter(|_| !payloads.is_empty());
    let inserted = match binding {
        Some(binding) => {
            let batch = trace_import::prepare_payload_batch(store, binding, &observations)
                .map_err(|_| COMMIT_ERROR)?;
            store
                .commit_telemetry_batch(binding, batch, payloads)
                .map_err(|_| COMMIT_ERROR.to_owned())
        }
        None => trace_import::commit_with_store(store, &options.source_id, &observations),
    }
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

fn prepare(options: &SpanJsonlImportOptions) -> Result<PreparedImport, String> {
    if options.file.as_os_str() == "-" {
        decode(io::stdin().lock(), options)
    } else {
        decode(
            trace_import::open_regular_export(&options.file, "Span JSONL input")?,
            options,
        )
    }
}

fn decode(reader: impl Read, options: &SpanJsonlImportOptions) -> Result<PreparedImport, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_EXPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read span JSONL input")?;
    decode_bytes(&bytes, options)
}

pub(super) fn decode_bytes(
    bytes: &[u8],
    options: &SpanJsonlImportOptions,
) -> Result<PreparedImport, String> {
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
        let malformed = || format!("Invalid span JSONL record at line {}", index + 1);
        let span: JsonlSpan = serde_json::from_slice(line).map_err(|_| malformed())?;
        let mut span = span.into_otlp(options.session_id.as_deref());
        if options.payload_binding.is_some() {
            let payload = decode_payload(line, options.payload_format).map_err(|_| malformed())?;
            if let Some(input) = payload.input {
                span.attributes.push(Attribute::text(PAYLOAD_INPUT, input));
            }
            if let Some(output) = payload.output {
                span.attributes
                    .push(Attribute::text(PAYLOAD_OUTPUT, output));
            }
        }
        spans.push(span);
    }
    let (observations, unscoped, payloads) = trace_import::project_payloads(
        spans,
        &options.source_id,
        None,
        options.payload_binding.as_ref(),
    )?;
    if unscoped != 0 {
        return Err("Every span needs an explicit session identity or --session-id".into());
    }
    validate_ancestry(&observations)?;
    let payload_bytes: usize = payloads
        .iter()
        .flat_map(|record| [&record.payload.input, &record.payload.output])
        .filter_map(Option::as_ref)
        .map(String::len)
        .sum();
    if payload_bytes > MAX_TELEMETRY_PAYLOAD_BATCH_BYTES {
        return Err(PAYLOAD_BUDGET_ERROR.into());
    }
    Ok(PreparedImport {
        observations,
        payloads,
    })
}

fn decode_payload(line: &[u8], format: PayloadFormat) -> serde_json::Result<TelemetryPayload> {
    // Borrow the original attribute object. Decode only the two slots for the
    // selected format and span type, without retaining arbitrary attributes.
    #[derive(Deserialize)]
    struct PayloadSpan<'a> {
        #[serde(borrow)]
        name: Option<&'a RawValue>,
        #[serde(borrow)]
        attributes: Option<&'a RawValue>,
    }
    let span: PayloadSpan<'_> = serde_json::from_slice(line)?;
    let Some(attributes) = span.attributes else {
        return Ok(TelemetryPayload {
            input: None,
            output: None,
        });
    };
    match format {
        PayloadFormat::Telemetry => {
            let fields: CanonicalPayload = serde_json::from_str(attributes.get())?;
            Ok(TelemetryPayload {
                input: fields.input,
                output: fields.output,
            })
        }
        PayloadFormat::AgentHarness => match span
            .name
            .map(|name| serde_json::from_str::<LegacySpanName>(name.get()))
            .transpose()?
        {
            Some(LegacySpanName::AgentRun) => {
                let fields: LegacyRunPayload = serde_json::from_str(attributes.get())?;
                Ok(TelemetryPayload {
                    input: fields.task.filter(|text| !text.is_empty()),
                    output: fields.summary.filter(|text| !text.is_empty()),
                })
            }
            Some(LegacySpanName::ToolCall) => {
                let fields: LegacyToolPayload = serde_json::from_str(attributes.get())?;
                Ok(TelemetryPayload {
                    input: fields.arguments.filter(|text| !text.is_empty()),
                    output: fields.result.filter(|text| !text.is_empty()),
                })
            }
            _ => Ok(TelemetryPayload {
                input: None,
                output: None,
            }),
        },
    }
}

#[derive(Deserialize)]
struct CanonicalPayload {
    #[serde(
        default,
        rename = "agenturm.payload.input",
        deserialize_with = "present_text"
    )]
    input: Option<String>,
    #[serde(
        default,
        rename = "agenturm.payload.output",
        deserialize_with = "present_text"
    )]
    output: Option<String>,
}

#[derive(Deserialize)]
enum LegacySpanName {
    #[serde(rename = "agent.run")]
    AgentRun,
    #[serde(rename = "tool.call")]
    ToolCall,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct LegacyRunPayload {
    #[serde(default, rename = "agent.task", deserialize_with = "present_text")]
    task: Option<String>,
    #[serde(default, rename = "agent.summary", deserialize_with = "present_text")]
    summary: Option<String>,
}

#[derive(Deserialize)]
struct LegacyToolPayload {
    #[serde(default, rename = "tool.arguments", deserialize_with = "present_text")]
    arguments: Option<String>,
    #[serde(default, rename = "tool.result", deserialize_with = "present_text")]
    result: Option<String>,
}

fn present_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
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
        "agenturm.source.id"
        | "agenturm.turn.id"
        | "agenturm.worker.id"
        | trace_import::OPERATION_KEY => Some(key),
        _ => None,
    }
}
