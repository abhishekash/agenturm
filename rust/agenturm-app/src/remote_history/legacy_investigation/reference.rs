use super::{CapturePolicy, LocalPolicy};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

const MAX_TOKEN: usize = 8192;
const BASE64_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const INVALID: &str =
    "Invalid exact source reference; use the reference returned by the investigation";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Reference {
    pub receiver_id: String,
    pub event_id: String,
    source: SourceKind,
    source_id: String,
    #[serde(deserialize_with = "present")]
    pub service: Option<String>,
    #[serde(deserialize_with = "present")]
    agent_id: Option<String>,
    #[serde(deserialize_with = "present")]
    session_id: Option<String>,
    #[serde(deserialize_with = "present")]
    trace_id: Option<String>,
    #[serde(deserialize_with = "present")]
    span_id: Option<String>,
    #[serde(deserialize_with = "present")]
    turn_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SourceKind {
    Span,
    Log,
}

impl Reference {
    pub(super) fn owner_fields(&self) -> [Option<&str>; 6] {
        [
            self.service.as_deref(),
            self.agent_id.as_deref(),
            self.session_id.as_deref(),
            self.trace_id.as_deref(),
            self.span_id.as_deref(),
            self.turn_id.as_deref(),
        ]
    }

    fn valid(&self) -> bool {
        hex(&self.receiver_id, 32)
            && safe_label(&self.event_id, 512)
            && !self.event_id.starts_with("--")
            && self
                .service
                .as_deref()
                .is_some_and(|service| safe_label(service, 128))
            && [
                self.agent_id.as_deref(),
                self.session_id.as_deref(),
                self.turn_id.as_deref(),
            ]
            .into_iter()
            .flatten()
            .all(|part| safe_label(part, 512))
            && self.trace_id.as_deref().is_none_or(|part| hex(part, 32))
            && self.span_id.as_deref().is_none_or(|part| hex(part, 16))
            && match self.source {
                SourceKind::Span => {
                    let (span, index) = self
                        .source_id
                        .split_once(":event:")
                        .map_or((self.source_id.as_str(), None), |(span, index)| {
                            (span, Some(index))
                        });
                    self.trace_id.is_some()
                        && self.span_id.as_deref() == Some(span)
                        && index.is_none_or(|value| {
                            (1..=5).contains(&value.len())
                                && value.bytes().all(|b| b.is_ascii_digit())
                        })
                }
                SourceKind::Log => hex(&self.source_id, 64),
            }
    }

    pub(super) fn policy(&self, policy: &LocalPolicy) -> Result<CapturePolicy, String> {
        if !self.valid() {
            return Err(INVALID.into());
        }
        policy.for_investigation(
            self.service.as_deref().unwrap_or_default(),
            self.agent_id.as_deref(),
            self.session_id.as_deref(),
        )
    }

    pub(super) fn encode(&self) -> Result<String, String> {
        if !self.valid() {
            return Err(INVALID.into());
        }
        let raw = serde_json::to_vec(self).map_err(|_| INVALID)?;
        let encoded = encode_base64_url(&raw);
        if encoded.len() > MAX_TOKEN {
            return Err("Exact source reference exceeds its bounded size".into());
        }
        Ok(encoded)
    }

    pub(super) fn decode(token: &str) -> Result<Self, String> {
        let bytes = decode_base64_url(token).ok_or(INVALID)?;
        let result: Self = serde_json::from_slice(&bytes).map_err(|_| INVALID)?;
        if !result.valid() {
            return Err(INVALID.into());
        }
        Ok(result)
    }
}

#[derive(Deserialize, Serialize)]
pub(super) struct SourceResponse {
    receiver_id: String,
    event_id: String,
    source_reference: Reference,
    sources: Vec<SourceRecord>,
    references_total: usize,
    references_clipped: bool,
}

#[derive(Deserialize, Serialize)]
struct SourceRecord {
    #[serde(rename = "type")]
    kind: SourceKind,
    id: String,
    record: Option<Value>,
    complete: bool,
    reason: Option<String>,
    scope: Owner,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    #[serde(deserialize_with = "present")]
    service: Option<String>,
    #[serde(deserialize_with = "present")]
    agent_id: Option<String>,
    #[serde(deserialize_with = "present")]
    session_id: Option<String>,
    #[serde(deserialize_with = "present")]
    trace_id: Option<String>,
    #[serde(deserialize_with = "present")]
    span_id: Option<String>,
    #[serde(deserialize_with = "present")]
    turn_id: Option<String>,
}

impl Owner {
    fn fields(&self) -> [Option<&str>; 6] {
        [
            self.service.as_deref(),
            self.agent_id.as_deref(),
            self.session_id.as_deref(),
            self.trace_id.as_deref(),
            self.span_id.as_deref(),
            self.turn_id.as_deref(),
        ]
    }
}

impl SourceResponse {
    pub(super) fn verify(&self, expected: &Reference, initial: &LocalPolicy) -> Result<(), String> {
        if self.receiver_id != expected.receiver_id
            || self.event_id != expected.event_id
            || &self.source_reference != expected
            || self.sources.len() > 8
            || self.references_total != 1
            || self.references_clipped
            || self.sources.len() > self.references_total
        {
            return Err(
                "Exact source response differs from the selected investigation event".into(),
            );
        }
        for row in &self.sources {
            if row.kind != expected.source
                || row.id != expected.source_id
                || row.scope.fields() != expected.owner_fields()
                || row
                    .reason
                    .as_deref()
                    .is_some_and(|reason| !safe_label(reason, 512))
                || row.record.as_ref().is_some_and(|value| !safe_record(value))
            {
                return Err(
                    "Exact source owner, record or execution differs from the selected reference"
                        .into(),
                );
            }
            let [service, agent, session, ..] = row.scope.fields();
            initial.for_investigation(service.unwrap_or_default(), agent, session)?;
        }
        Ok(())
    }
}

fn present<'de, D: Deserializer<'de>>(input: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(input)
}

fn hex(value: &str, width: usize) -> bool {
    value.len() == width
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn safe_character(ch: char) -> bool {
    !ch.is_control()
        && !matches!(ch, '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}')
}

pub(super) fn safe_label(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && text.chars().all(safe_character)
}

pub(super) fn safe_text(text: &str, max: usize) -> bool {
    text.chars()
        .all(|ch| matches!(ch, '\t' | '\n') || safe_character(ch))
        && agenturm_store::bounded_text_preview(text.to_owned(), max).is_ok_and(|safe| safe == text)
}

fn safe_record(record: &Value) -> bool {
    match record {
        Value::String(text) => text.is_empty() || safe_text(text, 8192),
        Value::Array(rows) => rows.iter().all(safe_record),
        Value::Object(fields) => fields
            .iter()
            .all(|(key, value)| safe_label(key, 1024) && safe_record(value)),
        Value::Number(_) | Value::Bool(_) | Value::Null => true,
    }
}

fn encode_base64_url(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or_default();
        let c = chunk.get(2).copied().unwrap_or_default();
        output.push(char::from(BASE64_URL[usize::from(a >> 2)]));
        output.push(char::from(BASE64_URL[usize::from((a & 3) << 4 | b >> 4)]));
        if chunk.len() > 1 {
            output.push(char::from(BASE64_URL[usize::from((b & 15) << 2 | c >> 6)]));
        }
        if chunk.len() > 2 {
            output.push(char::from(BASE64_URL[usize::from(c & 63)]));
        }
    }
    output
}

fn decode_base64_url(token: &str) -> Option<Vec<u8>> {
    if token.is_empty() || token.len() > MAX_TOKEN {
        return None;
    }
    let data = token.trim_end_matches('=');
    let pad = token.len() - data.len();
    if pad > 2 || (pad != 0 && !token.len().is_multiple_of(4)) || data.len() % 4 == 1 {
        return None;
    }
    let mut bytes = Vec::with_capacity(data.len() * 3 / 4);
    let mut pending = 0_u16;
    let mut bits = 0;
    for byte in data.bytes() {
        let digit = BASE64_URL.iter().position(|literal| *literal == byte)?;
        pending = (pending << 6) | digit as u16;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((pending >> bits) as u8);
            pending &= (1 << bits) - 1;
        }
    }
    (pending == 0).then_some(bytes)
}
