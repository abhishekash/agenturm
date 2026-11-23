//! Explicit, bounded import of structural OTLP trace evidence.

mod proto;

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
pub(super) const MAX_EXPORT_SPANS: usize = 1_024;
const CURSOR_PREFIX: &str = "otlp-json/1:";
const TURN_KEY: &str = "agenturm.turn.id";
const WORKER_KEY: &str = "agenturm.worker.id";
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
    let (observations, unscoped) = prepare_export(options)?;
    let inserted = commit(&options.db, &options.source_id, &observations)?;
    Ok(report(inserted, unscoped))
}

#[cfg(target_os = "linux")]
pub(super) fn execute_with_store(
    options: &TraceImportOptions,
    store: &mut impl ObservationStore,
) -> Result<String, String> {
    let (observations, unscoped) = prepare_export(options)?;
    let inserted = commit_with_store(store, &options.source_id, &observations)?;
    Ok(report(inserted, unscoped))
}

fn report(inserted: usize, unscoped: usize) -> String {
    format!("Imported {inserted} spans. Skipped {unscoped} without session identity.\n")
}

fn prepare_export(options: &TraceImportOptions) -> Result<(Vec<Observation>, usize), String> {
    if options.file.as_os_str() == "-" {
        return prepare(io::stdin().lock(), &options.source_id);
    }
    let file = open_regular_export(&options.file, "OTLP export")?;
    prepare(file, &options.source_id)
}

pub(super) fn open_regular_export(path: &Path, format: &str) -> Result<File, String> {
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Open without blocking on a FIFO. Validate the opened inode.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| format!("Cannot open {format}"))?;
    let metadata = file
        .metadata()
        .map_err(|_| format!("Cannot inspect {format}"))?;
    if !metadata.is_file() || metadata.len() > MAX_EXPORT_BYTES {
        return Err(format!("{format} must be a regular file of at most 4 MiB"));
    }
    Ok(file)
}

pub(super) fn commit(
    db: &Path,
    source_id: &ExtensionId,
    observations: &[Observation],
) -> Result<usize, String> {
    if observations.is_empty() {
        return Ok(0);
    }
    let mut store =
        Store::open_collector(db).map_err(|error| format!("open history failed: {error}"))?;
    store
        .apply_current_automatic_retention()
        .map_err(|_| "Automatic retention could not run")?;
    commit_with_store(&mut store, source_id, observations)
}

pub(super) fn commit_with_store(
    store: &mut impl ObservationStore,
    source_id: &ExtensionId,
    observations: &[Observation],
) -> Result<usize, String> {
    if observations.is_empty() {
        return Ok(0);
    }
    let cursor = import_cursor(observations)?;
    let prior = admission_cursor(store, source_id)?;
    if prior.as_deref() == Some(cursor.as_str()) {
        Ok(0)
    } else {
        ObservationStore::commit(
            store,
            source_id.as_str(),
            prior.as_deref(),
            &cursor,
            observations,
        )
        .map(|receipt| receipt.inserted)
    }
}

pub(super) fn admission_cursor(
    store: &impl ObservationStore,
    source_id: &ExtensionId,
) -> Result<Option<String>, String> {
    let prior = ObservationStore::source_cursor(store, source_id.as_str())?;
    if prior
        .as_deref()
        .is_some_and(|checkpoint| !checkpoint.starts_with(CURSOR_PREFIX))
    {
        return Err("OTLP source identifier already belongs to another collector".into());
    }
    Ok(prior)
}

