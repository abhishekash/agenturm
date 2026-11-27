//! Admit closed metadata for explicit activity and ordinary error logs.

mod proto;
#[cfg(test)]
mod tests;

use crate::trace_import::{
    Attribute, DecodedExport, MAX_EXPORT_BYTES, Timestamp, TraceScope, cost, token_usage,
    trace_scope, valid_execution_id, valid_session_id,
};
use agenturm_kernel::telemetry_summary::{LogCost, OtlpActivityLog, OtlpErrorLog};
use agenturm_kernel::{ExtensionId, Observation, ObservationIdentity, ObservationKind};
use agenturm_store::{
    MAX_TELEMETRY_PAYLOAD_BATCH_BYTES, TelemetryBinding, TelemetryPayload, TelemetryPayloadRecord,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, btree_map::Entry};

const MALFORMED: &str = "Invalid OTLP log export";
const MAX_LOGS: usize = 1_024;
const MAX_ATTRIBUTES: usize = 64;
const EVENT_NAME: &str = "agent.activity";
const RECORD_ID_KEYS: &[&str] = &[
    "agenturm.event.id",
    "agenturm.log.id",
    "observatory.record_id",
];
const REJECTED: &str =
    "Only exact agent.activity records and identifiable error or fatal logs are retained";

pub(super) fn json_response(rejected: usize) -> Vec<u8> {
    if rejected == 0 {
        return b"{}".to_vec();
    }
    serde_json::json!({"partialSuccess": {
        "rejectedLogRecords": rejected.to_string(),
        "errorMessage": REJECTED,
    }})
    .to_string()
    .into_bytes()
}

pub(super) fn protobuf_response(rejected: usize) -> Vec<u8> {
    crate::trace_import::proto::partial_success(rejected, REJECTED.as_bytes())
}

#[derive(Default)]
struct Record {
    activity: bool,
    severity: u8,
    time: u64,
    observed_time: u64,
    attributes: Vec<Attribute>,
    scope: TraceScope,
    trace_id: Option<String>,
    span_id: Option<String>,
    body: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Export<'a> {
    #[serde(default, borrow)]
    resource_logs: Vec<ResourceLogs<'a>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceLogs<'a> {
    #[serde(borrow)]
    resource: Option<&'a RawValue>,
    #[serde(default, borrow)]
    scope_logs: Vec<ScopeLogs<'a>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScopeLogs<'a> {
    #[serde(default, borrow)]
    log_records: Vec<&'a RawValue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonRecord<'a> {
    event_name: Option<String>,
    severity_number: Option<u64>,
    time_unix_nano: Option<Timestamp>,
    observed_time_unix_nano: Option<Timestamp>,
    trace_id: Option<String>,
    span_id: Option<String>,
    #[serde(borrow)]
    body: Option<&'a RawValue>,
    #[serde(default, borrow)]
    attributes: Vec<JsonAttribute<'a>>,
}

#[derive(Deserialize)]
struct JsonAttribute<'a> {
    key: String,
    #[serde(borrow)]
    value: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct JsonResource<'a> {
    #[serde(default, borrow)]
    attributes: Vec<JsonAttribute<'a>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TextBody {
    string_value: String,
}

fn scope_key(key: &str) -> bool {
    matches!(
        key,
        "session.id"
            | "gen_ai.conversation.id"
            | "agenturm.source.id"
            | "agenturm.turn.id"
            | "agenturm.worker.id"
    )
}

fn log_key(key: &str) -> bool {
    scope_key(key) || RECORD_ID_KEYS.contains(&key)
}

fn activity_key(key: &str) -> bool {
    log_key(key)
        || key == "agent.activity.result"
        || cost::is_cost_key(key)
        || token_usage::is_token_key(key)
}

fn json_attributes(
    raw: Vec<JsonAttribute<'_>>,
    retained: fn(&str) -> bool,
) -> Result<Vec<Attribute>, &'static str> {
    let mut result = Vec::new();
    for attribute in raw.into_iter().filter(|attribute| retained(&attribute.key)) {
        if result.len() == MAX_ATTRIBUTES {
            return Err(MALFORMED);
        }
        let value = attribute
            .value
            .map(|value| serde_json::from_str(value.get()).map_err(|_| MALFORMED))
            .transpose()?;
        result.push(Attribute {
            key: attribute.key,
            value,
        });
    }
    Ok(result)
}

fn resource_attributes(raw: Option<&RawValue>) -> Result<Vec<Attribute>, &'static str> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let resource: JsonResource<'_> = serde_json::from_str(raw.get()).map_err(|_| MALFORMED)?;
    json_attributes(resource.attributes, scope_key)
}

