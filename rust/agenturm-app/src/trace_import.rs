//! Explicit, bounded import of structural OTLP trace evidence.

pub(super) mod cost;
pub(crate) mod proto;
pub(super) mod token_usage;

use agenturm_kernel::telemetry_summary::TraceSpanSummary;
use agenturm_kernel::{
    ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationStore, ReportedCost,
    TraceSpan, TraceSpanIdentity, TraceStatus,
};
use agenturm_store::{
    CommitOutcome, ObservationBatch, Store, TelemetryBinding, TelemetryPayload,
    TelemetryPayloadRecord,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub(super) const MAX_EXPORT_BYTES: u64 = 4 * 1024 * 1024;
pub(super) const MAX_EXPORT_SPANS: usize = agenturm_store::MAX_OTLP_BATCH_SPANS;
const CURSOR_PREFIX: &str = "otlp-json/1:";
const TURN_KEY: &str = "agenturm.turn.id";
const WORKER_KEY: &str = "agenturm.worker.id";
pub(super) const OPERATION_KEY: &str = "agenturm.operation.id";
pub(super) const PAYLOAD_INPUT: &str = "agenturm.payload.input";
pub(super) const PAYLOAD_OUTPUT: &str = "agenturm.payload.output";

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
    Ok(report(inserted, unscoped, &observations))
}

#[cfg(target_os = "linux")]
pub(super) fn execute_with_store(
    options: &TraceImportOptions,
    store: &mut impl ObservationStore,
) -> Result<String, String> {
    let (observations, unscoped) = prepare_export(options)?;
    let inserted = commit_with_store(store, &options.source_id, &observations)?;
    Ok(report(inserted, unscoped, &observations))
}

fn report(inserted: usize, unscoped: usize, observations: &[Observation]) -> String {
    let (summary, _) = import_summary(inserted, observations);
    format!("{summary}. Skipped {unscoped} without session identity.\n")
}

