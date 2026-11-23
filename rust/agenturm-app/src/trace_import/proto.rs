//! Decode the structural OTLP/HTTP trace fields we retain. Unknown fields
//! are skipped without decoding names, events, links, or arbitrary attributes.

use super::{
    Attribute, AttributeValue, MAX_EXPORT_SPANS, OPERATION_KEY, OtlpSpan, PAYLOAD_INPUT,
    PAYLOAD_OUTPUT, Status, TURN_KEY, Timestamp, WORKER_KEY,
};
use std::fmt::Write as _;

const MALFORMED: &str = "invalid OTLP protobuf trace export";

pub(crate) enum Field<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32,
}

fn split<'a>(input: &mut &'a [u8], size: usize) -> Result<&'a [u8], &'static str> {
    let (head, tail) = input.split_at_checked(size).ok_or(MALFORMED)?;
    *input = tail;
    Ok(head)
}

pub(crate) fn varint(input: &mut &[u8]) -> Result<u64, &'static str> {
    let mut value = 0;
    for shift in (0..=63).step_by(7) {
        let byte = split(input, 1)?[0];
        if shift == 63 && byte > 1 {
            return Err(MALFORMED);
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Ok(value);
        }
    }
    Err(MALFORMED)
}

pub(crate) fn fields<'a>(
    mut input: &'a [u8],
) -> impl Iterator<Item = Result<(u64, Field<'a>), &'static str>> {
    std::iter::from_fn(move || {
        if input.is_empty() {
            return None;
        }
        let field = (|| {
            let tag = varint(&mut input)?;
            let number = tag >> 3;
            if number == 0 || number > (1 << 29) - 1 {
                return Err(MALFORMED);
            }
            let value = match tag & 7 {
                0 => Field::Varint(varint(&mut input)?),
                1 => Field::Fixed64(u64::from_le_bytes(
                    split(&mut input, 8)?.try_into().expect("eight bytes"),
                )),
                2 => {
                    let length = usize::try_from(varint(&mut input)?).map_err(|_| MALFORMED)?;
                    Field::Bytes(split(&mut input, length)?)
                }
                5 => {
                    split(&mut input, 4)?;
                    Field::Fixed32
                }
                _ => return Err(MALFORMED),
            };
            Ok((number, value))
        })();
        if field.is_err() {
            input = &[];
        }
        Some(field)
    })
}

pub(super) fn decode(input: &[u8]) -> Result<Vec<OtlpSpan>, &'static str> {
    let mut spans = Vec::new();
    for resource in fields(input) {
        let (1, Field::Bytes(resource)) = resource? else {
            continue;
        };
        for scope in fields(resource) {
            let (2, Field::Bytes(scope)) = scope? else {
                continue;
            };
            for span in fields(scope) {
                let (2, Field::Bytes(span)) = span? else {
                    continue;
                };
                if spans.len() == MAX_EXPORT_SPANS {
                    return Err("OTLP export exceeds 1024 spans");
                }
                spans.push(decode_span(span)?);
            }
        }
    }
    Ok(spans)
}

fn decode_span(input: &[u8]) -> Result<OtlpSpan, &'static str> {
    let mut span = OtlpSpan {
        trace_id: String::new(),
        span_id: String::new(),
        parent_span_id: None,
        start_time_unix_nano: Timestamp::Number(0),
        end_time_unix_nano: Timestamp::Number(0),
        status: None,
        attributes: Vec::new(),
    };
    for field in fields(input) {
        match field? {
            (1, Field::Bytes(id)) => span.trace_id = id_hex(id, 16)?,
            (2, Field::Bytes(id)) => span.span_id = id_hex(id, 8)?,
            (4, Field::Bytes(id)) if !id.is_empty() => span.parent_span_id = Some(id_hex(id, 8)?),
            (7, Field::Fixed64(time)) => span.start_time_unix_nano = Timestamp::Number(time),
            (8, Field::Fixed64(time)) => span.end_time_unix_nano = Timestamp::Number(time),
            (9, Field::Bytes(value)) => {
                if let Some(attribute) = decode_attribute(value)? {
                    if span.attributes.len() == 64 {
                        return Err(MALFORMED);
                    }
                    span.attributes.push(attribute);
                }
            }
            (15, Field::Bytes(status)) => {
                let mut code = None;
                for field in fields(status) {
                    if let (3, Field::Varint(value)) = field? {
                        code = Some(u8::try_from(value).map_err(|_| MALFORMED)?);
                    }
                }
                span.status = Some(Status { code });
            }
            _ => {}
        }
    }
    Ok(span)
}

