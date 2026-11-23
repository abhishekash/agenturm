//! Project only OTLP log record framing and fields required for activity facts.

use super::{EVENT_NAME, MALFORMED, MAX_ATTRIBUTES, MAX_LOGS, Record, retained_key};
use crate::trace_import::proto::{Field, decode_attribute, fields};

pub(super) fn records(input: &[u8]) -> Result<Vec<&[u8]>, &'static str> {
    let mut records = Vec::new();
    for resource in fields(input) {
        let (1, Field::Bytes(resource)) = resource? else {
            continue;
        };
        for scope in fields(resource) {
            let (2, Field::Bytes(scope)) = scope? else {
                continue;
            };
            for record in fields(scope) {
                let (2, Field::Bytes(record)) = record? else {
                    continue;
                };
                if records.len() == MAX_LOGS {
                    return Err("OTLP export exceeds 1024 log records");
                }
                records.push(record);
            }
        }
    }
    Ok(records)
}

pub(super) fn decode_record(input: &[u8]) -> Result<Option<Record>, &'static str> {
    let mut event_name = None;
    for field in fields(input) {
        if let (12, Field::Bytes(name)) = field? {
            if event_name.is_some_and(|prior| prior != name) {
                return Err(MALFORMED);
            }
            event_name = Some(name);
        }
    }
    if event_name != Some(EVENT_NAME.as_bytes()) {
        return Ok(None);
    }
    let mut record = Record::default();
    for field in fields(input) {
        match field? {
            (1, Field::Fixed64(time)) => record.time = time,
            (2, Field::Varint(severity)) => {
                record.severity = u8::try_from(severity).map_err(|_| MALFORMED)?;
            }
            (6, Field::Bytes(value)) => {
                if let Some(attribute) = decode_attribute(value, retained_key)? {
                    if record.attributes.len() == MAX_ATTRIBUTES {
                        return Err(MALFORMED);
                    }
                    record.attributes.push(attribute);
                }
            }
            (11, Field::Fixed64(time)) => record.observed_time = time,
            _ => {}
        }
    }
    Ok(Some(record))
}