pub(super) fn import_summary(inserted: usize, observations: &[Observation]) -> (String, usize) {
    let mut spans = 0;
    let mut classifications = 0;
    let mut cost = 0;
    let mut usage = 0;
    for row in observations {
        match row.kind {
            ObservationKind::TraceSpan(_) => spans += 1,
            ObservationKind::TraceSpanSummary(_) => classifications += 1,
            ObservationKind::ReportedCost(_) => cost += 1,
            ObservationKind::SpanTokenUsage(_) => usage += 1,
            _ => {}
        }
    }
    if cost == 0 && usage == 0 {
        if inserted == 0 {
            return ("Imported 0 spans".into(), spans);
        }
        if inserted == observations.len() {
            return (format!("Imported {spans} spans"), spans);
        }
    }
    let mut labels = vec![format!("{spans} spans")];
    if cost > 0 {
        labels.push(format!("{cost} reported cost records"));
    }
    if usage > 0 {
        labels.push(format!("{usage} token usage records"));
    }
    if classifications > 0 {
        labels.push(format!("{classifications} span classifications"));
    }
    let last = labels.pop().expect("a metadata label exists");
    (
        format!(
            "Imported {inserted} observations from {} and {last}",
            labels.join(", ")
        ),
        spans,
    )
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

pub(super) fn prepare_payload_batch(
    store: &impl ObservationStore,
    binding: &TelemetryBinding,
    rows: &[Observation],
) -> Result<ObservationBatch, String> {
    let prior = admission_cursor(store, &binding.source_id)?;
    let next = import_cursor(rows)?;
    ObservationBatch::from_observations(binding.source_id.as_str(), prior.as_deref(), &next, rows)
}

pub(super) fn commit_payloads(
    db: &Path,
    binding: &TelemetryBinding,
    rows: &[Observation],
    payloads: &[TelemetryPayloadRecord],
) -> Result<usize, String> {
    if payloads.is_empty() {
        return commit(db, &binding.source_id, rows);
    }
    let mut store =
        Store::open_collector(db).map_err(|error| format!("open history failed: {error}"))?;
    store
        .apply_current_automatic_retention()
        .map_err(|_| "Automatic retention could not run")?;
    let batch = prepare_payload_batch(&store, binding, rows)?;
    let result = if is_log_export(rows) {
        store.commit_telemetry_log_batch(&batch, &binding.into(), payloads)
    } else {
        store.commit_telemetry_batch(&batch, binding, payloads)
    };
    match result {
        Ok(CommitOutcome::Replay) => Ok(0),
        Ok(CommitOutcome::Committed { inserted }) => Ok(inserted),
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn is_log_export(rows: &[Observation]) -> bool {
    !rows.is_empty()
        && rows.iter().all(|row| {
            matches!(
                row.kind,
                ObservationKind::OtlpActivityLog(_) | ObservationKind::OtlpErrorLog(_)
            )
        })
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
        let serial = match &row.kind {
            ObservationKind::TraceSpan(span) => {
                let session_id = row.identity.session_id.as_deref();
                let legacy = (session_id, row.observed_at_unix_ns, span);
                match row.identity.turn_id.as_deref() {
                    Some(turn_id) => {
                        serde_json::to_vec(&(legacy, turn_id, row.identity.worker_id.as_deref()))
                    }
                    None => serde_json::to_vec(&legacy),
                }
            }
            ObservationKind::ReportedCost(cost) => serde_json::to_vec(&(
                "reported_cost",
                &row.identity,
                row.observed_at_unix_ns,
                cost,
            )),
            ObservationKind::SpanTokenUsage(usage) => serde_json::to_vec(&(
                "span_token_usage",
                &row.identity,
                row.observed_at_unix_ns,
                usage,
            )),
            ObservationKind::TraceProgress(progress) => serde_json::to_vec(&(
                "trace_progress",
                &row.event_id,
                &row.identity,
                row.observed_at_unix_ns,
                progress,
            )),
            ObservationKind::MetricPoint(point) => serde_json::to_vec(&(
                "metric_point",
                &row.event_id,
                &row.identity,
                row.observed_at_unix_ns,
                point,
            )),
            ObservationKind::TraceSpanSummary(summary) => serde_json::to_vec(&(
                "trace_span_summary",
                &row.event_id,
                &row.identity,
                row.observed_at_unix_ns,
                summary,
            )),
            ObservationKind::OtlpActivityLog(log) => serde_json::to_vec(&(
                "otlp_activity_log",
                &row.event_id,
                &row.identity,
                row.observed_at_unix_ns,
                log,
            )),
            ObservationKind::OtlpErrorLog(log) => serde_json::to_vec(&(
                "otlp_error_log",
                &row.event_id,
                &row.identity,
                row.observed_at_unix_ns,
                log,
            )),
            _ => return Err("invalid trace import observation".into()),
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
    let (observations, skipped, _) = decode_json(bytes, source_id, None, None)?;
    Ok((observations, skipped))
}

pub(super) type DecodedExport = (Vec<Observation>, usize, Vec<TelemetryPayloadRecord>);

pub(super) fn decode_json(
    bytes: &[u8],
    source_id: &ExtensionId,
    launch_session: Option<&str>,
    payload_binding: Option<&TelemetryBinding>,
) -> Result<DecodedExport, String> {
    if bytes.len() as u64 > MAX_EXPORT_BYTES {
        return Err("OTLP export exceeds 4 MiB".into());
    }
    let export: Export =
        serde_json::from_slice(bytes).map_err(|_| "invalid OTLP JSON trace export".to_owned())?;
    project_payloads(
        export
            .resource_spans
            .into_iter()
            .flat_map(|resource| resource.scope_spans)
            .flat_map(|scope| scope.spans),
        source_id,
        launch_session,
        payload_binding,
    )
}

pub(super) fn decode_protobuf(
    bytes: &[u8],
    source_id: &ExtensionId,
    launch_session: Option<&str>,
    payload_binding: Option<&TelemetryBinding>,
) -> Result<DecodedExport, String> {
    if bytes.len() as u64 > MAX_EXPORT_BYTES {
        return Err("OTLP export exceeds 4 MiB".into());
    }
    project_payloads(
        proto::decode(bytes)?,
        source_id,
        launch_session,
        payload_binding,
    )
}

pub(super) fn protobuf_response(rejected: usize) -> Vec<u8> {
    proto::partial_success(rejected, b"Span lacks an explicit session identity")
}

pub(super) fn project_payloads(
    spans: impl IntoIterator<Item = OtlpSpan>,
    source_id: &ExtensionId,
    launch_session: Option<&str>,
    payload_binding: Option<&TelemetryBinding>,
) -> Result<DecodedExport, String> {
    let mut observations = BTreeMap::new();
    let mut unscoped = 0;
    for (index, span) in spans.into_iter().enumerate() {
        if index >= MAX_EXPORT_SPANS {
            return Err("OTLP export exceeds 1024 spans".into());
        }
        let reported = cost::reported_amount(&span.attributes);
        let scope = trace_scope(&span.attributes)?;
        let payload = match (scope.input, scope.output) {
            (None, None) => None,
            (input, output) => {
                let binding = payload_binding.ok_or("OTLP payload capture is disabled")?;
                if scope.session_id.is_none()
                    || scope.target_source_id.as_deref()
                        != binding.native_source_id.as_ref().map(ExtensionId::as_str)
                {
                    return Err(
                        "OTLP payload requires its configured explicit source and session".into(),
                    );
                }
                let payload = TelemetryPayload { input, output };
                payload
                    .validate()
                    .map_err(|_| "OTLP payload text was withheld")?;
                Some(payload)
            }
        };
        if scope.turn_id.is_some()
            && (scope.target_source_id.is_none() || scope.session_id.is_none())
        {
            return Err("An OTLP span turn requires an explicit harness source and session".into());
        }
        if scope.worker_id.is_some() && scope.turn_id.is_none() {
            return Err("An OTLP span worker requires an explicit turn".into());
        }
        if scope.operation_id.is_some() && scope.turn_id.is_none() {
            return Err("An OTLP span operation requires an explicit turn".into());
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
            target_operation_id: scope.operation_id,
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
        if let Some((currency, nanounits)) = reported {
            let cost_event = format!("otlp-cost:{}:{}", native.trace_id, native.span_id);
            insert_projected(
                &mut observations,
                Observation {
                    harness_id: source_id.clone(),
                    event_id: cost_event,
                    revision: 0,
                    identity: identity.clone(),
                    observed_at_unix_ns: ended_at,
                    kind: ObservationKind::ReportedCost(ReportedCost {
                        currency,
                        nanounits,
                        span: Some(TraceSpanIdentity {
                            trace_id: native.trace_id.clone(),
                            span_id: native.span_id.clone(),
                        }),
                        native_origin: None,
                    }),
                },
                None,
            )?;
        }
        if let Some(usage) = token_usage::reported(&span.attributes, &native) {
            insert_projected(
                &mut observations,
                Observation {
                    harness_id: source_id.clone(),
                    event_id: format!("otlp-usage:{}:{}", native.trace_id, native.span_id),
                    revision: 0,
                    identity: identity.clone(),
                    observed_at_unix_ns: ended_at,
                    kind: ObservationKind::SpanTokenUsage(usage),
                },
                None,
            )?;
        }
        let summary = Observation {
            harness_id: source_id.clone(),
            event_id: format!("{event_id}:summary"),
            revision: 0,
            identity: identity.clone(),
            observed_at_unix_ns: ended_at,
            kind: ObservationKind::TraceSpanSummary(TraceSpanSummary {
                trace_id: native.trace_id.clone(),
                span_id: native.span_id.clone(),
                agent_run: span.agent_run,
                reported_error: native.status == TraceStatus::Error,
            }),
        };
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
                entry.insert((observation, payload));
            }
            std::collections::btree_map::Entry::Occupied(entry) => {
                if entry.get() != &(observation, payload) {
                    return Err("OTLP export contains a conflicting span identity".into());
                }
            }
        }
        insert_projected(&mut observations, summary, None)?;
    }
    let mut rows = Vec::with_capacity(observations.len());
    let mut payloads = Vec::new();
    for (event_id, (row, payload)) in observations {
        rows.push(row);
        if let Some(payload) = payload {
            payloads.push(TelemetryPayloadRecord { event_id, payload });
        }
    }
    Ok((rows, unscoped, payloads))
}

fn insert_projected(
    observations: &mut BTreeMap<String, (Observation, Option<TelemetryPayload>)>,
    observation: Observation,
    payload: Option<TelemetryPayload>,
) -> Result<(), String> {
    match observations.entry(observation.event_id.clone()) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert((observation, payload));
        }
        std::collections::btree_map::Entry::Occupied(entry) => {
            if entry.get() != &(observation, payload) {
                return Err("OTLP export contains conflicting span metadata evidence".into());
            }
        }
    }
    Ok(())
}

#[derive(Default)]
pub(super) struct TraceScope {
    pub(super) session_id: Option<String>,
    pub(super) target_source_id: Option<String>,
    pub(super) turn_id: Option<String>,
    pub(super) worker_id: Option<String>,
    operation_id: Option<String>,
    input: Option<String>,
    output: Option<String>,
}

pub(super) fn trace_scope<'a>(
    attributes: impl IntoIterator<Item = &'a Attribute>,
) -> Result<TraceScope, String> {
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
            OPERATION_KEY => (
                &mut scope.operation_id,
                valid_execution_id,
                "invalid or conflicting OTLP span operation identity",
            ),
            PAYLOAD_INPUT => (
                &mut scope.input,
                valid_payload_size,
                "invalid OTLP input text",
            ),
            PAYLOAD_OUTPUT => (
                &mut scope.output,
                valid_payload_size,
                "invalid OTLP output text",
            ),
            _ => continue,
        };
        let value = attribute
            .value
            .as_ref()
            .and_then(|value| value.string_value.as_str().map(str::to_owned))
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