fn decode_attribute(input: &[u8]) -> Result<Option<Attribute>, &'static str> {
    let mut key = None;
    let mut value = None;
    for field in fields(input) {
        match field? {
            (1, Field::Bytes(data)) => key = Some(data),
            (2, Field::Bytes(data)) => value = Some(data),
            _ => {}
        }
    }
    let Some(key) = key.and_then(|bytes| std::str::from_utf8(bytes).ok()) else {
        return Ok(None);
    };
    let cost_key = super::cost::is_cost_key(key);
    let numeric_key = cost_key || super::token_usage::is_token_key(key);
    if !numeric_key
        && !matches!(
            key,
            "session.id"
                | "gen_ai.conversation.id"
                | "agenturm.source.id"
                | TURN_KEY
                | WORKER_KEY
                | OPERATION_KEY
                | PAYLOAD_INPUT
                | PAYLOAD_OUTPUT
        )
    {
        return Ok(None);
    }
    let max_len = if matches!(key, PAYLOAD_INPUT | PAYLOAD_OUTPUT) {
        agenturm_store::MAX_TELEMETRY_PAYLOAD_TEXT_BYTES
    } else {
        512
    };
    let mut scalar = AttributeValue::default();
    if let Some(value) = value {
        for field in fields(value) {
            let (number, field) = field?;
            if (1..=7).contains(&number) {
                scalar = AttributeValue::default();
                match (number, field) {
                    (1, Field::Bytes(data)) if data.len() <= max_len => {
                        if let Ok(text) = std::str::from_utf8(data) {
                            scalar.string_value = text.into();
                        } else if !numeric_key {
                            return Err(MALFORMED);
                        }
                    }
                    (2, Field::Varint(value @ 0..=1)) if cost_key => {
                        scalar.bool_value = (value != 0).into()
                    }
                    (3, Field::Varint(value)) if numeric_key => {
                        scalar.int_value = serde_json::value::to_raw_value(&(value as i64)).ok();
                    }
                    (4, Field::Fixed64(value)) if cost_key => {
                        scalar.double_value =
                            serde_json::value::to_raw_value(&f64::from_bits(value)).ok();
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(Some(Attribute {
        key: key.to_owned(),
        value: Some(scalar),
    }))
}

fn id_hex(bytes: &[u8], size: usize) -> Result<String, &'static str> {
    if bytes.len() != size {
        return Err(MALFORMED);
    }
    let mut id = String::with_capacity(size * 2);
    for byte in bytes {
        write!(id, "{byte:02x}").expect("writing a string");
    }
    Ok(id)
}

pub(super) fn response(rejected: usize) -> Vec<u8> {
    if rejected == 0 {
        return Vec::new();
    }
    let message = b"Span lacks an explicit session identity";
    let mut partial = vec![0x08];
    encode_varint(&mut partial, rejected);
    partial.push(0x12);
    encode_varint(&mut partial, message.len());
    partial.extend_from_slice(message);
    let mut result = vec![0x0a];
    encode_varint(&mut result, partial.len());
    result.extend_from_slice(&partial);
    result
}

fn encode_varint(output: &mut Vec<u8>, mut value: usize) {
    while value >= 0x80 {
        output.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(tag: u8, fields: &[u8]) -> Vec<u8> {
        let mut result = vec![tag << 3 | 2];
        encode_varint(&mut result, fields.len());
        result.extend_from_slice(fields);
        result
    }

    #[test]
    fn token_usage_projects_exact_protobuf_integer_fields() {
        let attribute = |key: &str, scalar: &[u8]| {
            let mut value = message(1, key.as_bytes());
            value.extend(message(2, scalar));
            message(9, &value)
        };
        let mut span = message(1, &[1; 16]);
        span.extend(message(2, &[2; 8]));
        span.push(7 << 3 | 1);
        span.extend(10_u64.to_le_bytes());
        span.push(8 << 3 | 1);
        span.extend(20_u64.to_le_bytes());
        span.extend(attribute("session.id", &message(1, b"usage-session")));
        span.extend(attribute(
            "agenturm.source.id",
            &message(1, b"harness.codex"),
        ));
        span.extend(attribute(TURN_KEY, &message(1, b"native-turn")));
        span.extend(attribute(OPERATION_KEY, &message(1, b"native-mcp-call")));
        span.extend(attribute("llm.token_count.prompt", &[3 << 3, 0]));
        span.extend(attribute(
            "gen_ai.usage.cache_read.input_tokens",
            &[3 << 3, 4],
        ));
        span.extend(attribute("gen_ai.usage.output_tokens", &[2 << 3, 1]));
        span.extend(attribute(
            "gen_ai.usage.total_tokens",
            &message(1, b"9223372036854775807"),
        ));
        let source = agenturm_kernel::ExtensionId::parse("otlp.fixture").unwrap();
        let export = message(1, &message(2, &message(2, &span)));
        let (rows, unscoped, payloads) =
            super::super::decode_protobuf(&export, &source, None, None).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(unscoped, 0);
        assert!(payloads.is_empty());
        assert_eq!(rows[0].identity, rows[1].identity);
        assert!(rows[0].identity.operation_id.is_none());
        assert!(rows.iter().any(|row| matches!(
            &row.kind,
            agenturm_kernel::ObservationKind::TraceSpan(span)
                if span.target_operation_id.as_deref() == Some("native-mcp-call")
        )));
        let usage = rows
            .iter()
            .find_map(|row| match &row.kind {
                agenturm_kernel::ObservationKind::SpanTokenUsage(usage) => Some(usage),
                _ => None,
            })
            .unwrap();
        assert_eq!(usage.input_tokens, Some(0));
        assert_eq!(usage.output_tokens, None);
        assert_eq!(usage.cache_read_input_tokens, Some(4));
        assert_eq!(usage.total_tokens, Some(i64::MAX as u64));
        assert_eq!(usage.span.span_id, "02".repeat(8));
    }

    #[test]
    fn selective_cost_attributes_preserve_protobuf_currency_and_precision() {
        let mut span = message(1, &[3; 16]);
        span.extend(message(2, &[4; 8]));
        span.push(7 << 3 | 1);
        span.extend(10_u64.to_le_bytes());
        span.push(8 << 3 | 1);
        span.extend(20_u64.to_le_bytes());

        let attribute = |key: &str, scalar: &[u8]| {
            let mut field = message(1, key.as_bytes());
            field.extend(message(2, scalar));
            message(9, &field)
        };
        span.extend(attribute("session.id", &message(1, b"native-session")));
        span.extend(attribute("gen_ai.usage.cost.currency", &message(1, b"EUR")));
        let mut amount = vec![4 << 3 | 1];
        amount.extend(0.000000123_f64.to_bits().to_le_bytes());
        span.extend(attribute("gen_ai.usage.cost", &amount));
        span.extend(attribute("prompt", &message(1, b"private prompt")));
        let id = agenturm_kernel::ExtensionId::parse("otlp.fixture").unwrap();
        let project = |span: &[u8]| {
            let export = message(1, &message(2, &message(2, span)));
            super::super::decode_protobuf(&export, &id, None, None)
                .unwrap()
                .0
        };
        let rows = project(&span);
        assert_eq!(rows.len(), 2);
        let cost = rows
            .iter()
            .find_map(|row| match &row.kind {
                agenturm_kernel::ObservationKind::ReportedCost(cost) => Some(cost),
                _ => None,
            })
            .unwrap();
        assert_eq!(cost.currency, "EUR");
        assert_eq!(cost.nanounits, 123);
        assert_eq!(cost.span.as_ref().unwrap().span_id, "04".repeat(8));
        assert!(
            !serde_json::to_string(&rows)
                .unwrap()
                .contains("private prompt")
        );
        span.extend(attribute("llm.cost_known", &[2 << 3, 0]));
        assert_eq!(project(&span).len(), 1);
    }

    #[test]
    fn decodes_exact_native_protobuf_trace_evidence() {
        let mut span = message(1, &[3; 16]);
        span.extend(message(2, &[4; 8]));
        span.extend(message(4, &[5; 8]));
        span.extend(message(5, b"private span name"));
        span.push(7 << 3 | 1);
        span.extend(10_u64.to_le_bytes());
        span.push(8 << 3 | 1);
        span.extend(20_u64.to_le_bytes());
        let mut identity = message(1, b"session.id");
        identity.extend(message(2, &message(1, b"native-session")));
        span.extend(message(9, &identity));
        span.extend(message(15, &[3 << 3, 2]));
        let export = message(1, &message(2, &message(2, &span)));

        let id = agenturm_kernel::ExtensionId::parse("otlp.fixture").unwrap();
        let (rows, skipped, _) = super::super::decode_protobuf(&export, &id, None, None).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].identity.session_id.as_deref(),
            Some("native-session")
        );
        let agenturm_kernel::ObservationKind::TraceSpan(span) = &rows[0].kind else {
            panic!("expected a native span");
        };
        assert_eq!(span.trace_id, "03".repeat(16));
        assert_eq!(span.parent_span_id, Some("05".repeat(8)));
        assert_eq!(span.started_at_unix_ns, 10);
        assert_eq!(rows[0].observed_at_unix_ns, 20);
        assert_eq!(span.status, agenturm_kernel::TraceStatus::Error);
        assert!(
            super::super::decode_protobuf(&export[..export.len() - 1], &id, None, None).is_err()
        );
    }

    #[test]
    fn explicit_payload_fields_require_a_matching_receiver_scope() {
        let source = agenturm_kernel::ExtensionId::parse("otlp.fixture").unwrap();
        let binding = agenturm_store::TelemetryBinding {
            source_id: source.clone(),
            native_source_id: None,
        };
        let mut span = message(1, &[3; 16]);
        span.extend(message(2, &[4; 8]));
        span.push(7 << 3 | 1);
        span.extend(10_u64.to_le_bytes());
        span.push(8 << 3 | 1);
        span.extend(20_u64.to_le_bytes());
        for (key, value) in [
            ("session.id", "fixture-session"),
            (PAYLOAD_INPUT, "recorded text"),
            ("input.value", "excluded provider request"),
        ] {
            let mut attribute = message(1, key.as_bytes());
            attribute.extend(message(2, &message(1, value.as_bytes())));
            span.extend(message(9, &attribute));
        }
        let export = message(1, &message(2, &message(2, &span)));
        assert!(super::super::decode_protobuf(&export, &source, None, None).is_err());

        let (rows, skipped, private) =
            super::super::decode_protobuf(&export, &source, None, Some(&binding)).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(rows.len(), 1);
        assert_eq!(private.len(), 1);
        assert_eq!(private[0].event_id, rows[0].event_id);
        assert_eq!(private[0].payload.input.as_deref(), Some("recorded text"));
        let metadata = serde_json::to_string(&rows).unwrap();
        assert!(!metadata.contains("recorded text"));
        assert!(!metadata.contains("excluded provider request"));
    }

    #[test]
    fn reports_partial_success_as_a_protobuf_message() {
        assert!(response(0).is_empty());
        let encoded = response(513);
        let (number, Field::Bytes(partial)) = fields(&encoded).next().unwrap().unwrap() else {
            panic!("expected the partial success field");
        };
        assert_eq!(number, 1);
        assert!(matches!(
            fields(partial).next().unwrap().unwrap(),
            (1, Field::Varint(513))
        ));
    }
}