pub(super) fn decode_json(
    bytes: &[u8],
    source: &ExtensionId,
    launch_session: Option<&str>,
    capture: Option<&TelemetryBinding>,
) -> Result<DecodedExport, String> {
    if bytes.len() as u64 > MAX_EXPORT_BYTES {
        return Err(MALFORMED.into());
    }
    let export: Export<'_> = serde_json::from_slice(bytes).map_err(|_| MALFORMED)?;
    let mut records = Vec::new();
    for resource in export.resource_logs {
        let resource_scope = resource_attributes(resource.resource);
        for raw in resource
            .scope_logs
            .into_iter()
            .flat_map(|scope| scope.log_records)
        {
            if records.len() == MAX_LOGS {
                return Err("OTLP export exceeds 1024 log records".into());
            }
            records.push(decode_json_record(raw, &resource_scope, capture.is_some())?);
        }
    }
    project(records, source, launch_session, capture)
}

fn decode_json_record(
    raw: &RawValue,
    resource: &Result<Vec<Attribute>, &'static str>,
    capture: bool,
) -> Result<Option<Record>, &'static str> {
    let raw: JsonRecord<'_> = serde_json::from_str(raw.get()).map_err(|_| MALFORMED)?;
    let activity = raw.event_name.as_deref() == Some(EVENT_NAME);
    let severity = raw.severity_number.unwrap_or_default();
    if !activity && !is_error_severity(severity) {
        return Ok(None);
    }
    let timestamp =
        |value: Option<Timestamp>| value.map_or(Ok(0), |time| time.value().ok_or(MALFORMED));
    let attributes = json_attributes(
        raw.attributes,
        if activity { activity_key } else { log_key },
    )?;
    let scope = record_scope(&attributes, resource, activity)?;
    let mut record = Record {
        activity,
        severity: u8::try_from(severity).map_err(|_| MALFORMED)?,
        time: timestamp(raw.time_unix_nano)?,
        observed_time: timestamp(raw.observed_time_unix_nano)?,
        attributes,
        scope,
        ..Record::default()
    };
    if !activity {
        record.trace_id = exact_id(raw.trace_id, 32)?;
        record.span_id = exact_id(raw.span_id, 16)?;
        if capture {
            record.body = raw
                .body
                .and_then(|raw| serde_json::from_str::<TextBody>(raw.get()).ok())
                .map(|body| body.string_value);
        }
    }
    Ok(Some(record))
}

fn is_error_severity(severity: u64) -> bool {
    (17..=24).contains(&severity)
}

fn record_scope(
    attributes: &[Attribute],
    resource: &Result<Vec<Attribute>, &'static str>,
    activity: bool,
) -> Result<TraceScope, &'static str> {
    if activity {
        trace_scope(attributes).map_err(|_| MALFORMED)
    } else {
        let resource = resource.as_ref().map_err(|error| *error)?;
        trace_scope(attributes.iter().chain(resource)).map_err(|_| MALFORMED)
    }
}

fn exact_id(value: Option<String>, size: usize) -> Result<Option<String>, &'static str> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() != size || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(MALFORMED);
    }
    Ok(value
        .bytes()
        .any(|byte| byte != b'0')
        .then(|| value.to_ascii_lowercase()))
}

pub(super) fn decode_protobuf(
    bytes: &[u8],
    source: &ExtensionId,
    launch_session: Option<&str>,
    capture: Option<&TelemetryBinding>,
) -> Result<DecodedExport, String> {
    if bytes.len() as u64 > MAX_EXPORT_BYTES {
        return Err(MALFORMED.into());
    }
    project(
        proto::decode(bytes, capture.is_some())?,
        source,
        launch_session,
        capture,
    )
}

fn project(
    records: Vec<Option<Record>>,
    source: &ExtensionId,
    launch_session: Option<&str>,
    capture: Option<&TelemetryBinding>,
) -> Result<DecodedExport, String> {
    let mut rows = BTreeMap::new();
    let mut skipped = 0;
    let mut private_bytes = 0;
    for record in records {
        let Some((row, payload)) = record
            .map(|record| project_record(record, source, launch_session, capture))
            .transpose()?
            .flatten()
        else {
            skipped += 1;
            continue;
        };
        match rows.entry(row.event_id.clone()) {
            Entry::Vacant(entry) => {
                if let Some(payload) = &payload {
                    private_bytes += payload.validate().map_err(|error| error.to_string())?;
                    if private_bytes > MAX_TELEMETRY_PAYLOAD_BATCH_BYTES {
                        return Err("OTLP log text exceeds its batch limit".into());
                    }
                }
                entry.insert((row, payload));
            }
            Entry::Occupied(entry) if entry.get() != &(row, payload) => {
                return Err("OTLP export contains conflicting log record identities".into());
            }
            Entry::Occupied(_) => {}
        }
    }
    let mut observations = Vec::with_capacity(rows.len());
    let mut payloads = Vec::new();
    for (event_id, (row, payload)) in rows {
        if let Some(payload) = payload {
            payloads.push(TelemetryPayloadRecord { event_id, payload });
        }
        observations.push(row);
    }
    Ok((observations, skipped, payloads))
}

