//! Read bounded OTLP protobuf log export framing and selected fields.

use super::{
    EVENT_NAME, MALFORMED, MAX_ATTRIBUTES, MAX_LOGS, Record, activity_key, exact_id,
    is_error_severity, log_key, record_scope, scope_key,
};
use crate::trace_import::{
    Attribute,
    proto::{Field, decode_attribute, fields, id_hex},
};

pub(super) fn decode(input: &[u8], capture: bool) -> Result<Vec<Option<Record>>, &'static str> {
    let mut records = Vec::new();
    for resource in fields(input) {
        let (1, Field::Bytes(resource)) = resource? else {
            continue;
        };
        let resource_scope = resource_attributes(resource);
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
                records.push(decode_record(record, &resource_scope, capture)?);
            }
        }
    }
    Ok(records)
}

fn resource_attributes(input: &[u8]) -> Result<Vec<Attribute>, &'static str> {
    let mut attributes = Vec::new();
    for resource in fields(input) {
        let (1, Field::Bytes(resource)) = resource? else {
            continue;
        };
        for attribute in fields(resource) {
            if let (1, Field::Bytes(value)) = attribute? {
                push_attribute(&mut attributes, value, scope_key)?;
            }
        }
    }
    Ok(attributes)
}

fn push_attribute(
    attributes: &mut Vec<Attribute>,
    value: &[u8],
    retain: fn(&str) -> bool,
) -> Result<(), &'static str> {
    if let Some(attribute) = decode_attribute(value, retain)? {
        if attributes.len() == MAX_ATTRIBUTES {
            return Err(MALFORMED);
        }
        attributes.push(attribute);
    }
    Ok(())
}

fn remember<T: PartialEq>(slot: &mut Option<T>, value: T) -> Result<(), &'static str> {
    if slot.as_ref().is_some_and(|prior| prior != &value) {
        return Err(MALFORMED);
    }
    *slot = Some(value);
    Ok(())
}

fn decode_record(
    input: &[u8],
    resource: &Result<Vec<Attribute>, &'static str>,
    capture: bool,
) -> Result<Option<Record>, &'static str> {
    let (mut event_name, mut severity) = (None, None);
    for field in fields(input) {
        match field? {
            (12, Field::Bytes(name)) => remember(&mut event_name, name)?,
            (2, Field::Varint(value)) => remember(&mut severity, value)?,
            _ => {}
        }
    }
    let activity = event_name == Some(EVENT_NAME.as_bytes());
    let severity = severity.unwrap_or_default();
    if !activity && !is_error_severity(severity) {
        return Ok(None);
    }
    let mut record = Record {
        activity,
        severity: u8::try_from(severity).map_err(|_| MALFORMED)?,
        ..Record::default()
    };
    let (mut time, mut observed_time) = (None, None);
    let (mut trace_id, mut span_id, mut body) = (None, None, None);
    let retain = if activity { activity_key } else { log_key };
    for field in fields(input) {
        match field? {
            (1, Field::Fixed64(value)) => remember(&mut time, value)?,
            (11, Field::Fixed64(value)) => remember(&mut observed_time, value)?,
            (6, Field::Bytes(value)) => push_attribute(&mut record.attributes, value, retain)?,
            (9, Field::Bytes(value)) if !activity => remember(&mut trace_id, value)?,
            (10, Field::Bytes(value)) if !activity => remember(&mut span_id, value)?,
            (5, Field::Bytes(value)) if !activity && capture => remember(&mut body, value)?,
            _ => {}
        }
    }
    record.time = time.unwrap_or_default();
    record.observed_time = observed_time.unwrap_or_default();
    record.scope = record_scope(&record.attributes, resource, activity)?;
    record.trace_id = decode_id(trace_id, 16)?;
    record.span_id = decode_id(span_id, 8)?;
    record.body = body.map(decode_text_body).transpose()?.flatten();
    Ok(Some(record))
}

fn decode_id(value: Option<&[u8]>, bytes: usize) -> Result<Option<String>, &'static str> {
    let hex = value
        .filter(|value| !value.is_empty())
        .map(|value| id_hex(value, bytes).map_err(|_| MALFORMED))
        .transpose()?;
    exact_id(hex, bytes * 2)
}

fn decode_text_body(input: &[u8]) -> Result<Option<String>, &'static str> {
    let mut fields = fields(input);
    match fields.next().transpose()? {
        Some((1, Field::Bytes(text))) if fields.next().transpose()?.is_none() => Ok(Some(
            std::str::from_utf8(text).map_err(|_| MALFORMED)?.to_owned(),
        )),
        _ => Ok(None),
    }
}