fn import_cursor(observations: &[Observation]) -> Result<String, String> {
    let mut hasher = Sha256::new();
    for row in observations {
        let ObservationKind::TraceSpan(span) = &row.kind else {
            return Err("invalid trace import observation".into());
        };
        let session_id = row.identity.session_id.as_deref();
        let legacy = (session_id, row.observed_at_unix_ns, span);
        let serial = match row.identity.turn_id.as_deref() {
            Some(turn_id) => {
                serde_json::to_vec(&(legacy, turn_id, row.identity.worker_id.as_deref()))
            }
            None => serde_json::to_vec(&legacy),
        }
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
    decode_json(bytes, source_id, None)
}

pub(super) fn decode_json(
    bytes: &[u8],
    source_id: &ExtensionId,
    launch_session: Option<&str>,
) -> Result<(Vec<Observation>, usize), String> {
    if bytes.len() as u64 > MAX_EXPORT_BYTES {
        return Err("OTLP export exceeds 4 MiB".into());
    }
    let export: Export =
        serde_json::from_slice(bytes).map_err(|_| "invalid OTLP JSON trace export".to_owned())?;
    project(
        export
            .resource_spans
            .into_iter()
            .flat_map(|resource| resource.scope_spans)
            .flat_map(|scope| scope.spans),
        source_id,
        launch_session,
    )
}

pub(super) fn decode_protobuf(
    bytes: &[u8],
    source_id: &ExtensionId,
    launch_session: Option<&str>,
) -> Result<(Vec<Observation>, usize), String> {
    if bytes.len() as u64 > MAX_EXPORT_BYTES {
        return Err("OTLP export exceeds 4 MiB".into());
    }
    project(proto::decode(bytes)?, source_id, launch_session)
}

pub(super) fn protobuf_response(rejected: usize) -> Vec<u8> {
    proto::response(rejected)
}

pub(super) fn project(
    spans: impl IntoIterator<Item = OtlpSpan>,
    source_id: &ExtensionId,
    launch_session: Option<&str>,
) -> Result<(Vec<Observation>, usize), String> {
    let mut observations = BTreeMap::new();
    let mut unscoped = 0;
    for (index, span) in spans.into_iter().enumerate() {
        if index >= MAX_EXPORT_SPANS {
            return Err("OTLP export exceeds 1024 spans".into());
        }
        let scope = trace_scope(span.attributes)?;
        if scope.turn_id.is_some()
            && (scope.target_source_id.is_none() || scope.session_id.is_none())
        {
            return Err("An OTLP span turn requires an explicit harness source and session".into());
        }
        if scope.worker_id.is_some() && scope.turn_id.is_none() {
            return Err("An OTLP span worker requires an explicit turn".into());
        }
        let Some(session_id) = scope
            .session_id
            .or_else(|| launch_session.map(str::to_owned))
        else {
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
            target_source_id: scope.target_source_id,
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
        let mut identity = ObservationIdentity::session(session_id);
        identity.turn_id = scope.turn_id;
        identity.worker_id = scope.worker_id;
        let observation = Observation {
            harness_id: source_id.clone(),
            event_id: event_id.clone(),
            revision: 0,
            identity,
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

#[derive(Default)]
struct TraceScope {
    session_id: Option<String>,
    target_source_id: Option<String>,
    turn_id: Option<String>,
    worker_id: Option<String>,
}

fn trace_scope(attributes: Vec<Attribute>) -> Result<TraceScope, String> {
    let mut scope = TraceScope::default();
    for attribute in attributes {
        let (slot, valid, error): (_, fn(&str) -> bool, _) = match attribute.key.as_str() {
            "session.id" | "gen_ai.conversation.id" => (
                &mut scope.session_id,
                valid_session_id,
                "invalid or conflicting OTLP span session identity",
            ),
            "agenturm.source.id" => (
                &mut scope.target_source_id,
                |value| !value.starts_with("otlp.") && ExtensionId::parse(value).is_ok(),
                "invalid or conflicting OTLP span harness source",
            ),
            TURN_KEY => (
                &mut scope.turn_id,
                valid_execution_id,
                "invalid or conflicting OTLP span turn identity",
            ),
            WORKER_KEY => (
                &mut scope.worker_id,
                valid_execution_id,
                "invalid or conflicting OTLP span worker identity",
            ),
            _ => continue,
        };
        let value = attribute
            .value
            .and_then(|value| value.string_value)
            .filter(|value| valid(value))
            .ok_or(error)?;
        if slot.as_ref().is_some_and(|prior| prior != &value) {
            return Err(error.into());
        }
        *slot = Some(value);
    }
    Ok(scope)
}

pub(super) fn valid_session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

fn valid_execution_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
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
pub(super) struct OtlpSpan {
    pub(super) trace_id: String,
    pub(super) span_id: String,
    pub(super) parent_span_id: Option<String>,
    pub(super) start_time_unix_nano: Timestamp,
    pub(super) end_time_unix_nano: Timestamp,
    pub(super) status: Option<Status>,
    #[serde(default)]
    pub(super) attributes: Vec<Attribute>,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum Timestamp {
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
pub(super) struct Status {
    pub(super) code: Option<u8>,
}

#[derive(Deserialize)]
pub(super) struct Attribute {
    key: String,
    value: Option<AttributeValue>,
}

impl Attribute {
    pub(super) fn text(key: &str, value: String) -> Self {
        Self {
            key: key.into(),
            value: Some(AttributeValue {
                string_value: Some(value),
            }),
        }
    }
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
                target_source_id: None,
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

    #[test]
    fn durable_trace_links_require_exact_source_and_session_at_the_selected_snapshot() {
        use agenturm_kernel::ObservationQuery;

        let mut store = Store::open(":memory:").unwrap();
        let add_session = |store: &mut Store, source: &str| {
            let row = Observation {
                harness_id: ExtensionId::parse(source).unwrap(),
                event_id: "native-start".into(),
                revision: 0,
                identity: ObservationIdentity::session("turn-session"),
                observed_at_unix_ns: 10,
                kind: ObservationKind::SessionStarted,
            };
            ObservationStore::commit(store, source, None, "1", &[row]).unwrap();
        };
        add_session(&mut store, "harness.codex");
        add_session(&mut store, "harness.pi");
        let bound = |span_id: &str, target: &str| {
            let mut span = fixture();
            span["spanId"] = json!(span_id);
            span["attributes"].as_array_mut().unwrap().push(json!({
                "key":"agenturm.source.id", "value":{"stringValue":target}
            }));
            span
        };
        let (observations, skipped) = from_spans(vec![
            fixture(),
            bound("83d69f6fc3261112", "harness.codex"),
            bound("83d69f6fc3261113", "harness.pi"),
            bound("83d69f6fc3261114", "harness.late"),
        ])
        .unwrap();
        assert_eq!(skipped, 0);
        ObservationStore::commit(&mut store, "otlp.synthetic", None, "1", &observations).unwrap();

        let query = |source: &str, snapshot_id| ObservationQuery {
            harness_id: Some(source.into()),
            session_id: Some("turn-session".into()),
            snapshot_id,
            before: None,
            limit: 1,
        };
        for (target, expected) in [
            ("harness.codex", "83d69f6fc3261112"),
            ("harness.pi", "83d69f6fc3261113"),
        ] {
            let page = store.query_linked_traces(&query(target, None)).unwrap();
            assert_eq!(page.observations.len(), 1);
            assert!(!page.has_more);
            assert_eq!(page.observations[0].harness_id.as_str(), "otlp.synthetic");
            assert!(matches!(
                &page.observations[0].kind,
                ObservationKind::TraceSpan(span)
                    if span.span_id == expected && span.target_source_id.as_deref() == Some(target)
            ));
        }
        let old = store
            .query_linked_traces(&query("harness.late", None))
            .unwrap();
        assert!(old.observations.is_empty());
        add_session(&mut store, "harness.late");
        assert!(
            store
                .query_linked_traces(&query("harness.late", Some(old.snapshot_id)))
                .unwrap()
                .observations
                .is_empty()
        );
        assert_eq!(
            store
                .query_linked_traces(&query("harness.late", None))
                .unwrap()
                .observations
                .len(),
            1
        );
        let imported = store
            .query_observations(&ObservationQuery {
                limit: 10,
                ..query("otlp.synthetic", None)
            })
            .unwrap();
        assert_eq!(imported.observations.len(), 4);
        assert!(imported.observations.iter().any(|row| matches!(
            &row.kind,
            ObservationKind::TraceSpan(span) if span.target_source_id.is_none()
        )));

        let (new_span, _) = from_spans(vec![bound("83d69f6fc3261115", "harness.codex")]).unwrap();
        ObservationStore::commit(&mut store, "otlp.synthetic", Some("1"), "2", &new_span).unwrap();
        let page = store
            .query_linked_traces(&query("harness.codex", None))
            .unwrap();
        assert!(page.has_more);
        let first = &page.observations[0];
        let mut older = query("harness.codex", Some(page.snapshot_id));
        older.before = Some(agenturm_kernel::ObservationPosition {
            observed_at_unix_ns: first.observed_at_unix_ns,
            harness_id: first.harness_id.as_str().into(),
            event_id: first.event_id.clone(),
            revision: first.revision,
            snapshot_id: page.snapshot_id,
        });
        let page = store.query_linked_traces(&older).unwrap();
        assert!(!page.has_more);
        assert!(matches!(
            &page.observations[0].kind,
            ObservationKind::TraceSpan(span) if span.span_id == "83d69f6fc3261112"
        ));
    }

    #[test]
    fn invalid_or_conflicting_explicit_harness_binding_is_rejected() {
        for values in [
            vec!["../invalid"],
            vec!["otlp.synthetic"],
            vec!["harness.codex", "harness.pi"],
        ] {
            let mut span = fixture();
            for source in values {
                span["attributes"].as_array_mut().unwrap().push(json!({
                    "key":"agenturm.source.id", "value":{"stringValue":source}
                }));
            }
            assert!(from_spans(vec![span]).is_err());
        }
    }

    #[test]
    fn turn_scope_requires_complete_explicit_ids_even_with_a_launch_session() {
        fn decode_attributes(
            attributes: Value,
            launch_session: Option<&str>,
        ) -> Result<Observation, String> {
            let mut span = fixture();
            span["attributes"] = attributes;
            let export = json!({"resourceSpans":[{"scopeSpans":[{"spans":[span]}]}]});
            decode_json(
                export.to_string().as_bytes(),
                &ExtensionId::parse("otlp.synthetic").unwrap(),
                launch_session,
            )?
            .0
            .pop()
            .ok_or("No observation".into())
        }

        let attribute = |key: &str, value: &str| json!({"key":key,"value":{"stringValue":value}});
        let source = attribute("agenturm.source.id", "harness.codex");
        let session = attribute("session.id", "turn-session");
        let turn = attribute(TURN_KEY, "turn/1");
        let worker = attribute(WORKER_KEY, "worker/1");
        let valid = vec![
            source.clone(),
            session.clone(),
            turn.clone(),
            worker.clone(),
        ];
        let observation = decode_attributes(json!(valid), None).unwrap();
        assert_eq!(observation.identity.turn_id.as_deref(), Some("turn/1"));
        assert_eq!(observation.identity.worker_id.as_deref(), Some("worker/1"));

        for invalid in [
            vec![source.clone(), turn.clone()],
            vec![session.clone(), turn],
            vec![source.clone(), session.clone(), worker],
            vec![
                source.clone(),
                session.clone(),
                attribute(TURN_KEY, "contains space"),
            ],
            [&valid[..], &[attribute(WORKER_KEY, "another-worker")]].concat(),
            [
                &valid[..],
                &[json!({"key": TURN_KEY,"value":{"intValue":"1"}})],
            ]
            .concat(),
        ] {
            assert!(decode_attributes(json!(invalid), Some("launch-session")).is_err());
        }

        let mut changed = observation.clone();
        changed.identity.worker_id = None;
        assert_ne!(
            import_cursor(&[observation]).unwrap(),
            import_cursor(&[changed]).unwrap()
        );
    }
}