type ProjectedLog = (Observation, Option<TelemetryPayload>);

fn project_record(
    mut record: Record,
    source: &ExtensionId,
    launch_session: Option<&str>,
    capture: Option<&TelemetryBinding>,
) -> Result<Option<ProjectedLog>, String> {
    if record.scope.worker_id.is_some() && record.scope.turn_id.is_none() {
        return Err("An OTLP log worker requires an explicit turn".into());
    }
    if record.scope.turn_id.is_some()
        && (record.scope.target_source_id.is_none() || record.scope.session_id.is_none())
    {
        return Err("An OTLP log turn requires an explicit harness source and session".into());
    }
    let mut record_id = None;
    let mut outcome = None;
    for attribute in &record.attributes {
        let (slot, valid): (_, fn(&str) -> bool) = match attribute.key.as_str() {
            key if RECORD_ID_KEYS.contains(&key) => (&mut record_id, valid_execution_id),
            "agent.activity.result" => (&mut outcome, |text| text.len() <= 64),
            _ => continue,
        };
        let value = attribute
            .value
            .as_ref()
            .and_then(|value| value.string_value.as_str())
            .filter(|value| valid(value))
            .ok_or(MALFORMED)?;
        if slot.is_some_and(|prior| prior != value) {
            return Err(MALFORMED.into());
        }
        *slot = Some(value);
    }
    let primary_time = if record.time == 0 {
        record.observed_time
    } else {
        record.time
    };
    let (event_id, kind, payload) = if record.activity {
        let (Some(_), Some(record_id)) = (&record.scope.session_id, record_id) else {
            return Ok(None);
        };
        if primary_time == 0 || primary_time > i64::MAX as u64 || record.severity > 24 {
            return Err(MALFORMED.into());
        }
        let activity = OtlpActivityLog {
            reported_error: record.severity >= 17 || matches!(outcome, Some("error" | "failed")),
            target_source_id: record.scope.target_source_id.clone(),
            usage: token_usage::reported_counters(&record.attributes),
            cost: cost::reported_amount(&record.attributes).map(|(currency, nanounits)| LogCost {
                currency,
                nanounits,
            }),
        };
        (
            format!("otlp-log:{:x}", Sha256::digest(record_id.as_bytes())),
            ObservationKind::OtlpActivityLog(activity),
            None,
        )
    } else {
        if record.scope.target_source_id.is_some() && record.scope.session_id.is_none() {
            return Err("An OTLP error log native link requires an explicit session".into());
        }
        if record.scope.session_id.is_none() {
            record.scope.session_id = launch_session
                .filter(|value| valid_session_id(value))
                .map(str::to_owned);
        }
        let log = OtlpErrorLog {
            severity_number: record.severity,
            source_time_unix_ns: (record.time > 0).then_some(record.time),
            observed_time_unix_ns: (record.observed_time > 0).then_some(record.observed_time),
            trace_id: record.trace_id,
            span_id: record.span_id,
            target_source_id: record.scope.target_source_id.clone(),
        };
        if !log.is_valid_at(primary_time) {
            return Err(MALFORMED.into());
        }
        if record_id.is_none() && primary_time == 0 {
            return Ok(None);
        }
        let mut digest = Sha256::new();
        digest.update(b"agenturm-otlp-error-log/1\0");
        digest.update(source.as_str());
        digest.update([0]);
        if let Some(id) = record_id {
            digest.update(b"record\0");
            digest.update(id);
        } else {
            digest.update(b"metadata\0");
            digest.update(
                serde_json::to_vec(&(
                    &record.scope.session_id,
                    &record.scope.turn_id,
                    &record.scope.worker_id,
                    &log,
                ))
                .map_err(|_| MALFORMED)?,
            );
        }
        let payload = match (capture, record.body) {
            (Some(binding), Some(text)) => {
                if binding.source_id != *source
                    || log.target_source_id.as_deref()
                        != binding.native_source_id.as_ref().map(ExtensionId::as_str)
                {
                    return Err("OTLP log text does not match the configured capture source".into());
                }
                Some(TelemetryPayload {
                    input: Some(text),
                    output: None,
                })
            }
            _ => None,
        };
        (
            format!("otlp-error-log:{:x}", digest.finalize()),
            ObservationKind::OtlpErrorLog(log),
            payload,
        )
    };
    let mut identity = record
        .scope
        .session_id
        .map_or_else(ObservationIdentity::source, ObservationIdentity::session);
    identity.turn_id = record.scope.turn_id;
    identity.worker_id = record.scope.worker_id;
    Ok(Some((
        Observation {
            harness_id: source.clone(),
            event_id,
            revision: 0,
            identity,
            observed_at_unix_ns: primary_time,
            kind,
        },
        payload,
    )))
}
