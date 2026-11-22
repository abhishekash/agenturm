//! Explicit, bounded import of structural OTLP JSON trace evidence.

use agenturm_kernel::{
    ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationStore, TraceSpan,
    TraceStatus,
};
use agenturm_store::Store;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub(super) const MAX_EXPORT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_EXPORT_SPANS: usize = 1_024;
const CURSOR_PREFIX: &str = "otlp-json/1:";
pub(super) const TRACE_IMPORT_USAGE: &str =
    "agenturm import-traces --file <otlp-json-path|-> --db <path> --source-id otlp.<id>";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceImportOptions {
    file: PathBuf,
    db: PathBuf,
    source_id: ExtensionId,
}

pub(super) fn parse(args: &[String]) -> Result<TraceImportOptions, String> {
    if args.len() != 6 {
        return Err(TRACE_IMPORT_USAGE.into());
    }
    let (mut file, mut db, mut source_id) = (None, None, None);
    for pair in args.as_chunks::<2>().0 {
        let value = &pair[1];
        if value.is_empty() || value.starts_with("--") {
            return Err(TRACE_IMPORT_USAGE.into());
        }
        match pair[0].as_str() {
            "--file" if file.is_none() => file = Some(PathBuf::from(value)),
            "--db" if db.is_none() && value != "-" && value != ":memory:" => {
                db = Some(PathBuf::from(value));
            }
            "--source-id" if source_id.is_none() && value.starts_with("otlp.") => {
                source_id = Some(parse_source_id(value)?);
            }
            _ => return Err(TRACE_IMPORT_USAGE.into()),
        }
    }
    Ok(TraceImportOptions {
        file: file.ok_or(TRACE_IMPORT_USAGE)?,
        db: db.ok_or(TRACE_IMPORT_USAGE)?,
        source_id: source_id.ok_or(TRACE_IMPORT_USAGE)?,
    })
}

pub(super) fn parse_source_id(value: &str) -> Result<ExtensionId, String> {
    if !value.starts_with("otlp.") {
        return Err("OTLP source identifier must start with otlp.".into());
    }
    ExtensionId::parse(value).map_err(|_| "invalid OTLP source identifier".to_owned())
}

pub fn execute_trace_import(options: &TraceImportOptions) -> Result<String, String> {
    let (observations, unscoped) = if options.file.as_os_str() == "-" {
        prepare(io::stdin().lock(), &options.source_id)?
    } else {
        let metadata = options
            .file
            .metadata()
            .map_err(|error| format!("cannot open OTLP export: {error}"))?;
        if !metadata.is_file() || metadata.len() > MAX_EXPORT_BYTES {
            return Err("OTLP export must be a regular file of at most 4 MiB".into());
        }
        let file = File::open(&options.file)
            .map_err(|error| format!("cannot open OTLP export: {error}"))?;
        prepare(file, &options.source_id)?
    };
    let inserted = commit(&options.db, &options.source_id, &observations)?;
    Ok(format!(
        "Imported {inserted} spans. Skipped {unscoped} without session identity.\n"
    ))
}

pub(super) fn commit(
    db: &Path,
    source_id: &ExtensionId,
    observations: &[Observation],
) -> Result<usize, String> {
    if observations.is_empty() {
        return Ok(0);
    }
    let cursor = import_cursor(observations)?;
    let mut store =
        Store::open_collector(db).map_err(|error| format!("open history failed: {error}"))?;
    let source = source_id.as_str();
    let prior = ObservationStore::source_cursor(&store, source)?;
    if prior
        .as_deref()
        .is_some_and(|checkpoint| !checkpoint.starts_with(CURSOR_PREFIX))
    {
        return Err("OTLP source identifier already belongs to another collector".into());
    }
    if prior.as_deref() == Some(cursor.as_str()) {
        Ok(0)
    } else {
        ObservationStore::commit(&mut store, source, prior.as_deref(), &cursor, observations)
            .map(|receipt| receipt.inserted)
    }
}