fn valid_payload_size(value: &str) -> bool {
    !value.is_empty() && value.len() <= agenturm_store::MAX_TELEMETRY_PAYLOAD_TEXT_BYTES
}

pub(super) fn valid_execution_id(value: &str) -> bool {
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
    #[serde(default, rename = "name", deserialize_with = "classify_span_name")]
    pub(super) agent_run: bool,
    #[serde(default)]
    pub(super) attributes: Vec<Attribute>,
}

pub(super) fn classify_span_name<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    let value = <&RawValue>::deserialize(deserializer)?;
    Ok(
        serde_json::from_str::<std::borrow::Cow<'_, str>>(value.get())
            .is_ok_and(|name| name == "agent.run"),
    )
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum Timestamp {
    Decimal(String),
    Number(u64),
}

impl Timestamp {
    pub(super) fn value(self) -> Option<u64> {
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
    pub(super) key: String,
    pub(super) value: Option<AttributeValue>,
}

impl Attribute {
    pub(super) fn text(key: &str, value: String) -> Self {
        Self {
            key: key.into(),
            value: Some(AttributeValue {
                string_value: value.into(),
                ..AttributeValue::default()
            }),
        }
    }

    pub(super) fn raw_literal(key: &str, value: Box<RawValue>) -> Self {
        let mut scalar = AttributeValue::default();
        match value.get().as_bytes()[0] {
            b'"' => scalar.string_value = serde_json::from_str(value.get()).unwrap_or_default(),
            b't' | b'f' => scalar.bool_value = (value.get() == "true").into(),
            b'0'..=b'9' if value.get().bytes().all(|byte| byte.is_ascii_digit()) => {
                scalar.int_value = Some(value);
            }
            b'-' | b'0'..=b'9' => scalar.double_value = Some(value),
            _ => {}
        }
        Self {
            key: key.into(),
            value: Some(scalar),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AttributeValue {
    #[serde(default)]
    pub(super) string_value: serde_json::Value,
    int_value: Option<Box<RawValue>>,
    double_value: Option<Box<RawValue>>,
    #[serde(default)]
    bool_value: serde_json::Value,
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
        assert_eq!(observations.len(), 2);
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
                target_operation_id: None,
                started_at_unix_ns: 1_795_300_000_000_000_000,
                status: TraceStatus::Error,
            })
        );
        assert!(matches!(
            &observations[1].kind,
            ObservationKind::TraceSpanSummary(summary)
                if summary.reported_error && !summary.agent_run
                    && summary.matches_event(&observations[1].event_id)
        ));
        assert_eq!(row.identity, observations[1].identity);
        assert_eq!(row.observed_at_unix_ns, observations[1].observed_at_unix_ns);
        assert!(!format!("{observations:?}").contains("private"));
    }

