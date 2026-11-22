//! Decode the structural OTLP/HTTP trace fields we retain. Unknown fields
//! are skipped without decoding names, events, links, or payload attributes.

use super::{
    Attribute, AttributeValue, MAX_EXPORT_SPANS, OtlpSpan, Status, TURN_KEY, Timestamp, WORKER_KEY,
};
use std::fmt::Write as _;

const MALFORMED: &str = "invalid OTLP protobuf trace export";

enum Field<'a> {
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

fn varint(input: &mut &[u8]) -> Result<u64, &'static str> {
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

fn fields<'a>(mut input: &'a [u8]) -> impl Iterator<Item = Result<(u64, Field<'a>), &'static str>> {
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
                    if span.attributes.len() == 16 {
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
    if !matches!(
        key,
        "session.id" | "gen_ai.conversation.id" | "agenturm.source.id" | TURN_KEY | WORKER_KEY
    ) {
        return Ok(None);
    }
    let mut string_value = None;
    if let Some(value) = value {
        for field in fields(value) {
            let (number, field) = field?;
            if (1..=7).contains(&number) {
                string_value = match (number, field) {
                    (1, Field::Bytes(data)) if data.len() <= 512 => {
                        Some(std::str::from_utf8(data).map_err(|_| MALFORMED)?.to_owned())
                    }
                    _ => None,
                };
            }
        }
    }
    Ok(Some(Attribute {
        key: key.to_owned(),
        value: Some(AttributeValue { string_value }),
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
        let (rows, skipped) = super::super::decode_protobuf(&export, &id, None).unwrap();
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
        assert!(super::super::decode_protobuf(&export[..export.len() - 1], &id, None).is_err());
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