fn import_cursor(observations: &[Observation]) -> Result<String, String> {
    let mut hasher = Sha256::new();
    for row in observations {
        let ObservationKind::TraceSpan(span) = &row.kind else {
            return Err("invalid trace import observation".into());
        };
        let serial = serde_json::to_vec(&(
            row.identity.session_id.as_deref(),
            row.observed_at_unix_ns,
            span,
        ))
        .map_err(|_| "cannot encode trace import metadata".to_owned())?;
        hasher.update(serial);
        hasher.update([0]);
    }
    Ok(format!("{CURSOR_PREFIX}{:x}", hasher.finalize()))
}

fn prepare(
    reader: impl Read,
    source_id: &ExtensionId,
) -> Result<(Vec<Observation>, usize), String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_EXPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read OTLP export: {error}"))?;
    decode(&bytes, source_id)
}

pub(super) fn decode(
    bytes: &[u8],
    source_id: &ExtensionId,
) -> Result<(Vec<Observation>, usize), String> {
    if bytes.len() as u64 > MAX_EXPORT_BYTES {
        return Err("OTLP export exceeds 4 MiB".into());
    }
    let export: Export =
        serde_json::from_slice(bytes).map_err(|_| "invalid OTLP JSON trace export".to_owned())?;
    let mut observations = BTreeMap::new();
    let mut unscoped = 0;
    for (index, span) in export
        .resource_spans
        .into_iter()
        .flat_map(|resource| resource.scope_spans)
        .flat_map(|scope| scope.spans)
        .enumerate()
    {
        if index >= MAX_EXPORT_SPANS {
            return Err("OTLP export exceeds 1024 spans".into());
        }
        let Some(session_id) = session_id(span.attributes)? else {
            unscoped += 1;
            continue;
        };
        let ended_at = span
            .end_time_unix_nano
            .value()
            .ok_or("invalid span end time")?;
        let native = TraceSpan {
            trace_id: span.trace_id.to_ascii_lowercase(),
            span_id: span.span_id.to_ascii_lowercase(),
            parent_span_id: span
                .parent_span_id
                .filter(|id| !id.is_empty())
                .map(|id| id.to_ascii_lowercase()),
            started_at_unix_ns: span
                .start_time_unix_nano
                .value()
                .ok_or("invalid span start time")?,
            status: match span.status.and_then(|status| status.code).unwrap_or(0) {
                0 => TraceStatus::Unset,
                1 => TraceStatus::Ok,
                2 => TraceStatus::Error,
                _ => return Err("invalid OTLP span status code".into()),
            },
        };
        if !native.is_valid(ended_at) {
            return Err("invalid OTLP span identity or timing".into());
        }
        let event_id = format!("otlp:{}:{}", native.trace_id, native.span_id);
        let observation = Observation {
            harness_id: source_id.clone(),
            event_id: event_id.clone(),
            revision: 0,
            identity: ObservationIdentity::session(session_id),
            observed_at_unix_ns: ended_at,
            kind: ObservationKind::TraceSpan(native),
        };
        match observations.entry(event_id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(observation);
            }
            std::collections::btree_map::Entry::Occupied(entry) => {
                if entry.get() != &observation {
                    return Err("OTLP export contains a conflicting span identity".into());
                }
            }
        }
    }
    Ok((observations.into_values().collect(), unscoped))
}