    #[test]
    fn classifies_exact_agent_run_names_and_rejects_divergent_duplicate_metadata() {
        let ordinary = fixture();
        let mut run = ordinary.clone();
        run["name"] = json!("agent.run");
        run["status"]["code"] = json!(1);
        assert!(matches!(
            &from_spans(vec![run.clone()]).unwrap().0[1].kind,
            ObservationKind::TraceSpanSummary(summary)
                if summary.agent_run && !summary.reported_error
        ));
        let mut conflicting = run.clone();
        conflicting["name"] = json!("agent.run.child");
        assert!(from_spans(vec![run, conflicting]).is_err());
    }

    #[test]
    fn newly_classified_export_replays_legacy_span_without_counting_a_second_span() {
        let mut store = Store::open(":memory:").unwrap();
        let source = ExtensionId::parse("otlp.synthetic").unwrap();
        let (rows, _) = from_spans(vec![fixture()]).unwrap();
        assert!(matches!(rows[0].kind, ObservationKind::TraceSpan(_)));
        assert_eq!(commit_with_store(&mut store, &source, &rows[..1]), Ok(1));
        let mut classified = fixture();
        classified["name"] = json!("agent.run");
        let (rows, _) = from_spans(vec![classified]).unwrap();
        assert_eq!(commit_with_store(&mut store, &source, &rows), Ok(1));
        assert_eq!(commit_with_store(&mut store, &source, &rows), Ok(0));
        let (summary, spans) = import_summary(1, &rows);
        assert_eq!(
            summary,
            "Imported 1 observations from 1 spans and 1 span classifications"
        );
        assert_eq!(spans, 1);
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
        assert_eq!(imported.observations.len(), 8);
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
                None,
            )?
            .0
            .into_iter()
            .find(|row| matches!(row.kind, ObservationKind::TraceSpan(_)))
            .ok_or("No span observation".into())
        }

        let attribute = |key: &str, value: &str| json!({"key":key,"value":{"stringValue":value}});
        let source = attribute("agenturm.source.id", "harness.codex");
        let session = attribute("session.id", "turn-session");
        let turn = attribute(TURN_KEY, "turn/1");
        let worker = attribute(WORKER_KEY, "worker/1");
        let operation = attribute(OPERATION_KEY, "mcp-call/1");
        let valid = vec![
            source.clone(),
            session.clone(),
            turn.clone(),
            worker.clone(),
            operation.clone(),
        ];
        let observation = decode_attributes(json!(valid), None).unwrap();
        assert_eq!(observation.identity.turn_id.as_deref(), Some("turn/1"));
        assert_eq!(observation.identity.worker_id.as_deref(), Some("worker/1"));
        assert!(observation.identity.operation_id.is_none());
        assert!(matches!(
            &observation.kind,
            ObservationKind::TraceSpan(span) if span.target_operation_id.as_deref() == Some("mcp-call/1")
        ));

        for invalid in [
            vec![source.clone(), turn.clone()],
            vec![session.clone(), turn],
            vec![source.clone(), session.clone(), worker],
            vec![source.clone(), session.clone(), operation],
            vec![
                source.clone(),
                session.clone(),
                attribute(TURN_KEY, "contains space"),
            ],
            [&valid[..], &[attribute(WORKER_KEY, "another-worker")]].concat(),
            [&valid[..], &[attribute(OPERATION_KEY, "another-call")]].concat(),
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
