//! Admit explicit OTLP activity as closed metadata and preserve producer identity.

mod proto;
#[cfg(test)]
mod tests;

use crate::trace_import::{
    Attribute, DecodedExport, MAX_EXPORT_BYTES, Timestamp, cost, token_usage, trace_scope,
    valid_execution_id,
};
use agenturm_kernel::telemetry_summary::{LogCost, OtlpActivityLog};
use agenturm_kernel::{ExtensionId, Observation, ObservationIdentity, ObservationKind};
use serde::Deserialize;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, btree_map::Entry};

const MALFORMED: &str = "Invalid OTLP activity log export";
const MAX_LOGS: usize = 1_024;
const MAX_ATTRIBUTES: usize = 64;
const EVENT_NAME: &str = "agent.activity";
const RECORD_ID_KEYS: &[&str] = &[
    "agenturm.event.id",
    "agenturm.log.id",
    "observatory.record_id",
];
const REJECTED: &str =
    "Only explicit agent.activity records with a record ID and session are retained";

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
    severity: u8,
    time: u64,
    observed_time: u64,
    attributes: Vec<Attribute>,
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
    severity_number: Option<u8>,
    time_unix_nano: Option<Timestamp>,
    observed_time_unix_nano: Option<Timestamp>,
    #[serde(default, borrow)]
    attributes: Vec<JsonAttribute<'a>>,
}

#[derive(Deserialize)]
struct JsonAttribute<'a> {
    key: String,
    #[serde(borrow)]
    value: Option<&'a RawValue>,
}

fn retained_key(key: &str) -> bool {
    matches!(
        key,
        "session.id"
            | "gen_ai.conversation.id"
            | "agenturm.source.id"
            | "agenturm.turn.id"
            | "agenturm.worker.id"
            | "agent.activity.result"
    ) || RECORD_ID_KEYS.contains(&key)
        || cost::is_cost_key(key)
        || token_usage::is_token_key(key)
}

pub(super) fn decode_json(bytes: &[u8], source: &ExtensionId) -> Result<DecodedExport, String> {
    if bytes.len() as u64 > MAX_EXPORT_BYTES {
        return Err(MALFORMED.into());
    }
    let export: Export<'_> = serde_json::from_slice(bytes).map_err(|_| MALFORMED)?;
    let records = export
        .resource_logs
        .into_iter()
        .flat_map(|resource| resource.scope_logs)
        .flat_map(|scope| scope.log_records)
        .map(decode_json_record);
    project(records, source)
}

fn decode_json_record(raw: &RawValue) -> Result<Option<Record>, &'static str> {
    let record: JsonRecord<'_> = serde_json::from_str(raw.get()).map_err(|_| MALFORMED)?;
    if record.event_name.as_deref() != Some(EVENT_NAME) {
        return Ok(None);
    }
    let mut attributes = Vec::new();
    for attribute in record
        .attributes
        .into_iter()
        .filter(|attribute| retained_key(&attribute.key))
    {
        if attributes.len() >= MAX_ATTRIBUTES {
            return Err(MALFORMED);
        }
        let value = attribute
            .value
            .map(|value| serde_json::from_str(value.get()).map_err(|_| MALFORMED))
            .transpose()?;
        attributes.push(Attribute {
            key: attribute.key,
            value,
        });
    }
    let timestamp =
        |value: Option<Timestamp>| value.map_or(Ok(0), |time| time.value().ok_or(MALFORMED));
    Ok(Some(Record {
        severity: record.severity_number.unwrap_or(0),
        time: timestamp(record.time_unix_nano)?,
        observed_time: timestamp(record.observed_time_unix_nano)?,
        attributes,
    }))
}

pub(super) fn decode_protobuf(bytes: &[u8], source: &ExtensionId) -> Result<DecodedExport, String> {
    if bytes.len() as u64 > MAX_EXPORT_BYTES {
        return Err(MALFORMED.into());
    }
    project(
        proto::records(bytes)?.into_iter().map(proto::decode_record),
        source,
    )
}

fn project(
    records: impl IntoIterator<Item = Result<Option<Record>, &'static str>>,
    source: &ExtensionId,
) -> Result<DecodedExport, String> {
    let mut rows = BTreeMap::new();
    let mut skipped = 0;
    for (index, record) in records.into_iter().enumerate() {
        if index >= MAX_LOGS {
            return Err("OTLP export exceeds 1024 log records".into());
        }
        let Some(row) = record?
            .map(|record| project_record(record, source))
            .transpose()?
            .flatten()
        else {
            skipped += 1;
            continue;
        };
        match rows.entry(row.event_id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(row);
            }
            Entry::Occupied(entry) if entry.get() != &row => {
                return Err("OTLP export contains conflicting activity record identities".into());
            }
            Entry::Occupied(_) => {}
        }
    }
    Ok((rows.into_values().collect(), skipped, Vec::new()))
}

fn project_record(record: Record, source: &ExtensionId) -> Result<Option<Observation>, String> {
    let scope = trace_scope(&record.attributes)?;
    if scope.turn_id.is_some() && (scope.target_source_id.is_none() || scope.session_id.is_none()) {
        return Err("An OTLP log turn requires an explicit harness source and session".into());
    }
    if scope.worker_id.is_some() && scope.turn_id.is_none() {
        return Err("An OTLP log worker requires an explicit turn".into());
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
    let (Some(session_id), Some(record_id)) = (scope.session_id, record_id) else {
        return Ok(None);
    };
    let observed_at_unix_ns = if record.time == 0 {
        record.observed_time
    } else {
        record.time
    };
    if observed_at_unix_ns == 0 || observed_at_unix_ns > i64::MAX as u64 || record.severity > 24 {
        return Err(MALFORMED.into());
    }
    let mut identity = ObservationIdentity::session(session_id);
    identity.turn_id = scope.turn_id;
    identity.worker_id = scope.worker_id;
    let activity = OtlpActivityLog {
        reported_error: record.severity >= 17 || matches!(outcome, Some("error" | "failed")),
        target_source_id: scope.target_source_id,
        usage: token_usage::reported_counters(&record.attributes),
        cost: cost::reported_amount(&record.attributes).map(|(currency, nanounits)| LogCost {
            currency,
            nanounits,
        }),
    };
    Ok(Some(Observation {
        harness_id: source.clone(),
        event_id: format!("otlp-log:{:x}", Sha256::digest(record_id.as_bytes())),
        revision: 0,
        identity,
        observed_at_unix_ns,
        kind: ObservationKind::OtlpActivityLog(activity),
    }))
}