fn session_id(attributes: Vec<Attribute>) -> Result<Option<String>, String> {
    let mut session = None;
    for attribute in attributes {
        if !matches!(
            attribute.key.as_str(),
            "session.id" | "gen_ai.conversation.id"
        ) {
            continue;
        }
        let value = attribute
            .value
            .and_then(|value| value.string_value)
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 512
                    && value.as_bytes()[0].is_ascii_alphanumeric()
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
            })
            .ok_or("invalid OTLP span session identity")?;
        if session.as_ref().is_some_and(|prior| prior != &value) {
            return Err("conflicting OTLP span session identities".into());
        }
        session = Some(value);
    }
    Ok(session)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Export {
    resource_spans: Vec<ResourceSpans>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceSpans {
    #[serde(default)]
    scope_spans: Vec<ScopeSpans>,
}

#[derive(Deserialize)]
struct ScopeSpans {
    #[serde(default)]
    spans: Vec<OtlpSpan>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OtlpSpan {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    start_time_unix_nano: Timestamp,
    end_time_unix_nano: Timestamp,
    status: Option<Status>,
    #[serde(default)]
    attributes: Vec<Attribute>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Timestamp {
    Decimal(String),
    Number(u64),
}

impl Timestamp {
    fn value(self) -> Option<u64> {
        match self {
            Self::Decimal(value) if value.bytes().all(|byte| byte.is_ascii_digit()) => {
                value.parse().ok()
            }
            Self::Decimal(_) => None,
            Self::Number(value) => Some(value),
        }
    }
}

#[derive(Deserialize)]
struct Status {
    code: Option<u8>,
}

#[derive(Deserialize)]
struct Attribute {
    key: String,
    value: Option<AttributeValue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttributeValue {
    string_value: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn fixture() -> Value {
        json!({
            "traceId":"5B8EFFF798038103D269B633813FC60C",
            "spanId":"83d69f6fc3261111",
            "parentSpanId":"83d69f6fc3261212",
            "startTimeUnixNano":"1795300000000000000",
            "endTimeUnixNano":1795300000005000000u64,
            "status":{"code":2,"message":"private output"},
            "name":"private span name",
            "attributes":[
                {"key":"gen_ai.conversation.id","value":{"stringValue":"turn-session"}},
                {"key":"gen_ai.input.messages","value":{"stringValue":"private input"}}
            ]
        })
    }

    fn from_spans(spans: Vec<Value>) -> Result<(Vec<Observation>, usize), String> {
        let export = json!({"resourceSpans":[{"scopeSpans":[{"spans":spans}]}]});
        prepare(
            export.to_string().as_bytes(),
            &ExtensionId::parse("otlp.synthetic").unwrap(),
        )
    }

    #[test]
    fn import_retains_only_explicit_trace_structure_with_session_and_time() {
        let mut unscoped = fixture();
        unscoped["attributes"] = json!([]);
        let (observations, skipped) = from_spans(vec![fixture(), fixture(), unscoped]).unwrap();
        assert_eq!(skipped, 1);
        assert_eq!(observations.len(), 1);
        let row = &observations[0];
        assert_eq!(row.identity.session_id.as_deref(), Some("turn-session"));
        assert_eq!(row.observed_at_unix_ns, 1_795_300_000_005_000_000);
        assert_eq!(
            row.kind,
            ObservationKind::TraceSpan(TraceSpan {
                trace_id: "5b8efff798038103d269b633813fc60c".into(),
                span_id: "83d69f6fc3261111".into(),
                parent_span_id: Some("83d69f6fc3261212".into()),
                started_at_unix_ns: 1_795_300_000_000_000_000,
                status: TraceStatus::Error,
            })
        );
        assert!(!format!("{observations:?}").contains("private"));
    }

    #[test]
    fn conflicting_span_or_session_identity_fails_without_projecting_partial_records() {
        let mut changed = fixture();
        changed["status"]["code"] = json!(1);
        assert!(from_spans(vec![fixture(), changed]).is_err());

        for (field, invalid) in [
            ("spanId", json!("0000000000000000")),
            ("parentSpanId", json!("83d69f6fc3261111")),
            ("endTimeUnixNano", json!("1")),
            (
                "attributes",
                json!([
                    {"key":"session.id","value":{"stringValue":"first-session"}},
                    {"key":"gen_ai.conversation.id","value":{"stringValue":"second-session"}}
                ]),
            ),
        ] {
            let mut span = fixture();
            span[field] = invalid;
            assert!(from_spans(vec![fixture(), span]).is_err());
        }
    }
}
